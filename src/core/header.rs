//! Leading service JSDoc headers, read without changing the script around them.

use super::scan::{self, Kind};

#[derive(Clone, Debug)]
pub(crate) struct HeaderPolicy {
    pub(crate) require_result_shape: bool,
    pub(crate) max_columns: usize,
}

impl Default for HeaderPolicy {
    fn default() -> Self {
        Self {
            require_result_shape: true,
            max_columns: 100,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) base_type: String,
    pub(crate) description: String,
    pub(crate) ordinal: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Definition {
    pub(crate) description: String,
    pub(crate) inputs: Vec<Field>,
    pub(crate) result: Field,
    pub(crate) result_shape: Option<String>,
}

/// One JSDoc tag: its value runs on over the lines below it until the next tag, as JSDoc reads it.
#[derive(Clone, Debug)]
pub(crate) struct Tag {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) line: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Header {
    /// 1-based lines of the `/**` and of the `*/`.
    pub(crate) line: usize,
    pub(crate) end_line: usize,
    pub(crate) tags: Vec<Tag>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) rule: &'static str,
    pub(crate) line: usize,
    pub(crate) message: String,
}

fn value(src: &[u8], token: &scan::Token, name: &str) -> Option<String> {
    let span = scan::attribute(src, token, name).ok()??;
    std::str::from_utf8(span.of(src))
        .ok()
        .map(scan::decode_entities)
}

/// Read the definition sidecar. A malformed or unrelated sidecar is intentionally skipped by
/// the contract gate: it must not turn an absent entity export into a script error.
pub(crate) fn definition(src: &str) -> Option<Definition> {
    let bytes = src.as_bytes();
    let tokens = scan::tokenize(bytes).ok()?;
    let service = tokens.iter().position(|token| {
        matches!(token.kind, Kind::Start | Kind::Empty)
            && token.name.of(bytes) == b"ServiceDefinition"
    })?;
    let parameter_definitions = scan::child_tags(&tokens, bytes, "ParameterDefinitions", service)
        .into_iter()
        .next();
    let mut inputs = parameter_definitions
        .into_iter()
        .flat_map(|parent| scan::child_tags(&tokens, bytes, "FieldDefinition", parent))
        .filter_map(|index| field(bytes, &tokens[index]))
        .collect::<Vec<_>>();
    inputs.sort_by_key(|field| field.ordinal);
    let result = scan::child_tags(&tokens, bytes, "ResultType", service)
        .into_iter()
        .next()
        .and_then(|index| field(bytes, &tokens[index]))?;
    let result_shape = scan::child_tags(&tokens, bytes, "ResultType", service)
        .into_iter()
        .next()
        .and_then(|index| value(bytes, &tokens[index], "aspect.dataShape"));
    Some(Definition {
        description: value(bytes, &tokens[service], "description").unwrap_or_default(),
        inputs,
        result,
        result_shape,
    })
}

fn field(src: &[u8], token: &scan::Token) -> Option<Field> {
    Some(Field {
        name: value(src, token, "name").unwrap_or_default(),
        base_type: value(src, token, "baseType").unwrap_or_else(|| "*".to_string()),
        description: value(src, token, "description").unwrap_or_default(),
        ordinal: value(src, token, "ordinal")
            .and_then(|n| n.parse().ok())
            .unwrap_or(usize::MAX),
    })
}

/// The text of one header line without its `/**`, leading `*` and `*/`.
fn body(line: &str) -> &str {
    let line = line.trim_end_matches(['\r', '\n']);
    let line = line.split("*/").next().unwrap_or(line).trim_start();
    let line = line.strip_prefix("/**").unwrap_or(line);
    line.trim_start().strip_prefix('*').unwrap_or(line).trim()
}

/// Parse only the first JSDoc block, after optional blank lines.
pub(crate) fn parse(source: &str) -> Option<Header> {
    let mut lines = source.split('\n').enumerate();
    let (start, first) = lines.find(|(_, text)| !text.trim().is_empty())?;
    if !first.trim_start().starts_with("/**") {
        return None;
    }
    let mut tags: Vec<Tag> = Vec::new();
    for (index, text) in std::iter::once((start, first)).chain(lines) {
        let content = body(text);
        if let Some(rest) = content.strip_prefix('@') {
            let (name, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            tags.push(Tag {
                name: name.to_string(),
                value: value.trim().to_string(),
                line: index + 1,
            });
        } else if let Some(tag) = tags.last_mut().filter(|_| !content.is_empty()) {
            tag.value = format!("{} {content}", tag.value).trim().to_string();
        }
        if text.contains("*/") {
            return Some(Header {
                line: start + 1,
                end_line: index + 1,
                tags,
            });
        }
    }
    None
}

fn param_name(value: &str) -> Option<&str> {
    let rest = value.trim_start();
    let rest = rest
        .strip_prefix('{')
        .and_then(|text| text.find('}').map(|at| &text[at + 1..]))
        .unwrap_or(rest);
    rest.split_whitespace().next()
}

pub(crate) fn findings(
    source: &str,
    service: &str,
    definition: Option<&Definition>,
    policy: &HeaderPolicy,
) -> Vec<Finding> {
    let header = parse(source);
    let line = header.as_ref().map_or(1, |header| header.line);
    let mut out = Vec::new();
    if let Some(definition) = definition {
        let Some(header) = header.as_ref() else {
            return vec![Finding {
                rule: "contract",
                line,
                message: "no JSDoc header; `twaco lint fix-headers` writes one from definition.xml"
                    .to_string(),
            }];
        };
        let functions: Vec<_> = header
            .tags
            .iter()
            .filter(|tag| tag.name == "function")
            .collect();
        if functions.len() != 1 || functions.first().is_none_or(|tag| tag.value != service) {
            let message = match functions.as_slice() {
                [] => format!("no @function; the service is {service}"),
                [one] => format!("@function names {}, the service is {service}", one.value),
                _ => format!(
                    "{} @function tags; keep one, for {service}",
                    functions.len()
                ),
            };
            out.push(Finding {
                rule: "contract",
                line: header.line,
                message,
            });
        }
        let params: Vec<_> = header
            .tags
            .iter()
            .filter(|tag| tag.name == "param")
            .filter_map(|tag| param_name(&tag.value))
            .collect();
        let wanted: Vec<_> = definition
            .inputs
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        for name in &params {
            if !wanted.contains(name) {
                out.push(Finding {
                    rule: "contract",
                    line: header.line,
                    message: format!("@param {name} is not an input"),
                });
            }
        }
        for name in &wanted {
            if !params.contains(name) {
                out.push(Finding {
                    rule: "contract",
                    line: header.line,
                    message: format!("input {name} has no @param"),
                });
            }
        }
        let present: Vec<_> = params
            .into_iter()
            .filter(|name| wanted.contains(name))
            .collect();
        if present != wanted {
            out.push(Finding {
                rule: "contract",
                line: header.line,
                message: format!(
                    "@param order is {}; the definition's is {}",
                    present.join(", "),
                    wanted.join(", ")
                ),
            });
        }
        let returns: Vec<_> = header
            .tags
            .iter()
            .filter(|tag| tag.name == "returns" || tag.name == "return")
            .collect();
        if returns.len() != 1 {
            out.push(Finding {
                rule: "contract",
                line: header.line,
                message: "no @returns".to_string(),
            });
        } else if policy.require_result_shape
            && definition
                .result_shape
                .as_ref()
                .is_some_and(|shape| !returns[0].value.contains(shape))
        {
            out.push(Finding {
                rule: "contract",
                line: returns[0].line,
                message: format!(
                    "@returns does not name the result DataShape {}",
                    definition.result_shape.as_ref().unwrap()
                ),
            });
        }
    }
    if let Some(header) = header {
        for (number, text) in source.lines().enumerate().take(header.end_line) {
            if number + 1 >= header.line && text.chars().count() > policy.max_columns {
                out.push(Finding {
                    rule: "wrap",
                    line: number + 1,
                    message: format!("header line is longer than {} columns", policy.max_columns),
                });
            }
        }
        let after = source
            .lines()
            .enumerate()
            .skip(header.end_line)
            .find(|(_, text)| !text.trim().is_empty());
        if let Some((number, text)) = after {
            let trimmed = text.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("/*") {
                out.push(Finding { rule: "first-statement", line: number + 1, message: "the first statement follows the header directly; fold this comment into the header or move it beside the code it explains".to_string() });
            }
        }
    }
    out
}

pub(crate) fn jsdoc_type(base_type: &str) -> &'static str {
    match base_type {
        "DATETIME" => "Date",
        "NUMBER" | "INTEGER" | "LONG" => "number",
        "BOOLEAN" => "boolean",
        "INFOTABLE" | "JSON" => "Object",
        "NOTHING" => "void",
        name if name.ends_with("NAME") || matches!(name, "STRING" | "TEXT" | "HTML" | "XML") => {
            "string"
        }
        _ => "*",
    }
}

fn wrapped(out: &mut String, prefix: &str, text: &str, width: usize) {
    let mut line = prefix.to_string();
    for word in text.split_whitespace() {
        if line.len() > prefix.len() && line.chars().count() + 1 + word.chars().count() > width {
            out.push_str(line.trim_end());
            out.push('\n');
            // Prose wraps flush; a tag's description continues indented under the tag.
            line = if prefix == " * " { " * " } else { " *   " }.to_string();
        }
        if line.len() > prefix.len() && !line.ends_with(' ') {
            line.push(' ');
        }
        line.push_str(word);
    }
    // An empty description leaves `- ` or a type with nothing after it; no line ends in a space.
    out.push_str(line.trim_end());
    out.push('\n');
}

pub(crate) fn generate(
    service: &str,
    definition: &Definition,
    policy: &HeaderPolicy,
    ending: &str,
) -> String {
    let mut out = String::from("/**\n");
    if !definition.description.trim().is_empty() {
        wrapped(&mut out, " * ", &definition.description, policy.max_columns);
        out.push_str(" *\n");
    }
    out.push_str(&format!(" * @function {service}\n"));
    for field in &definition.inputs {
        wrapped(
            &mut out,
            &format!(
                " * @param {{{}}} {} - ",
                jsdoc_type(&field.base_type),
                field.name
            ),
            &field.description,
            policy.max_columns,
        );
    }
    let result_text = definition
        .result_shape
        .as_ref()
        .map(|shape| format!("An InfoTable shaped by {shape}."))
        .unwrap_or_else(|| definition.result.description.clone());
    wrapped(
        &mut out,
        &format!(
            " * @returns {{{}}} ",
            jsdoc_type(&definition.result.base_type)
        ),
        &result_text,
        policy.max_columns,
    );
    out.push_str(" */\n");
    if ending != "\n" {
        out.replace('\n', ending)
    } else {
        out
    }
}

/// `generate`, with the definition's description cut until the header passes the comment rules
/// `enabled` names (by finding rule, such as `length`):
/// the whole description, then fewer leading sentences, then none. The definition keeps the full
/// text; only the header's copy is shortened, so generated code passes the solution's own preset.
pub(crate) fn generate_fitting(
    service: &str,
    definition: &Definition,
    policy: &HeaderPolicy,
    comments: &super::comments::CommentPolicy,
    enabled: &dyn Fn(&str) -> bool,
    ending: &str,
) -> String {
    let sentences = leading_sentences(&definition.description);
    let mut candidates: Vec<String> = (1..=sentences.len())
        .rev()
        .map(|count| sentences[..count].concat().trim().to_string())
        .collect();
    candidates.push(String::new());
    let mut shortened = definition.clone();
    for description in candidates {
        shortened.description = description;
        let header = generate(service, &shortened, policy, "\n");
        let measure = super::comments::measure(&header);
        // The whole-script budgets would measure the header alone here, so they cannot judge it.
        let fits = super::comments::findings(&measure, comments)
            .iter()
            .all(|finding| matches!(finding.rule, "ratio" | "line-run") || !enabled(finding.rule));
        if fits || shortened.description.is_empty() {
            return if ending != "\n" {
                header.replace('\n', ending)
            } else {
                header
            };
        }
    }
    unreachable!("the empty description is always the last candidate")
}

/// The description split after each `.`, `!` or `?` that ends a sentence, each piece keeping its
/// own text and the space after it.
fn leading_sentences(text: &str) -> Vec<&str> {
    let text = text.trim();
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    for (at, byte) in bytes.iter().enumerate() {
        let ends = matches!(byte, b'.' | b'!' | b'?')
            && bytes
                .get(at + 1)
                .is_none_or(|next| next.is_ascii_whitespace());
        if ends {
            out.push(&text[start..at + 1]);
            start = at + 1;
        }
    }
    if start < text.len() && !text[start..].trim().is_empty() {
        out.push(&text[start..]);
    }
    out
}

/// What `lint fix-headers` would do to one script.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Repair {
    /// The repaired script, when anything changed.
    pub(crate) fixed: Option<String>,
    pub(crate) changes: Vec<String>,
    /// Header findings this repair leaves for a person, because fixing them means rewriting prose.
    pub(crate) not_fixable: Vec<String>,
}

/// One tag and the lines that run on from it, kept byte for byte.
struct Group {
    name: String,
    value: String,
    lines: Vec<String>,
}

/// Bring the header's tags to the definition's contract: `@function`, one `@param` per input in
/// ordinal order, and `@returns`. Every existing tag keeps its own bytes and moves whole; only a
/// missing tag is generated. Prose, other tags, and line lengths are left alone.
pub(crate) fn repair(
    source: &str,
    service: &str,
    definition: &Definition,
    policy: &HeaderPolicy,
) -> Repair {
    let ending = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut repair = Repair::default();
    for finding in findings(source, service, Some(definition), policy) {
        if finding.rule != "contract" {
            repair
                .not_fixable
                .push(format!("line {}: {}", finding.line, finding.message));
        }
    }
    let Some(header) = parse(source) else {
        repair.fixed = Some(format!(
            "{}{source}",
            generate(service, definition, policy, ending)
        ));
        repair
            .changes
            .push("added a header from definition.xml".to_string());
        return repair;
    };
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let (open, close) = (header.line - 1, header.end_line - 1);
    if open == close || body(lines[open]).starts_with('@') || body(lines[close]).starts_with('@') {
        repair.not_fixable.push(
            "a tag shares a line with `/**` or `*/`; put each on its own line and run again"
                .to_string(),
        );
        return repair;
    }
    let region = (open + 1..close)
        .find(|&index| body(lines[index]).starts_with('@'))
        .unwrap_or(close);
    let mut groups: Vec<Group> = Vec::new();
    for line in &lines[region..close] {
        let content = body(line);
        match content.strip_prefix('@') {
            Some(rest) => {
                let (name, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                groups.push(Group {
                    name: name.to_string(),
                    value: value.trim().to_string(),
                    lines: vec![(*line).to_string()],
                });
            }
            None => {
                let group = groups.last_mut().expect("the region starts at a tag");
                group.lines.push((*line).to_string());
                if !content.is_empty() {
                    group.value = format!("{} {content}", group.value).trim().to_string();
                }
            }
        }
    }

    let generated = |prefix: String, text: &str| {
        let mut out = String::new();
        wrapped(&mut out, &prefix, text, policy.max_columns);
        out.replace('\n', ending)
    };
    let mut used = vec![false; groups.len()];

    let functions: Vec<usize> = (0..groups.len())
        .filter(|&index| groups[index].name == "function")
        .collect();
    let function = match functions.as_slice() {
        [one] if groups[*one].value == service => groups[*one].lines.concat(),
        _ => {
            repair.changes.push(format!("set @function to {service}"));
            format!(" * @function {service}{ending}")
        }
    };
    for &index in &functions {
        used[index] = true;
    }

    let present: Vec<String> = groups
        .iter()
        .filter(|group| group.name == "param")
        .filter_map(|group| param_name(&group.value).map(str::to_string))
        .filter(|name| definition.inputs.iter().any(|field| &field.name == name))
        .collect();
    let mut params = String::new();
    for field in &definition.inputs {
        let found = (0..groups.len()).find(|&index| {
            !used[index]
                && groups[index].name == "param"
                && param_name(&groups[index].value) == Some(field.name.as_str())
        });
        match found {
            Some(index) => {
                used[index] = true;
                params.push_str(&groups[index].lines.concat());
            }
            None => {
                let prefix = format!(
                    " * @param {{{}}} {} - ",
                    jsdoc_type(&field.base_type),
                    field.name
                );
                params.push_str(&generated(prefix, &field.description));
                repair.changes.push(format!("added @param {}", field.name));
            }
        }
    }
    for index in 0..groups.len() {
        if !used[index] && groups[index].name == "param" {
            used[index] = true;
            let name = param_name(&groups[index].value).unwrap_or("").to_string();
            repair
                .changes
                .push(format!("removed @param {name}, which is not an input"));
        }
    }
    let wanted: Vec<&str> = definition
        .inputs
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    let kept: Vec<&str> = present
        .iter()
        .map(String::as_str)
        .filter(|name| wanted.contains(name))
        .collect();
    let in_order: Vec<&str> = wanted
        .iter()
        .copied()
        .filter(|name| kept.contains(name))
        .collect();
    if kept != in_order {
        repair.changes.push(format!(
            "put the {} @param tags in the definition's order",
            kept.len()
        ));
    }

    let returns = (0..groups.len())
        .find(|&index| !used[index] && matches!(groups[index].name.as_str(), "returns" | "return"));
    // The contract is one @returns; a second is dropped, as a second @function is.
    for index in 0..groups.len() {
        if Some(index) != returns && matches!(groups[index].name.as_str(), "returns" | "return") {
            used[index] = true;
            repair.changes.push("removed a second @returns".to_string());
        }
    }
    let returns_text = match returns {
        Some(index) => {
            used[index] = true;
            let group = &groups[index];
            let mut lines = group.lines.clone();
            let shape = definition.result_shape.as_ref().filter(|shape| {
                policy.require_result_shape && !group.value.contains(shape.as_str())
            });
            if let Some(shape) = shape {
                let last = lines
                    .iter()
                    .rposition(|line| !body(line).is_empty())
                    .unwrap_or(0);
                let text = lines[last].trim_end_matches(['\r', '\n']).trim_end();
                let described = group
                    .value
                    .strip_prefix('{')
                    .and_then(|rest| rest.split_once('}'))
                    .map_or(group.value.as_str(), |(_, after)| after)
                    .trim();
                let added = if described.is_empty() {
                    format!(" An InfoTable shaped by {shape}.")
                } else {
                    format!(" Shaped by {shape}.")
                };
                let line_ending =
                    lines[last][lines[last].trim_end_matches(['\r', '\n']).len()..].to_string();
                let ending = if line_ending.is_empty() {
                    "\n"
                } else {
                    &line_ending
                };
                // Too long with the name on it: the name continues on a line of its own.
                lines[last] = if text.chars().count() + added.chars().count() > policy.max_columns {
                    format!("{text}{ending} *  {added}{line_ending}")
                } else {
                    format!("{text}{added}{line_ending}")
                };
                repair
                    .changes
                    .push(format!("named the result DataShape {shape} in @returns"));
            }
            lines.concat()
        }
        None => {
            let prefix = format!(
                " * @returns {{{}}} ",
                jsdoc_type(&definition.result.base_type)
            );
            let text = definition
                .result_shape
                .as_ref()
                .map(|shape| format!("An InfoTable shaped by {shape}."))
                .unwrap_or_else(|| definition.result.description.clone());
            repair.changes.push("added @returns".to_string());
            generated(prefix, &text)
        }
    };

    // Each tag stays where it is: the @function and @returns in their own places, the inputs in
    // the place of the first @param. Only a tag the header lacks takes a new place, as the
    // generated header orders them.
    let mut pieces: Vec<(&str, String)> = Vec::new();
    let mut emitted_params = false;
    for (index, group) in groups.iter().enumerate() {
        match group.name.as_str() {
            "function" if Some(&index) == functions.first() => {
                pieces.push(("function", function.clone()));
            }
            "function" => {}
            "param" if !emitted_params => {
                emitted_params = true;
                pieces.push(("params", params.clone()));
            }
            "param" => {}
            _ if Some(index) == returns => pieces.push(("returns", returns_text.clone())),
            _ if !used[index] => pieces.push(("other", group.lines.concat())),
            _ => {}
        }
    }
    let at = |pieces: &Vec<(&str, String)>, kind: &str| pieces.iter().position(|(k, _)| *k == kind);
    if functions.is_empty() {
        pieces.insert(0, ("function", function));
    }
    if !emitted_params && !params.is_empty() {
        let after = at(&pieces, "function").map_or(0, |index| index + 1);
        pieces.insert(after, ("params", params));
    }
    if returns.is_none() {
        let after = at(&pieces, "params")
            .or_else(|| at(&pieces, "function"))
            .map_or(pieces.len(), |index| index + 1);
        pieces.insert(after, ("returns", returns_text));
    }
    let tags: String = pieces.into_iter().map(|(_, text)| text).collect();

    // Prose with no tags below it needs the blank line the generated header has.
    let separator = region == close
        && (open + 1..close).any(|index| !body(lines[index]).is_empty())
        || region == close && !body(lines[open]).is_empty();
    let mut fixed: String = lines[..region].concat();
    if separator {
        fixed.push_str(&format!(" *{ending}"));
    }
    fixed.push_str(&tags);
    fixed.push_str(&lines[close..].concat());
    if fixed != source {
        if repair.changes.is_empty() {
            repair
                .changes
                .push("put the tags in contract order".to_string());
        }
        repair.fixed = Some(fixed);
    }
    repair
}

#[cfg(test)]
mod fitting_tests {
    use super::*;

    fn definition(description: &str) -> Definition {
        Definition {
            description: description.to_string(),
            inputs: Vec::new(),
            result: Field {
                name: "result".to_string(),
                base_type: "NOTHING".to_string(),
                description: String::new(),
                ordinal: 0,
            },
            result_shape: None,
        }
    }

    fn fitted(description: &str) -> String {
        generate_fitting(
            "Run",
            &definition(description),
            &HeaderPolicy::default(),
            &crate::core::comments::CommentPolicy::default(),
            &|_| true,
            "\n",
        )
    }

    #[test]
    fn a_rule_the_solution_turns_off_does_not_cut_the_description() {
        let long = "It reads every row of the stream, newest first, and keeps only the rows that \
                    the dashboard grid, the alarm panel and the weekly report need today.";
        let header = generate_fitting(
            "Run",
            &definition(long),
            &HeaderPolicy::default(),
            &crate::core::comments::CommentPolicy::default(),
            &|rule| rule != "length",
            "\n",
        );
        assert!(header.contains("weekly report"), "{header}");
    }

    #[test]
    fn a_short_description_is_kept_whole() {
        assert!(fitted("Runs the job. Then stops.").contains(" * Runs the job. Then stops.\n"));
    }

    #[test]
    fn a_long_description_keeps_the_leading_sentences_that_fit() {
        let header = fitted(
            "Runs the job. It reads every row of the stream, newest first, and keeps only the \
             rows that the dashboard grid, the alarm panel and the weekly report need today.",
        );
        assert!(header.contains(" * Runs the job.\n"), "{header}");
        assert!(!header.contains("alarm"), "{header}");
    }

    #[test]
    fn a_description_with_no_fitting_sentence_is_left_out() {
        let header = fitted(
            "It reads every row of the stream, newest first, and keeps only the rows that the \
             dashboard grid, the alarm panel and the weekly report need today",
        );
        assert!(header.starts_with("/**\n * @function Run\n"), "{header}");
    }

    #[test]
    fn sentences_split_only_at_a_stop_before_a_space() {
        assert_eq!(
            leading_sentences("Uses v1.2 here. Then ends"),
            ["Uses v1.2 here.", " Then ends"]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample_definition() -> Definition {
        super::definition(r#"<ServiceDefinition description="Do work"><ResultType baseType="INFOTABLE" aspect.dataShape="Acme.Rows" description=""/><ParameterDefinitions><FieldDefinition name="b" baseType="STRING" ordinal="2"/><FieldDefinition name="a" baseType="NUMBER" ordinal="1"/></ParameterDefinitions></ServiceDefinition>"#).unwrap()
    }
    #[test]
    fn contract_and_generation_agree() {
        let d = sample_definition();
        let source = generate("Run", &d, &HeaderPolicy::default(), "\n") + "go();\n";
        assert!(findings(&source, "Run", Some(&d), &HeaderPolicy::default()).is_empty());
    }
    #[test]
    fn missing_header_is_actionable() {
        assert!(findings(
            "go();",
            "Run",
            Some(&sample_definition()),
            &HeaderPolicy::default()
        )[0]
        .message
        .contains("fix-headers"));
    }
    #[test]
    fn return_alias_is_accepted() {
        let source = "/**\n * @function Run\n * @param {number} a -\n * @param {string} b -\n * @return {Object} Acme.Rows\n */\ngo();";
        assert!(findings(
            source,
            "Run",
            Some(&sample_definition()),
            &HeaderPolicy::default()
        )
        .is_empty());
    }

    #[test]
    fn contract_reports_function_order_and_extra_inputs() {
        let source = "/**\n * @function Other\n * @param {string} b - kept\n * @param {number} a - kept\n * @param {string} extra - kept\n * @returns {Object} Acme.Rows\n */\ngo();";
        let messages: Vec<_> = findings(
            source,
            "Run",
            Some(&sample_definition()),
            &HeaderPolicy::default(),
        )
        .into_iter()
        .map(|finding| finding.message)
        .collect();
        assert!(messages
            .iter()
            .any(|message| message.contains("@function names Other")));
        assert!(messages
            .iter()
            .any(|message| message.contains("extra is not an input")));
        assert!(messages
            .iter()
            .any(|message| message.contains("order is b, a")));
    }

    #[test]
    fn first_statement_and_wrap_have_their_own_lines() {
        let source = "/**\n * @function Run this is intentionally long enough to cross a very narrow configured width\n */\n\n// explain\ngo();";
        let findings = findings(
            source,
            "Run",
            None,
            &HeaderPolicy {
                require_result_shape: true,
                max_columns: 40,
            },
        );
        assert!(findings
            .iter()
            .any(|finding| finding.rule == "wrap" && finding.line == 2));
        assert!(findings
            .iter()
            .any(|finding| finding.rule == "first-statement" && finding.line == 5));
    }

    #[test]
    fn nothing_generates_void() {
        let definition = super::definition(
            r#"<ServiceDefinition><ResultType baseType="NOTHING"/><ParameterDefinitions/></ServiceDefinition>"#,
        )
        .unwrap();
        assert!(generate("Run", &definition, &HeaderPolicy::default(), "\n")
            .contains("@returns {void}"));
    }

    fn fixed(source: &str) -> Repair {
        repair(
            source,
            "Run",
            &sample_definition(),
            &HeaderPolicy::default(),
        )
    }

    #[test]
    fn repair_reorders_tags_whole_and_keeps_their_bytes() {
        let source = "/**\n * Does the work.\n *\n * @function Run\n * @param {string} b - the second,\n *   wrapped onto a line of its own.\n * @param {number} a - the first\n * @returns {Object} Rows shaped by Acme.Rows.\n */\ngo();";
        let repaired = fixed(source);
        assert_eq!(
            repaired.fixed.as_deref(),
            Some("/**\n * Does the work.\n *\n * @function Run\n * @param {number} a - the first\n * @param {string} b - the second,\n *   wrapped onto a line of its own.\n * @returns {Object} Rows shaped by Acme.Rows.\n */\ngo();")
        );
        assert_eq!(
            repaired.changes,
            ["put the 2 @param tags in the definition's order"]
        );
        let again = fixed(repaired.fixed.as_deref().unwrap());
        assert_eq!(again.fixed, None, "a second run changes nothing");
        assert!(findings(
            repaired.fixed.as_deref().unwrap(),
            "Run",
            Some(&sample_definition()),
            &HeaderPolicy::default()
        )
        .is_empty());
    }

    #[test]
    fn repair_generates_what_is_missing_and_drops_what_is_not_an_input() {
        let source = "/**\n * @function Old\n * @param {string} gone - not an input\n * @param {number} a - kept as written\n */\ngo();";
        let repaired = fixed(source);
        assert_eq!(
            repaired.fixed.as_deref(),
            Some("/**\n * @function Run\n * @param {number} a - kept as written\n * @param {string} b -\n * @returns {Object} An InfoTable shaped by Acme.Rows.\n */\ngo();")
        );
        assert_eq!(
            repaired.changes,
            [
                "set @function to Run",
                "added @param b",
                "removed @param gone, which is not an input",
                "added @returns",
            ]
        );
    }

    #[test]
    fn repair_names_the_data_shape_and_keeps_crlf() {
        let source = "/**\r\n * @function Run\r\n * @param {number} a - one\r\n * @param {string} b - two\r\n * @returns {Object} The rows.\r\n */\r\ngo();";
        let repaired = fixed(source);
        assert_eq!(
            repaired.fixed.as_deref(),
            Some("/**\r\n * @function Run\r\n * @param {number} a - one\r\n * @param {string} b - two\r\n * @returns {Object} The rows. Shaped by Acme.Rows.\r\n */\r\ngo();")
        );
        let bare = "/**\n * @function Run\n * @param {number} a -\n * @param {string} b -\n * @returns {Object}\n */\ngo();";
        assert!(fixed(bare)
            .fixed
            .unwrap()
            .contains(" * @returns {Object} An InfoTable shaped by Acme.Rows.\n"));
    }

    #[test]
    fn repair_keeps_one_returns_and_its_result_passes_the_gate() {
        let source = "/**\n * @function Run\n * @param {number} a -\n * @param {string} b -\n * @returns {Object} Acme.Rows\n * @return {Object} again\n */\ngo();";
        let repaired = fixed(source);
        assert_eq!(repaired.changes, ["removed a second @returns"]);
        let text = repaired.fixed.unwrap();
        assert!(!text.contains("again"), "{text}");
        assert!(findings(
            &text,
            "Run",
            Some(&sample_definition()),
            &HeaderPolicy::default()
        )
        .is_empty());
        assert_eq!(fixed(&text).fixed, None);
    }

    #[test]
    fn repair_leaves_a_header_the_gate_accepts_as_it_is() {
        // @param above @function is not what the generator writes, and not what the gate asks.
        let source = "/**\n * @param {number} a - one\n * @param {string} b - two\n * @function Run\n * @returns {Object} Acme.Rows\n */\ngo();";
        assert!(findings(
            source,
            "Run",
            Some(&sample_definition()),
            &HeaderPolicy::default()
        )
        .is_empty());
        assert_eq!(fixed(source), Repair::default());
        // Out of order, the inputs are reordered in place, and the rest stays where it is.
        let swapped = source.replacen(
            "{number} a - one\n * @param {string} b - two",
            "{string} b - two\n * @param {number} a - one",
            1,
        );
        assert_eq!(fixed(&swapped).fixed.as_deref(), Some(source));
    }

    #[test]
    fn a_data_shape_name_that_would_overflow_the_line_continues_below_it() {
        let returns = format!(" * @returns {{Object}} {}.", "word ".repeat(14).trim_end());
        let source = format!(
            "/**\n * @function Run\n * @param {{number}} a -\n * @param {{string}} b -\n{returns}\n */\ngo();"
        );
        let repaired = fixed(&source).fixed.unwrap();
        assert!(
            repaired.contains(&format!("{returns}\n *   Shaped by Acme.Rows.\n */")),
            "{repaired}"
        );
        assert!(findings(
            &repaired,
            "Run",
            Some(&sample_definition()),
            &HeaderPolicy::default()
        )
        .is_empty());
    }

    #[test]
    fn repair_writes_a_whole_header_and_refuses_one_line_headers() {
        let repaired = fixed("go();");
        assert!(repaired
            .fixed
            .as_deref()
            .unwrap()
            .starts_with("/**\n * Do work\n *\n * @function Run\n"));
        assert!(repaired.fixed.as_deref().unwrap().ends_with(" */\ngo();"));
        assert_eq!(fixed(repaired.fixed.as_deref().unwrap()).fixed, None);

        let one_line = fixed("/** @function Run */\ngo();");
        assert_eq!(one_line.fixed, None);
        assert!(one_line.not_fixable[0].contains("own line"), "{one_line:?}");
    }

    #[test]
    fn repair_adds_tags_below_prose_with_a_blank_line_and_leaves_other_findings() {
        let source = "/**\n * Prose only.\n */\n// a comment where code belongs\ngo();";
        let repaired = fixed(source);
        assert_eq!(
            repaired.fixed.as_deref(),
            Some("/**\n * Prose only.\n *\n * @function Run\n * @param {number} a -\n * @param {string} b -\n * @returns {Object} An InfoTable shaped by Acme.Rows.\n */\n// a comment where code belongs\ngo();")
        );
        assert_eq!(repaired.not_fixable.len(), 1);
        assert!(repaired.not_fixable[0].starts_with("line 4: the first statement"));
    }

    #[test]
    fn a_data_shape_wrapped_onto_a_continuation_line_is_named() {
        let source = "/**\n * @function Run\n * @param {number} a -\n * @param {string} b -\n * @returns {Object} An InfoTable shaped by\n *   Acme.Rows.\n */\ngo();";
        assert!(findings(
            source,
            "Run",
            Some(&sample_definition()),
            &HeaderPolicy::default()
        )
        .is_empty());
        assert_eq!(fixed(source).fixed, None);
    }
}
