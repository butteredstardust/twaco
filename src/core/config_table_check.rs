//! Whether a multi-row ConfigurationTable's copy agrees with the DataShape its definition names.
//!
//! Verified on ThingWorx 10.2 with a definition that names a DataShape and lists no fields of its
//! own (the Exporter's form). The server rebuilds the table copy's fields from the named DataShape:
//! its ordinals, its descriptions, its key. A copy that says otherwise imports, then reads back
//! not kept. Three cases lose data or fail: a DataShape without a primary key fails the import;
//! rows under a copy with no fields are all dropped; a row cell naming a field the DataShape lacks
//! is dropped. A definition with inline fields is `validate`'s concern, and a table whose
//! definition is inherited is not checked: the definition that applies is not in the file.

use super::datashape::{Aspect, Field};
use super::scan::{self, Kind, Token};
use super::types::DataShape;
use std::collections::{BTreeMap, BTreeSet};

/// One problem with a table, and whether it stops a deploy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableProblem {
    pub(crate) rule: &'static str,
    pub(crate) message: String,
    pub(crate) blocks: bool,
}

/// The entity-level definitions and copies in `src`, checked against `shapes`.
pub(crate) fn check(src: &[u8], shapes: &[DataShape]) -> Vec<TableProblem> {
    let Ok(tokens) = scan::tokenize(src) else {
        return Vec::new();
    };
    let text = |token: &Token, name: &str| {
        scan::attribute(src, token, name)
            .ok()
            .flatten()
            .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(src))))
            .unwrap_or_default()
    };
    let is = |index: usize, name: &[u8]| {
        let token = &tokens[index];
        matches!(token.kind, Kind::Start | Kind::Empty) && token.name.of(src) == name
    };
    let end_of = |index: usize| scan::element_end_in(&tokens, src, index).unwrap_or(index);
    // Only the entity's own sections: services and subscriptions keep handler tables
    // (`Script`, `Query`) under the same element names.
    let Some(entity) = super::sidecar::entity_element(&tokens, src) else {
        return Vec::new();
    };
    let own = |section: &[u8], member: &[u8]| -> Vec<usize> {
        direct_children(&tokens, entity, end_of(entity))
            .into_iter()
            .filter(|&child| is(child, section))
            .flat_map(|child| direct_children(&tokens, child, end_of(child)))
            .filter(|&inner| is(inner, member))
            .collect()
    };

    // Named-only multi-row definitions, by table name.
    let mut named = BTreeMap::new();
    let mut problems = Vec::new();
    for index in own(
        b"ConfigurationTableDefinitions",
        b"ConfigurationTableDefinition",
    ) {
        let token = &tokens[index];
        let shape = text(token, "dataShapeName");
        if text(token, "isMultiRow") != "true" || shape.is_empty() {
            continue;
        }
        let Some(end) = scan::element_end_in(&tokens, src, index) else {
            continue;
        };
        if (index + 1..end).any(|inner| is(inner, b"FieldDefinition")) {
            continue;
        }
        let table = text(token, "name");
        if let Some(fields) = resolved(shapes, &shape) {
            if !fields.iter().any(is_key) {
                problems.push(TableProblem {
                    rule: "config-table-shape-without-key",
                    message: format!(
                        "multi-row configuration table {table}: {shape} marks no field aspect.isPrimaryKey=\"true\", so the import fails; mark its key"
                    ),
                    blocks: true,
                });
            }
        }
        named.insert(table, shape);
    }

    for index in own(b"ConfigurationTables", b"ConfigurationTable") {
        let token = &tokens[index];
        let table = text(token, "name");
        let Some(shape) = named.get(&table) else {
            continue;
        };
        let Some(end) = scan::element_end_in(&tokens, src, index) else {
            continue;
        };
        let children = |parent: usize, parent_end: usize, name: &[u8]| -> Vec<usize> {
            direct_children(&tokens, parent, parent_end)
                .into_iter()
                .filter(|&child| is(child, name))
                .collect()
        };
        let inside = |parents: Vec<usize>, name: &[u8]| -> Vec<usize> {
            parents
                .into_iter()
                .flat_map(|parent| children(parent, end_of(parent), name))
                .collect()
        };
        let copy_fields: Vec<&Token> = inside(
            inside(children(index, end, b"DataShape"), b"FieldDefinitions"),
            b"FieldDefinition",
        )
        .into_iter()
        .map(|field| &tokens[field])
        .collect();
        let rows = inside(children(index, end, b"Rows"), b"Row");
        if !rows.is_empty() && copy_fields.is_empty() {
            problems.push(TableProblem {
                rule: "config-table-rows-without-fields",
                message: format!(
                    "configuration table {table}: {} row(s) but no DataShape fields in the table, so the import drops every row; add {shape}'s fields under its DataShape",
                    rows.len()
                ),
                blocks: true,
            });
        }
        let Some(fields) = resolved(shapes, shape) else {
            continue;
        };
        let known: BTreeSet<&str> = fields.iter().map(|field| field.name.as_str()).collect();
        let mut lost = BTreeSet::new();
        for &row in &rows {
            for cell in direct_children(&tokens, row, end_of(row)) {
                let name = String::from_utf8_lossy(tokens[cell].name.of(src)).into_owned();
                if !known.contains(name.as_str()) {
                    lost.insert(name);
                }
            }
        }
        if !lost.is_empty() {
            problems.push(TableProblem {
                rule: "config-table-cell-not-in-shape",
                message: format!(
                    "configuration table {table}: rows hold {} that {shape} does not define, and the import drops them; add the field(s) to {shape} or remove the cells",
                    lost.into_iter().collect::<Vec<_>>().join(", ")
                ),
                blocks: true,
            });
        }
        if copy_fields.is_empty() {
            continue;
        }
        let differences = differences(fields, &copy_fields, &text);
        if !differences.is_empty() {
            problems.push(TableProblem {
                rule: "config-table-copy-differs",
                message: format!(
                    "configuration table {table}: its fields differ from {shape} ({}); the server replaces them with {shape}'s, so the entity reads back not kept. Write {shape}'s fields, ordinals and descriptions there",
                    differences.join("; ")
                ),
                blocks: false,
            });
        }
    }
    problems
}

/// The fields of the DataShape `name`, when this model holds all of them.
fn resolved<'a>(shapes: &'a [DataShape], name: &str) -> Option<&'a [Field]> {
    shapes
        .iter()
        .find(|shape| shape.name == name && shape.base_data_shape.is_none())
        .map(|shape| shape.fields.as_slice())
}

/// `01` and `1` are one ordinal; text that is not a number compares as written.
fn same_ordinal(left: &str, right: &str) -> bool {
    match (left.trim().parse::<u64>(), right.trim().parse::<u64>()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn is_key(field: &Field) -> bool {
    matches!(field.aspects.get("isPrimaryKey"), Some(Aspect::Bool(true)))
}

/// How the copy's fields differ from the DataShape's, by name.
fn differences(
    fields: &[Field],
    copy: &[&Token],
    text: &dyn Fn(&Token, &str) -> String,
) -> Vec<String> {
    let copied: BTreeMap<String, &Token> = copy
        .iter()
        .map(|field| (text(field, "name"), *field))
        .collect();
    let mut out = Vec::new();
    let missing: Vec<&str> = fields
        .iter()
        .filter(|field| !copied.contains_key(&field.name))
        .map(|field| field.name.as_str())
        .collect();
    if !missing.is_empty() {
        out.push(format!("missing {}", missing.join(", ")));
    }
    let extra: Vec<&str> = copied
        .keys()
        .filter(|name| !fields.iter().any(|field| &field.name == *name))
        .map(String::as_str)
        .collect();
    if !extra.is_empty() {
        out.push(format!("not in the DataShape {}", extra.join(", ")));
    }
    for field in fields {
        let Some(token) = copied.get(&field.name) else {
            continue;
        };
        let mut what = Vec::new();
        if text(token, "baseType") != field.base_type {
            what.push("baseType");
        }
        if !same_ordinal(&text(token, "ordinal"), &field.ordinal) {
            what.push("ordinal");
        }
        if text(token, "description") != field.description {
            what.push("description");
        }
        if (text(token, "aspect.isPrimaryKey") == "true") != is_key(field) {
            what.push("key");
        }
        if !what.is_empty() {
            out.push(format!("{} {}", field.name, what.join(", ")));
        }
    }
    out
}

/// The start or empty tags directly inside the element that opens at `parent`.
fn direct_children(tokens: &[Token], parent: usize, end: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(parent + 1) {
        match token.kind {
            Kind::Start => {
                if depth == 0 {
                    out.push(index);
                }
                depth += 1;
            }
            Kind::Empty if depth == 0 => out.push(index),
            Kind::End => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(name: &str, xml: &str) -> DataShape {
        DataShape {
            name: name.to_string(),
            base_data_shape: None,
            fields: super::super::datashape::extract(
                format!("<Entities><DataShapes><DataShape name=\"{name}\"><FieldDefinitions>{xml}</FieldDefinitions></DataShape></DataShapes></Entities>")
                    .as_bytes(),
            )
            .unwrap(),
        }
    }

    const SHAPE: &str = r#"<FieldDefinition name="a" baseType="STRING" ordinal="2" description="A"/><FieldDefinition name="b" baseType="STRING" ordinal="1" description="B" aspect.isPrimaryKey="true"/>"#;
    const COPY: &str = r#"<FieldDefinition name="a" baseType="STRING" ordinal="2" description="A"/><FieldDefinition name="b" baseType="STRING" ordinal="1" description="B" aspect.isPrimaryKey="true"/>"#;

    fn thing(copy: &str, rows: &str) -> String {
        format!(
            r#"<Entities><Things><Thing name="T"><ConfigurationTableDefinitions><ConfigurationTableDefinition name="T" isMultiRow="true" dataShapeName="Acme.Row_DS"/></ConfigurationTableDefinitions><ConfigurationTables><ConfigurationTable name="T" dataShapeName="Acme.Row_DS" isMultiRow="true">{copy}<Rows>{rows}</Rows></ConfigurationTable></ConfigurationTables><ThingShape><ServiceImplementations><ServiceImplementation name="S"><ConfigurationTables><ConfigurationTable name="T"><Rows><Row><code>x</code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>"#
        )
    }

    fn rules(src: &str, shapes: &[DataShape]) -> Vec<(&'static str, bool)> {
        check(src.as_bytes(), shapes)
            .into_iter()
            .map(|problem| (problem.rule, problem.blocks))
            .collect()
    }

    const ROW: &str = "<Row><a>x</a><b>k</b></Row>";

    #[test]
    fn the_export_form_passes_and_service_tables_are_left_alone() {
        let shapes = [shape("Acme.Row_DS", SHAPE)];
        let copy = format!("<DataShape><FieldDefinitions>{COPY}</FieldDefinitions></DataShape>");
        assert_eq!(rules(&thing(&copy, ROW), &shapes), []);
    }

    #[test]
    fn handler_tables_of_services_and_subscriptions_are_not_the_entitys() {
        let shapes = [shape("Acme.Row_DS", SHAPE)];
        // An own table named like a handler table, and a subscription's handler table.
        let src = r#"<Entities><Things><Thing name="T"><ConfigurationTableDefinitions><ConfigurationTableDefinition name="Script" isMultiRow="true" dataShapeName="Acme.Row_DS"/></ConfigurationTableDefinitions><ConfigurationTables></ConfigurationTables><ThingShape><Subscriptions><Subscription name="OnStart"><ServiceImplementation name="OnStart" handlerName="Script"><ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code>x</code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></Subscription></Subscriptions></ThingShape></Thing></Things></Entities>"#;
        assert_eq!(rules(src, &shapes), []);
    }

    #[test]
    fn ordinals_compare_as_numbers() {
        let shapes = [shape(
            "Acme.Row_DS",
            r#"<FieldDefinition name="b" baseType="STRING" ordinal="01" aspect.isPrimaryKey="true"/>"#,
        )];
        let copy = r#"<DataShape><FieldDefinitions><FieldDefinition name="b" baseType="STRING" ordinal="1" aspect.isPrimaryKey="true"/></FieldDefinitions></DataShape>"#;
        assert_eq!(rules(&thing(copy, ""), &shapes), []);
    }

    #[test]
    fn a_shape_without_a_key_fails_the_import() {
        let shapes = [shape(
            "Acme.Row_DS",
            r#"<FieldDefinition name="a" baseType="STRING" ordinal="1"/>"#,
        )];
        let copy = r#"<DataShape><FieldDefinitions><FieldDefinition name="a" baseType="STRING" ordinal="1"/></FieldDefinitions></DataShape>"#;
        assert_eq!(
            rules(&thing(copy, "<Row><a>x</a></Row>"), &shapes),
            [("config-table-shape-without-key", true)]
        );
    }

    #[test]
    fn rows_without_copy_fields_are_dropped_even_when_the_shape_is_elsewhere() {
        assert_eq!(
            rules(&thing("", ROW), &[]),
            [("config-table-rows-without-fields", true)]
        );
        let empty = "<DataShape><FieldDefinitions/></DataShape>";
        assert_eq!(
            rules(&thing(empty, ROW), &[]),
            [("config-table-rows-without-fields", true)]
        );
        // No rows, nothing to lose.
        assert_eq!(rules(&thing("", ""), &[]), []);
    }

    #[test]
    fn a_cell_the_shape_lacks_is_dropped() {
        let shapes = [shape("Acme.Row_DS", SHAPE)];
        let copy = format!("<DataShape><FieldDefinitions>{COPY}</FieldDefinitions></DataShape>");
        let problems = check(
            thing(&copy, "<Row><a>x</a><b>k</b><d>gone</d></Row>").as_bytes(),
            &shapes,
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].rule, "config-table-cell-not-in-shape");
        assert!(problems[0].blocks);
        assert!(
            problems[0].message.contains("rows hold d"),
            "{}",
            problems[0].message
        );
    }

    #[test]
    fn a_copy_that_differs_warns_and_names_each_difference() {
        let shapes = [shape("Acme.Row_DS", SHAPE)];
        let copy = r#"<DataShape><FieldDefinitions><FieldDefinition name="a" baseType="INTEGER" ordinal="2" description="A" aspect.isPrimaryKey="true"/><FieldDefinition name="z" baseType="STRING"/></FieldDefinitions></DataShape>"#;
        let problems = check(thing(copy, "").as_bytes(), &shapes);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].rule, "config-table-copy-differs");
        assert!(!problems[0].blocks);
        let message = &problems[0].message;
        assert!(
            message.contains("missing b")
                && message.contains("not in the DataShape z")
                && message.contains("a baseType, key"),
            "{message}"
        );
    }

    #[test]
    fn inline_definitions_inherited_definitions_and_unresolved_shapes_are_not_judged() {
        let shapes = [shape("Acme.Row_DS", SHAPE)];
        let differing = r#"<DataShape><FieldDefinitions><FieldDefinition name="q" baseType="STRING"/></FieldDefinitions></DataShape>"#;
        // An inline definition is validate's concern.
        let inline = thing(differing, "").replace(
            r#"dataShapeName="Acme.Row_DS"/>"#,
            r#"dataShapeName="Acme.Row_DS"><FieldDefinitions><FieldDefinition name="q" aspect.isPrimaryKey="true"/></FieldDefinitions></ConfigurationTableDefinition>"#,
        );
        assert_eq!(rules(&inline, &shapes), []);
        // No local definition: the one that applies is inherited.
        let inherited = thing(differing, "").replace(
            r#"<ConfigurationTableDefinition name="T" isMultiRow="true" dataShapeName="Acme.Row_DS"/>"#,
            "",
        );
        assert_eq!(rules(&inherited, &shapes), []);
        // A shape outside the model, or a derived one, cannot be compared.
        assert_eq!(rules(&thing(differing, ""), &[]), []);
        let mut derived = shape("Acme.Row_DS", SHAPE);
        derived.base_data_shape = Some("Acme.Base_DS".into());
        assert_eq!(rules(&thing(differing, ""), &[derived]), []);
    }
}
