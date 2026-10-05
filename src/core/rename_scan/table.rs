use super::super::{scan, script};
use super::findings::lexical_mentions;
use super::findings::{add_review, add_table_edit, span_text, Place, XmlPass};
use std::collections::BTreeMap;

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
