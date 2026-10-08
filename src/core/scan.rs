//! A lexical index of a ThingWorx entity document.
//!
//! This never rebuilds the document. It records where things *are*, as byte ranges, so an edit
//! can replace one range and leave every other byte exactly as it was found. Why that
//! matters: a parse-and-serialise round trip through any XML library loses the
//! declaration's spelling, attribute order, empty-element style and CDATA boundaries, and 160
//! committed entity files would all rewrite themselves on first run.
//!
//! **It fails closed.** Anything it cannot account for is an error, never a guessed span. A
//! wrong span silently corrupts a file, which is the one outcome worse than refusing to work.

/// A half-open byte range into the source document.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// The bytes this span covers, or `None` when it does not fit the document.
    ///
    /// Spans are public and can be constructed by a caller, so this cannot assume it is in
    /// range. Indexing directly would panic on a span built against a different document.
    pub fn try_of<'a>(&self, src: &'a [u8]) -> Option<&'a [u8]> {
        if self.start <= self.end && self.end <= src.len() {
            Some(&src[self.start..self.end])
        } else {
            None
        }
    }

    /// The bytes this span covers, empty when the span does not fit the document.
    pub fn of<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        self.try_of(src).unwrap_or(&[])
    }
}

/// What a token is. Enough to find things; not a parse tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// `<?xml ... ?>` or any other processing instruction.
    Pi,
    /// `<!-- ... -->`
    Comment,
    /// `<![CDATA[ ... ]]>`
    Cdata,
    /// `<!DOCTYPE ... >`, internal subset included.
    DocType,
    /// `<name ...>`
    Start,
    /// `</name>`
    End,
    /// `<name ... />`
    Empty,
    /// Anything between markup, including whitespace.
    Text,
}

#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub kind: Kind,
    /// The whole token, markers included.
    pub span: Span,
    /// The element name, for `Start`, `End` and `Empty`. Empty span otherwise.
    pub name: Span,
    /// The payload between `<![CDATA[` and `]]>`, for `Cdata`. Empty span otherwise.
    pub inner: Span,
}

/// One attribute's name and value, both as spans into the source tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub name: Span,
    pub value: Span,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    /// A construct was opened and never closed.
    #[error("unterminated {what} starting at byte {at}")]
    Unterminated { what: &'static str, at: usize },
    /// Markup this scanner refuses to guess at.
    #[error("malformed {what} at byte {at}")]
    Malformed { what: &'static str, at: usize },
    /// The document is not UTF-8, which twaco requires rather than guesses at.
    #[error("not valid UTF-8 at byte {at}")]
    NotUtf8 { at: usize },
    /// A byte-order mark for an encoding this tool does not handle.
    #[error("{what} encoding is not supported; entity XML must be UTF-8")]
    UnsupportedEncoding { what: &'static str },
}

/// A UTF-8 byte-order mark, which ThingWorx sometimes writes and which must be preserved.
pub const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// A scanned document: its bytes, whether it began with a BOM, and its tokens.
pub struct Scanned {
    pub has_bom: bool,
    pub tokens: Vec<Token>,
}

/// Tokenize a document.
///
/// The returned tokens tile the input exactly, BOM included: concatenating every span reproduces
/// the source byte for byte.
pub fn tokenize(src: &[u8]) -> Result<Vec<Token>, ScanError> {
    Ok(scan(src)?.tokens)
}

/// Tokenize a document, reporting its byte-order mark separately.
pub fn scan(src: &[u8]) -> Result<Scanned, ScanError> {
    reject_unsupported_encoding(src)?;
    if let Err(e) = std::str::from_utf8(src) {
        return Err(ScanError::NotUtf8 {
            at: e.valid_up_to(),
        });
    }

    let has_bom = src.starts_with(UTF8_BOM);
    let mut tokens = Vec::new();
    let mut i = 0usize;
    if has_bom {
        // Carried as Text so the tokens still tile the document and the BOM survives a splice.
        tokens.push(text_token(0, UTF8_BOM.len()));
        i = UTF8_BOM.len();
    }

    while i < src.len() {
        if src[i] == b'<' {
            let token = scan_markup(src, i)?;
            i = token.span.end;
            tokens.push(token);
        } else {
            let start = i;
            while i < src.len() && src[i] != b'<' {
                i += 1;
            }
            tokens.push(text_token(start, i));
        }
    }
    Ok(Scanned { has_bom, tokens })
}

fn reject_unsupported_encoding(src: &[u8]) -> Result<(), ScanError> {
    const CASES: [(&[u8], &str); 4] = [
        (&[0xFF, 0xFE, 0x00, 0x00], "UTF-32LE"),
        (&[0x00, 0x00, 0xFE, 0xFF], "UTF-32BE"),
        (&[0xFF, 0xFE], "UTF-16LE"),
        (&[0xFE, 0xFF], "UTF-16BE"),
    ];
    for (bom, name) in CASES {
        if src.starts_with(bom) {
            return Err(ScanError::UnsupportedEncoding { what: name });
        }
    }
    Ok(())
}

fn text_token(start: usize, end: usize) -> Token {
    Token {
        kind: Kind::Text,
        span: Span::new(start, end),
        name: Span::new(start, start),
        inner: Span::new(start, start),
    }
}

fn scan_markup(src: &[u8], start: usize) -> Result<Token, ScanError> {
    let empty = Span::new(start, start);
    if starts_with(src, start, b"<!--") {
        let end = find(src, start + 4, b"-->").ok_or(ScanError::Unterminated {
            what: "comment",
            at: start,
        })?;
        return Ok(Token {
            kind: Kind::Comment,
            span: Span::new(start, end + 3),
            name: empty,
            inner: empty,
        });
    }
    if starts_with(src, start, b"<![CDATA[") {
        let body = start + 9;
        let end = find(src, body, b"]]>").ok_or(ScanError::Unterminated {
            what: "CDATA section",
            at: start,
        })?;
        return Ok(Token {
            kind: Kind::Cdata,
            span: Span::new(start, end + 3),
            name: empty,
            inner: Span::new(body, end),
        });
    }
    if starts_with(src, start, b"<!DOCTYPE") {
        let end = scan_doctype(src, start)?;
        return Ok(Token {
            kind: Kind::DocType,
            span: Span::new(start, end),
            name: empty,
            inner: empty,
        });
    }
    if starts_with(src, start, b"<!") {
        // `<!ENTITY`, `<!ELEMENT` and friends only appear inside a DOCTYPE internal subset,
        // which scan_doctype consumes whole. One out here is markup this tool does not model,
        // and guessing it is an element would name it `!ENTITY`.
        return Err(ScanError::Malformed {
            what: "declaration outside a DOCTYPE",
            at: start,
        });
    }
    if starts_with(src, start, b"<?") {
        let end = find(src, start + 2, b"?>").ok_or(ScanError::Unterminated {
            what: "processing instruction",
            at: start,
        })?;
        return Ok(Token {
            kind: Kind::Pi,
            span: Span::new(start, end + 2),
            name: empty,
            inner: empty,
        });
    }

    let closing = starts_with(src, start, b"</");
    let name_start = start + if closing { 2 } else { 1 };
    let mut n = name_start;
    while n < src.len() && !is_name_end(src[n]) {
        n += 1;
    }
    let name = Span::new(name_start, n);
    if !is_valid_name(&src[name_start..n.min(src.len())]) {
        return Err(ScanError::Malformed {
            what: "element name",
            at: start,
        });
    }
    let end = scan_tag(src, n, start)?;
    let self_closing = end >= 2 && src[end - 2] == b'/';
    let kind = if closing {
        if self_closing {
            return Err(ScanError::Malformed {
                what: "end tag written as self-closing",
                at: start,
            });
        }
        Kind::End
    } else if self_closing {
        Kind::Empty
    } else {
        Kind::Start
    };
    Ok(Token {
        kind,
        span: Span::new(start, end),
        name,
        inner: empty,
    })
}

/// Consume a DOCTYPE, including an internal subset.
///
/// `<!DOCTYPE root [<!ELEMENT root (#PCDATA)>]>` closes at the final `>`, not at the one inside
/// the subset. Bracket depth is what tells them apart.
fn scan_doctype(src: &[u8], start: usize) -> Result<usize, ScanError> {
    let mut i = start + 9;
    let mut depth = 0usize;
    let mut quote = 0u8;
    while i < src.len() {
        let b = src[i];
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
        } else {
            match b {
                b'"' | b'\'' => quote = b,
                b'[' => depth += 1,
                b']' => depth = depth.saturating_sub(1),
                b'>' if depth == 0 => return Ok(i + 1),
                _ => {}
            }
        }
        i += 1;
    }
    Err(ScanError::Unterminated {
        what: "doctype",
        at: start,
    })
}

/// Advance to just past the `>` that closes a tag.
///
/// Attribute values are quoted and may contain `>`, which is legal. They may not contain a
/// literal `<`: that is illegal XML, and accepting it would swallow the rest of the document
/// into one enormous token whose attribute values are nonsense.
fn scan_tag(src: &[u8], from: usize, tag_start: usize) -> Result<usize, ScanError> {
    let mut i = from;
    let mut quote = 0u8;
    while i < src.len() {
        let b = src[i];
        if quote != 0 {
            if b == quote {
                quote = 0;
            } else if b == b'<' {
                return Err(ScanError::Malformed {
                    what: "'<' inside an attribute value",
                    at: i,
                });
            }
        } else if b == b'"' || b == b'\'' {
            quote = b;
        } else if b == b'<' {
            return Err(ScanError::Malformed {
                what: "'<' inside a tag",
                at: i,
            });
        } else if b == b'>' {
            return Ok(i + 1);
        }
        i += 1;
    }
    Err(ScanError::Unterminated {
        what: "tag",
        at: tag_start,
    })
}

fn is_name_end(b: u8) -> bool {
    b.is_ascii_whitespace() || b == b'>' || b == b'/'
}

/// Whether a byte slice is a usable XML name.
///
/// Deliberately permissive about non-ASCII, which XML allows and ThingWorx does not use, and
/// strict about the ASCII punctuation that would indicate this is not an element at all.
fn is_valid_name(name: &[u8]) -> bool {
    let Some(&first) = name.first() else {
        return false;
    };
    let start_ok = first.is_ascii_alphabetic() || first == b'_' || first == b':' || first >= 0x80;
    if !start_ok {
        return false;
    }
    name[1..]
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b':' | b'-' | b'.') || b >= 0x80)
}

fn starts_with(src: &[u8], at: usize, prefix: &[u8]) -> bool {
    src.len() >= at + prefix.len() && &src[at..at + prefix.len()] == prefix
}

fn find(src: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || src.len() < needle.len() || from > src.len() - needle.len() {
        return None;
    }
    let last = src.len() - needle.len();
    (from..=last).find(|&i| &src[i..i + needle.len()] == needle)
}

/// Every CDATA section in the document, in order.
pub fn cdata_sections(tokens: &[Token]) -> Vec<Token> {
    tokens
        .iter()
        .copied()
        .filter(|t| t.kind == Kind::Cdata)
        .collect()
}

/// The value of one attribute on a tag token, as a span into the source.
///
/// Returns the span of the value between its quotes, so an edit can replace the value without
/// disturbing the quoting style or the order of the attributes around it. An attribute whose
/// value is unquoted or unterminated is an error rather than a guess, because returning a span
/// that runs past the tag would corrupt the document on the next write.
pub fn attribute(src: &[u8], tag: &Token, wanted: &str) -> Result<Option<Span>, ScanError> {
    Ok(attributes(src, tag)?
        .into_iter()
        .find(|attribute| attribute.name.of(src) == wanted.as_bytes())
        .map(|attribute| attribute.value))
}

/// Every quoted attribute on a start or empty tag, in source order.
///
/// Normalisation needs the complete set so it can sort attributes structurally. Keeping the
/// lexical scanner as the one parser for attribute boundaries avoids a second, subtly different
/// implementation that accepts malformed input the splice engine would refuse.
pub fn attributes(src: &[u8], tag: &Token) -> Result<Vec<Attribute>, ScanError> {
    if !matches!(tag.kind, Kind::Start | Kind::Empty) {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    let limit = tag.span.end.min(src.len());
    let mut i = tag.name.end.min(limit);
    while i < limit {
        while i < limit && src[i].is_ascii_whitespace() {
            i += 1;
        }
        // The tag's own terminator, not an attribute.
        if i >= limit || src[i] == b'>' || src[i] == b'/' {
            return Ok(found);
        }
        let key_start = i;
        while i < limit
            && src[i] != b'='
            && !src[i].is_ascii_whitespace()
            && src[i] != b'>'
            && src[i] != b'/'
        {
            i += 1;
        }
        let key = &src[key_start..i];
        if key.is_empty() {
            return Err(ScanError::Malformed {
                what: "attribute name",
                at: key_start,
            });
        }
        while i < limit && src[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= limit || src[i] != b'=' {
            return Err(ScanError::Malformed {
                what: "attribute without a value",
                at: key_start,
            });
        }
        i += 1;
        while i < limit && src[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= limit || (src[i] != b'"' && src[i] != b'\'') {
            return Err(ScanError::Malformed {
                what: "unquoted attribute value",
                at: i.min(limit),
            });
        }
        let quote = src[i];
        i += 1;
        let value_start = i;
        while i < limit && src[i] != quote {
            i += 1;
        }
        if i >= limit {
            return Err(ScanError::Unterminated {
                what: "attribute value",
                at: value_start,
            });
        }
        found.push(Attribute {
            name: Span::new(key_start, key_start + key.len()),
            value: Span::new(value_start, i),
        });
        i += 1;
    }
    Ok(found)
}

/// `value` written inside a double-quoted attribute: `&`, `<`, `>` and `"` escaped, so the value
/// is read back as given and cannot end the attribute.
pub fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Whether `span` holds only CDATA sections and text, the text only whitespace when
/// `whitespace_only`: nothing that writing the whole span would silently drop. A comment, a
/// processing instruction or an element between two CDATA sections fails it.
pub fn only_cdata_and_text(
    tokens: &[Token],
    src: &[u8],
    span: Span,
    whitespace_only: bool,
) -> bool {
    tokens
        .iter()
        .filter(|token| token.span.start >= span.start && token.span.end <= span.end)
        .all(|token| match token.kind {
            Kind::Cdata => true,
            Kind::Text => {
                !whitespace_only || token.span.of(src).iter().all(u8::is_ascii_whitespace)
            }
            _ => false,
        })
}

/// Render a payload for a CDATA section, splitting it if it contains the terminator.
///
/// `]]>` cannot appear inside one CDATA section, so a payload containing it is written as two
/// adjacent sections split across the sequence. That decision is made once, here, and
/// tested. The corpus contains no such payload today, which is exactly why it
/// would otherwise be discovered by corrupting someone's file.
pub fn render_cdata(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 16);
    out.extend_from_slice(b"<![CDATA[");
    let mut rest = payload;
    while let Some(at) = rest.windows(3).position(|w| w == b"]]>") {
        // Split between "]]" and ">" so neither half contains the sequence.
        out.extend_from_slice(&rest[..at + 2]);
        out.extend_from_slice(b"]]><![CDATA[");
        rest = &rest[at + 2..];
    }
    out.extend_from_slice(rest);
    out.extend_from_slice(b"]]>");
    out
}

/// The index of the token that closes the element opening at `start`.
///
/// An empty element closes itself. Anything that is not a tag has no range.
pub fn element_end(tokens: &[Token], start: usize) -> Option<usize> {
    element_end_in(tokens, &[], start)
}

/// As `element_end`, but checking that the closing tag carries the same name.
///
/// Depth alone accepts `<a><b></a></b>`, which is not well-formed and whose spans would be
/// wrong. Pass the document so the names can be compared; pass an empty slice to skip the check
/// only where the caller has already established well-formedness.
pub fn element_end_in(tokens: &[Token], src: &[u8], start: usize) -> Option<usize> {
    let opening = tokens.get(start)?;
    match opening.kind {
        Kind::Empty => return Some(start),
        Kind::Start => {}
        _ => return None,
    }
    let want = opening.name;
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        match token.kind {
            Kind::Start => depth += 1,
            Kind::End => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    if !src.is_empty() && token.name.of(src) != want.of(src) {
                        return None;
                    }
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// The span covering a whole element, opening tag through closing tag.
pub fn element_span(tokens: &[Token], start: usize) -> Option<Span> {
    let end = element_end(tokens, start)?;
    Some(Span::new(tokens[start].span.start, tokens[end].span.end))
}

/// The index of the first tag named `name` at or after `from`, within `limit`.
pub fn find_tag(
    tokens: &[Token],
    src: &[u8],
    name: &str,
    from: usize,
    limit: usize,
) -> Option<usize> {
    (from..limit.min(tokens.len())).find(|&i| {
        matches!(tokens[i].kind, Kind::Start | Kind::Empty)
            && tokens[i].name.of(src) == name.as_bytes()
    })
}

/// Every tag named `name` strictly inside the element opening at `parent`.
pub fn tags_within(tokens: &[Token], src: &[u8], name: &str, parent: usize) -> Vec<usize> {
    let Some(end) = element_end(tokens, parent) else {
        return Vec::new();
    };
    (parent + 1..end)
        .filter(|&i| {
            matches!(tokens[i].kind, Kind::Start | Kind::Empty)
                && tokens[i].name.of(src) == name.as_bytes()
        })
        .collect()
}

/// Tags named `name` that are direct children of the element opening at `parent`.
///
/// Unlike `tags_within`, this does not descend. A ThingTemplate can carry an inline ThingShape
/// with its own `ServiceDefinitions`, and treating that as the template's own merges two
/// entities' services into one sidecar tree.
pub fn child_tags(tokens: &[Token], src: &[u8], name: &str, parent: usize) -> Vec<usize> {
    let Some(end) = element_end(tokens, parent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut index = parent + 1;
    while index < end {
        match tokens[index].kind {
            Kind::Start => {
                if tokens[index].name.of(src) == name.as_bytes() {
                    out.push(index);
                }
                // Skip the whole subtree: only direct children count.
                index = element_end(tokens, index).map_or(end, |e| e + 1);
            }
            Kind::Empty => {
                if tokens[index].name.of(src) == name.as_bytes() {
                    out.push(index);
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
    out
}

/// Expand the XML entity references a parser would resolve.
///
/// Attribute values reach this scanner as raw bytes, so `name="A&amp;B"` is literally `A&amp;B`
/// until something decodes it. A name compared or written back undecoded is the wrong name.
pub fn decode_entities(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(semi) = tail.find(';').filter(|&s| s <= 12) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..semi];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => numeric_entity(name),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn numeric_entity(name: &str) -> Option<char> {
    let digits = name.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    char::from_u32(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &[u8]) -> Vec<Kind> {
        tokenize(src).unwrap().iter().map(|t| t.kind).collect()
    }

    /// Expected offsets are computed independently of the scanner. Tiling alone is tautological.
    #[test]
    fn spans_are_exact() {
        let src = b"<a b=\"1\">hi</a>";
        let t = tokenize(src).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].kind, Kind::Start);
        assert_eq!(t[0].span, Span::new(0, 9));
        assert_eq!(t[0].name.of(src), b"a");
        assert_eq!(t[1].kind, Kind::Text);
        assert_eq!(t[1].span.of(src), b"hi");
        assert_eq!(t[2].kind, Kind::End);
        assert_eq!(t[2].span, Span::new(11, 15));
        assert_eq!(t[2].name.of(src), b"a");
    }

    #[test]
    fn a_doctype_with_an_internal_subset_is_one_token() {
        let src = b"<!DOCTYPE root [<!ELEMENT root (#PCDATA)>]><root/>";
        let t = tokenize(src).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].kind, Kind::DocType);
        assert_eq!(
            t[0].span.of(src),
            b"<!DOCTYPE root [<!ELEMENT root (#PCDATA)>]>"
        );
        assert_eq!(t[1].kind, Kind::Empty);
    }

    #[test]
    fn a_quoted_gt_does_not_close_a_tag() {
        let src = b"<a t=\"x>y\">z</a>";
        let t = tokenize(src).unwrap();
        assert_eq!(t[0].span.of(src), b"<a t=\"x>y\">");
        assert_eq!(attribute(src, &t[0], "t").unwrap().unwrap().of(src), b"x>y");
    }

    #[test]
    fn cdata_payload_is_exact_and_may_be_empty() {
        let src = b"<c><![CDATA[]]></c>";
        let t = tokenize(src).unwrap();
        let cd = t.iter().find(|t| t.kind == Kind::Cdata).unwrap();
        assert!(cd.inner.is_empty());
        assert_eq!(cd.span.of(src), b"<![CDATA[]]>");
    }

    #[test]
    fn markup_that_cannot_be_accounted_for_is_an_error() {
        for bad in [
            &b"<>"[..],
            &b"<!ENTITY x \"y\">"[..],
            &b"<a?b>"[..],
            &b"<a t=\"oops<b>\">"[..],
            &b"<a"[..],
            &b"<!-- unterminated"[..],
            &b"<![CDATA[ unterminated"[..],
            &b"</a/>"[..],
        ] {
            assert!(
                tokenize(bad).is_err(),
                "expected an error for {:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn names_with_colons_dots_and_dashes_are_accepted() {
        assert_eq!(kinds(b"<ns:a.b-c/>"), vec![Kind::Empty]);
    }

    #[test]
    fn a_utf8_bom_is_preserved_and_reported() {
        let mut src = UTF8_BOM.to_vec();
        src.extend_from_slice(b"<a/>");
        let scanned = scan(&src).unwrap();
        assert!(scanned.has_bom);
        let rebuilt: Vec<u8> = scanned
            .tokens
            .iter()
            .flat_map(|t| t.span.of(&src).to_vec())
            .collect();
        assert_eq!(rebuilt, src);
    }

    #[test]
    fn utf16_is_refused_rather_than_guessed_at() {
        let src = [0xFF, 0xFE, b'<', 0x00];
        assert!(matches!(
            scan(&src),
            Err(ScanError::UnsupportedEncoding { .. })
        ));
    }

    #[test]
    fn invalid_utf8_is_refused() {
        let src = [b'<', b'a', b'/', b'>', 0xC3, 0x28];
        assert!(matches!(scan(&src), Err(ScanError::NotUtf8 { .. })));
    }

    #[test]
    fn an_unterminated_attribute_value_is_an_error_not_a_span() {
        // Reachable only through a hand-built token, which the public fields permit.
        let src = b"<a t=\"unterminated>";
        let tag = Token {
            kind: Kind::Start,
            span: Span::new(0, src.len()),
            name: Span::new(1, 2),
            inner: Span::new(0, 0),
        };
        assert!(attribute(src, &tag, "t").is_err());
    }

    #[test]
    fn a_missing_attribute_is_none_not_an_error() {
        let src = b"<a t=\"1\"/>";
        let t = tokenize(src).unwrap();
        assert_eq!(attribute(src, &t[0], "absent").unwrap(), None);
    }

    #[test]
    fn a_cdata_payload_containing_the_terminator_is_split() {
        let rendered = render_cdata(b"before ]]> after");
        let text = String::from_utf8(rendered.clone()).unwrap();
        assert_eq!(text, "<![CDATA[before ]]]]><![CDATA[> after]]>");
        // Re-scanning gives back the original payload across the two sections.
        let tokens = tokenize(&rendered).unwrap();
        let joined: Vec<u8> = tokens
            .iter()
            .filter(|t| t.kind == Kind::Cdata)
            .flat_map(|t| t.inner.of(&rendered).to_vec())
            .collect();
        assert_eq!(joined, b"before ]]> after");
    }

    #[test]
    fn an_out_of_range_span_yields_nothing_rather_than_panicking() {
        assert_eq!(Span::new(0, 99).try_of(b"short"), None);
        assert_eq!(Span::new(0, 99).of(b"short"), b"");
    }
}
