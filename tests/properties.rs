//! Laws of the byte-preserving core, asserted over generated inputs.
//!
//! `core::scan` promises spans that tile a document, and `core::splice` promises edits that touch
//! only the bytes they name. The unit tests and the real-corpus tests (`tests/corpus.rs`) prove
//! the cases someone thought of; these prove the laws for the cases nobody did, and a failure
//! shrinks to a small input. The parsers that read files twaco does not control are also held to
//! "never panics, whatever the bytes".
//!
//! Each property runs 256 cases by default. For a deeper run:
//! `PROPTEST_CASES=5000 cargo test --test properties`. A failing case is saved beside this file
//! (`proptest-regressions/`) and replayed first on the next run; commit it.

use proptest::prelude::*;
use twaco::core::scan::{self, Kind, Span, Token};
use twaco::core::script;
use twaco::core::splice::{self, Edit, SpliceError};
use twaco::core::{entity, normalise};

fn config() -> ProptestConfig {
    ProptestConfig {
        // Shrinking a failure is bounded so a bad run still ends quickly.
        max_shrink_iters: 4096,
        ..ProptestConfig::default()
    }
}

// ---- generators -------------------------------------------------------------------------------

/// A generated document and, in document order, the attribute names of every start or empty tag.
#[derive(Clone, Debug)]
struct Doc {
    bytes: Vec<u8>,
    attributes: Vec<Vec<String>>,
}

#[derive(Clone, Debug)]
enum Node {
    Element {
        name: String,
        attributes: Vec<(String, char, String)>,
        children: Vec<Node>,
        short: bool,
    },
    Text(String),
    Comment(String),
    Pi(String),
    Cdata(String),
}

fn name() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "a", "b", "Thing", "ns:tag", "x.y", "a-b", "_q", "Entities",
    ])
    .prop_map(String::from)
}

/// An attribute value that may hold `>`, `/`, the other kind of quote, an entity and multi-byte
/// text, but never the quote that closes it and never `<`.
fn attribute() -> impl Strategy<Value = (String, char, String)> {
    (
        prop::sample::select(vec!["id", "name", "baseType", "xml:lang", "data-x", "p_1"])
            .prop_map(String::from),
        any::<bool>(),
        "[a-zA-Z0-9 >/=;.é日-]{0,10}",
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(attribute, double, mut value, other_quote, entity)| {
            let quote = if double { '"' } else { '\'' };
            if other_quote {
                value.push(if double { '\'' } else { '"' });
            }
            if entity {
                value.push_str("&amp;");
            }
            (attribute, quote, value)
        })
}

fn attributes() -> impl Strategy<Value = Vec<(String, char, String)>> {
    prop::collection::vec(attribute(), 0..4).prop_map(|mut list| {
        let mut seen = std::collections::BTreeSet::new();
        list.retain(|(name, _, _)| seen.insert(name.clone()));
        list
    })
}

fn cdata_payload() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 \\]>\\-<\n\té日]{0,20}".prop_map(|mut payload| {
        while payload.contains("]]>") {
            payload = payload.replace("]]>", "]] >");
        }
        payload
    })
}

fn node() -> impl Strategy<Value = Node> {
    let leaf = prop_oneof![
        "[a-zA-Z0-9 \n\t.,é日-]{0,12}".prop_map(Node::Text),
        "[a-zA-Z0-9 >&é日]{0,10}".prop_map(Node::Comment),
        // XML 1.0 [17]: `xml`, in any case, is not a processing instruction's target.
        "[a-z]{1,4} [a-z0-9 =\"]{0,8}"
            .prop_filter("xml is reserved", |pi| !pi.starts_with("xml "))
            .prop_map(Node::Pi),
        cdata_payload().prop_map(Node::Cdata),
        (name(), attributes(), any::<bool>()).prop_map(|(name, attributes, short)| Node::Element {
            name,
            attributes,
            children: Vec::new(),
            short
        }),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        (
            name(),
            attributes(),
            prop::collection::vec(inner, 0..4),
            any::<bool>(),
        )
            .prop_map(|(name, attributes, children, short)| Node::Element {
                name,
                attributes,
                children,
                short,
            })
    })
}

fn render(node: &Node, out: &mut String, log: &mut Vec<Vec<String>>) {
    match node {
        Node::Text(text) => out.push_str(text),
        Node::Comment(text) => out.push_str(&format!("<!--{text}-->")),
        Node::Pi(text) => out.push_str(&format!("<?{text}?>")),
        Node::Cdata(text) => out.push_str(&format!("<![CDATA[{text}]]>")),
        Node::Element {
            name,
            attributes,
            children,
            short,
        } => {
            log.push(attributes.iter().map(|(name, _, _)| name.clone()).collect());
            out.push('<');
            out.push_str(name);
            for (attribute, quote, value) in attributes {
                out.push_str(&format!(" {attribute}={quote}{value}{quote}"));
            }
            if children.is_empty() && *short {
                out.push_str("/>");
            } else {
                out.push('>');
                for child in children {
                    render(child, out, log);
                }
                out.push_str(&format!("</{name}>"));
            }
        }
    }
}

/// A well-formed document: an optional BOM, declaration and doctype, then one element.
fn xml_doc() -> impl Strategy<Value = Doc> {
    (
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        name(),
        attributes(),
        prop::collection::vec(node(), 0..5),
    )
        .prop_map(
            |(bom, declaration, doctype, root, mut attributes, children)| {
                // `ns:tag` keeps a prefixed name in play; declaring the prefix makes the document
                // well-formed with namespaces too, as a strict parser reads it.
                attributes.push(("xmlns:ns".to_string(), '"', "urn:twaco:test".to_string()));
                let mut text = String::new();
                if declaration {
                    text.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
                }
                if doctype {
                    text.push_str("<!DOCTYPE Entities>\n");
                }
                let mut log = Vec::new();
                render(
                    &Node::Element {
                        name: root,
                        attributes,
                        children,
                        short: false,
                    },
                    &mut text,
                    &mut log,
                );
                text.push('\n');
                let mut bytes = Vec::new();
                if bom {
                    bytes.extend_from_slice(scan::UTF8_BOM);
                }
                bytes.extend_from_slice(text.as_bytes());
                Doc {
                    bytes,
                    attributes: log,
                }
            },
        )
}

fn arbitrary_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..160)
}

/// A document with a few flipped, inserted or dropped bytes, so the parsers see almost-valid input.
fn mutated_doc() -> impl Strategy<Value = Vec<u8>> {
    (
        xml_doc(),
        prop::collection::vec((0u8..3, any::<u16>(), any::<u8>()), 1..4),
    )
        .prop_map(|(doc, steps)| {
            let mut bytes = doc.bytes;
            for (kind, at, byte) in steps {
                if bytes.is_empty() {
                    break;
                }
                let at = usize::from(at) % bytes.len();
                match kind {
                    0 => bytes[at] = byte,
                    1 => bytes.insert(at, byte),
                    _ => bytes.truncate(at),
                }
            }
            bytes
        })
}

/// Fragments of the JavaScript a ThingWorx service holds, glued together without regard to sense.
fn script_soup() -> impl Strategy<Value = String> {
    let vocabulary = vec![
        "me",
        ".",
        "Things",
        "[",
        "]",
        "\"A\"",
        "'b'",
        "(",
        ")",
        "{",
        "}",
        "var",
        "x",
        "=",
        ";",
        ",",
        ":",
        "function",
        "return",
        "`t${x}`",
        "/re/g",
        "// c\n",
        "/* c */",
        "1",
        "+",
        "é",
        "if",
        "else",
        "for",
        "in",
        "new",
        "this",
        "=>",
        "?",
        "!",
        "&&",
        "tableName",
        "result",
        "{ a: 1 }",
    ];
    prop::collection::vec(prop::sample::select(vocabulary), 0..30).prop_map(|parts| parts.join(" "))
}

/// Non-overlapping edits over a document of `len` bytes: sorted cut points paired up, some pairs
/// empty (insertions), no two insertions at one point.
fn edits(len: usize) -> impl Strategy<Value = Vec<Edit>> {
    (
        prop::collection::vec(0..=len, 0..8),
        prop::collection::vec(prop::collection::vec(any::<u8>(), 0..4), 4),
    )
        .prop_map(|(mut points, replacements)| {
            points.sort_unstable();
            let mut out: Vec<Edit> = Vec::new();
            for (index, [from, to]) in points.as_chunks::<2>().0.iter().enumerate() {
                let span = Span::new(*from, *to);
                let coincident = span.is_empty()
                    && out
                        .last()
                        .is_some_and(|last| last.span.is_empty() && last.span.start == span.start);
                if !coincident {
                    out.push(Edit::new(
                        span,
                        replacements[index % replacements.len()].clone(),
                    ));
                }
            }
            out
        })
}

// ---- scan -------------------------------------------------------------------------------------

/// The tokens, in order, start at 0, each begin where the previous ended and the last ends at the
/// end of the input; every inner span sits inside its token; the markers are where they claim.
fn assert_tiles(src: &[u8], tokens: &[Token]) -> Result<(), TestCaseError> {
    let text = std::str::from_utf8(src).ok();
    let mut cursor = 0;
    for token in tokens {
        prop_assert_eq!(
            token.span.start,
            cursor,
            "a gap or an overlap before {:?}",
            token
        );
        prop_assert!(
            token.span.end > token.span.start,
            "an empty token {:?}",
            token
        );
        cursor = token.span.end;
        for inner in [token.name, token.inner] {
            if !inner.is_empty() {
                prop_assert!(
                    inner.start >= token.span.start && inner.end <= token.span.end,
                    "{:?}",
                    token
                );
            }
        }
        match token.kind {
            Kind::Start | Kind::End | Kind::Empty => {
                prop_assert!(!token.name.is_empty(), "a tag without a name {:?}", token)
            }
            Kind::Cdata => {
                let raw = token.span.of(src);
                prop_assert!(
                    raw.starts_with(b"<![CDATA[") && raw.ends_with(b"]]>"),
                    "{:?}",
                    token
                );
                prop_assert_eq!(
                    token.inner,
                    Span::new(token.span.start + 9, token.span.end - 3)
                );
            }
            _ => {}
        }
        if let Some(text) = text {
            prop_assert!(
                text.is_char_boundary(token.span.start) && text.is_char_boundary(token.span.end)
            );
        }
    }
    prop_assert_eq!(cursor, src.len(), "the tokens stop short of the end");
    Ok(())
}

proptest! {
    #![proptest_config(config())]

    /// Tiling: every well-formed document tokenises, and its tokens tile it exactly.
    #[test]
    fn tokens_tile_every_generated_document(doc in xml_doc()) {
        let tokens = scan::tokenize(&doc.bytes).expect("a generated document is well-formed");
        assert_tiles(&doc.bytes, &tokens)?;
    }

    /// Matching: each start tag's element ends at the end tag of the same name, and its span runs
    /// from the start tag to that end tag.
    #[test]
    fn every_start_tag_finds_its_own_end_tag(doc in xml_doc()) {
        let src = &doc.bytes;
        let tokens = scan::tokenize(src).unwrap();
        for (index, token) in tokens.iter().enumerate().filter(|(_, token)| token.kind == Kind::Start) {
            let end = scan::element_end_in(&tokens, src, index).expect("the generated document is balanced");
            prop_assert_eq!(tokens[end].kind, Kind::End);
            prop_assert_eq!(tokens[end].name.of(src), token.name.of(src));
            let span = scan::element_span(&tokens, index).unwrap();
            prop_assert_eq!(span, Span::new(token.span.start, tokens[end].span.end));
        }
    }

    /// Attributes: the generated names come back in order, every span lies inside its tag, a value
    /// span is the bytes between the quotes, and `attribute` agrees with `attributes`.
    #[test]
    fn attributes_come_back_in_order_inside_their_quotes(doc in xml_doc()) {
        let src = &doc.bytes;
        let tokens = scan::tokenize(src).unwrap();
        let tags: Vec<&Token> = tokens.iter().filter(|t| matches!(t.kind, Kind::Start | Kind::Empty)).collect();
        prop_assert_eq!(tags.len(), doc.attributes.len());
        for (tag, expected) in tags.iter().zip(&doc.attributes) {
            let found = scan::attributes(src, tag).expect("quoted attributes scan");
            let names: Vec<String> = found.iter().map(|a| String::from_utf8_lossy(a.name.of(src)).into_owned()).collect();
            prop_assert_eq!(&names, expected);
            for attribute in &found {
                prop_assert!(attribute.name.start >= tag.span.start && attribute.value.end <= tag.span.end);
                let quote = src[attribute.value.start - 1];
                prop_assert!(quote == b'"' || quote == b'\'', "a value not inside quotes");
                prop_assert_eq!(src[attribute.value.end], quote);
            }
            if let (Some(first), Some(name)) = (found.first(), expected.first()) {
                prop_assert_eq!(scan::attribute(src, tag, name).unwrap(), Some(first.value));
            }
        }
    }

    /// No panic: the scanner answers any bytes with a result, and an accepted one still tiles.
    #[test]
    fn the_scanner_never_panics_and_what_it_accepts_tiles(bytes in prop_oneof![arbitrary_bytes(), mutated_doc()]) {
        if let Ok(tokens) = scan::tokenize(&bytes) {
            assert_tiles(&bytes, &tokens)?;
            for token in &tokens {
                let _ = scan::attributes(&bytes, token);
                let _ = scan::attribute(&bytes, token, "name");
            }
        }
        let _ = scan::scan(&bytes);
    }

    /// No panic: the readers of files twaco did not write refuse bad input instead of crashing.
    #[test]
    fn the_entity_readers_never_panic(bytes in prop_oneof![arbitrary_bytes(), mutated_doc()]) {
        let _ = entity::parse(&bytes);
        let _ = normalise::normalise(&bytes);
    }

    /// The script parser never panics, and whatever it accepts reports spans that fit the bytes it
    /// was given (a law about twaco's own facts, not about swc).
    #[test]
    fn script_facts_always_fit_the_bytes(source in prop_oneof![script_soup().prop_map(String::into_bytes), arbitrary_bytes()]) {
        if let Ok(facts) = script::parse(&source) {
            prop_assert_eq!(facts.verify_spans(&source), Ok(()));
        }
    }
}

// ---- splice -----------------------------------------------------------------------------------

/// The straightforward model: apply the edits from the last to the first on a copy.
fn model(src: &[u8], edits: &[Edit]) -> Vec<u8> {
    let mut ordered: Vec<&Edit> = edits.iter().collect();
    ordered.sort_by_key(|edit| (edit.span.start, edit.span.end));
    let mut out = src.to_vec();
    for edit in ordered.into_iter().rev() {
        out.splice(
            edit.span.start..edit.span.end,
            edit.replacement.iter().copied(),
        );
    }
    out
}

proptest! {
    #![proptest_config(config())]

    /// No edits is the identity, for any bytes.
    #[test]
    fn no_edits_is_the_identity(src in arbitrary_bytes()) {
        prop_assert_eq!(splice::splice(&src, &[]).unwrap(), src);
    }

    /// Replacing any set of non-overlapping spans with their own bytes changes nothing.
    #[test]
    fn replacing_spans_with_their_own_bytes_is_the_identity(
        (src, cuts) in arbitrary_bytes().prop_flat_map(|src| { let len = src.len(); (Just(src), edits(len)) })
    ) {
        let same: Vec<Edit> = cuts.iter().map(|edit| Edit::new(edit.span, edit.span.of(&src).to_vec())).collect();
        prop_assert_eq!(splice::splice(&src, &same).unwrap(), src);
    }

    /// The result is the model's, and its length is the input's, less what was removed, plus what
    /// was added.
    #[test]
    fn splice_equals_the_model_and_obeys_the_length_law(
        (src, cuts) in arbitrary_bytes().prop_flat_map(|src| { let len = src.len(); (Just(src), edits(len)) })
    ) {
        let out = splice::splice(&src, &cuts).unwrap();
        prop_assert_eq!(&out, &model(&src, &cuts));
        let removed: usize = cuts.iter().map(|edit| edit.span.len()).sum();
        let added: usize = cuts.iter().map(|edit| edit.replacement.len()).sum();
        prop_assert_eq!(out.len(), src.len() - removed + added);
    }

    /// The order the edits are passed in does not change the result.
    #[test]
    fn the_order_of_the_edits_does_not_matter(
        (src, cuts, shuffled) in arbitrary_bytes().prop_flat_map(|src| {
            let len = src.len();
            (Just(src), edits(len)).prop_flat_map(|(src, cuts)| { let shuffled = Just(cuts.clone()).prop_shuffle(); (Just(src), Just(cuts), shuffled) })
        })
    ) {
        prop_assert_eq!(splice::splice(&src, &cuts).unwrap(), splice::splice(&src, &shuffled).unwrap());
    }

    /// Two edits that share an interior byte are refused in either order.
    #[test]
    fn overlapping_edits_are_refused(
        (len, a_start, a_len, b_offset, b_len) in (2usize..40).prop_flat_map(|len| (Just(len), 0..len - 1, 1usize..8, 0usize..8, 1usize..8))
    ) {
        let src = vec![b'x'; len];
        let a_end = (a_start + a_len).min(len);
        prop_assume!(a_end > a_start);
        let b_start = a_start + b_offset % (a_end - a_start);
        let b_end = (b_start + b_len).min(len);
        prop_assume!(b_end > b_start);
        let first = Edit::new(Span::new(a_start, a_end), b"A".to_vec());
        let second = Edit::new(Span::new(b_start, b_end), b"B".to_vec());
        for edits in [[first.clone(), second.clone()], [second, first]] {
            let result = splice::splice(&src, &edits);
            prop_assert!(matches!(result, Err(SpliceError::Overlap { .. })), "{:?}", result);
        }
    }

    /// Two insertions at one point, and an edit outside the document, are refused.
    #[test]
    fn coincident_insertions_and_out_of_bounds_edits_are_refused(src in arbitrary_bytes(), at in 0usize..200, past in 1usize..20) {
        let at = at.min(src.len());
        let twice = [Edit::new(Span::new(at, at), b"1".to_vec()), Edit::new(Span::new(at, at), b"2".to_vec())];
        let coincident = splice::splice(&src, &twice);
        prop_assert!(matches!(coincident, Err(SpliceError::CoincidentInsert { .. })), "{:?}", coincident);
        let outside = [Edit::new(Span::new(src.len(), src.len() + past), b"x".to_vec())];
        let out_of_bounds = splice::splice(&src, &outside);
        prop_assert!(matches!(out_of_bounds, Err(SpliceError::OutOfBounds { .. })), "{:?}", out_of_bounds);
        if src.len() >= 2 {
            let backwards = [Edit::new(Span::new(src.len(), src.len() - 1), b"x".to_vec())];
            let result = splice::splice(&src, &backwards);
            prop_assert!(matches!(result, Err(SpliceError::OutOfBounds { .. })), "{:?}", result);
        }
    }

    /// Edits that touch, and an insertion at either end of a replaced span, are legal.
    #[test]
    fn touching_edits_and_insertions_at_the_ends_of_a_replacement_are_legal(len in 3usize..40, start in 0usize..20, width in 1usize..10) {
        let src = vec![b'x'; len];
        let start = start % (len - 1);
        let end = (start + width).min(len);
        prop_assume!(end > start);
        let replaced = Edit::new(Span::new(start, end), b"R".to_vec());
        let before = Edit::new(Span::new(start, start), b"<".to_vec());
        let after = Edit::new(Span::new(end, end), b">".to_vec());
        let out = splice::splice(&src, &[replaced.clone(), before.clone(), after.clone()]).unwrap();
        prop_assert_eq!(&out, &model(&src, &[replaced, before, after]));
        prop_assert!(out.iter().filter(|b| **b == b'R').count() == 1);
    }

    /// The pair: replacing CDATA payloads with their own bytes is the identity, and replacing them
    /// with new text leaves the document tokenising to the same shapes.
    #[test]
    fn rewriting_cdata_payloads_keeps_the_document_intact(doc in xml_doc(), replacement in "[a-zA-Z0-9 ]{0,10}") {
        let src = &doc.bytes;
        let tokens = scan::tokenize(src).unwrap();
        let inners: Vec<Span> = scan::cdata_sections(&tokens).iter().map(|t| t.inner).collect();
        let same: Vec<Edit> = inners.iter().map(|span| Edit::new(*span, span.of(src).to_vec())).collect();
        prop_assert_eq!(&splice::splice(src, &same).unwrap(), src);
        let changed: Vec<Edit> = inners.iter().map(|span| Edit::new(*span, replacement.clone().into_bytes())).collect();
        let out = splice::splice(src, &changed).unwrap();
        let after = scan::tokenize(&out).expect("new text inside CDATA cannot break the markup");
        let kinds = |tokens: &[Token]| tokens.iter().map(|t| t.kind).collect::<Vec<_>>();
        prop_assert_eq!(kinds(&after), kinds(&tokens));
        prop_assert!(strict_parse(&out).is_ok(), "{}", String::from_utf8_lossy(&out));
    }

    /// Every property above leans on the generator making well-formed documents; a strict parser
    /// that shares no code with twaco checks that it does.
    #[test]
    fn the_generated_documents_are_well_formed(doc in xml_doc()) {
        let parsed = strict_parse(&doc.bytes);
        prop_assert!(parsed.is_ok(), "{:?}\n{}", parsed.err(), String::from_utf8_lossy(&doc.bytes));
    }
}

/// roxmltree's reading of a document, DOCTYPE allowed. A BOM is not part of the text it takes.
fn strict_parse(bytes: &[u8]) -> Result<(), String> {
    let bytes = bytes.strip_prefix(scan::UTF8_BOM).unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    roxmltree::Document::parse_with_options(text, options)
        .map(|_| ())
        .map_err(|e| e.to_string())
}
