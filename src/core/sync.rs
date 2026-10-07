//! Writing service sidecars back into an entity document.
//!
//! Only the definition blocks and script payloads move; every other byte is copied through by
//! the splice engine.
//!
//! Script layout is not content. Extraction dedents, drops blank lines at the edges, and joins
//! several CDATA nodes, but sync leaves the raw payload untouched when that extracted script
//! equals the sidecar. An edited script is written in the configured layout; `--relayout` makes
//! that rewrite explicit without a content edit. Either kind of write settles after one pass.

use super::relocate;
use super::scan::{self, Kind, Token};
use super::sidecar::{self, ServiceSidecar, SidecarError};
use super::splice::{self, Edit};
use std::collections::{BTreeMap, BTreeSet};

/// What a sync would do, or did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Services whose definition or script the sidecars change.
    pub changed: Vec<String>,
    /// Services already matching their sidecars.
    pub unchanged: Vec<String>,
    /// In the entity, with no sidecar to write back. Removing a service needs saying so.
    pub only_in_entity: Vec<String>,
    /// A sidecar the entity has no writable service for. Adding one needs saying so.
    pub only_in_sidecars: Vec<String>,
    /// Run-time permission resources removed with the services they named.
    pub dropped_permissions: Vec<String>,
}

impl SyncReport {
    /// Whether anything would be added or removed rather than updated in place.
    pub fn has_structural_change(&self) -> bool {
        !self.only_in_entity.is_empty() || !self.only_in_sidecars.is_empty()
    }
}

/// Write sidecars back into an entity document.
///
/// A sidecar with no service, or a script service with no sidecar, is refused unless
/// `allow_structural_change` says the add or remove is meant: a sidecar that has quietly
/// disappeared should not silently delete a service, and a stray one should not silently add
/// one. When it is meant, the service is added (its definition from the sidecar, its Script
/// implementation shaped like one already in the entity) or removed, definition and
/// implementation both.
///
/// `indent_cdata_payload` governs edited scripts. `relayout` applies it to unchanged scripts too.
pub fn sync(
    src: &[u8],
    sidecars: &BTreeMap<String, ServiceSidecar>,
    allow_structural_change: bool,
    indent_cdata_payload: bool,
    relayout: bool,
) -> Result<(Vec<u8>, SyncReport), SidecarError> {
    let tokens = scan::tokenize(src)?;
    let host = sidecar::member_host_of(&tokens, src).ok_or(SidecarError::NotAnEntity)?;
    let definitions = sidecar::named_children_of(
        &tokens,
        src,
        host,
        "ServiceDefinitions",
        "ServiceDefinition",
    )?;
    let implementations = sidecar::named_children_of(
        &tokens,
        src,
        host,
        "ServiceImplementations",
        "ServiceImplementation",
    )?;

    let mut report = SyncReport::default();
    let mut edits: Vec<Edit> = Vec::new();
    // Tracked rather than inferred from the implementation names: a sidecar naming a SQL service
    // or an inherited override matches an implementation but is still not written anywhere, and
    // reporting it as handled would lose the intended edit.
    let mut consumed: BTreeSet<&str> = BTreeSet::new();

    for (name, &implementation) in &implementations {
        if sidecar::handler_of(&tokens, src, implementation)? != "Script" {
            continue;
        }
        let Some(&definition) = definitions.get(name) else {
            // Implemented here, defined in a shape. There is no local block to write.
            continue;
        };
        let Some(sidecar) = sidecars.get(name) else {
            report.only_in_entity.push(name.clone());
            continue;
        };
        consumed.insert(name.as_str());

        let mut touched = false;

        // The definition block. The sidecar holds it trimmed and LF-normalised, and the span
        // starts after the line's own indentation, so writing the trimmed text back is
        // position-exact. The line ending comes from the block being replaced rather than from
        // the document, so a stray CRLF in one block cannot reclassify the whole file and turn
        // the next sync's output into `\r\r\n`.
        let block = scan::element_span(&tokens, definition).ok_or(SidecarError::Unnamed {
            what: "ServiceDefinition",
            at: tokens[definition].span.start,
        })?;
        let existing_block = String::from_utf8_lossy(block.of(src)).into_owned();
        let wanted = sidecar
            .definition
            .trim_end_matches(['\n', '\r'])
            .replace("\r\n", "\n");
        let rendered = match newline_of(&existing_block) {
            "\r\n" => wanted.replace('\n', "\r\n"),
            _ => wanted,
        };
        if rendered != existing_block {
            edits.push(Edit::new(block, rendered.into_bytes()));
            touched = true;
        }

        // Layout is not content. Preserve an unchanged script's payload byte for byte, even
        // when its shape disagrees with the configured layout. `--relayout` is the explicit
        // migration path and deliberately skips this guard. The comparison reuses extraction's
        // own reader so sync cannot grow a subtly different de-indenter.
        let existing_script = sidecar::script_of(&tokens, src, implementation, name)?;
        if relayout || existing_script != sidecar.script {
            // The whole CDATA region is rebuilt rather than its payload patched, so a script
            // containing `]]>` is split across sections instead of closing the first one early,
            // and a `<code>` holding text or nothing at all can still receive a script.
            if let Some((region, existing)) = script_region(&tokens, src, implementation) {
                let payload = render_payload(
                    &existing,
                    &sidecar.script,
                    newline_of(&existing),
                    indent_cdata_payload,
                );
                let replacement = scan::render_cdata(payload.as_bytes());
                if replacement != region.of(src) {
                    edits.push(Edit::new(region, replacement));
                    touched = true;
                }
            }
        }

        if touched {
            report.changed.push(name.clone());
        } else {
            report.unchanged.push(name.clone());
        }
    }

    for name in sidecars.keys() {
        if !consumed.contains(name.as_str()) {
            report.only_in_sidecars.push(name.clone());
        }
    }

    if report.has_structural_change() {
        if !allow_structural_change {
            return Err(SidecarError::StructuralChange {
                added: report.only_in_sidecars.clone(),
                removed: report.only_in_entity.clone(),
            });
        }
        let (structural, dropped) = structural_edits(
            &tokens,
            src,
            host,
            &definitions,
            &implementations,
            sidecars,
            &report,
            indent_cdata_payload,
        )?;
        edits.extend(structural);
        report.dropped_permissions = dropped;
    }

    let out = splice::splice(src, &edits).map_err(SidecarError::Splice)?;
    Ok((out, report))
}

/// The edits that add the services only the sidecars have and remove the script services only
/// the entity has. Every added service goes into each section as one insertion, since two
/// insertions at one point would be ambiguous.
#[allow(clippy::too_many_arguments)]
fn structural_edits(
    tokens: &[Token],
    src: &[u8],
    host: usize,
    definitions: &BTreeMap<String, usize>,
    implementations: &BTreeMap<String, usize>,
    sidecars: &BTreeMap<String, ServiceSidecar>,
    report: &SyncReport,
    indent_cdata_payload: bool,
) -> Result<(Vec<Edit>, Vec<String>), SidecarError> {
    let mut edits = Vec::new();
    let definitions_at = scan::child_tags(tokens, src, "ServiceDefinitions", host)
        .first()
        .copied();
    let implementations_at = scan::child_tags(tokens, src, "ServiceImplementations", host)
        .first()
        .copied();

    for name in &report.only_in_entity {
        for (section, element) in [
            (definitions_at, definitions[name]),
            (implementations_at, implementations[name]),
        ] {
            let section = section.ok_or(SidecarError::Scan(scan::ScanError::Malformed {
                what: "service section",
                at: tokens[element].span.start,
            }))?;
            let block = relocate::block_of(tokens, src, section, element)?;
            edits.push(Edit::new(block.span, Vec::new()));
        }
    }
    // A run-time permission for a removed service grants on nothing; it goes with the service,
    // unless a property or event of the same name is what it names.
    let mut dropped = Vec::new();
    if !report.only_in_entity.is_empty() {
        let mut other_members = BTreeSet::new();
        for (section, member) in [
            ("PropertyDefinitions", "PropertyDefinition"),
            ("EventDefinitions", "EventDefinition"),
        ] {
            other_members.extend(
                sidecar::named_children_of(tokens, src, host, section, member)?.into_keys(),
            );
        }
        // The entity's own block, a direct child of the entity element, never one nested deeper.
        let entity = sidecar::entity_element(tokens, src).or_else(|| {
            tokens
                .iter()
                .position(|token| matches!(token.kind, Kind::Start))
        });
        let run_time = entity.into_iter().flat_map(|entity| {
            scan::child_tags(tokens, src, "RunTimePermissions", entity)
                .into_iter()
                .filter(|&at| matches!(tokens[at].kind, Kind::Start))
        });
        for run_time in run_time {
            for permissions in scan::child_tags(tokens, src, "Permissions", run_time) {
                let resource = scan::attribute(src, &tokens[permissions], "resourceName")?
                    .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(src))));
                let Some(resource) = resource else { continue };
                if report.only_in_entity.contains(&resource) && !other_members.contains(&resource) {
                    let block = relocate::block_of(tokens, src, run_time, permissions)?;
                    edits.push(Edit::new(block.span, Vec::new()));
                    dropped.push(resource);
                }
            }
        }
    }

    if report.only_in_sidecars.is_empty() {
        return Ok((edits, dropped));
    }
    for name in &report.only_in_sidecars {
        let why = if implementations.contains_key(name) && !definitions.contains_key(name) {
            Some("the entity implements it already, overriding an inherited service".to_string())
        } else if implementations.contains_key(name) {
            Some("the entity has it already, and it is not a script service".to_string())
        } else if definitions.contains_key(name) {
            Some("the entity defines it already, with no implementation of its own".to_string())
        } else if name.contains(['"', '<', '>', '&']) || name.trim().is_empty() {
            Some("the name is not a service name".to_string())
        } else {
            match defined_name(&sidecars[name].definition) {
                Some(defined) if defined == *name => None,
                Some(defined) => Some(format!(
                    "its definition.xml names the service {defined:?}; the folder and the \
                     definition's name must agree"
                )),
                None => Some(
                    "its definition.xml is not one <ServiceDefinition> with a name".to_string(),
                ),
            }
        };
        if let Some(why) = why {
            return Err(SidecarError::CannotAdd {
                name: name.clone(),
                why,
            });
        }
    }
    let cannot_add = |why: String| SidecarError::CannotAdd {
        name: report.only_in_sidecars.join(", "),
        why,
    };

    // The template every new implementation copies: a script service of this entity, so the
    // new one is laid out as the document already is.
    let template = implementations.values().copied().find(|&implementation| {
        matches!(
            sidecar::handler_of(tokens, src, implementation).as_deref(),
            Ok("Script")
        ) && script_region(tokens, src, implementation).is_some()
    });
    let template = match (template, implementations_at) {
        (Some(element), Some(section)) => {
            Some((element, relocate::block_of(tokens, src, section, element)?))
        }
        _ => None,
    };
    let own_lines = template.as_ref().map_or_else(
        || {
            definitions_at
                .or(implementations_at)
                .is_none_or(|at| relocate::starts_line(src, tokens[at].span.start))
        },
        |(_, block)| block.own_lines,
    );
    let newline = newline_of(&String::from_utf8_lossy(src));
    // The nesting unit: how far a section sits inside the element that holds it.
    let section_indent = definitions_at.or(implementations_at).map_or(0, |at| {
        relocate::indent_at(src, tokens[at].span.start).len()
    });
    let host_indent = relocate::indent_at(src, tokens[host].span.start).len();
    let unit = section_indent.saturating_sub(host_indent).max(1);

    // Definitions: each sidecar's block, starting where the entity's own definitions start.
    let definition_indent = definitions
        .values()
        .next()
        .map(|&at| relocate::indent_at(src, tokens[at].span.start).len())
        .unwrap_or(section_indent + unit);
    let mut definition_bytes = Vec::new();
    for name in &report.only_in_sidecars {
        let text = sidecars[name]
            .definition
            .trim_end_matches(['\n', '\r'])
            .replace("\r\n", "\n")
            .replace('\n', newline);
        if own_lines {
            definition_bytes.extend(std::iter::repeat_n(b' ', definition_indent));
        }
        definition_bytes.extend_from_slice(text.as_bytes());
        if own_lines {
            definition_bytes.extend_from_slice(newline.as_bytes());
        }
    }
    let definition_block = relocate::Block {
        span: scan::Span::new(0, definition_bytes.len()),
        own_lines,
        indent: definition_indent,
        section_indent: definition_indent.saturating_sub(unit),
        cdata: Vec::new(),
    };
    let definition_edit = relocate::insert_edit(
        tokens,
        src,
        host,
        "ServiceDefinitions",
        &definition_block,
        &definition_bytes,
    )
    .map_err(cannot_add)?;

    // Implementations: the template renamed and holding the sidecar's script.
    let (indent, implementation_section_indent) = match &template {
        Some((_, block)) => (block.indent, block.section_indent),
        None => (section_indent + unit, section_indent),
    };
    let mut implementation_bytes = Vec::new();
    let mut cdata = Vec::new();
    for name in &report.only_in_sidecars {
        let script = &sidecars[name].script;
        let (bytes, payload) = match &template {
            Some((element, block)) => templated_implementation(
                tokens,
                src,
                *element,
                block,
                name,
                script,
                indent_cdata_payload,
            )?,
            None => composed_implementation(
                name,
                script,
                own_lines,
                indent,
                unit,
                newline,
                indent_cdata_payload,
            ),
        };
        let offset = implementation_bytes.len();
        cdata.push(scan::Span::new(
            offset + payload.start,
            offset + payload.end,
        ));
        implementation_bytes.extend_from_slice(&bytes);
    }
    let implementation_block = relocate::Block {
        span: scan::Span::new(0, implementation_bytes.len()),
        own_lines,
        indent,
        section_indent: implementation_section_indent,
        cdata,
    };
    let implementation_edit = relocate::insert_edit(
        tokens,
        src,
        host,
        "ServiceImplementations",
        &implementation_block,
        &implementation_bytes,
    )
    .map_err(cannot_add)?;
    // Both sections missing put both new sections at one point: write them as one insertion,
    // definitions first, as Composer orders them.
    if definition_edit.span.is_empty() && definition_edit.span == implementation_edit.span {
        let mut both = definition_edit.replacement;
        both.extend_from_slice(&implementation_edit.replacement);
        edits.push(Edit::new(definition_edit.span, both));
    } else {
        edits.push(definition_edit);
        edits.push(implementation_edit);
    }
    Ok((edits, dropped))
}

/// The `name` of the one `ServiceDefinition` a definition sidecar holds.
fn defined_name(definition: &str) -> Option<String> {
    let tokens = scan::tokenize(definition.as_bytes()).ok()?;
    let mut elements = tokens
        .iter()
        .filter(|token| matches!(token.kind, Kind::Start | Kind::Empty));
    let first = elements.next()?;
    if first.name.of(definition.as_bytes()) != b"ServiceDefinition" {
        return None;
    }
    // Nothing but this one element: a second one would be inserted beside it unimplemented.
    let start = tokens
        .iter()
        .position(|token| std::ptr::eq(token, first))
        .expect("the token came from this list");
    let end = if matches!(first.kind, Kind::Empty) {
        start
    } else {
        scan::element_end(&tokens, start)?
    };
    if tokens[end + 1..]
        .iter()
        .any(|token| matches!(token.kind, Kind::Start | Kind::Empty))
    {
        return None;
    }
    scan::attribute(definition.as_bytes(), first, "name")
        .ok()
        .flatten()
        .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(definition.as_bytes()))))
}

/// An existing script implementation's block, renamed and holding `script`, with where its
/// CDATA now sits within it.
fn templated_implementation(
    tokens: &[Token],
    src: &[u8],
    element: usize,
    block: &relocate::Block,
    name: &str,
    script: &str,
    indent_cdata_payload: bool,
) -> Result<(Vec<u8>, scan::Span), SidecarError> {
    let (region, existing) =
        script_region(tokens, src, element).ok_or_else(|| SidecarError::NoScript {
            name: name.to_string(),
        })?;
    let payload = render_payload(
        &existing,
        script,
        newline_of(&existing),
        indent_cdata_payload,
    );
    let replacement = scan::render_cdata(payload.as_bytes());
    let start = region.start - block.span.start;
    let end = region.end - block.span.start;
    // The script comes after the name attribute, so it is replaced first and the name's
    // position stays valid.
    let original = block.span.of(src);
    let mut bytes = original[..start].to_vec();
    bytes.extend_from_slice(&replacement);
    bytes.extend_from_slice(&original[end..]);
    let renamed =
        relocate::with_name(tokens, src, element, block, &bytes, name).map_err(|why| {
            SidecarError::CannotAdd {
                name: name.to_string(),
                why,
            }
        })?;
    // The name came before the script, so the script moved by the change in the name's length.
    let shifted = start + renamed.len() - bytes.len();
    Ok((
        renamed,
        scan::Span::new(shifted, shifted + replacement.len()),
    ))
}

/// A Script implementation written out the way Composer writes one, for an entity that has no
/// script service to copy the layout from.
fn composed_implementation(
    name: &str,
    script: &str,
    own_lines: bool,
    indent: usize,
    unit: usize,
    newline: &str,
    indent_cdata_payload: bool,
) -> (Vec<u8>, scan::Span) {
    let cdata =
        scan::render_cdata(render_payload("", script, newline, indent_cdata_payload).as_bytes());
    let open =
        format!("<ServiceImplementation description=\"\" handlerName=\"Script\" name=\"{name}\">");
    let lines: [(usize, &str); 16] = [
        (0, &open),
        (1, "<ConfigurationTables>"),
        (
            2,
            "<ConfigurationTable dataShapeName=\"\" description=\"\" isMultiRow=\"false\" name=\"Script\" ordinal=\"0\">",
        ),
        (3, "<DataShape>"),
        (4, "<FieldDefinitions>"),
        (
            5,
            "<FieldDefinition baseType=\"STRING\" description=\"code\" name=\"code\" ordinal=\"0\"></FieldDefinition>",
        ),
        (4, "</FieldDefinitions>"),
        (3, "</DataShape>"),
        (3, "<Rows>"),
        (4, "<Row>"),
        (5, "<code>\u{0}</code>"),
        (4, "</Row>"),
        (3, "</Rows>"),
        (2, "</ConfigurationTable>"),
        (1, "</ConfigurationTables>"),
        (0, "</ServiceImplementation>"),
    ];
    let mut bytes = Vec::new();
    let mut payload = scan::Span::new(0, 0);
    for (depth, line) in lines {
        if own_lines {
            bytes.extend(std::iter::repeat_n(b' ', indent + depth * unit));
        }
        match line.split_once('\u{0}') {
            Some((before, after)) => {
                bytes.extend_from_slice(before.as_bytes());
                payload = scan::Span::new(bytes.len(), bytes.len() + cdata.len());
                bytes.extend_from_slice(&cdata);
                bytes.extend_from_slice(after.as_bytes());
            }
            None => bytes.extend_from_slice(line.as_bytes()),
        }
        if own_lines {
            bytes.extend_from_slice(newline.as_bytes());
        }
    }
    (bytes, payload)
}

/// The byte range a service's script occupies, and the text currently in it.
///
/// For a `<code>` holding CDATA this is the region spanning every CDATA node, so the whitespace
/// the document puts between `<code>` and `<![CDATA[` survives untouched. For a `<code>` holding
/// text or nothing it is the element's whole inner range, because there is no CDATA to preserve
/// the shape of.
fn script_region(
    tokens: &[Token],
    src: &[u8],
    implementation: usize,
) -> Option<(scan::Span, String)> {
    let code = sidecar::code_element_of(tokens, src, implementation)?;
    if tokens[code].kind == Kind::Empty {
        // `<code/>` has no inner range to write into; it would have to become `<code>...</code>`,
        // which is a shape change this does not make. Extraction reads it as an empty script, so
        // an unchanged sidecar round-trips; a changed one is reported by the caller's diff.
        return None;
    }
    let end = scan::element_end(tokens, code)?;
    let cdata: Vec<usize> = (code + 1..end)
        .filter(|&i| tokens[i].kind == Kind::Cdata)
        .collect();

    if let (Some(&first), Some(&last)) = (cdata.first(), cdata.last()) {
        let region = scan::Span::new(tokens[first].span.start, tokens[last].span.end);
        // Several nodes are joined, exactly as extraction reads them.
        let existing: String = cdata
            .iter()
            .map(|&i| String::from_utf8_lossy(tokens[i].inner.of(src)).into_owned())
            .collect();
        return Some((region, existing));
    }

    let region = scan::Span::new(tokens[code].span.end, tokens[end].span.start);
    Some((region, String::from_utf8_lossy(region.of(src)).into_owned()))
}

/// Lay a script out inside its `<code>` element.
///
/// Flush-left is the default because ThingWorx stores the payload verbatim. In compatibility
/// mode, indentation is read from the payload being replaced rather than chosen, so a sync of
/// an unchanged sidecar reproduces the original bytes. Twelve spaces is the fallback for a
/// payload with no indented line to learn from, preserving the established extraction behavior.
///
/// A blank line is written empty rather than as trailing whitespace, which is the one place the
/// indented output is deliberately not a mirror of the input.
pub fn render_payload(
    existing: &str,
    script: &str,
    newline: &str,
    indent_cdata_payload: bool,
) -> String {
    let normalized_script = script.replace("\r\n", "\n");
    if !indent_cdata_payload {
        let body = normalized_script
            .split('\n')
            .collect::<Vec<&str>>()
            .join(newline);
        return format!("{newline}{body}{newline}");
    }

    let normalized = existing.replace("\r\n", "\n");
    let indent = normalized
        .split('\n')
        .find(|line| !line.trim().is_empty())
        .map(|line| &line[..line.len() - line.trim_start_matches([' ', '\t']).len()])
        .filter(|i| !i.is_empty())
        .unwrap_or("            ");

    let body = normalized_script
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<String>>()
        .join(newline);
    format!("{newline}{body}{newline}{indent}")
}

/// The line ending a piece of text uses, judged by which is more common in it.
///
/// Local rather than document-wide: one CRLF after an XML declaration should not make every
/// rewritten block CRLF, and a CRLF introduced into one block should not change how the next
/// sync treats every other block.
pub fn newline_of(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if crlf > lf {
        "\r\n"
    } else {
        "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity_with(code: &str) -> Vec<u8> {
        format!(
            "<Entities><Things><Thing name=\"T\"><ThingShape>\
             <ServiceDefinitions><ServiceDefinition name=\"S\" description=\"d\"></ServiceDefinition></ServiceDefinitions>\
             <ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\">\
             <ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>{code}</Row></Rows>\
             </ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>\
             </ThingShape></Thing></Things></Entities>"
        )
        .into_bytes()
    }

    fn entity(script: &str) -> Vec<u8> {
        entity_with(&format!(
            "<code><![CDATA[\n{script}\n            ]]></code>"
        ))
    }

    fn sidecars_of(src: &[u8]) -> BTreeMap<String, ServiceSidecar> {
        sidecar::extract(src)
            .unwrap()
            .services
            .into_iter()
            .map(|s| (s.name.clone(), s))
            .collect()
    }

    fn with_script(src: &[u8], script: &str) -> BTreeMap<String, ServiceSidecar> {
        let mut s = sidecars_of(src);
        s.get_mut("S").unwrap().script = script.to_string();
        s
    }

    #[test]
    fn syncing_unchanged_sidecars_changes_nothing() {
        let src = entity("            var a = 1;\n            result = a;");
        let (out, report) = sync(&src, &sidecars_of(&src), false, true, false).unwrap();
        assert_eq!(out, src, "a no-op sync must be the identity");
        assert_eq!(report.changed, Vec::<String>::new());
        assert_eq!(report.unchanged, vec!["S"]);
    }

    #[test]
    fn a_script_containing_the_cdata_terminator_is_split_not_truncated() {
        // Regression: writing the payload raw closed the section early and truncated the script.
        let src = entity("            a();");
        let (out, _) = sync(
            &src,
            &with_script(&src, "a(); ]]> b();"),
            false,
            true,
            false,
        )
        .unwrap();
        let back = sidecar::extract(&out).unwrap();
        assert_eq!(back.services[0].script, "a(); ]]> b();");
    }

    #[test]
    fn a_code_element_holding_text_can_still_receive_a_script() {
        // Regression: no CDATA node meant the edit was skipped and the service reported
        // unchanged, so sync was not the inverse of extract.
        let src = entity_with("<code>old();</code>");
        let (out, report) = sync(&src, &with_script(&src, "new();"), false, true, false).unwrap();
        assert_eq!(report.changed, vec!["S"]);
        assert_eq!(sidecar::extract(&out).unwrap().services[0].script, "new();");
    }

    #[test]
    fn an_empty_code_element_can_still_receive_a_script() {
        let src = entity_with("<code></code>");
        let (out, report) = sync(&src, &with_script(&src, "fresh();"), false, true, false).unwrap();
        assert_eq!(report.changed, vec!["S"]);
        assert_eq!(
            sidecar::extract(&out).unwrap().services[0].script,
            "fresh();"
        );
    }

    #[test]
    fn several_cdata_nodes_are_replaced_as_one() {
        let src = entity_with("<code><![CDATA[a();\n]]><![CDATA[b();]]></code>");
        let (out, _) = sync(&src, &with_script(&src, "only();"), false, true, false).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.matches("<![CDATA[").count(), 1);
        assert!(text.contains("only();"));
    }

    #[test]
    fn one_crlf_elsewhere_does_not_convert_an_lf_payload() {
        // Regression: dominant_newline meant "contains any CRLF", so an XML declaration ending
        // CRLF rewrote every LF payload in the file.
        let mut src = b"<?xml version=\"1.0\"?>\r\n".to_vec();
        src.extend_from_slice(&entity("            a();"));
        let (out, report) = sync(&src, &sidecars_of(&src), false, true, false).unwrap();
        assert_eq!(out, src, "an LF payload must stay LF");
        assert!(report.changed.is_empty());
    }

    #[test]
    fn a_definition_with_its_own_crlf_does_not_oscillate() {
        let src = entity("            a();");
        let mut sidecars = sidecars_of(&src);
        sidecars.get_mut("S").unwrap().definition =
            "<ServiceDefinition name=\"S\" description=\"d\">\r\n</ServiceDefinition>\n"
                .to_string();
        let (once, _) = sync(&src, &sidecars, false, true, false).unwrap();
        let (twice, second) = sync(&once, &sidecars_of(&once), false, true, false).unwrap();
        assert_eq!(once, twice, "a second sync must change nothing");
        assert!(second.changed.is_empty());
        assert!(
            !String::from_utf8_lossy(&twice).contains("\r\r\n"),
            "no doubled carriage return"
        );
    }

    #[test]
    fn a_never_normalised_payload_is_preserved_unless_relayout_is_requested() {
        // The payload sits flush left while the configured compatibility layout is indented.
        let src = entity_with("<code><![CDATA[\na();\n]]></code>");
        let sidecars = sidecars_of(&src);
        let (untouched, ordinary) = sync(&src, &sidecars, false, true, false).unwrap();
        assert_eq!(untouched, src, "layout alone is not an implicit edit");
        assert!(ordinary.changed.is_empty());

        let (once, first) = sync(&src, &sidecars, false, true, true).unwrap();
        assert_eq!(
            first.changed,
            vec!["S"],
            "relayout is an explicit migration"
        );
        let (twice, second) = sync(&once, &sidecars, false, true, true).unwrap();
        assert_eq!(once, twice);
        assert!(
            second.changed.is_empty(),
            "a second relayout pass must change nothing"
        );
    }

    #[test]
    fn a_sidecar_for_a_sql_service_is_reported_rather_than_dropped() {
        // Regression: the name matched an implementation, so it was neither written nor
        // reported, and the intended edit vanished.
        let src = br#"<Entities><Things><Thing name="T"><ThingShape>
            <ServiceDefinitions></ServiceDefinitions>
            <ServiceImplementations><ServiceImplementation name="Sql" handlerName="SQLCommand">
            <ConfigurationTables><ConfigurationTable name="SQL"><Rows><Row><sql>x</sql></Row></Rows>
            </ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>
            </ThingShape></Thing></Things></Entities>"#;
        let mut sidecars = BTreeMap::new();
        sidecars.insert(
            "Sql".to_string(),
            ServiceSidecar {
                name: "Sql".to_string(),
                definition: "<ServiceDefinition name=\"Sql\"></ServiceDefinition>\n".to_string(),
                script: "nope();".to_string(),
            },
        );
        match sync(src, &sidecars, false, true, false) {
            Err(SidecarError::StructuralChange { added, .. }) => assert_eq!(added, vec!["Sql"]),
            other => panic!("expected the stray sidecar to be reported, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_sidecar_will_not_silently_delete_a_service() {
        let src = entity("            var a = 1;");
        let empty = BTreeMap::new();
        match sync(&src, &empty, false, true, false) {
            Err(SidecarError::StructuralChange { removed, .. }) => assert_eq!(removed, vec!["S"]),
            other => panic!("expected a refusal, got {other:?}"),
        }
        let (out, report) = sync(&src, &empty, true, true, false).unwrap();
        assert_eq!(report.only_in_entity, vec!["S"]);
        assert!(sidecar::extract(&out).unwrap().services.is_empty());
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("name=\"S\""), "{text}");
    }

    fn new_service(name: &str, script: &str) -> ServiceSidecar {
        ServiceSidecar {
            name: name.to_string(),
            definition: format!(
                "<ServiceDefinition name=\"{name}\" description=\"new\"></ServiceDefinition>"
            ),
            script: script.to_string(),
        }
    }

    /// What extraction reads back, by name, as (definition, script).
    fn read_back(src: &[u8]) -> BTreeMap<String, (String, String)> {
        sidecars_of(src)
            .into_iter()
            .map(|(name, s)| (name, (s.definition.trim_end().to_string(), s.script)))
            .collect()
    }

    #[test]
    fn an_allowed_new_sidecar_adds_the_service_and_the_next_sync_is_a_no_op() {
        let src = entity("            var a = 1;");
        let mut sidecars = sidecars_of(&src);
        sidecars.insert("N".into(), new_service("N", "var n = 2;\nresult = n;"));
        sidecars.insert("M".into(), new_service("M", "a(); ]]> b();"));
        assert!(matches!(
            sync(&src, &sidecars, false, true, false),
            Err(SidecarError::StructuralChange { .. })
        ));

        let (out, report) = sync(&src, &sidecars, true, true, false).unwrap();
        assert_eq!(report.only_in_sidecars, vec!["M", "N"]);
        let back = read_back(&out);
        assert_eq!(back.keys().collect::<Vec<_>>(), ["M", "N", "S"]);
        assert_eq!(back["N"].1, "var n = 2;\nresult = n;");
        assert_eq!(back["M"].1, "a(); ]]> b();");
        assert_eq!(back["N"].0, sidecars["N"].definition);
        assert_eq!(back["S"].1, "var a = 1;");
        let (again, report) = sync(&out, &sidecars_of(&out), false, true, false).unwrap();
        assert_eq!(again, out);
        assert!(report.changed.is_empty());

        // Taking the sidecars away again restores the original document exactly.
        let (restored, report) = sync(&out, &sidecars_of(&src), true, true, false).unwrap();
        assert_eq!(report.only_in_entity, vec!["M", "N"]);
        assert_eq!(restored, src);
    }

    #[test]
    fn one_service_can_be_added_while_another_is_removed() {
        let src = entity("            var a = 1;");
        let sidecars = BTreeMap::from([("N".to_string(), new_service("N", "n();"))]);
        let (out, report) = sync(&src, &sidecars, true, false, false).unwrap();
        assert_eq!(report.only_in_sidecars, vec!["N"]);
        assert_eq!(report.only_in_entity, vec!["S"]);
        let back = read_back(&out);
        assert_eq!(back.keys().collect::<Vec<_>>(), ["N"]);
        assert_eq!(back["N"].1, "n();");
    }

    #[test]
    fn a_service_is_added_in_the_documents_own_layout() {
        let src = b"<Entities>\n    <Things>\n        <Thing name=\"T\">\n            <ThingShape>\n                <ServiceDefinitions>\n                    <ServiceDefinition name=\"S\"></ServiceDefinition>\n                </ServiceDefinitions>\n                <ServiceImplementations>\n                    <ServiceImplementation name=\"S\" handlerName=\"Script\">\n                        <ConfigurationTables>\n                            <ConfigurationTable name=\"Script\">\n                                <Rows>\n                                    <Row>\n                                        <code><![CDATA[\ns();\n]]></code>\n                                    </Row>\n                                </Rows>\n                            </ConfigurationTable>\n                        </ConfigurationTables>\n                    </ServiceImplementation>\n                </ServiceImplementations>\n            </ThingShape>\n        </Thing>\n    </Things>\n</Entities>\n";
        let mut sidecars = sidecars_of(src);
        sidecars.insert("N".into(), new_service("N", "n();"));
        let (out, _) = sync(src, &sidecars, true, false, false).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(
            text.contains("                    <ServiceDefinition name=\"N\" description=\"new\"></ServiceDefinition>\n                </ServiceDefinitions>"),
            "{text}"
        );
        assert!(
            text.contains("                    <ServiceImplementation name=\"N\" handlerName=\"Script\">\n                        <ConfigurationTables>"),
            "{text}"
        );
        assert_eq!(read_back(&out)["N"].1, "n();");
    }

    #[test]
    fn an_entity_with_no_script_service_gets_one_written_as_composer_writes_it() {
        let src = b"<Entities>\n  <Things>\n    <Thing name=\"T\">\n      <ThingShape>\n        <ServiceDefinitions></ServiceDefinitions>\n        <ServiceImplementations/>\n      </ThingShape>\n    </Thing>\n  </Things>\n</Entities>\n";
        let sidecars = BTreeMap::from([("N".to_string(), new_service("N", "n();\nm();"))]);
        let (out, report) = sync(src, &sidecars, true, false, false).unwrap();
        assert_eq!(report.only_in_sidecars, vec!["N"]);
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(
            text.contains("        <ServiceDefinitions>\n          <ServiceDefinition name=\"N\" description=\"new\"></ServiceDefinition>\n        </ServiceDefinitions>"),
            "{text}"
        );
        assert!(
            text.contains("          <ServiceImplementation description=\"\" handlerName=\"Script\" name=\"N\">\n            <ConfigurationTables>"),
            "{text}"
        );
        assert_eq!(read_back(&out)["N"].1, "n();\nm();");
        let (again, _) = sync(&out, &sidecars_of(&out), false, false, false).unwrap();
        assert_eq!(again, out);
    }

    #[test]
    fn an_entity_with_neither_service_section_gets_both_in_one_insertion() {
        for newline in ["\n", "\r\n"] {
            let src = "<Entities>\n  <Things>\n    <Thing name=\"T\">\n      <ThingShape>\n        <PropertyDefinitions/>\n      </ThingShape>\n    </Thing>\n  </Things>\n</Entities>\n"
                .replace('\n', newline);
            let sidecars = BTreeMap::from([("N".to_string(), new_service("N", "n();"))]);
            let (out, report) = sync(src.as_bytes(), &sidecars, true, false, false).unwrap();
            assert_eq!(report.only_in_sidecars, vec!["N"]);
            let text = String::from_utf8(out.clone()).unwrap();
            assert!(
                text.find("<ServiceDefinitions>").unwrap()
                    < text.find("<ServiceImplementations>").unwrap()
            );
            assert_eq!(read_back(&out)["N"].1, "n();");
            let (again, _) = sync(&out, &sidecars_of(&out), false, false, false).unwrap();
            assert_eq!(again, out);
        }
    }

    #[test]
    fn a_definition_naming_another_service_than_its_folder_is_refused() {
        let src = entity("            var a = 1;");
        let mut sidecars = sidecars_of(&src);
        let mut stray = new_service("M", "m();");
        stray.name = "N".to_string();
        sidecars.insert("N".into(), stray);
        match sync(&src, &sidecars, true, true, false) {
            Err(SidecarError::CannotAdd { name, why }) => {
                assert_eq!(name, "N");
                assert!(why.contains("\"M\""), "{why}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        let mut broken = new_service("N", "n();");
        broken.definition = "<Nonsense/>".to_string();
        sidecars.insert("N".into(), broken);
        assert!(matches!(
            sync(&src, &sidecars, true, true, false),
            Err(SidecarError::CannotAdd { .. })
        ));
    }

    #[test]
    fn a_removed_services_run_time_permissions_go_with_it_but_a_propertys_stay() {
        let src = b"<Entities><Things><Thing name=\"T\">\n<RunTimePermissions>\n<Permissions resourceName=\"*\"><ServiceInvoke/></Permissions>\n<Permissions resourceName=\"S\"><ServiceInvoke><Principal isPermitted=\"true\" name=\"Users\" type=\"Group\"/></ServiceInvoke></Permissions>\n<Permissions resourceName=\"P\"><PropertyRead/></Permissions>\n</RunTimePermissions>\n<ThingShape><PropertyDefinitions><PropertyDefinition name=\"P\"/></PropertyDefinitions>\
             <ServiceDefinitions><ServiceDefinition name=\"S\"></ServiceDefinition><ServiceDefinition name=\"P\"></ServiceDefinition></ServiceDefinitions>\
             <ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[s();]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation>\
             <ServiceImplementation name=\"P\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[p();]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>\
             </ThingShape></Thing></Things></Entities>";
        let (out, report) = sync(src, &BTreeMap::new(), true, true, false).unwrap();
        assert_eq!(report.only_in_entity, vec!["P", "S"]);
        assert_eq!(report.dropped_permissions, vec!["S"]);
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("resourceName=\"S\""), "{text}");
        assert!(text.contains("resourceName=\"P\""), "{text}");
        assert!(text.contains("resourceName=\"*\""), "{text}");
    }

    #[test]
    fn only_the_entitys_own_run_time_permissions_lose_a_removed_service() {
        let src = b"<Entities><Things><Thing name=\"T\">
<ThingShape><Nested><RunTimePermissions>
<Permissions resourceName=\"S\"><ServiceInvoke/></Permissions>
</RunTimePermissions></Nested>             <ServiceDefinitions><ServiceDefinition name=\"S\"></ServiceDefinition></ServiceDefinitions>             <ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[s();]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>             </ThingShape>
<RunTimePermissions>
<Permissions resourceName=\"S\"><ServiceInvoke/></Permissions>
</RunTimePermissions>
</Thing></Things></Entities>";
        let (out, report) = sync(src, &BTreeMap::new(), true, true, false).unwrap();
        assert_eq!(report.dropped_permissions, vec!["S"]);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.matches("resourceName=\"S\"").count(), 1, "{text}");
        assert!(
            text.contains(
                "<Nested><RunTimePermissions>
<Permissions resourceName=\"S\">"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_definition_sidecar_holding_two_definitions_is_refused() {
        let src = entity("            var a = 1;");
        let mut sidecars = sidecars_of(&src);
        let mut two = new_service("N", "n();");
        two.definition
            .push_str("<ServiceDefinition name=\"Unexpected\"></ServiceDefinition>");
        sidecars.insert("N".into(), two);
        assert!(matches!(
            sync(&src, &sidecars, true, true, false),
            Err(SidecarError::CannotAdd { .. })
        ));
    }

    #[test]
    fn a_sidecar_that_cannot_become_a_new_script_service_is_refused_even_when_allowed() {
        let src = br#"<Entities><Things><Thing name="T"><ThingShape>
            <ServiceDefinitions><ServiceDefinition name="Abstract"></ServiceDefinition></ServiceDefinitions>
            <ServiceImplementations><ServiceImplementation name="Sql" handlerName="SQLCommand">
            <ConfigurationTables><ConfigurationTable name="SQL"><Rows><Row><sql>x</sql></Row></Rows>
            </ConfigurationTable></ConfigurationTables></ServiceImplementation>
            <ServiceImplementation name="Inherited" handlerName="Script">
            <ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code><![CDATA[i();]]></code></Row></Rows>
            </ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>
            </ThingShape></Thing></Things></Entities>"#;
        for name in ["Sql", "Abstract", "Inherited"] {
            let sidecars = BTreeMap::from([(name.to_string(), new_service(name, "x();"))]);
            match sync(src, &sidecars, true, true, false) {
                Err(SidecarError::CannotAdd { name: refused, .. }) => assert_eq!(refused, name),
                other => panic!("expected {name} to be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn indented_output_is_unchanged_from_the_legacy_layout() {
        let src = entity("            var a = 1;");
        let (out, report) = sync(
            &src,
            &with_script(&src, "var a = 2;\nvar b = 3;"),
            false,
            true,
            false,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("\n            var a = 2;\n            var b = 3;\n"));
        assert_eq!(report.changed, vec!["S"]);
    }

    #[test]
    fn an_edited_flush_script_uses_indented_mode_when_configured() {
        let src = entity_with("<code><![CDATA[\nvar a = 1;\n]]></code>");
        let (out, report) = sync(
            &src,
            &with_script(&src, "var a = 2;\nvar b = 3;"),
            false,
            true,
            false,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("\n            var a = 2;\n            var b = 3;\n            "));
        assert_eq!(report.changed, vec!["S"]);
    }

    #[test]
    fn flush_left_output_has_no_indented_script_line_or_trailing_indent() {
        let src = entity("            var a = 1;");
        let (out, report) = sync(
            &src,
            &with_script(&src, "var a = 2;\n    var b = 3;"),
            false,
            false,
            false,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("<code><![CDATA[\nvar a = 2;\n    var b = 3;\n]]></code>"));
        assert!(!text.contains("\n            var a = 2;"));
        assert_eq!(report.changed, vec!["S"]);
    }

    #[test]
    fn switching_layout_modes_only_rewrites_with_relayout() {
        let indented = entity("            var a = 1;\n            result = a;");
        let sidecars = sidecars_of(&indented);

        let (preserved, ordinary) = sync(&indented, &sidecars, false, false, false).unwrap();
        assert_eq!(preserved, indented);
        assert!(ordinary.changed.is_empty());

        let (flush, to_flush) = sync(&indented, &sidecars, false, false, true).unwrap();
        assert_eq!(to_flush.changed, vec!["S"]);
        assert!(String::from_utf8_lossy(&flush).contains("<![CDATA[\nvar a = 1;\nresult = a;\n]]>"));

        let (still_flush, ordinary_switch) = sync(&flush, &sidecars, false, true, false).unwrap();
        assert_eq!(still_flush, flush);
        assert!(ordinary_switch.changed.is_empty());

        let (indented_again, to_indented) = sync(&flush, &sidecars, false, true, true).unwrap();
        assert_eq!(to_indented.changed, vec!["S"]);
        assert_eq!(indented_again, indented);
    }

    #[test]
    fn both_layout_modes_are_idempotent() {
        for (src, indent) in [
            (entity("            var a = 1;"), false),
            (entity_with("<code><![CDATA[\nvar a = 1;\n]]></code>"), true),
        ] {
            let sidecars = sidecars_of(&src);
            let (once, first) = sync(&src, &sidecars, false, indent, true).unwrap();
            assert_eq!(
                first.changed,
                vec!["S"],
                "the opposite layout must be rewritten"
            );
            let (twice, second) = sync(&once, &sidecars, false, indent, true).unwrap();
            assert_eq!(
                twice, once,
                "a second sync in mode {indent} must be byte-identical"
            );
            assert!(
                second.changed.is_empty(),
                "a second sync in mode {indent} must be unchanged"
            );
        }
    }

    #[test]
    fn a_blank_line_is_written_empty_rather_than_as_trailing_whitespace() {
        let src = entity("            a();");
        let (out, _) = sync(&src, &with_script(&src, "a();\n\nb();"), false, true, false).unwrap();
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("a();\n\n            b();"));
    }

    #[test]
    fn a_tab_indented_payload_keeps_its_tabs() {
        let src = entity_with("<code><![CDATA[\n\t\t\ta();\n\t\t\t]]></code>");
        let (out, report) = sync(&src, &with_script(&src, "b();"), false, true, false).unwrap();
        assert!(
            String::from_utf8_lossy(&out).contains("\n\t\t\tb();\n\t\t\t"),
            "tabs are the document's choice, not ours"
        );
        assert_eq!(report.changed, vec!["S"]);
    }

    #[test]
    fn the_line_ending_is_judged_by_which_is_more_common() {
        assert_eq!(newline_of("a\r\nb\r\nc"), "\r\n");
        assert_eq!(newline_of("a\nb\nc\r\n"), "\n");
        assert_eq!(newline_of("no newlines"), "\n");
    }

    #[test]
    fn an_empty_payload_falls_back_to_twelve_spaces() {
        assert_eq!(
            render_payload("", "x();", "\n", true),
            "\n            x();\n            "
        );
    }
}
