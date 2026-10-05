//! Writing service sidecars back into an entity document.
//!
//! Only the definition blocks and script payloads move; every other byte is copied through by
//! the splice engine.
//!
//! Script layout is not content. Extraction dedents, drops blank lines at the edges, and joins
//! several CDATA nodes, but sync leaves the raw payload untouched when that extracted script
//! equals the sidecar. An edited script is written in the configured layout; `--relayout` makes
//! that rewrite explicit without a content edit. Either kind of write settles after one pass.

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
}

impl SyncReport {
    /// Whether anything would be added or removed rather than updated in place.
    pub fn has_structural_change(&self) -> bool {
        !self.only_in_entity.is_empty() || !self.only_in_sidecars.is_empty()
    }
}

/// Write sidecars back into an entity document.
///
/// `allow_structural_change` only suppresses the refusal; nothing here creates or deletes a
/// service. A sidecar that has quietly disappeared should not silently delete one, and a stray
/// sidecar should not be silently ignored.
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

    if report.has_structural_change() && !allow_structural_change {
        return Err(SidecarError::StructuralChange {
            added: report.only_in_sidecars.clone(),
            removed: report.only_in_entity.clone(),
        });
    }

    let out = splice::splice(src, &edits).map_err(SidecarError::Splice)?;
    Ok((out, report))
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
        let (_, report) = sync(&src, &empty, true, true, false).unwrap();
        assert_eq!(report.only_in_entity, vec!["S"]);
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
