use super::super::{scan, script, sidecar};
use super::findings::{add_review, add_service_edit, span_text, Place, XmlPass};
use super::param::is_caller_receiver;
use std::collections::BTreeSet;

/// Renames local service declarations and calls in script CDATA without rebuilding the XML.
pub fn scan_service_entity(
    src: &[u8],
    old: &str,
    new: &str,
    rename_declaration: bool,
    local_calls: bool,
    callers: &BTreeSet<String>,
) -> Result<XmlPass, scan::ScanError> {
    let tokens = scan::tokenize(src)?;
    let mut pass = XmlPass::new(src);
    let mut function_cdata = BTreeSet::new();
    if rename_declaration {
        if let Some(host) = sidecar::member_host_of(&tokens, src) {
            for section_name in ["ServiceDefinitions", "ServiceImplementations"] {
                for section in scan::child_tags(&tokens, src, section_name, host) {
                    let element = if section_name == "ServiceDefinitions" {
                        "ServiceDefinition"
                    } else {
                        "ServiceImplementation"
                    };
                    for item in scan::child_tags(&tokens, src, element, section) {
                        if let Some(value) = scan::attribute(src, &tokens[item], "name")? {
                            if value.of(src) == old.as_bytes() {
                                add_service_edit(
                                    src,
                                    value,
                                    new,
                                    Place::Service {
                                        element: element.to_string(),
                                    },
                                    &mut pass,
                                );
                                if element == "ServiceImplementation" {
                                    if let Some(end) = scan::element_end_in(&tokens, src, item) {
                                        for token in &tokens[item + 1..end] {
                                            if token.kind == scan::Kind::Cdata {
                                                function_cdata.insert(token.inner.start);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // Run-time permissions are keyed by the member's name (`<Permissions resourceName="Old">`), on the
    // entity that grants them: the scope, or a descendant that grants access to the inherited service.
    if local_calls {
        for token in &tokens {
            if matches!(token.kind, scan::Kind::Start | scan::Kind::Empty)
                && token.name.of(src) == b"Permissions"
            {
                if let Some(value) = scan::attribute(src, token, "resourceName")? {
                    if value.of(src) == old.as_bytes() {
                        add_service_edit(
                            src,
                            value,
                            new,
                            Place::Service {
                                element: "Permissions".to_string(),
                            },
                            &mut pass,
                        );
                    }
                }
            }
        }
    }
    let mut elements = Vec::<String>::new();
    for token in &tokens {
        match token.kind {
            scan::Kind::Start => elements.push(span_text(src, token.name).to_string()),
            scan::Kind::End => {
                elements.pop();
            }
            scan::Kind::Cdata if elements.last().is_some_and(|name| name == "code") => {
                merge_service_script(
                    src,
                    token.inner,
                    old,
                    new,
                    local_calls,
                    function_cdata.contains(&token.inner.start),
                    callers,
                    &mut pass,
                );
            }
            _ => {}
        }
    }
    Ok(pass)
}

/// Whether the member host declares or implements this service locally.
pub fn has_local_service(src: &[u8], name: &str) -> Result<bool, scan::ScanError> {
    let tokens = scan::tokenize(src)?;
    let Some(host) = sidecar::member_host_of(&tokens, src) else {
        return Ok(false);
    };
    for (section_name, element) in [
        ("ServiceDefinitions", "ServiceDefinition"),
        ("ServiceImplementations", "ServiceImplementation"),
    ] {
        for section in scan::child_tags(&tokens, src, section_name, host) {
            for item in scan::child_tags(&tokens, src, element, section) {
                if scan::attribute(src, &tokens[item], "name")?
                    .is_some_and(|span| span.of(src) == name.as_bytes())
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Renames the definition name in a service sidecar.
pub fn scan_service_definition(
    src: &[u8],
    old: &str,
    new: &str,
) -> Result<XmlPass, scan::ScanError> {
    let tokens = scan::tokenize(src)?;
    let mut pass = XmlPass::new(src);
    for token in &tokens {
        if matches!(token.kind, scan::Kind::Start | scan::Kind::Empty)
            && token.name.of(src) == b"ServiceDefinition"
        {
            if let Some(value) = scan::attribute(src, token, "name")? {
                if value.of(src) == old.as_bytes() {
                    add_service_edit(
                        src,
                        value,
                        new,
                        Place::Service {
                            element: "ServiceDefinition".to_string(),
                        },
                        &mut pass,
                    );
                }
            }
        }
    }
    Ok(pass)
}

/// Scans a JavaScript file. Edits come from the parsed calls; the review is lexical, so a string,
/// comment or member with the old name that is not a proven caller is left for a person.
pub fn scan_service_script(
    src: &[u8],
    old: &str,
    new: &str,
    local_calls: bool,
    rename_function: bool,
    callers: &BTreeSet<String>,
) -> Result<XmlPass, std::str::Utf8Error> {
    std::str::from_utf8(src)?;
    let mut pass = XmlPass::new(src);
    merge_service_script(
        src,
        scan::Span::new(0, src.len()),
        old,
        new,
        local_calls,
        rename_function,
        callers,
        &mut pass,
    );
    Ok(pass)
}

#[allow(clippy::too_many_arguments)]
fn merge_service_script(
    src: &[u8],
    span: scan::Span,
    old: &str,
    new: &str,
    local_calls: bool,
    rename_function: bool,
    callers: &BTreeSet<String>,
    pass: &mut XmlPass,
) {
    let text = span.of(src);
    let mut applied = BTreeSet::<(usize, usize)>::new();
    // A script the parser refuses gets no edits; the review below still names its mentions.
    if let Ok(script) = script::parse(text) {
        for call in &script.calls {
            if call.property.of(text) == old.as_bytes()
                && is_caller_receiver(&script, &call.receiver, local_calls, callers)
            {
                applied.insert((call.property.start, call.property.end));
            }
        }
        if rename_function {
            for comment in &script.comments {
                for found in jsdoc_function_spans(comment.span.of(text), old) {
                    let start = comment.span.start;
                    applied.insert((start + found.start, start + found.end));
                }
            }
        }
    }
    for &(start, end) in &applied {
        let absolute = scan::Span::new(span.start + start, span.start + end);
        add_service_edit(src, absolute, new, Place::Script, pass);
    }
    let mut review = BTreeSet::<(usize, usize)>::new();
    for (start, _) in String::from_utf8_lossy(text).match_indices(old) {
        let end = start + old.len();
        if applied.contains(&(start, end)) {
            continue;
        }
        let before = &text[..start];
        let after = &text[end..];
        let member_end = after
            .first()
            .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_');
        let dot = member_end
            && before
                .iter()
                .rposition(|b| !b.is_ascii_whitespace())
                .is_some_and(|i| before[i] == b'.');
        let bracket = start > 2
            && ((before.ends_with(b"[\"") && after.starts_with(b"\"]"))
                || (before.ends_with(b"['") && after.starts_with(b"']")));
        let quoted = start > 0
            && end < text.len()
            && ((text[start - 1] == b'"' && text[end] == b'"')
                || (text[start - 1] == b'\'' && text[end] == b'\''));
        if dot || bracket || quoted {
            review.insert((start, end));
        }
    }
    for (start, end) in review {
        add_review(
            src,
            scan::Span::new(span.start + start, span.start + end),
            Place::Script,
            pass,
        );
    }
}

pub(super) fn skip_ws(src: &[u8], mut at: usize) -> usize {
    while src.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// The name after each `@function` tag in one comment.
fn jsdoc_function_spans(comment: &[u8], old: &str) -> Vec<scan::Span> {
    const TAG: &[u8] = b"@function";
    comment
        .windows(TAG.len())
        .enumerate()
        .filter(|(_, window)| *window == TAG)
        .filter_map(|(tag, _)| {
            let name = skip_ws(comment, tag + TAG.len());
            let end = name + old.len();
            let named = comment.get(name..end) == Some(old.as_bytes());
            let whole = comment
                .get(end)
                .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_');
            (named && whole).then(|| scan::Span::new(name, end))
        })
        .collect()
}
