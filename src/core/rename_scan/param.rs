use super::super::{refs, scan, script, sidecar};
use super::findings::{
    add_review, add_review_reason, add_service_edit, lexical_mentions, span_text, Place, XmlPass,
    UNPARSED,
};
use super::service::skip_ws;
use std::collections::BTreeSet;

/// Presence and edits for one service parameter declaration.
#[derive(Debug)]
pub struct ParamPass {
    pub pass: XmlPass,
    pub old_found: bool,
    pub new_found: bool,
}

/// Renames one input declaration and scans scripts in an entity without rebuilding XML.
pub fn scan_param_entity(
    src: &[u8],
    service: &str,
    old: &str,
    new: &str,
    affected: bool,
    local_calls: bool,
    callers: &BTreeSet<String>,
) -> Result<ParamPass, scan::ScanError> {
    let tokens = scan::tokenize(src)?;
    let mut pass = XmlPass::new(src);
    let mut old_found = false;
    let mut new_found = false;
    let mut own_scripts = BTreeSet::new();
    if let Some(host) = sidecar::member_host_of(&tokens, src) {
        for definitions in scan::child_tags(&tokens, src, "ServiceDefinitions", host) {
            for definition in scan::child_tags(&tokens, src, "ServiceDefinition", definitions) {
                if scan::attribute(src, &tokens[definition], "name")?.map(|span| span.of(src))
                    != Some(service.as_bytes())
                {
                    continue;
                }
                for parameters in scan::child_tags(&tokens, src, "ParameterDefinitions", definition)
                {
                    for field in scan::child_tags(&tokens, src, "FieldDefinition", parameters) {
                        let Some(value) = scan::attribute(src, &tokens[field], "name")? else {
                            continue;
                        };
                        if value.of(src) == old.as_bytes() {
                            old_found = true;
                            if affected {
                                add_service_edit(
                                    src,
                                    value,
                                    new,
                                    Place::Parameter {
                                        service: service.to_string(),
                                    },
                                    &mut pass,
                                );
                            }
                        } else if value.of(src) == new.as_bytes() {
                            new_found = true;
                        }
                    }
                }
            }
        }
        if affected {
            for implementations in scan::child_tags(&tokens, src, "ServiceImplementations", host) {
                for implementation in
                    scan::child_tags(&tokens, src, "ServiceImplementation", implementations)
                {
                    if scan::attribute(src, &tokens[implementation], "name")?
                        .map(|span| span.of(src))
                        == Some(service.as_bytes())
                    {
                        if let Some(end) = scan::element_end_in(&tokens, src, implementation) {
                            for token in &tokens[implementation + 1..end] {
                                if token.kind == scan::Kind::Cdata {
                                    own_scripts.insert(token.inner.start);
                                }
                            }
                        }
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
                merge_param_script(
                    src,
                    token.inner,
                    service,
                    old,
                    new,
                    own_scripts.contains(&token.inner.start),
                    local_calls,
                    callers,
                    &mut pass,
                );
            }
            // A SQLQuery/SQLCommand service names its inputs inside the SQL text, as [[name]] or
            // <<name>>: no script pass reads that cell, so say so rather than leave a broken query.
            scan::Kind::Cdata
                if affected
                    && own_scripts.contains(&token.inner.start)
                    && elements.last().is_some_and(|name| name == "sql") =>
            {
                let text = span_text(src, token.inner);
                for placeholder in [format!("[[{old}]]"), format!("<<{old}>>")] {
                    for (at, _) in text.match_indices(&placeholder) {
                        let span = scan::Span::new(
                            token.inner.start + at,
                            token.inner.start + at + placeholder.len(),
                        );
                        add_review_reason(
                            src,
                            span,
                            Place::Script,
                            "an SQL placeholder names the parameter; rename it by hand",
                            &mut pass,
                        );
                    }
                }
            }
            _ => {}
        }
    }
    Ok(ParamPass {
        pass,
        old_found,
        new_found,
    })
}

/// Renames a parameter declaration in one service definition sidecar.
pub fn scan_param_definition(
    src: &[u8],
    service: &str,
    old: &str,
    new: &str,
) -> Result<ParamPass, scan::ScanError> {
    let tokens = scan::tokenize(src)?;
    let mut pass = XmlPass::new(src);
    let mut old_found = false;
    let mut new_found = false;
    for (at, token) in tokens.iter().enumerate() {
        if !matches!(token.kind, scan::Kind::Start | scan::Kind::Empty)
            || token.name.of(src) != b"ServiceDefinition"
        {
            continue;
        }
        for parameters in scan::child_tags(&tokens, src, "ParameterDefinitions", at) {
            for field in scan::child_tags(&tokens, src, "FieldDefinition", parameters) {
                let Some(value) = scan::attribute(src, &tokens[field], "name")? else {
                    continue;
                };
                if value.of(src) == old.as_bytes() {
                    old_found = true;
                    add_service_edit(
                        src,
                        value,
                        new,
                        Place::Parameter {
                            service: service.to_string(),
                        },
                        &mut pass,
                    );
                } else if value.of(src) == new.as_bytes() {
                    new_found = true;
                }
            }
        }
    }
    Ok(ParamPass {
        pass,
        old_found,
        new_found,
    })
}

/// Scans a sidecar script for the service's free input and calls to that service.
pub fn scan_param_script(
    src: &[u8],
    service: &str,
    old: &str,
    new: &str,
    own_script: bool,
    local_calls: bool,
    callers: &BTreeSet<String>,
) -> Result<XmlPass, std::str::Utf8Error> {
    std::str::from_utf8(src)?;
    let mut pass = XmlPass::new(src);
    merge_param_script(
        src,
        scan::Span::new(0, src.len()),
        service,
        old,
        new,
        own_script,
        local_calls,
        callers,
        &mut pass,
    );
    Ok(pass)
}

/// Whether a call's receiver is one the rename reaches: `me` and `this` when the entity's own
/// service is meant (`local`), or a Thing, named directly or through a variable, that is in
/// `callers`.
pub(super) fn is_caller_receiver(
    script: &script::Script,
    receiver: &script::Receiver,
    local: bool,
    callers: &BTreeSet<String>,
) -> bool {
    match receiver {
        script::Receiver::Me | script::Receiver::This => local,
        other => script
            .thing_of(other)
            .is_some_and(|entity| callers.contains(entity)),
    }
}

#[allow(clippy::too_many_arguments)]
fn merge_param_script(
    src: &[u8],
    span: scan::Span,
    service: &str,
    old: &str,
    new: &str,
    own_script: bool,
    local_calls: bool,
    callers: &BTreeSet<String>,
    pass: &mut XmlPass,
) {
    let text = span.of(src);
    let parsed = script::parse(text).ok();
    let mut applied = BTreeSet::new();
    if let Some(script) = &parsed {
        if own_script {
            if let Some((at, why)) = unsafe_param_use(text, script, old) {
                add_review_reason(
                    src,
                    scan::Span::new(span.start + at, span.start + at + old.len()),
                    Place::Script,
                    why,
                    pass,
                );
                return;
            }
            for identifier in &script.identifiers {
                if identifier.role == script::Role::Reference
                    && identifier.span.of(text) == old.as_bytes()
                {
                    applied.insert((identifier.span.start, identifier.span.end));
                }
            }
            for comment in &script.comments {
                for found in jsdoc_param_spans(comment.span.of(text), old) {
                    let start = comment.span.start;
                    applied.insert((start + found.start, start + found.end));
                }
            }
        }
        for call in &script.calls {
            if call.property.of(text) != service.as_bytes()
                || !is_caller_receiver(script, &call.receiver, local_calls, callers)
            {
                continue;
            }
            match &call.first {
                script::FirstArgument::Object(keys) => {
                    for key in keys.iter().filter(|key| key.text == old) {
                        applied.insert((key.span.start, key.span.end));
                    }
                }
                script::FirstArgument::Other => add_review_reason(
                    src,
                    scan::Span::new(
                        span.start + call.property.start,
                        span.start + call.property.end,
                    ),
                    Place::Script,
                    "non-literal first argument; parameter keys cannot be proved",
                    pass,
                ),
            }
        }
    }
    for &(start, end) in &applied {
        add_service_edit(
            src,
            scan::Span::new(span.start + start, span.start + end),
            new,
            Place::Script,
            pass,
        );
    }
    for hit in refs::find(
        std::str::from_utf8(text).expect("script is UTF-8"),
        old,
        refs::Mode::Prefix,
    ) {
        if applied.contains(&(hit.start, hit.end)) {
            continue;
        }
        let hit_span = scan::Span::new(span.start + hit.start, span.start + hit.end);
        if parsed.is_some() {
            add_review(src, hit_span, Place::Script, pass);
        } else {
            add_review_reason(src, hit_span, Place::Script, UNPARSED, pass);
        }
    }
}

/// The first place the script gives `old` a meaning of its own: a redeclaration, a parameter, or a
/// shorthand property. Renaming the free input would change what each of those means.
fn unsafe_param_use(
    src: &[u8],
    script: &script::Script,
    old: &str,
) -> Option<(usize, &'static str)> {
    script
        .identifiers
        .iter()
        .filter(|identifier| identifier.span.of(src) == old.as_bytes())
        .find_map(|identifier| {
            let why = match identifier.role {
                script::Role::Shorthand => {
                    "shorthand property would change meaning; the whole script was left for review"
                }
                script::Role::Declaration => {
                    "local re-declaration would change meaning; the whole script was left for review"
                }
                script::Role::Parameter => {
                    "nested function or catch parameter would change meaning; the whole script was left for review"
                }
                script::Role::Reference | script::Role::ObjectKey | script::Role::MemberProperty => {
                    return None;
                }
            };
            Some((identifier.span.start, why))
        })
}

fn jsdoc_param_spans(src: &[u8], old: &str) -> Vec<scan::Span> {
    let mut out = Vec::new();
    for (line_at, line) in split_lines_with_offsets(src) {
        let Some(tag) = find_bytes(line, b"@param") else {
            continue;
        };
        let mut at = skip_ws(line, tag + 6);
        if line.get(at) == Some(&b'{') {
            while line.get(at).is_some_and(|byte| *byte != b'}') {
                at += 1;
            }
            at = skip_ws(line, at.saturating_add(1));
        }
        if line.get(at..at + old.len()) == Some(old.as_bytes())
            && line
                .get(at + old.len())
                .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
        {
            out.push(scan::Span::new(line_at + at, line_at + at + old.len()));
        }
    }
    out
}

fn split_lines_with_offsets(src: &[u8]) -> Vec<(usize, &[u8])> {
    let mut out = Vec::new();
    let mut start = 0;
    for end in 0..=src.len() {
        if end == src.len() || src[end] == b'\n' {
            out.push((start, &src[start..end]));
            start = end + 1;
        }
    }
    out
}

fn find_bytes(src: &[u8], needle: &[u8]) -> Option<usize> {
    src.windows(needle.len()).position(|part| part == needle)
}

/// Whether `name` occurs in the script as an identifier (not in a string, comment or regex, and
/// not as a member name): a parameter renamed to it would be captured by that use. A script that
/// cannot be parsed is refused rather than guessed at: `true` if `name` occurs in it as a whole
/// word anywhere.
pub fn script_uses_identifier(src: &[u8], name: &str) -> bool {
    match script::parse(src) {
        Ok(script) => script.identifiers.iter().any(|identifier| {
            identifier.role != script::Role::MemberProperty
                && identifier.span.of(src) == name.as_bytes()
        }),
        Err(_) => !lexical_mentions(src, name).is_empty(),
    }
}
