use super::super::{refs, scan, splice};

/// The XML location containing a name occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A hit in a non-XML file scanned as one text blob.
    File,
    /// A raw quoted attribute value on a start or empty tag.
    Attribute { element: String, attribute: String },
    /// The `name` attribute of an entity's root element.
    EntityName { element: String },
    /// A non-whitespace text node owned by its open element.
    Text { element: String },
    /// A CDATA payload owned by its open element.
    Cdata { element: String },
    /// A DataShape field declaration, either on the shape or in a configuration table copy.
    FieldDefinition { element: String },
    /// A direct child of a configuration-table row.
    RowElement { table: String },
    /// A service definition or implementation name.
    Service { element: String },
    /// A service call or ambiguous service-name use in JavaScript.
    Script,
    /// A service identity in mashup content JSON.
    Mashup { key: String, context: String },
    /// A service identity in twaco.toml.
    Config { key: String },
    /// A configuration-table definition, instance or script selector.
    Table { element: String },
    /// A service input declaration.
    Parameter { service: String },
    /// A property read or write in a script.
    Property,
}

/// The reason attached to every mention in a script the parser refused.
pub(super) const UNPARSED: &str = "script could not be parsed; left for review";

/// A script the parser refused is never edited. This reports every whole-word occurrence of
/// `name` in it, wherever it stands, so that a person decides.
pub(crate) fn lexical_mentions(text: &[u8], name: &str) -> Vec<scan::Span> {
    let needle = name.as_bytes();
    let word = |byte: &u8| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'$');
    if needle.is_empty() {
        return Vec::new();
    }
    text.windows(needle.len())
        .enumerate()
        .filter(|(start, window)| {
            let end = start + needle.len();
            *window == needle
                && !start
                    .checked_sub(1)
                    .is_some_and(|before| word(&text[before]))
                && !text.get(end).is_some_and(word)
        })
        .map(|(start, _)| scan::Span::new(start, start + needle.len()))
        .collect()
}

pub(crate) fn add_review_reason(
    src: &[u8],
    span: scan::Span,
    place: Place,
    reason: &str,
    pass: &mut XmlPass,
) {
    pass.findings.push(Finding {
        place,
        tier: refs::Tier::Review,
        line: pass.line_of(span.start),
        excerpt: format!(
            "{} ({reason})",
            excerpt(
                span_text(src, scan::Span::new(0, src.len())),
                span.start,
                true
            )
        ),
        applied: false,
    });
}

pub(super) fn add_table_edit(
    src: &[u8],
    span: scan::Span,
    new: &str,
    place: Place,
    pass: &mut XmlPass,
) {
    pass.findings.push(Finding {
        place,
        tier: refs::Tier::Exact,
        line: pass.line_of(span.start),
        excerpt: excerpt(span_text(src, span), 0, false),
        applied: true,
    });
    pass.edits
        .push(splice::Edit::new(span, new.as_bytes().to_vec()));
}

pub(crate) fn add_service_edit(
    src: &[u8],
    span: scan::Span,
    new: &str,
    place: Place,
    pass: &mut XmlPass,
) {
    pass.findings.push(Finding {
        place,
        tier: refs::Tier::Exact,
        line: pass.line_of(span.start),
        excerpt: excerpt(
            span_text(src, scan::Span::new(0, src.len())),
            span.start,
            true,
        ),
        applied: true,
    });
    pass.edits
        .push(splice::Edit::new(span, new.as_bytes().to_vec()));
}

pub(crate) fn add_review(src: &[u8], span: scan::Span, place: Place, pass: &mut XmlPass) {
    pass.findings.push(Finding {
        place,
        tier: refs::Tier::Review,
        line: pass.line_of(span.start),
        excerpt: excerpt(
            span_text(src, scan::Span::new(0, src.len())),
            span.start,
            true,
        ),
        applied: false,
    });
}

/// Every whole-token mention of `old` in a script or JSON file as a Review finding, with no edit:
/// what a person must look at after a field rename, because the field is read by name.
pub fn review_mentions(src: &[u8], old: &str) -> Result<XmlPass, std::str::Utf8Error> {
    let text = std::str::from_utf8(src)?;
    let mut pass = XmlPass::new(src);
    // A field is read as `row.old`, `{ old: 1 }`, `["old"]` and `'old'`: unlike an entity name, a
    // preceding dot is the common case, so only identifier characters bound the match.
    let identifier = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$';
    for (start, _) in text.match_indices(old) {
        let end = start + old.len();
        let before_ok = start == 0 || !identifier(src[start - 1]);
        let after_ok = end == src.len() || !identifier(src[end]);
        if before_ok && after_ok {
            add_review(src, scan::Span::new(start, end), Place::File, &mut pass);
        }
    }
    Ok(pass)
}

/// Represents a generated sidecar replacement as one exact finding and one whole-file edit.
pub fn replace_file(src: &[u8], replacement: Vec<u8>, excerpt: &str) -> XmlPass {
    // A regenerated sidecar is written with LF; a file that was committed with CRLF keeps its
    // style, so renaming a field does not rewrite every line ending in it.
    let replacement = if src.windows(2).any(|pair| pair == [13, 10]) && !replacement.contains(&13) {
        let mut converted = Vec::with_capacity(replacement.len() + replacement.len() / 20);
        for byte in replacement {
            if byte == 10 {
                converted.push(13);
            }
            converted.push(byte);
        }
        converted
    } else {
        replacement
    };
    let mut pass = XmlPass::new(src);
    pass.findings.push(Finding {
        place: Place::File,
        tier: refs::Tier::Exact,
        line: 1,
        excerpt: excerpt.to_string(),
        applied: true,
    });
    pass.edits.push(splice::Edit::new(
        scan::Span::new(0, src.len()),
        replacement,
    ));
    pass
}

pub(crate) fn add_field_edit(
    src: &[u8],
    span: scan::Span,
    new: &str,
    place: Place,
    pass: &mut XmlPass,
) {
    pass.findings.push(Finding {
        place,
        tier: refs::Tier::Exact,
        line: pass.line_of(span.start),
        excerpt: String::from_utf8_lossy(span.of(src)).into_owned(),
        applied: true,
    });
    pass.edits
        .push(splice::Edit::new(span, new.as_bytes().to_vec()));
}

/// One occurrence reported by the XML pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub place: Place,
    pub tier: refs::Tier,
    /// One-based source line containing the first byte of the hit.
    pub line: usize,
    /// A trimmed value or CDATA line around the hit, limited to 120 characters.
    pub excerpt: String,
    /// Whether this tier produced an edit.
    pub applied: bool,
}

/// Ordered byte edits and all applied-or-review findings for one document.
#[derive(Debug)]
pub struct XmlPass {
    pub edits: Vec<splice::Edit>,
    pub findings: Vec<Finding>,
    /// Byte offset of every line feed, so a hit's line is a binary search rather than a rescan of
    /// everything before it.
    newlines: Vec<usize>,
}

impl XmlPass {
    /// An empty pass over `src`, for callers that add their own edits.
    pub(crate) fn new_for(src: &[u8]) -> Self {
        Self::new(src)
    }

    pub(super) fn new(src: &[u8]) -> Self {
        let newlines = src
            .iter()
            .enumerate()
            .filter_map(|(at, &byte)| (byte == b'\n').then_some(at))
            .collect();
        XmlPass {
            edits: Vec::new(),
            findings: Vec::new(),
            newlines,
        }
    }

    pub(super) fn line_of(&self, at: usize) -> usize {
        1 + self.newlines.partition_point(|&newline| newline < at)
    }
}

pub(crate) fn span_text(src: &[u8], span: scan::Span) -> &str {
    // scan validates the entire input as UTF-8 and only emits character-boundary spans.
    std::str::from_utf8(span.of(src)).expect("scanner returned a non-UTF-8 span")
}

pub(super) fn excerpt(value: &str, hit_start: usize, line_only: bool) -> String {
    let (candidate, hit_in_candidate) = if line_only {
        let line_start = value[..hit_start].rfind('\n').map_or(0, |at| at + 1);
        let line_end = value[hit_start..]
            .find('\n')
            .map_or(value.len(), |at| hit_start + at);
        (&value[line_start..line_end], hit_start - line_start)
    } else {
        (value, hit_start)
    };
    let leading = candidate.len() - candidate.trim_start().len();
    let trimmed = candidate.trim();
    let hit_in_trimmed = hit_in_candidate.saturating_sub(leading).min(trimmed.len());
    let boundaries: Vec<usize> = trimmed
        .char_indices()
        .map(|(at, _)| at)
        .chain([trimmed.len()])
        .collect();
    let chars = boundaries.len().saturating_sub(1);
    if chars <= 120 {
        return trimmed.to_owned();
    }
    let hit_char = boundaries
        .partition_point(|&at| at <= hit_in_trimmed)
        .saturating_sub(1);
    let start_char = hit_char.saturating_sub(60).min(chars - 120);
    trimmed[boundaries[start_char]..boundaries[start_char + 120]].to_owned()
}
