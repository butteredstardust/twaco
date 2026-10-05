//! Byte-preserving entity-name discovery and edits for one XML document.
//!
//! The pass consumes bytes and returns spans; it performs no file I/O and never serialises XML.
//! Comments and processing instructions are deliberately ignored, while raw attribute values,
//! non-whitespace text nodes and CDATA payloads are inspected without decoding XML entities.

use super::{refs, scan, script, sidecar, splice};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

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

/// The reason attached to every mention in a script the parser refused.
const UNPARSED: &str = "script could not be parsed; left for review";

/// Whether a call's receiver is one the rename reaches: `me` and `this` when the entity's own
/// service is meant (`local`), or a Thing, named directly or through a variable, that is in
/// `callers`.
fn is_caller_receiver(
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
                && !start.checked_sub(1).is_some_and(|before| word(&text[before]))
                && !text.get(end).is_some_and(word)
        })
        .map(|(start, _)| scan::Span::new(start, start + needle.len()))
        .collect()
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

pub(crate) fn add_review_reason(src: &[u8], span: scan::Span, place: Place, reason: &str, pass: &mut XmlPass) {
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

/// Structural and script result for one entity in a table rename.
#[derive(Debug)]
pub struct TablePass {
    pub pass: XmlPass,
    pub old_definition: bool,
    pub new_tables: usize,
    pub renamed_tables: usize,
}

/// Renames table declarations and instances selected by the planner, and scans script CDATA.
///
/// Script edits are limited to a literal value of a `tableName` key. Other quoted literals and
/// member accesses are Review findings, including on entities outside the inheritance scope.
pub fn scan_configuration_table(
    src: &[u8],
    old: &str,
    new: &str,
    structural: bool,
    apply_scripts: bool,
) -> Result<TablePass, scan::ScanError> {
    let tokens = scan::tokenize(src)?;
    let mut pass = XmlPass::new(src);
    let mut old_definition = false;
    let mut new_tables = 0;
    let mut renamed_tables = 0;
    let mut elements = Vec::<String>::new();
    for token in &tokens {
        if matches!(token.kind, scan::Kind::Start | scan::Kind::Empty) {
            let element = token.name.of(src);
            let section = elements.last().map(String::as_str);
            let nested_in_service = elements
                .iter()
                .rev()
                .take(2)
                .any(|name| name == "ServiceImplementation");
            let configuration_member = (element == b"ConfigurationTableDefinition"
                && section == Some("ConfigurationTableDefinitions"))
                || (element == b"ConfigurationTable"
                    && section == Some("ConfigurationTables")
                    && !nested_in_service);
            if configuration_member {
                if let Some(value) = scan::attribute(src, token, "name")? {
                    if element == b"ConfigurationTableDefinition" && value.of(src) == old.as_bytes()
                    {
                        old_definition = true;
                    }
                    if value.of(src) == new.as_bytes() {
                        new_tables += 1;
                    } else if structural && value.of(src) == old.as_bytes() {
                        renamed_tables += 1;
                        add_table_edit(
                            src,
                            value,
                            new,
                            Place::Table {
                                element: String::from_utf8_lossy(element).into_owned(),
                            },
                            &mut pass,
                        );
                    }
                }
            }
        }
        match token.kind {
            scan::Kind::Start => elements.push(span_text(src, token.name).to_string()),
            scan::Kind::End => {
                elements.pop();
            }
            scan::Kind::Cdata if elements.last().is_some_and(|name| name == "code") => {
                merge_table_script(src, token.inner, old, new, apply_scripts, &mut pass);
            }
            _ => {}
        }
    }
    Ok(TablePass {
        pass,
        old_definition,
        new_tables,
        renamed_tables,
    })
}

/// Scans one sidecar script for table selectors and ambiguous occurrences.
pub fn scan_table_script(
    src: &[u8],
    old: &str,
    new: &str,
    apply_scripts: bool,
) -> Result<XmlPass, std::str::Utf8Error> {
    std::str::from_utf8(src)?;
    let mut pass = XmlPass::new(src);
    merge_table_script(
        src,
        scan::Span::new(0, src.len()),
        old,
        new,
        apply_scripts,
        &mut pass,
    );
    Ok(pass)
}

fn merge_table_script(
    src: &[u8],
    span: scan::Span,
    old: &str,
    new: &str,
    apply_scripts: bool,
    pass: &mut XmlPass,
) {
    let text = span.of(src);
    let absolute =
        |inner: scan::Span| scan::Span::new(span.start + inner.start, span.start + inner.end);
    let Ok(script) = script::parse(text) else {
        for mention in lexical_mentions(text, old) {
            add_review(src, absolute(mention), Place::Script, pass);
        }
        return;
    };
    // Each occurrence, in source order, with whether it is the value of a `tableName` key.
    let mut found = BTreeMap::<(usize, usize), bool>::new();
    for string in script.strings.iter().filter(|string| string.value == old) {
        let selector = script
            .object_strings
            .iter()
            .any(|property| property.value_span == string.span && property.key == "tableName");
        found.insert((string.span.start, string.span.end), selector);
    }
    for member in &script.members {
        if !member.string_index && member.property.of(text) == old.as_bytes() {
            found.insert((member.property.start, member.property.end), false);
        }
    }
    for ((start, end), selector) in found {
        let hit = absolute(scan::Span::new(start, end));
        if selector && apply_scripts {
            add_table_edit(src, hit, new, Place::Script, pass);
        } else {
            add_review(src, hit, Place::Script, pass);
        }
    }
}

fn add_table_edit(src: &[u8], span: scan::Span, new: &str, place: Place, pass: &mut XmlPass) {
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

fn skip_ws(src: &[u8], mut at: usize) -> usize {
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

pub(crate) fn add_service_edit(src: &[u8], span: scan::Span, new: &str, place: Place, pass: &mut XmlPass) {
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

#[derive(Debug)]
enum JsonNode {
    Object(Vec<(String, scan::Span, JsonNode)>),
    Array(Vec<JsonNode>),
    String(String, scan::Span),
    Other,
}

/// Renames only service-identity strings tied to an affected `Data.<DataName>` declaration.
///
/// The applied `(key, context)` pairs, verified against mashups containing
/// `ListItems`, `CreateItem` and `GetItemData`, are exactly:
/// `Name` and `Target` in `Data.<DataName>.Services[]`; `SourceId` and `SourceName` in a
/// `DataBindings[]` object whose `SourceSection` is that DataName; `TargetId` where
/// `TargetSection` is that DataName; `EventHandlerService` where `EventHandlerId` is that
/// DataName; and `EventTriggerId` where `EventTriggerSection` is that DataName. Every other
/// JSON string exactly equal to the old service name is left as a Review finding.
pub fn scan_service_mashup(
    src: &[u8],
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
    xml: bool,
) -> Result<XmlPass, String> {
    let mut pass = XmlPass::new(src);
    if xml {
        let tokens = scan::tokenize(src).map_err(|error| error.to_string())?;
        let mut elements = Vec::<String>::new();
        for token in &tokens {
            match token.kind {
                scan::Kind::Start => elements.push(span_text(src, token.name).to_string()),
                scan::Kind::End => {
                    elements.pop();
                }
                scan::Kind::Cdata
                    if elements.last().is_some_and(|name| name == "mashupContent") =>
                {
                    scan_mashup_payload(src, token.inner, old, new, callers, &mut pass)?;
                }
                _ => {}
            }
        }
    } else {
        scan_mashup_payload(
            src,
            scan::Span::new(0, src.len()),
            old,
            new,
            callers,
            &mut pass,
        )?;
    }
    Ok(pass)
}

/// Renames only service-input keys tied to a resolved service in mashup content.
///
/// The applied `(key, context)` pairs, verified in representative `CreateItem` and
/// `GetItemData` data entries, are exactly: an object key in
/// `Data.<DataName>.Services[].Parameters` when that service's `Name` or `Target` is the selected
/// service; and `TargetProperty` in `DataBindings[].PropertyMaps[]` when `TargetArea` is `Data`,
/// `TargetSection` is that DataName and `TargetId` is the selected service. Every other JSON key
/// or string exactly equal to the old parameter is Review.
pub fn scan_param_mashup(
    src: &[u8],
    service: &str,
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
    xml: bool,
) -> Result<XmlPass, String> {
    let mut pass = XmlPass::new(src);
    if xml {
        let tokens = scan::tokenize(src).map_err(|error| error.to_string())?;
        let mut elements = Vec::<String>::new();
        for token in &tokens {
            match token.kind {
                scan::Kind::Start => elements.push(span_text(src, token.name).to_string()),
                scan::Kind::End => {
                    elements.pop();
                }
                scan::Kind::Cdata
                    if elements.last().is_some_and(|name| name == "mashupContent") =>
                {
                    scan_param_mashup_payload(
                        src,
                        token.inner,
                        service,
                        old,
                        new,
                        callers,
                        &mut pass,
                    )?;
                }
                _ => {}
            }
        }
    } else {
        scan_param_mashup_payload(
            src,
            scan::Span::new(0, src.len()),
            service,
            old,
            new,
            callers,
            &mut pass,
        )?;
    }
    Ok(pass)
}

fn scan_param_mashup_payload(
    src: &[u8],
    span: scan::Span,
    service: &str,
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
    pass: &mut XmlPass,
) -> Result<(), String> {
    let mut parser = JsonParser {
        src: span.of(src),
        at: 0,
    };
    let root = parser.value()?;
    parser.white();
    if parser.at != parser.src.len() {
        return Err("trailing content in mashup JSON".to_string());
    }
    let mut applied = BTreeMap::<(usize, usize), (String, String)>::new();
    let mut data_names = BTreeSet::new();
    if let Some(JsonNode::Object(entries)) = object_get(&root, "Data") {
        for (data_name, _, entry) in entries {
            if !node_text(object_get(entry, "EntityName"))
                .is_some_and(|entity| callers.contains(entity))
            {
                continue;
            }
            if let Some(JsonNode::Array(services)) = object_get(entry, "Services") {
                for item in services {
                    let selected = node_text(object_get(item, "Name")) == Some(service)
                        || node_text(object_get(item, "Target")) == Some(service);
                    if !selected {
                        continue;
                    }
                    data_names.insert(data_name.clone());
                    if let Some(JsonNode::Object(parameters)) = object_get(item, "Parameters") {
                        for (key, key_span, _) in parameters {
                            if key == old {
                                applied.insert(
                                    (key_span.start, key_span.end),
                                    (
                                        "object key".to_string(),
                                        "Data.<DataName>.Services[].Parameters".to_string(),
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(JsonNode::Array(bindings)) = object_get(&root, "DataBindings") {
        for binding in bindings {
            if node_text(object_get(binding, "TargetArea")) != Some("Data")
                || node_text(object_get(binding, "TargetId")) != Some(service)
                || !node_text(object_get(binding, "TargetSection"))
                    .is_some_and(|name| data_names.contains(name))
            {
                continue;
            }
            if let Some(JsonNode::Array(properties)) = object_get(binding, "PropertyMaps") {
                for property in properties {
                    mark_json(
                        property,
                        "TargetProperty",
                        old,
                        "DataBindings[].PropertyMaps[] targeting the service",
                        &mut applied,
                    );
                }
            }
        }
    }
    for ((start, end), (key, context)) in &applied {
        add_service_edit(
            src,
            scan::Span::new(span.start + start, span.start + end),
            new,
            Place::Mashup {
                key: key.clone(),
                context: context.clone(),
            },
            pass,
        );
    }
    let mut strings = Vec::new();
    collect_json_keys_and_strings(&root, &mut strings);
    for (value, found) in strings {
        if value == old && !applied.contains_key(&(found.start, found.end)) {
            add_review(
                src,
                scan::Span::new(span.start + found.start, span.start + found.end),
                Place::Mashup {
                    key: String::new(),
                    context: "other JSON key or string".to_string(),
                },
                pass,
            );
        }
    }
    Ok(())
}

fn collect_json_keys_and_strings<'a>(node: &'a JsonNode, out: &mut Vec<(&'a str, scan::Span)>) {
    match node {
        JsonNode::Object(entries) => {
            for (key, span, value) in entries {
                out.push((key, *span));
                collect_json_keys_and_strings(value, out);
            }
        }
        JsonNode::Array(items) => {
            for item in items {
                collect_json_keys_and_strings(item, out);
            }
        }
        JsonNode::String(value, span) => out.push((value, *span)),
        JsonNode::Other => {}
    }
}

fn scan_mashup_payload(
    src: &[u8],
    span: scan::Span,
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
    pass: &mut XmlPass,
) -> Result<(), String> {
    let mut parser = JsonParser {
        src: span.of(src),
        at: 0,
    };
    let root = parser.value()?;
    parser.white();
    if parser.at != parser.src.len() {
        return Err("trailing content in mashup JSON".to_string());
    }
    let mut applied = BTreeMap::<(usize, usize), (String, String)>::new();
    let mut data_names = BTreeSet::new();
    if let Some(JsonNode::Object(entries)) = object_get(&root, "Data") {
        for (data_name, _, entry) in entries {
            if node_text(object_get(entry, "EntityName"))
                .is_some_and(|entity| callers.contains(entity))
            {
                if let Some(JsonNode::Array(services)) = object_get(entry, "Services") {
                    let matching = services
                        .iter()
                        .any(|service| node_text(object_get(service, "Name")) == Some(old));
                    if matching {
                        data_names.insert(data_name.clone());
                        for service in services {
                            for key in ["Name", "Target"] {
                                mark_json(
                                    service,
                                    key,
                                    old,
                                    "Data.<DataName>.Services[]",
                                    &mut applied,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(JsonNode::Array(bindings)) = object_get(&root, "DataBindings") {
        for binding in bindings {
            if node_text(object_get(binding, "SourceSection"))
                .is_some_and(|name| data_names.contains(name))
            {
                mark_json(
                    binding,
                    "SourceId",
                    old,
                    "DataBindings[] with SourceSection",
                    &mut applied,
                );
                mark_json(
                    binding,
                    "SourceName",
                    old,
                    "DataBindings[] with SourceSection",
                    &mut applied,
                );
            }
            if node_text(object_get(binding, "TargetSection"))
                .is_some_and(|name| data_names.contains(name))
            {
                mark_json(
                    binding,
                    "TargetId",
                    old,
                    "DataBindings[] with TargetSection",
                    &mut applied,
                );
            }
        }
    }
    if let Some(JsonNode::Array(events)) = object_get(&root, "Events") {
        for event in events {
            if node_text(object_get(event, "EventHandlerId"))
                .is_some_and(|name| data_names.contains(name))
            {
                mark_json(
                    event,
                    "EventHandlerService",
                    old,
                    "Events[] with EventHandlerId",
                    &mut applied,
                );
            }
            if node_text(object_get(event, "EventTriggerSection"))
                .is_some_and(|name| data_names.contains(name))
            {
                mark_json(
                    event,
                    "EventTriggerId",
                    old,
                    "Events[] with EventTriggerSection",
                    &mut applied,
                );
            }
        }
    }
    for ((start, end), (key, context)) in &applied {
        add_service_edit(
            src,
            scan::Span::new(span.start + start, span.start + end),
            new,
            Place::Mashup {
                key: key.clone(),
                context: context.clone(),
            },
            pass,
        );
    }
    let mut strings = Vec::new();
    collect_json_strings(&root, &mut strings);
    for (value, found) in strings {
        if value == old && !applied.contains_key(&(found.start, found.end)) {
            add_review(
                src,
                scan::Span::new(span.start + found.start, span.start + found.end),
                Place::Mashup {
                    key: String::new(),
                    context: "other JSON string".to_string(),
                },
                pass,
            );
        }
    }
    Ok(())
}

fn object_get<'a>(node: &'a JsonNode, key: &str) -> Option<&'a JsonNode> {
    let JsonNode::Object(entries) = node else {
        return None;
    };
    entries
        .iter()
        .find(|(name, _, _)| name == key)
        .map(|(_, _, value)| value)
}

fn node_text(node: Option<&JsonNode>) -> Option<&str> {
    match node? {
        JsonNode::String(value, _) => Some(value),
        _ => None,
    }
}

fn mark_json(
    object: &JsonNode,
    key: &str,
    old: &str,
    context: &str,
    out: &mut BTreeMap<(usize, usize), (String, String)>,
) {
    if let Some(JsonNode::String(value, span)) = object_get(object, key) {
        if value == old {
            out.insert(
                (span.start, span.end),
                (key.to_string(), context.to_string()),
            );
        }
    }
}

fn collect_json_strings<'a>(node: &'a JsonNode, out: &mut Vec<(&'a str, scan::Span)>) {
    match node {
        JsonNode::Object(entries) => {
            for (_, _, value) in entries {
                collect_json_strings(value, out);
            }
        }
        JsonNode::Array(items) => {
            for item in items {
                collect_json_strings(item, out);
            }
        }
        JsonNode::String(value, span) => out.push((value, *span)),
        JsonNode::Other => {}
    }
}

struct JsonParser<'a> {
    src: &'a [u8],
    at: usize,
}

impl JsonParser<'_> {
    fn white(&mut self) {
        while self.src.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn value(&mut self) -> Result<JsonNode, String> {
        self.white();
        match self.src.get(self.at) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => {
                let (value, span) = self.string()?;
                Ok(JsonNode::String(value, span))
            }
            Some(_) => {
                while self.src.get(self.at).is_some_and(|byte| {
                    !byte.is_ascii_whitespace() && !matches!(*byte, b',' | b']' | b'}')
                }) {
                    self.at += 1;
                }
                Ok(JsonNode::Other)
            }
            None => Err("unexpected end of mashup JSON".to_string()),
        }
    }

    fn object(&mut self) -> Result<JsonNode, String> {
        self.at += 1;
        let mut entries = Vec::new();
        loop {
            self.white();
            if self.src.get(self.at) == Some(&b'}') {
                self.at += 1;
                break;
            }
            let (key, key_span) = self.string()?;
            self.white();
            if self.src.get(self.at) != Some(&b':') {
                return Err("object key without ':' in mashup JSON".to_string());
            }
            self.at += 1;
            entries.push((key, key_span, self.value()?));
            self.white();
            match self.src.get(self.at) {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    break;
                }
                _ => return Err("object without ',' or '}' in mashup JSON".to_string()),
            }
        }
        Ok(JsonNode::Object(entries))
    }

    fn array(&mut self) -> Result<JsonNode, String> {
        self.at += 1;
        let mut items = Vec::new();
        loop {
            self.white();
            if self.src.get(self.at) == Some(&b']') {
                self.at += 1;
                break;
            }
            items.push(self.value()?);
            self.white();
            match self.src.get(self.at) {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    break;
                }
                _ => return Err("array without ',' or ']' in mashup JSON".to_string()),
            }
        }
        Ok(JsonNode::Array(items))
    }

    fn string(&mut self) -> Result<(String, scan::Span), String> {
        if self.src.get(self.at) != Some(&b'"') {
            return Err("expected string in mashup JSON".to_string());
        }
        let quote = self.at;
        self.at += 1;
        let start = self.at;
        let mut escaped = false;
        while let Some(&byte) = self.src.get(self.at) {
            if byte == b'"' && !escaped {
                let end = self.at;
                self.at += 1;
                let value: String = serde_json::from_slice(&self.src[quote..self.at])
                    .map_err(|error| error.to_string())?;
                return Ok((value, scan::Span::new(start, end)));
            }
            escaped = byte == b'\\' && !escaped;
            self.at += 1;
        }
        Err("unterminated string in mashup JSON".to_string())
    }
}

#[derive(Deserialize, Default)]
struct RenameConfig {
    #[serde(default)]
    project: Vec<RenameProject>,
    #[serde(default)]
    validate: RenameValidate,
}

#[derive(Deserialize, Default)]
struct RenameProject {
    #[serde(default)]
    deploy: RenameDeploy,
}

#[derive(Deserialize, Default)]
struct RenameDeploy {
    entry_point_thing: Option<toml::Spanned<String>>,
    deploy_service: Option<toml::Spanned<String>>,
    deploy_parameters: Option<toml::Spanned<toml::Table>>,
    #[serde(default)]
    post_import: Vec<RenamePostImport>,
}

#[derive(Deserialize)]
struct RenamePostImport {
    thing: toml::Spanned<String>,
    service: toml::Spanned<String>,
    target: Option<toml::Spanned<String>>,
    parameters: Option<toml::Spanned<toml::Table>>,
}

#[derive(Deserialize, Default)]
struct RenameValidate {
    #[serde(default)]
    inherited_overrides: Vec<toml::Spanned<String>>,
}

/// Renames service selectors in deploy configuration and qualified inherited overrides.
pub fn scan_service_config(
    src: &[u8],
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
) -> Result<XmlPass, String> {
    let text = std::str::from_utf8(src).map_err(|error| error.to_string())?;
    let config: RenameConfig = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut pass = XmlPass::new(src);
    for project in &config.project {
        let deploy = &project.deploy;
        if let (Some(entity), Some(service)) = (&deploy.entry_point_thing, &deploy.deploy_service) {
            if callers.contains(entity.get_ref()) && service.get_ref() == old {
                add_toml_edit(
                    src,
                    service,
                    new,
                    "project.deploy.deploy_service",
                    &mut pass,
                )?;
            }
        }
        for item in &deploy.post_import {
            let entity = item
                .target
                .as_ref()
                .map(|target| {
                    target
                        .get_ref()
                        .split_once('/')
                        .map_or(target.get_ref().as_str(), |(_, name)| name)
                })
                .unwrap_or_else(|| item.thing.get_ref());
            if callers.contains(entity) && item.service.get_ref() == old {
                add_toml_edit(
                    src,
                    &item.service,
                    new,
                    "project.deploy.post_import.service",
                    &mut pass,
                )?;
            }
        }
    }
    for item in &config.validate.inherited_overrides {
        if item.get_ref() == old {
            let span = toml_string_span(src, item.span(), old)?;
            add_review(
                src,
                span,
                Place::Config {
                    key: "validate.inherited_overrides".to_string(),
                },
                &mut pass,
            );
        } else if let Some(entity) = item.get_ref().strip_suffix(&format!(".{old}")) {
            if callers.contains(entity) {
                let whole = toml_string_span(src, item.span(), item.get_ref())?;
                let span = scan::Span::new(whole.end - old.len(), whole.end);
                add_service_edit(
                    src,
                    span,
                    new,
                    Place::Config {
                        key: "validate.inherited_overrides".to_string(),
                    },
                    &mut pass,
                );
            }
        }
    }
    Ok(pass)
}

/// Result of scanning configured calls for one renamed parameter.
#[derive(Debug)]
pub struct ParamConfigPass {
    pub pass: XmlPass,
    pub conflicts: Vec<String>,
}

/// Renames direct parameter keys for configured calls to the selected service.
pub fn scan_param_config(
    src: &[u8],
    service: &str,
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
) -> Result<ParamConfigPass, String> {
    let text = std::str::from_utf8(src).map_err(|error| error.to_string())?;
    let config: RenameConfig = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut pass = XmlPass::new(src);
    let mut conflicts = Vec::new();
    for project in &config.project {
        let deploy = &project.deploy;
        if let (Some(entity), Some(selected), Some(parameters)) = (
            &deploy.entry_point_thing,
            &deploy.deploy_service,
            &deploy.deploy_parameters,
        ) {
            if callers.contains(entity.get_ref()) && selected.get_ref() == service {
                rename_toml_parameter(
                    src,
                    parameters,
                    old,
                    new,
                    "project.deploy.deploy_parameters",
                    &mut pass,
                    &mut conflicts,
                )?;
            }
        }
        for item in &deploy.post_import {
            let entity = item
                .target
                .as_ref()
                .map(|target| {
                    target
                        .get_ref()
                        .split_once('/')
                        .map_or(target.get_ref().as_str(), |(_, name)| name)
                })
                .unwrap_or_else(|| item.thing.get_ref());
            if callers.contains(entity) && item.service.get_ref() == service {
                if let Some(parameters) = &item.parameters {
                    rename_toml_parameter(
                        src,
                        parameters,
                        old,
                        new,
                        "project.deploy.post_import.parameters",
                        &mut pass,
                        &mut conflicts,
                    )?;
                }
            }
        }
    }
    Ok(ParamConfigPass { pass, conflicts })
}

fn rename_toml_parameter(
    src: &[u8],
    table: &toml::Spanned<toml::Table>,
    old: &str,
    new: &str,
    context: &str,
    pass: &mut XmlPass,
    conflicts: &mut Vec<String>,
) -> Result<(), String> {
    if table.get_ref().contains_key(new) {
        conflicts.push(format!("parameter {new} in {context}"));
    }
    if table.get_ref().contains_key(old) {
        let span = toml_table_key_span(src, table.span(), old)?;
        add_service_edit(
            src,
            span,
            new,
            Place::Config {
                key: context.to_string(),
            },
            pass,
        );
    }
    Ok(())
}

fn toml_table_key_span(
    src: &[u8],
    range: std::ops::Range<usize>,
    key: &str,
) -> Result<scan::Span, String> {
    let raw = src
        .get(range.clone())
        .ok_or_else(|| "TOML returned an out-of-range table span".to_string())?;
    let mut at = 0;
    let mut depth = 0isize;
    while at < raw.len() {
        match raw[at] {
            b'{' => {
                depth += 1;
                at += 1;
            }
            b'}' => {
                depth -= 1;
                at += 1;
            }
            b'\'' | b'"' => {
                let quote = raw[at];
                let start = at + 1;
                at += 1;
                while at < raw.len() && raw[at] != quote {
                    at += if raw[at] == b'\\' { 2 } else { 1 };
                }
                let end = at.min(raw.len());
                at = (at + 1).min(raw.len());
                let equals = skip_ws(raw, at);
                if depth <= 1
                    && raw.get(equals) == Some(&b'=')
                    && raw.get(start..end) == Some(key.as_bytes())
                {
                    return Ok(scan::Span::new(range.start + start, range.start + end));
                }
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = at;
                at += 1;
                while raw.get(at).is_some_and(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-')
                }) {
                    at += 1;
                }
                let equals = skip_ws(raw, at);
                if depth <= 1
                    && raw.get(equals) == Some(&b'=')
                    && raw.get(start..at) == Some(key.as_bytes())
                {
                    return Ok(scan::Span::new(range.start + start, range.start + at));
                }
            }
            _ => at += 1,
        }
    }
    Err(format!(
        "cannot locate parameter key {key:?} in its TOML table"
    ))
}

fn add_toml_edit(
    src: &[u8],
    value: &toml::Spanned<String>,
    new: &str,
    key: &str,
    pass: &mut XmlPass,
) -> Result<(), String> {
    let span = toml_string_span(src, value.span(), value.get_ref())?;
    add_service_edit(
        src,
        span,
        new,
        Place::Config {
            key: key.to_string(),
        },
        pass,
    );
    Ok(())
}

fn toml_string_span(
    src: &[u8],
    range: std::ops::Range<usize>,
    value: &str,
) -> Result<scan::Span, String> {
    let raw = src
        .get(range.clone())
        .ok_or_else(|| "TOML returned an out-of-range span".to_string())?;
    let at = raw
        .windows(value.len())
        .position(|part| part == value.as_bytes())
        .ok_or_else(|| format!("cannot locate {value:?} in its TOML value"))?;
    Ok(scan::Span::new(
        range.start + at,
        range.start + at + value.len(),
    ))
}

/// Structural result for the field-only XML pass.
#[derive(Debug)]
pub struct FieldPass {
    pub pass: XmlPass,
    pub old_found: bool,
    pub new_found: bool,
    pub table_conflicts: Vec<String>,
    pub tables: usize,
}

/// Renames one field declaration in a DataShape document.
pub fn scan_data_shape_field(
    src: &[u8],
    old: &str,
    new: &str,
) -> Result<FieldPass, scan::ScanError> {
    let tokens = scan::tokenize(src)?;
    let mut pass = XmlPass::new(src);
    let mut old_found = false;
    let mut new_found = false;
    if let Some(shape) = tokens.iter().position(|token| {
        matches!(token.kind, scan::Kind::Start | scan::Kind::Empty)
            && token.name.of(src) == b"DataShape"
    }) {
        for definitions in scan::child_tags(&tokens, src, "FieldDefinitions", shape) {
            for field in scan::child_tags(&tokens, src, "FieldDefinition", definitions) {
                let Some(value) = scan::attribute(src, &tokens[field], "name")? else {
                    continue;
                };
                if value.of(src) == old.as_bytes() {
                    old_found = true;
                    add_field_edit(
                        src,
                        value,
                        new,
                        Place::FieldDefinition {
                            element: "DataShape".to_string(),
                        },
                        &mut pass,
                    );
                } else if value.of(src) == new.as_bytes() {
                    new_found = true;
                }
            }
        }
    }
    Ok(FieldPass {
        pass,
        old_found,
        new_found,
        table_conflicts: Vec::new(),
        tables: 0,
    })
}

/// Renames inline field declarations and row element names in matching configuration tables.
pub fn scan_configuration_field(
    src: &[u8],
    scope: &str,
    old: &str,
    new: &str,
) -> Result<FieldPass, scan::ScanError> {
    let tokens = scan::tokenize(src)?;
    let mut pass = XmlPass::new(src);
    let mut table_conflicts = Vec::new();
    let mut tables = 0;
    for (table_index, table) in tokens.iter().enumerate() {
        if !matches!(table.kind, scan::Kind::Start | scan::Kind::Empty)
            || table.name.of(src) != b"ConfigurationTable"
            || scan::attribute(src, table, "dataShapeName")?.map(|span| span.of(src))
                != Some(scope.as_bytes())
        {
            continue;
        }
        tables += 1;
        let table_name = scan::attribute(src, table, "name")?
            .map(|span| String::from_utf8_lossy(span.of(src)).into_owned())
            .unwrap_or_default();
        rename_in_container(
            src,
            &tokens,
            table_index,
            &table_name,
            old,
            new,
            &mut pass,
            &mut table_conflicts,
        )?;
    }
    Ok(FieldPass {
        pass,
        old_found: false,
        new_found: false,
        table_conflicts,
        tables,
    })
}

/// Renames one field inside a container that holds an inline `DataShape/FieldDefinitions` and
/// `Rows/Row/<field>` elements: a configuration table, or the `infoTable` of a property value.
/// A container that already has a field called `new` is reported in `conflicts`, never edited.
#[allow(clippy::too_many_arguments)]
fn rename_in_container(
    src: &[u8],
    tokens: &[scan::Token],
    container: usize,
    label: &str,
    old: &str,
    new: &str,
    pass: &mut XmlPass,
    conflicts: &mut Vec<String>,
) -> Result<(), scan::ScanError> {
    for shape in scan::child_tags(tokens, src, "DataShape", container) {
        for definitions in scan::child_tags(tokens, src, "FieldDefinitions", shape) {
            for field in scan::child_tags(tokens, src, "FieldDefinition", definitions) {
                let Some(value) = scan::attribute(src, &tokens[field], "name")? else {
                    continue;
                };
                if value.of(src) == new.as_bytes() {
                    conflicts.push(label.to_string());
                } else if value.of(src) == old.as_bytes() {
                    add_field_edit(
                        src,
                        value,
                        new,
                        Place::FieldDefinition {
                            element: label.to_string(),
                        },
                        pass,
                    );
                }
            }
        }
    }
    for rows in scan::child_tags(tokens, src, "Rows", container) {
        for row in scan::child_tags(tokens, src, "Row", rows) {
            for child in scan::child_tags(tokens, src, old, row) {
                add_field_edit(
                    src,
                    tokens[child].name,
                    new,
                    Place::RowElement {
                        table: label.to_string(),
                    },
                    pass,
                );
                if tokens[child].kind == scan::Kind::Start {
                    if let Some(end) = scan::element_end_in(tokens, src, child) {
                        add_field_edit(
                            src,
                            tokens[end].name,
                            new,
                            Place::RowElement {
                                table: label.to_string(),
                            },
                            pass,
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Every direct child element (start tag) of `parent`, whatever its name.
pub(crate) fn child_elements(tokens: &[scan::Token], parent: usize) -> Vec<usize> {
    let Some(end) = scan::element_end(tokens, parent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut index = parent + 1;
    while index < end {
        if tokens[index].kind == scan::Kind::Start {
            out.push(index);
            index = scan::element_end(tokens, index).map_or(end, |e| e + 1);
        } else {
            index += 1;
        }
    }
    out
}

/// The data shape each declared property of an entity is typed by (`aspect.dataShape`), by name.
pub fn property_shapes(
    src: &[u8],
) -> Result<std::collections::BTreeMap<String, String>, scan::ScanError> {
    let tokens = scan::tokenize(src)?;
    let mut out = std::collections::BTreeMap::new();
    for token in &tokens {
        if matches!(token.kind, scan::Kind::Start | scan::Kind::Empty)
            && token.name.of(src) == b"PropertyDefinition"
        {
            let name = scan::attribute(src, token, "name")?;
            let shape = scan::attribute(src, token, "aspect.dataShape")?;
            if let (Some(name), Some(shape)) = (name, shape) {
                out.insert(
                    String::from_utf8_lossy(name.of(src)).into_owned(),
                    String::from_utf8_lossy(shape.of(src)).into_owned(),
                );
            }
        }
    }
    Ok(out)
}

/// Renames a field inside the InfoTable values of the named properties of one entity: the
/// `ThingProperties/<property>/Value/infoTable` of a Thing or ThingTemplate. `properties` are the
/// properties whose declared type is the data shape being renamed in (the caller resolves
/// inheritance, since a Thing's value is usually typed by its template).
pub fn scan_infotable_field(
    src: &[u8],
    properties: &std::collections::BTreeSet<String>,
    old: &str,
    new: &str,
) -> Result<FieldPass, scan::ScanError> {
    let tokens = scan::tokenize(src)?;
    let mut pass = XmlPass::new(src);
    let mut table_conflicts = Vec::new();
    let mut tables = 0;
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != scan::Kind::Start || token.name.of(src) != b"ThingProperties" {
            continue;
        }
        for property in child_elements(&tokens, index) {
            let name = String::from_utf8_lossy(tokens[property].name.of(src)).into_owned();
            if !properties.contains(&name) {
                continue;
            }
            for value in scan::child_tags(&tokens, src, "Value", property) {
                for table in scan::child_tags(&tokens, src, "infoTable", value) {
                    tables += 1;
                    rename_in_container(
                        src,
                        &tokens,
                        table,
                        &name,
                        old,
                        new,
                        &mut pass,
                        &mut table_conflicts,
                    )?;
                }
            }
        }
    }
    Ok(FieldPass {
        pass,
        old_found: false,
        new_found: false,
        table_conflicts,
        tables,
    })
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

pub(crate) fn add_field_edit(src: &[u8], span: scan::Span, new: &str, place: Place, pass: &mut XmlPass) {
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

    fn new(src: &[u8]) -> Self {
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

    fn line_of(&self, at: usize) -> usize {
        1 + self.newlines.partition_point(|&newline| newline < at)
    }
}

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

pub(crate) fn span_text(src: &[u8], span: scan::Span) -> &str {
    // scan validates the entire input as UTF-8 and only emits character-boundary spans.
    std::str::from_utf8(span.of(src)).expect("scanner returned a non-UTF-8 span")
}

fn excerpt(value: &str, hit_start: usize, line_only: bool) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(src: &[u8], pass: &XmlPass) -> Vec<u8> {
        splice::splice(src, &pass.edits).unwrap()
    }

    #[test]
    fn finds_attributes_text_and_script_with_their_places() {
        let src = br#"<?xml version="1.0"?>
<!-- Acme.App.Manager -->
<Thing name="Acme.App.Manager" projectName="Acme.App.Manager" thingTemplate="Acme.App.Manager">
  <Owner>Acme.App.Manager</Owner>
  <Code><![CDATA[var x = Things["Acme.App.Manager"]; // Acme.App.Manager]]></Code>
</Thing>"#;
        let pass = scan_xml(
            src,
            "Acme.App.Manager",
            refs::Mode::Entity,
            "Acme.New.Manager",
        )
        .unwrap();
        assert_eq!(pass.findings.len(), 6);
        assert!(
            matches!(pass.findings[0].place, Place::Attribute { ref element, ref attribute } if element == "Thing" && attribute == "name")
        );
        assert!(
            matches!(pass.findings[1].place, Place::Attribute { ref attribute, .. } if attribute == "projectName")
        );
        assert!(
            matches!(pass.findings[2].place, Place::Attribute { ref attribute, .. } if attribute == "thingTemplate")
        );
        assert!(
            matches!(pass.findings[3].place, Place::Text { ref element } if element == "Owner")
        );
        assert!(pass.findings[4..].iter().all(
            |finding| matches!(finding.place, Place::Cdata { ref element } if element == "Code")
        ));
        assert_eq!(pass.edits.len(), 6);
    }

    #[test]
    fn entity_and_prefix_modes_treat_dotted_suffixes_differently() {
        let src = br#"<R a="Acme.App.Manager.Child" b="Acme.App.ManagerX"/>"#;
        let entity = scan_xml(
            src,
            "Acme.App.Manager",
            refs::Mode::Entity,
            "Acme.New.Manager",
        )
        .unwrap();
        assert!(entity.findings.is_empty());
        let prefix = scan_xml(src, "Acme.App", refs::Mode::Prefix, "Acme.New").unwrap();
        assert_eq!(prefix.edits.len(), 2);
    }

    #[test]
    fn an_unqualified_name_applies_only_where_an_entity_is_named() {
        let src = br#"<Entities><Things><Thing name="T" thingTemplate="T"><PropertyDefinitions><PropertyDefinition name="T"/></PropertyDefinitions><Path>T/T1/A1</Path><Level>T</Level><Code><![CDATA[return "T"; Things["T"].Run(); Things_T;]]></Code></Thing></Things></Entities>"#;
        let pass = scan_xml(src, "T", refs::Mode::Entity, "U").unwrap();
        let tiers: Vec<(refs::Tier, bool)> = pass
            .findings
            .iter()
            .map(|finding| (finding.tier, finding.applied))
            .collect();
        assert_eq!(
            tiers,
            [
                (refs::Tier::Exact, true),   // the entity's own name
                (refs::Tier::Exact, true),   // thingTemplate names an entity
                (refs::Tier::Review, false), // a property that happens to share the word
                (refs::Tier::Review, false), // text inside a path
                (refs::Tier::Review, false), // a cell that equals the word
                (refs::Tier::Review, false), // `return "T"`: a label, not a lookup
                (refs::Tier::Exact, true),   // Things["T"]
                (refs::Tier::Exact, true),   // a mashup-derived id
            ]
        );
        assert_eq!(pass.edits.len(), 4);
    }

    #[test]
    fn derived_ids_inside_cdata_json_are_found() {
        let src =
            br#"<Content><![CDATA[{"DynamicThingShapes_Acme.App.Management_TS": {}}]]></Content>"#;
        let pass = scan_xml(
            src,
            "Acme.App.Management_TS",
            refs::Mode::Entity,
            "Acme.New.Management_TS",
        )
        .unwrap();
        assert_eq!(pass.findings.len(), 1);
        assert_eq!(pass.findings[0].tier, refs::Tier::Embedded);
        assert_eq!(pass.edits.len(), 1);
    }

    #[test]
    fn no_hit_has_no_edits_and_splices_identically() {
        let src = b"<R untouched=\"yes\">nothing here</R>";
        let pass = scan_xml(src, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
        assert!(pass.findings.is_empty());
        assert!(pass.edits.is_empty());
        assert_eq!(apply(src, &pass), src);
    }

    #[test]
    fn applied_rename_removes_old_safe_hits_and_preserves_the_count() {
        // Precondition: the replacement name does not occur in the original document.
        let src = br#"<R a="Acme.App.X"><T>Things_Acme.App.X</T><C><![CDATA[Things["Acme.App.X"]]]></C></R>"#;
        assert!(!String::from_utf8_lossy(src).contains("Acme.New.X"));
        let old = scan_xml(src, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
        let renamed = apply(src, &old);
        let remaining = scan_xml(&renamed, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
        assert!(remaining
            .findings
            .iter()
            .all(|finding| finding.tier == refs::Tier::Review));
        let new = scan_xml(&renamed, "Acme.New.X", refs::Mode::Entity, "Acme.App.X").unwrap();
        assert_eq!(new.findings.len(), old.findings.len());
    }

    #[test]
    fn rename_round_trip_restores_attributes_text_cdata_and_derived_ids() {
        let src = br#"<R a="Acme.App.X"><T>Acme.App.X</T><C><![CDATA[Things["Acme.App.X"] = DynamicThingShapes_Acme.App.X;]]></C></R>"#;
        let forward = scan_xml(src, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
        let renamed = apply(src, &forward);
        let reverse = scan_xml(&renamed, "Acme.New.X", refs::Mode::Entity, "Acme.App.X").unwrap();
        assert_eq!(apply(&renamed, &reverse), src);
    }

    #[test]
    fn malformed_xml_returns_the_scanner_error() {
        let error = scan_xml(b"<R><![CDATA[unfinished", "R", refs::Mode::Entity, "S").unwrap_err();
        assert!(matches!(
            error,
            scan::ScanError::Unterminated {
                what: "CDATA section",
                ..
            }
        ));
    }

    #[test]
    fn findings_report_lines_and_trimmed_bounded_excerpts() {
        let long = "x".repeat(140);
        let xml = format!("<R>\n  Acme.App.X  \n<C><![CDATA[{long} Acme.App.X tail]]></C>\n</R>");
        let pass = scan_xml(
            xml.as_bytes(),
            "Acme.App.X",
            refs::Mode::Entity,
            "Acme.New.X",
        )
        .unwrap();
        assert_eq!(
            pass.findings
                .iter()
                .map(|finding| finding.line)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(pass.findings[0].excerpt, "Acme.App.X");
        assert!(pass.findings[1].excerpt.contains("Acme.App.X"));
        assert_eq!(pass.findings[1].excerpt.chars().count(), 120);
    }

    #[test]
    fn text_pass_uses_file_places_lines_and_line_excerpts() {
        let src = b"first line\n  const x = Things[\"Acme.App.X\"];  \nlast line";
        let pass = scan_text(src, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
        assert_eq!(pass.findings.len(), 1);
        assert_eq!(pass.findings[0].place, Place::File);
        assert_eq!(pass.findings[0].line, 2);
        assert_eq!(
            pass.findings[0].excerpt,
            "const x = Things[\"Acme.App.X\"];"
        );
        assert_eq!(pass.edits.len(), 1);
    }

    #[test]
    fn text_pass_refuses_non_utf8_input() {
        assert!(scan_text(&[0xff], "Acme.App.X", refs::Mode::Entity, "Acme.New.X").is_err());
    }

    #[test]
    fn an_end_tag_that_does_not_close_the_open_element_is_refused() {
        // Attributing text to the wrong element would report a place that is not there.
        for src in [
            "<A><B></A>Acme.App.X</B>",
            "<A></B>Acme.App.X</A>",
            "</A>Acme.App.X",
        ] {
            let result = scan_xml(
                src.as_bytes(),
                "Acme.App.X",
                refs::Mode::Entity,
                "Acme.New.X",
            );
            assert!(
                matches!(result, Err(scan::ScanError::Malformed { .. })),
                "{src}: {result:?}"
            );
        }
    }

    #[test]
    fn lines_are_counted_from_one_with_crlf_and_a_hit_on_the_first_line() {
        let src = b"<A n=\"Acme.App.X\">\r\n<B>\r\n  <![CDATA[x\r\nAcme.App.X]]>\r\n</B></A>";
        let pass = scan_xml(src, "Acme.App.X", refs::Mode::Entity, "Acme.New.X").unwrap();
        let lines: Vec<usize> = pass.findings.iter().map(|finding| finding.line).collect();
        assert_eq!(lines, vec![1, 4]);
    }

    #[test]
    fn a_hit_dense_document_is_linear_in_its_size() {
        // Thousands of hits in one large payload: a rescan of the prefix per hit would take
        // minutes here, a binary search takes milliseconds.
        let mut body = String::from("<A><![CDATA[");
        for _ in 0..40_000 {
            body.push_str("Things[\"Acme.App.X\"].Run(); // padding to make the payload large\n");
        }
        body.push_str("]]></A>");
        let started = std::time::Instant::now();
        let pass = scan_xml(
            body.as_bytes(),
            "Acme.App.X",
            refs::Mode::Entity,
            "Acme.New.X",
        )
        .unwrap();
        assert_eq!(pass.findings.len(), 40_000);
        assert_eq!(pass.findings.last().unwrap().line, 40_000);
        assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
    }

    #[test]
    fn field_pass_is_structural_exact_and_keeps_other_tables_untouched() {
        let src = br#"<Thing><ConfigurationTables>
<ConfigurationTable dataShapeName="P.D" name="T"><DataShape><FieldDefinitions><FieldDefinition name="Period"/><FieldDefinition name="PeriodDisplayName"/></FieldDefinitions></DataShape><Rows><Row><Period><![CDATA[x<y]]></Period><PeriodDisplayName>x</PeriodDisplayName></Row><Row><Period/></Row></Rows></ConfigurationTable>
<ConfigurationTable dataShapeName="P.Other" name="U"><DataShape><FieldDefinitions><FieldDefinition name="Period"/></FieldDefinitions></DataShape><Rows><Row><Period>stay</Period></Row></Rows></ConfigurationTable>
</ConfigurationTables></Thing>"#;
        let field = scan_configuration_field(src, "P.D", "Period", "PeriodKey").unwrap();
        assert_eq!(field.tables, 1);
        assert!(field.table_conflicts.is_empty());
        let changed = splice::splice(src, &field.pass.edits).unwrap();
        let expected = String::from_utf8_lossy(src)
            .replacen("name=\"Period\"", "name=\"PeriodKey\"", 1)
            .replacen(
                "<Period><![CDATA[x<y]]></Period>",
                "<PeriodKey><![CDATA[x<y]]></PeriodKey>",
                1,
            )
            .replacen("<Period/>", "<PeriodKey/>", 1);
        assert_eq!(changed, expected.as_bytes());
        assert!(String::from_utf8(changed)
            .unwrap()
            .contains("<Period>stay</Period>"));
    }

    #[test]
    fn service_script_applies_only_resolved_callers_and_reviews_other_uses() {
        let src = br#"me . Run ();
this.Run();
Things["P.Shape"] . Run ();
Things['P.Child'].Run();
Things.Other.Run();
const a = Things["P.Shape"]; let b=Things.Other; var c = Things['P.Child'];
a.Run(); b.Run(); c.Run();
Things.Unrelated.Run(); unknown.Run(); const q = "Run"; x["Run"]; // .Run
/** @function Run */"#;
        let callers = BTreeSet::from([
            "P.Shape".to_string(),
            "P.Child".to_string(),
            "Other".to_string(),
        ]);
        let pass = scan_service_script(src, "Run", "Execute", true, true, &callers).unwrap();
        let changed = String::from_utf8(apply(src, &pass)).unwrap();
        assert_eq!(changed.matches("Execute").count(), 9);
        assert!(changed.contains("Things.Unrelated.Run()") && changed.contains("unknown.Run()"));
        assert!(
            changed.contains("\"Run\"")
                && changed.contains("[\"Run\"]")
                && changed.contains(".Run")
        );
        assert_eq!(
            pass.findings
                .iter()
                .filter(|finding| !finding.applied)
                .count(),
            5
        );
    }

    #[test]
    fn mashup_service_pairs_are_contextual_and_leftovers_are_review() {
        let src = br#"{
  "Data": {"D": {"DataName": "D", "EntityName": "P.Shape", "Services": [{"Name": "Run", "Target": "Run"}]},
             "U": {"DataName": "U", "EntityName": "P.Other", "Services": [{"Name": "Run", "Target": "Run"}]}},
  "DataBindings": [{"SourceId": "Run", "SourceName": "Run", "SourceSection": "D", "TargetId": "Run", "TargetSection": "D"}],
  "Events": [{"EventHandlerId": "D", "EventHandlerService": "Run", "EventTriggerId": "Run", "EventTriggerSection": "D"}],
  "Label": "Run"
}"#;
        let callers = BTreeSet::from(["P.Shape".to_string()]);
        let pass = scan_service_mashup(src, "Run", "Execute", &callers, false).unwrap();
        let changed = String::from_utf8(apply(src, &pass)).unwrap();
        assert_eq!(changed.matches("Execute").count(), 7);
        assert_eq!(
            pass.findings
                .iter()
                .filter(|finding| !finding.applied)
                .count(),
            3
        );
        assert!(changed.contains("\"EntityName\": \"P.Other\", \"Services\": [{\"Name\": \"Run\""));
        assert!(changed.contains("\"Label\": \"Run\""));
    }

    #[test]
    fn service_config_uses_spans_and_leaves_a_bare_override_for_review() {
        let src = br#"[validate]
inherited_overrides = ["P.Shape.Run", "Run", "P.Other.Run"]
[[project]]
name = "P"
[project.deploy]
entry_point_thing = "P.Child"
deploy_service = "Run"
[[project.deploy.post_import]]
thing = "P.Child"
service = "Run"
[[project.deploy.post_import]]
thing = "P.Other"
service = "Run"
"#;
        let callers = BTreeSet::from(["P.Shape".to_string(), "P.Child".to_string()]);
        let pass = scan_service_config(src, "Run", "Execute", &callers).unwrap();
        let changed = String::from_utf8(apply(src, &pass)).unwrap();
        assert!(
            changed.contains("P.Shape.Execute") && changed.contains("deploy_service = \"Execute\"")
        );
        assert!(
            changed.contains("\"Run\", \"P.Other.Run\"")
                && changed.contains("thing = \"P.Other\"\nservice = \"Run\"")
        );
        assert_eq!(
            pass.findings
                .iter()
                .filter(|finding| finding.applied)
                .count(),
            3
        );
        assert_eq!(
            pass.findings
                .iter()
                .filter(|finding| !finding.applied)
                .count(),
            1
        );
    }

    #[test]
    fn table_pass_changes_only_real_tables_and_contextual_script_literals() {
        let src = br#"<Thing><ConfigurationTableDefinitions><ConfigurationTableDefinition name="Limits_CT" dataShapeName="P.Limits_CT"/></ConfigurationTableDefinitions><ConfigurationTables><ConfigurationTable name="Limits_CT" dataShapeName="P.Limits_CT"><DataShape/><Rows/></ConfigurationTable></ConfigurationTables><ThingShape><ServiceImplementations><ServiceImplementation name="S"><ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code><![CDATA[
let a = {tableName : "Limits_CT"}; let b = {'tableName': 'Limits_CT'}; let c = {"tableName" : "Limits_CT"};
const TABLE = "Limits_CT"; x.Limits_CT; x["Limits_CT"];
]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></Thing>"#;
        let scanned = scan_configuration_table(src, "Limits_CT", "Bounds_CT", true, true).unwrap();
        assert!(scanned.old_definition);
        assert_eq!(scanned.renamed_tables, 2);
        assert_eq!(scanned.new_tables, 0);
        let changed = String::from_utf8(apply(src, &scanned.pass)).unwrap();
        assert_eq!(changed.matches("name=\"Bounds_CT\"").count(), 2);
        assert!(changed.contains("name=\"Script\""));
        assert_eq!(changed.matches("tableName : \"Bounds_CT\"").count(), 1);
        assert!(
            changed.contains("'tableName': 'Bounds_CT'")
                && changed.contains("\"tableName\" : \"Bounds_CT\"")
        );
        assert!(
            changed.contains("const TABLE = \"Limits_CT\"")
                && changed.contains("x.Limits_CT")
                && changed.contains("x[\"Limits_CT\"]")
        );
        assert_eq!(
            scanned
                .pass
                .findings
                .iter()
                .filter(|finding| finding.applied)
                .count(),
            5
        );
        assert_eq!(
            scanned
                .pass
                .findings
                .iter()
                .filter(|finding| !finding.applied)
                .count(),
            3
        );

        let reviews = scan_table_script(
            br#"const x = "Limits_CT"; a.Limits_CT; a['Limits_CT'];"#,
            "Limits_CT",
            "Bounds_CT",
            false,
        )
        .unwrap();
        assert!(reviews.edits.is_empty());
        assert_eq!(reviews.findings.len(), 3);
    }
    #[test]
    fn a_call_argument_is_not_a_shorthand_property() {
        // `f(a, cardUid, b)` has the same neighbours as `{ a, cardUid, b }`; only the bracket differs.
        let own = |script: &str| {
            scan_param_script(
                script.as_bytes(),
                "Run",
                "cardUid",
                "cardId",
                true,
                false,
                &BTreeSet::new(),
            )
            .unwrap()
        };
        let call = own("logger.warn(\"x\", me.name, cardUid, rows.length);
const n = Number(cardUid);
");
        assert_eq!(
            call.edits.len(),
            2,
            "both uses are identifiers of the parameter"
        );
        let shorthand = own("const o = { a, cardUid, b };
return Number(cardUid);
");
        assert!(
            shorthand.edits.is_empty(),
            "a shorthand property makes the whole script a review item"
        );
        assert!(shorthand.findings.iter().any(|finding| !finding.applied));
        let array = own("const list = [a, cardUid, b];
");
        assert_eq!(
            array.edits.len(),
            1,
            "an array element is an identifier use, not a property"
        );
    }

    #[test]
    fn a_script_uses_an_identifier_only_outside_strings_comments_and_member_access() {
        let uses = |script: &str| script_uses_identifier(script.as_bytes(), "result");
        assert!(uses("var result = 1;"));
        assert!(uses("return result;"));
        assert!(
            !uses(
                "const s = \"result\"; // result
"
            ),
            "a string and a comment are not uses"
        );
        assert!(
            !uses("return row.result;"),
            "a property is not the variable"
        );
    }

    #[test]
    fn a_service_rename_follows_run_time_permissions_keyed_by_its_name() {
        let src = br#"<Entities><ThingShapes><ThingShape name="P.S"><ServiceDefinitions><ServiceDefinition name="Run"/></ServiceDefinitions><RunTimePermissions><Permissions resourceName="Run"><ServiceInvoke><Principal name="G" type="Group"/></ServiceInvoke></Permissions><Permissions resourceName="Other"/></RunTimePermissions></ThingShape></ThingShapes></Entities>"#;
        let callers = BTreeSet::from(["P.S".to_string()]);
        let pass = scan_service_entity(src, "Run", "Execute", true, true, &callers).unwrap();
        let out = String::from_utf8(splice::splice(src, &pass.edits).unwrap()).unwrap();
        assert!(
            out.contains("resourceName=\"Execute\"") && out.contains("resourceName=\"Other\""),
            "{out}"
        );
        // An entity that is not in the callers' scope grants nothing on this service.
        let outside = scan_service_entity(src, "Run", "Execute", false, false, &callers).unwrap();
        assert!(outside.edits.is_empty());
    }

    #[test]
    fn a_param_rename_lists_sql_placeholders_for_a_person() {
        let src = br#"<Entities><Things><Thing name="P.T"><ServiceDefinitions><ServiceDefinition name="Q"><ParameterDefinitions><FieldDefinition name="dashboardId" baseType="STRING"/></ParameterDefinitions></ServiceDefinition></ServiceDefinitions><ServiceImplementations><ServiceImplementation name="Q" handlerName="SQLQuery"><ConfigurationTables><ConfigurationTable name="Query"><Rows><Row><sql><![CDATA[SELECT * FROM t WHERE id = [[dashboardId]] AND x = <<dashboardId>>]]></sql></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></Thing></Things></Entities>"#;
        let scanned = scan_param_entity(
            src,
            "Q",
            "dashboardId",
            "boardId",
            true,
            true,
            &BTreeSet::new(),
        )
        .unwrap();
        let review: Vec<&Finding> = scanned
            .pass
            .findings
            .iter()
            .filter(|finding| !finding.applied)
            .collect();
        assert_eq!(review.len(), 2, "{:?}", scanned.pass.findings);
        assert!(review
            .iter()
            .all(|finding| finding.excerpt.contains("SQL placeholder")));
    }

    /// The script with every service rename applied, for the one entity `A` and its own calls.
    fn service_rename(script: &str, rename_function: bool) -> (String, XmlPass) {
        let callers = BTreeSet::from(["A".to_string()]);
        let pass = scan_service_script(
            script.as_bytes(),
            "Run",
            "Execute",
            true,
            rename_function,
            &callers,
        )
        .unwrap();
        (String::from_utf8(apply(script.as_bytes(), &pass)).unwrap(), pass)
    }

    fn review_count(pass: &XmlPass) -> usize {
        pass.findings.iter().filter(|finding| !finding.applied).count()
    }

    #[test]
    fn a_service_call_is_found_wherever_the_parser_finds_it() {
        // A call split over lines, and one inside a template literal, which a byte recognizer
        // read as a single opaque token.
        let (changed, _) = service_rename("Things[\"A\"]\n  .Run(1);", false);
        assert_eq!(changed, "Things[\"A\"]\n  .Execute(1);");
        let (changed, _) = service_rename("var s = `${Things.A.Run()} and ${me.Run()}`;", false);
        assert_eq!(changed, "var s = `${Things.A.Execute()} and ${me.Execute()}`;");
        let (changed, _) = service_rename("var q = a / b / c; Things.A.Run();", false);
        assert_eq!(changed, "var q = a / b / c; Things.A.Execute();");
        let (changed, _) = service_rename("Things.A[\"Run\"]();", false);
        assert_eq!(changed, "Things.A[\"Execute\"]();");
    }

    #[test]
    fn text_that_only_looks_like_a_call_is_left_for_review() {
        let script = "var s = \"me.Run()\"; var r = /me.Run()/; // me.Run()\n";
        let (changed, pass) = service_rename(script, false);
        assert_eq!(changed, script);
        assert_eq!(review_count(&pass), 3, "{:?}", pass.findings);
    }

    #[test]
    fn a_variable_is_followed_only_when_it_can_only_be_one_caller() {
        let (changed, _) = service_rename("var t = Things.A; t.Run();", false);
        assert_eq!(changed, "var t = Things.A; t.Execute();");
        // Reassigned, redeclared as another Thing, or taken as a parameter: nothing is proved.
        for script in [
            "var t = Things.A; t = other; t.Run();",
            "var t = Things.A; var t = Things.B; t.Run();",
            "var t = Things.A; function f(t) { t.Run(); }",
            "var t = Things.A; for (t in o) { t.Run(); }",
        ] {
            let (changed, pass) = service_rename(script, false);
            assert_eq!(changed, script);
            assert!(review_count(&pass) > 0, "{script}");
        }
    }

    #[test]
    fn a_function_tag_is_renamed_inside_comments_only() {
        let script = "var s = \"@function Run\";\n/** @function Run */\nvar t = '@function Run';";
        let (changed, _) = service_rename(script, true);
        assert_eq!(
            changed,
            "var s = \"@function Run\";\n/** @function Execute */\nvar t = '@function Run';"
        );
        let (unchanged, _) = service_rename(script, false);
        assert_eq!(unchanged, script);
    }

    #[test]
    fn a_service_script_the_parser_refuses_is_not_edited() {
        // Rhino's `for each` is not ECMAScript; the call inside it is reported, not edited.
        let script = "for each (x in y) { Things.A.Run(); }";
        let (changed, pass) = service_rename(script, false);
        assert_eq!(changed, script);
        assert_eq!(review_count(&pass), 1, "{:?}", pass.findings);
    }

    fn param_rename(script: &str, own: bool, local: bool) -> (String, XmlPass) {
        let callers = BTreeSet::from(["A".to_string()]);
        let pass = scan_param_script(
            script.as_bytes(),
            "Svc",
            "old",
            "fresh",
            own,
            local,
            &callers,
        )
        .unwrap();
        (String::from_utf8(apply(script.as_bytes(), &pass)).unwrap(), pass)
    }

    fn reasons(pass: &XmlPass) -> Vec<&str> {
        pass.findings
            .iter()
            .filter(|finding| !finding.applied)
            .map(|finding| finding.excerpt.as_str())
            .collect()
    }

    #[test]
    fn a_free_input_is_renamed_through_templates_and_division() {
        let script = "var s = `${old}`; var q = a / old / b; f(old);";
        let (changed, _) = param_rename(script, true, false);
        assert_eq!(changed, "var s = `${fresh}`; var q = a / fresh / b; f(fresh);");
        // Not a member name, an object key or a string; the key and the string are reviewed, the
        // member name is not (a dotted occurrence is never a hit).
        let script = "x.old; var o = { old: 1 }; var s = 'old';";
        let (changed, pass) = param_rename(script, true, false);
        assert_eq!(changed, script);
        assert_eq!(review_count(&pass), 2, "{:?}", pass.findings);
    }

    #[test]
    fn a_script_that_gives_the_free_input_a_meaning_of_its_own_is_left_whole() {
        for (script, reason) in [
            ("var old = 1; use(old);", "local re-declaration"),
            ("var { a: old } = x; use(old);", "local re-declaration"),
            ("function old() {} use(old);", "local re-declaration"),
            ("f(function (old) {}); use(old);", "nested function or catch parameter"),
            ("f((old) => old); use(old);", "nested function or catch parameter"),
            ("try {} catch (old) {} use(old);", "nested function or catch parameter"),
            ("var o = { old }; use(old);", "shorthand property"),
            ("var { old } = x; use(old);", "shorthand property"),
        ] {
            let (changed, pass) = param_rename(script, true, false);
            assert_eq!(changed, script, "{script}");
            let reasons = reasons(&pass);
            assert!(reasons.iter().any(|text| text.contains(reason)), "{script}: {reasons:?}");
        }
    }

    #[test]
    fn a_call_to_the_service_renames_only_the_keys_of_a_literal_first_argument() {
        let script = "Things.A.Svc({ old: 1, 'old': 2, other: old, [old]: 3, ...old });\n\
                      Things.B.Svc({ old: 1 });\n\
                      Things[\"A\"]\n  .Svc(\n  { old: 1 });";
        let (changed, _) = param_rename(script, false, false);
        assert_eq!(
            changed,
            "Things.A.Svc({ fresh: 1, 'fresh': 2, other: old, [old]: 3, ...old });\n\
             Things.B.Svc({ old: 1 });\n\
             Things[\"A\"]\n  .Svc(\n  { fresh: 1 });"
        );
    }

    #[test]
    fn a_call_whose_keys_cannot_be_proved_is_reviewed() {
        for script in ["Things.A.Svc(args);", "Things.A.Svc();", "Things.A.Svc(...args);"] {
            let (changed, pass) = param_rename(script, false, false);
            assert_eq!(changed, script);
            assert!(
                reasons(&pass)
                    .iter()
                    .any(|text| text.contains("non-literal first argument")),
                "{script}"
            );
        }
    }

    #[test]
    fn a_param_call_through_me_needs_the_local_service_and_a_variable_needs_one_thing() {
        let (changed, _) = param_rename("me.Svc({ old: 1 }); this.Svc({ old: 2 });", false, true);
        assert_eq!(changed, "me.Svc({ fresh: 1 }); this.Svc({ fresh: 2 });");
        let (changed, _) = param_rename("me.Svc({ old: 1 });", false, false);
        assert_eq!(changed, "me.Svc({ old: 1 });");
        let (changed, _) = param_rename("var t = Things.A; t.Svc({ old: 1 });", false, false);
        assert_eq!(changed, "var t = Things.A; t.Svc({ fresh: 1 });");
        let ambiguous = "var t = Things.A; t = other; t.Svc({ old: 1 });";
        assert_eq!(param_rename(ambiguous, false, false).0, ambiguous);
    }

    #[test]
    fn a_param_script_the_parser_refuses_is_left_for_review_with_the_reason() {
        let script = "for each (x in y) { old = 1; Things.A.Svc({ old: 1 }); }";
        let (changed, pass) = param_rename(script, true, false);
        assert_eq!(changed, script);
        let reasons = reasons(&pass);
        assert_eq!(reasons.len(), 2, "{reasons:?}");
        assert!(reasons
            .iter()
            .all(|text| text.contains("script could not be parsed; left for review")));
    }

    #[test]
    fn an_identifier_use_is_found_in_a_template_and_refused_when_the_script_cannot_be_read() {
        let uses = |script: &str| script_uses_identifier(script.as_bytes(), "result");
        assert!(uses("var s = `${result}`;"));
        assert!(!uses("var s = `result`; var r = /result/; x.result;"));
        assert!(uses("var o = { result: 1 };"), "an object key counts, as before");
        // Unparseable: a whole-word mention anywhere is refused, a longer word is not a mention.
        assert!(uses("for each (a in b) { // result\n }"));
        assert!(!uses("for each (a in b) { results; $result; }"));
        assert!(script_uses_identifier(&[b'r', b'e', b's', b'u', b'l', b't', 0xff], "result"));
    }

    #[test]
    fn a_table_name_is_a_selector_only_as_the_value_of_a_table_name_key() {
        let apply_table = |script: &str| {
            let pass =
                scan_table_script(script.as_bytes(), "Limits_CT", "Bounds_CT", true).unwrap();
            (String::from_utf8(apply(script.as_bytes(), &pass)).unwrap(), pass)
        };
        let (changed, _) = apply_table("var s = `${f({ tableName: \"Limits_CT\" })}`;");
        assert_eq!(changed, "var s = `${f({ tableName: \"Bounds_CT\" })}`;");
        let (changed, _) = apply_table("f({ tableName\n  :\n  'Limits_CT' });");
        assert_eq!(changed, "f({ tableName\n  :\n  'Bounds_CT' });");
        // A conditional with the same neighbours is not an object property.
        let script = "var v = ok ? tableName : \"Limits_CT\";";
        let (changed, pass) = apply_table(script);
        assert_eq!(changed, script);
        assert_eq!(review_count(&pass), 1);
        // Another key, or a member name, is review.
        let script = "f({ other: \"Limits_CT\" }); x.Limits_CT;";
        let (changed, pass) = apply_table(script);
        assert_eq!(changed, script);
        assert_eq!(review_count(&pass), 2);
    }

    #[test]
    fn a_table_script_the_parser_refuses_is_not_edited() {
        let script = "for each (a in b) { f({ tableName: \"Limits_CT\" }); }";
        let pass = scan_table_script(script.as_bytes(), "Limits_CT", "Bounds_CT", true).unwrap();
        assert!(pass.edits.is_empty());
        assert_eq!(review_count(&pass), 1, "{:?}", pass.findings);
    }
}
