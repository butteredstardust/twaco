use super::super::{refs, scan, splice};
use super::findings::{excerpt, span_text, Finding, Place, XmlPass};

/// Finds and plans a name replacement in one UTF-8 XML document.
///
/// Attributes and plain text are values and use [`refs::find`]; CDATA is a blob and uses
/// [`refs::find_in_text`]. Only Exact and Embedded hits become edits. The scanner's malformed or
/// unsupported-input errors are returned unchanged. This function refuses to decode entities,
/// inspect comments or processing instructions, perform file I/O, or rebuild markup.
pub fn scan_xml(
    src: &[u8],
    name: &str,
    mode: refs::Mode,
    new: &str,
) -> Result<XmlPass, scan::ScanError> {
    debug_assert!(
        refs::validate_new_name(new).is_ok(),
        "scan_xml requires a validated replacement name"
    );
    let tokens = scan::tokenize(src)?;
    let mut elements = Vec::<String>::new();
    let mut pass = XmlPass::new(src);

    for token in &tokens {
        match token.kind {
            scan::Kind::Start | scan::Kind::Empty => {
                let element = span_text(src, token.name).to_owned();
                for attribute in scan::attributes(src, token)? {
                    let attribute_name = span_text(src, attribute.name).to_owned();
                    // `Entities > <Collection> > <Entity name=...>`: the root element's name is the
                    // entity's own name, wherever the entity is of a kind this list has never heard of.
                    let place = if attribute_name == "name" && elements.len() == 2 {
                        Place::EntityName {
                            element: element.clone(),
                        }
                    } else {
                        Place::Attribute {
                            element: element.clone(),
                            attribute: attribute_name,
                        }
                    };
                    inspect(
                        src,
                        attribute.value,
                        name,
                        mode,
                        new,
                        place,
                        false,
                        &mut pass,
                    );
                }
                if token.kind == scan::Kind::Start {
                    elements.push(element);
                }
            }
            scan::Kind::End => {
                // The scanner tokenises tags without pairing them; a text node's owner is only
                // trustworthy when every end tag closes the element that is open.
                match elements.pop() {
                    Some(open) if open == span_text(src, token.name) => {}
                    _ => {
                        return Err(scan::ScanError::Malformed {
                            what: "end tag that does not close the open element",
                            at: token.span.start,
                        })
                    }
                }
            }
            scan::Kind::Text => {
                let value = span_text(src, token.span);
                if !value.trim().is_empty() {
                    if let Some(element) = elements.last() {
                        inspect(
                            src,
                            token.span,
                            name,
                            mode,
                            new,
                            Place::Text {
                                element: element.clone(),
                            },
                            false,
                            &mut pass,
                        );
                    }
                }
            }
            scan::Kind::Cdata => {
                if let Some(element) = elements.last() {
                    inspect(
                        src,
                        token.inner,
                        name,
                        mode,
                        new,
                        Place::Cdata {
                            element: element.clone(),
                        },
                        true,
                        &mut pass,
                    );
                }
            }
            scan::Kind::Pi | scan::Kind::Comment | scan::Kind::DocType => {}
        }
    }
    Ok(pass)
}

/// Finds and plans a name replacement in one non-XML UTF-8 file.
///
/// The whole file is treated as a text blob, so quoted values and qualified embedded names use
/// the same rules as script CDATA. Non-UTF-8 input is refused and no file I/O is performed.
pub fn scan_text(
    src: &[u8],
    name: &str,
    mode: refs::Mode,
    new: &str,
) -> Result<XmlPass, std::str::Utf8Error> {
    debug_assert!(
        refs::validate_new_name(new).is_ok(),
        "scan_text requires a validated replacement name"
    );
    std::str::from_utf8(src)?;
    let mut pass = XmlPass::new(src);
    inspect(
        src,
        scan::Span::new(0, src.len()),
        name,
        mode,
        new,
        Place::File,
        true,
        &mut pass,
    );
    Ok(pass)
}

#[allow(clippy::too_many_arguments)]
fn inspect(
    src: &[u8],
    span: scan::Span,
    name: &str,
    mode: refs::Mode,
    new: &str,
    place: Place,
    blob: bool,
    pass: &mut XmlPass,
) {
    let value = span_text(src, span);
    let hits = if blob {
        refs::find_in_text(value, name, mode)
    } else {
        refs::find(value, name, mode)
    };
    for hit in hits {
        let absolute = scan::Span::new(span.start + hit.start, span.start + hit.end);
        let mut tier = hit.tier;
        // An unqualified name can be a common word (`Node`, `Network`): a value or a quoted string
        // that merely equals it is not evidence of a reference. Only the places that are known to
        // name an entity apply; everything else is a finding for a person.
        if mode == refs::Mode::Entity && !refs::is_qualified(name) {
            if follows_derived_prefix(value, hit.start) {
                // `Things_<name>` is an id built from the entity's name, inside a larger blob or not.
                tier = refs::Tier::Exact;
            } else if tier == refs::Tier::Exact && !names_an_entity_here(&place, value, hit.start) {
                tier = refs::Tier::Review;
            }
        }
        let applied = matches!(tier, refs::Tier::Exact | refs::Tier::Embedded);
        pass.findings.push(Finding {
            place: place.clone(),
            tier,
            line: pass.line_of(absolute.start),
            excerpt: excerpt(value, hit.start, blob),
            applied,
        });
        if applied {
            pass.edits
                .push(splice::Edit::new(absolute, new.as_bytes().to_vec()));
        }
    }
}

/// Whether the hit sits right after a mashup-derived id prefix such as `Things_`: that id is built
/// from an entity name, so it is a reference whatever the name looks like.
fn follows_derived_prefix(value: &str, start: usize) -> bool {
    refs::DERIVED_PREFIXES
        .iter()
        .any(|prefix| value[..start].ends_with(prefix))
}

/// Whether an exact, unqualified hit is in a place that holds an entity's name: an attribute that
/// is documented to, or a script literal used as
/// the index of an entity collection (`Things["Name"]`).
fn names_an_entity_here(place: &Place, value: &str, start: usize) -> bool {
    match place {
        Place::Attribute { element, attribute } => match attribute.as_str() {
            "projectName"
            | "thingTemplate"
            | "baseThingTemplate"
            | "valueStream"
            | "inheritedValueStream"
            | "aspect.dataShape"
            | "aspect.thingShape"
            | "aspect.thingTemplate"
            | "dataShapeName"
            | "source" => true,
            "name" => matches!(
                element.as_str(),
                "ImplementedShape" | "Principal" | "Member" | "OrganizationalUnit"
            ),
            "from" | "to" => element == "Connection",
            _ => false,
        },
        Place::EntityName { .. } => true,
        Place::Cdata { .. } | Place::File => is_entity_collection_index(value, start),
        _ => false,
    }
}

/// `Things["Name"]`, `ThingTemplates['Name']` and the other collection lookups: the literal that
/// starts at `start` (just after its opening quote) is an entity's name.
fn is_entity_collection_index(value: &str, start: usize) -> bool {
    let before = &value.as_bytes()[..start];
    let Some(quote) = before.last() else {
        return false;
    };
    if !matches!(quote, b'"' | b'\'' | b'`') {
        return false;
    }
    let mut at = before.len() - 1;
    while at > 0 && before[at - 1].is_ascii_whitespace() {
        at -= 1;
    }
    if at == 0 || before[at - 1] != b'[' {
        return false;
    }
    at -= 1;
    while at > 0 && before[at - 1].is_ascii_whitespace() {
        at -= 1;
    }
    let end = at;
    while at > 0 && (before[at - 1].is_ascii_alphanumeric() || before[at - 1] == b'_') {
        at -= 1;
    }
    matches!(
        &before[at..end],
        b"Things"
            | b"ThingTemplates"
            | b"ThingShapes"
            | b"DataShapes"
            | b"Mashups"
            | b"Groups"
            | b"Users"
            | b"Organizations"
            | b"Projects"
            | b"StyleThemes"
            | b"MediaEntities"
            | b"Networks"
    )
}
