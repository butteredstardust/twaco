use super::super::scan;
use super::findings::{add_field_edit, Place, XmlPass};

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
