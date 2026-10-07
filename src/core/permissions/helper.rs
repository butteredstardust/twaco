//! Helper mode: the Solution Framework permission helper's tables, kept equal to the policy.
//!
//! A permission helper Thing (template `PTCDTS.Base.ComponentPermissionHelper_TT`, from the
//! `PTCDTS.Base` common block) shows a project's permissions as three configuration tables, and its
//! mashup applies them: `RoleGroupsAndOrganizations` maps each role to its group and organizational
//! unit, `RunTimePermissionsTable` has a row per entity, resource and action with one boolean column
//! per role (`<role>Group`), and `VisibilityPermissionsTable` a row per entity with one column per
//! role (`<role>Org`). The two DataShapes behind the last two carry the same columns.
//!
//! In helper mode `permissions apply` writes these from the policy, so the helper's mashup shows
//! what the entity XML grants, and applying it there changes nothing. Existing run-time rows keep
//! their order and IDs; a row is added for each service of a Thing, ThingShape or ThingTemplate
//! that has none (as the helper's populate step would) and for each granted resource without one.

use super::audit::{run_time_kind, Loaded};
use super::model::ModelEntity;
use super::policy::Policy;
use super::{elements, Grant, Grants, KindKey, PermissionsError};
use crate::core::entity_carry::Kind;
use crate::core::normalise::{self, Element, Node};
use crate::core::scan;
use crate::core::splice::{self, Edit};
use std::collections::{BTreeMap, BTreeSet};

pub const ROLES_TABLE: &str = "RoleGroupsAndOrganizations";
pub const RUN_TIME_TABLE: &str = "RunTimePermissionsTable";
pub const VISIBILITY_TABLE: &str = "VisibilityPermissionsTable";

fn error(message: impl Into<String>) -> PermissionsError {
    PermissionsError(message.into())
}

/// One column of a table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ordinal: u32,
    pub base_type: String,
    /// `aspect.isPrimaryKey`: written when `Some`.
    pub primary_key: Option<bool>,
    /// `aspect.defaultValue`.
    pub default: Option<String>,
}

fn field(name: &str, ordinal: u32, base_type: &str) -> Field {
    Field {
        name: name.to_string(),
        ordinal,
        base_type: base_type.to_string(),
        primary_key: None,
        default: None,
    }
}

pub type Row = BTreeMap<String, String>;

/// A configuration table's columns, sorted as the helper writes them, and its rows in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub data_shape: String,
    pub fields: Vec<Field>,
    pub rows: Vec<Row>,
}

fn sort_fields(fields: &mut [Field]) {
    fields.sort_by_key(|f| f.name.to_lowercase());
}

/// The columns of the run-time table and its DataShape.
pub fn run_time_fields(policy: &Policy) -> Vec<Field> {
    let mut fields = vec![
        Field {
            primary_key: Some(true),
            ..field("ID", 0, "NUMBER")
        },
        Field {
            default: Some("Live".to_string()),
            ..field("Status", 1, "STRING")
        },
        field("entityType", 2, "STRING"),
        field("entityName", 3, "STRING"),
        field("resource", 4, "STRING"),
        field("type", 5, "STRING"),
    ];
    for (at, role) in policy.roles.iter().enumerate() {
        fields.push(field(
            &format!("{}Group", role.name),
            6 + at as u32,
            "BOOLEAN",
        ));
    }
    sort_fields(&mut fields);
    fields
}

/// The columns of the visibility table and its DataShape.
pub fn visibility_fields(policy: &Policy) -> Vec<Field> {
    let mut fields = vec![
        Field {
            default: Some("Live".to_string()),
            ..field("Status", 0, "STRING")
        },
        field("entityType", 1, "STRING"),
        Field {
            primary_key: Some(true),
            ..field("entityName", 2, "STRING")
        },
    ];
    for (at, role) in policy.roles.iter().enumerate() {
        fields.push(field(
            &format!("{}Org", role.name),
            3 + at as u32,
            "BOOLEAN",
        ));
    }
    sort_fields(&mut fields);
    fields
}

fn roles_fields() -> Vec<Field> {
    vec![
        Field {
            primary_key: Some(true),
            ..field("displayName", 1, "STRING")
        },
        Field {
            primary_key: Some(false),
            ..field("principal", 2, "STRING")
        },
        Field {
            primary_key: Some(false),
            ..field("principalType", 3, "STRING")
        },
    ]
}

/// The leaf text of an element: its CDATA or text, trimmed.
fn text_of(element: &Element) -> String {
    let mut out = String::new();
    for child in &element.children {
        match child {
            Node::Text(bytes) | Node::Cdata(bytes) => out.push_str(&String::from_utf8_lossy(bytes)),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn fields_of(definitions: &Element) -> Vec<Field> {
    elements(definitions)
        .filter(|e| e.name == b"FieldDefinition")
        .map(|e| {
            let get = |name: &str| super::attribute(e, name).map(str::to_string);
            Field {
                name: get("name").unwrap_or_default(),
                ordinal: get("ordinal")
                    .and_then(|o| o.parse().ok())
                    .unwrap_or_default(),
                base_type: get("baseType").unwrap_or_default(),
                primary_key: get("aspect.isPrimaryKey").map(|v| v == "true"),
                default: get("aspect.defaultValue"),
            }
        })
        .collect()
}

fn child<'a>(element: &'a Element, name: &str) -> Option<&'a Element> {
    elements(element).find(|e| e.name == name.as_bytes())
}

/// The helper's configuration tables, by name.
pub(crate) fn read_tables(entity: &Element) -> BTreeMap<String, Table> {
    let mut out = BTreeMap::new();
    let Some(tables) = child(entity, "ConfigurationTables") else {
        return out;
    };
    for table in elements(tables).filter(|e| e.name == b"ConfigurationTable") {
        let name = super::attribute(table, "name")
            .unwrap_or_default()
            .to_string();
        let fields = child(table, "DataShape")
            .and_then(|shape| child(shape, "FieldDefinitions"))
            .map(fields_of)
            .unwrap_or_default();
        let rows = child(table, "Rows")
            .map(|rows| {
                elements(rows)
                    .filter(|e| e.name == b"Row")
                    .map(|row| {
                        elements(row)
                            .map(|cell| {
                                (
                                    String::from_utf8_lossy(&cell.name).into_owned(),
                                    text_of(cell),
                                )
                            })
                            .collect()
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.insert(
            name,
            Table {
                data_shape: super::attribute(table, "dataShapeName")
                    .unwrap_or_default()
                    .to_string(),
                fields,
                rows,
            },
        );
    }
    out
}

/// What the policy wants on an entity, or what its blocks hold when the policy leaves it alone.
pub fn effective(policy: &Policy, entity: &ModelEntity, kind: Kind) -> Grants {
    let current = entity
        .sets
        .get(&KindKey::of(kind))
        .cloned()
        .unwrap_or_default();
    if policy.is_unmanaged(entity.name()) {
        return current;
    }
    super::audit::wanted(policy, entity, kind, &current)
}

fn bool_text(value: bool) -> String {
    value.to_string()
}

/// The ID text of a new row: one more than the largest, written as the helper writes a NUMBER.
fn next_id(rows: &[Row]) -> f64 {
    rows.iter()
        .filter_map(|row| row.get("ID").and_then(|id| id.parse::<f64>().ok()))
        .fold(0.0, f64::max)
        + 1.0
}

fn number_text(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

/// The three tables as the policy has them, given the helper's current ones.
pub fn wanted_tables(
    loaded: &Loaded,
    current: &BTreeMap<String, Table>,
) -> BTreeMap<String, Table> {
    let policy = &loaded.policy;
    let project = &policy.project;
    let by_name: BTreeMap<&str, &ModelEntity> =
        loaded.entities.iter().map(|e| (e.name(), e)).collect();
    // The helper's own naming (CreateVisilibilityandRuntimeDataShapes); another name is a slip.
    let data_shape = |suffix: &str| format!("{project}.{suffix}");
    let mut out = BTreeMap::new();

    let mut roles: Vec<Row> = Vec::new();
    for role in &policy.roles {
        roles.push(Row::from([
            ("displayName".to_string(), format!("{}Group", role.name)),
            ("principal".to_string(), role.group.clone()),
            ("principalType".to_string(), "Group".to_string()),
        ]));
        if let Some(org) = &role.org {
            roles.push(Row::from([
                ("displayName".to_string(), format!("{}Org", role.name)),
                ("principal".to_string(), org.name.clone()),
                ("principalType".to_string(), org.principal_type.clone()),
            ]));
        }
    }
    roles.sort_by_key(|row| row["displayName"].to_lowercase());
    out.insert(
        ROLES_TABLE.to_string(),
        Table {
            data_shape: current
                .get(ROLES_TABLE)
                .map(|t| t.data_shape.clone())
                .unwrap_or_default(),
            fields: roles_fields(),
            rows: roles,
        },
    );

    // Run time: the rows the helper has, for entities that still exist, then new ones.
    let mut run_time_grants: BTreeMap<&str, Grants> = BTreeMap::new();
    for entity in &loaded.entities {
        if let Some(kind) = run_time_kind(entity) {
            run_time_grants.insert(entity.name(), effective(policy, entity, kind));
        }
    }
    let granted = |entity: &str, resource: &str, action: &str, group: &str| {
        run_time_grants.get(entity).is_some_and(|grants| {
            grants
                .get(&Grant {
                    resource: resource.to_string(),
                    action: action.to_string(),
                    principal: group.to_string(),
                    principal_type: "Group".to_string(),
                })
                .copied()
                .unwrap_or(false)
        })
    };
    let mut rows: Vec<Row> = Vec::new();
    let mut seen: BTreeSet<(String, String, String)> = BTreeSet::new();
    let existing = current
        .get(RUN_TIME_TABLE)
        .map(|t| t.rows.clone())
        .unwrap_or_default();
    for old in &existing {
        let name = old.get("entityName").cloned().unwrap_or_default();
        if !by_name.contains_key(name.as_str()) {
            continue;
        }
        rows.push(old.clone());
        seen.insert((
            name,
            old.get("resource").cloned().unwrap_or_default(),
            old.get("type").cloned().unwrap_or_default(),
        ));
    }
    let mut id = next_id(&existing);
    let mut add = |rows: &mut Vec<Row>, entity: &ModelEntity, resource: &str, action: &str| {
        let key = (
            entity.name().to_string(),
            resource.to_string(),
            action.to_string(),
        );
        if seen.insert(key) {
            rows.push(Row::from([
                ("ID".to_string(), number_text(id)),
                ("entityType".to_string(), entity.entity_type.clone()),
                ("entityName".to_string(), entity.name().to_string()),
                ("resource".to_string(), resource.to_string()),
                ("type".to_string(), action.to_string()),
            ]));
            id += 1.0;
        }
    };
    let mut hosts: Vec<&ModelEntity> = loaded
        .entities
        .iter()
        .filter(|e| run_time_kind(e).is_some())
        .collect();
    let collection_rank = |e: &ModelEntity| match e.entity_type.as_str() {
        "Thing" => 0,
        "ThingTemplate" => 1,
        _ => 2,
    };
    hosts.sort_by(|a, b| (collection_rank(a), a.name()).cmp(&(collection_rank(b), b.name())));
    for entity in &hosts {
        for service in &entity.services {
            add(&mut rows, entity, service, "ServiceInvoke");
        }
    }
    for entity in &hosts {
        if let Some(grants) = run_time_grants.get(entity.name()) {
            for grant in grants.keys() {
                add(&mut rows, entity, &grant.resource, &grant.action);
            }
        }
    }
    for row in &mut rows {
        let entity = row.get("entityName").cloned().unwrap_or_default();
        let resource = row.get("resource").cloned().unwrap_or_default();
        let action = row.get("type").cloned().unwrap_or_default();
        row.retain(|key, _| {
            matches!(
                key.as_str(),
                "ID" | "entityType" | "entityName" | "resource" | "type"
            )
        });
        row.insert("Status".to_string(), "Live".to_string());
        for role in &policy.roles {
            row.insert(
                format!("{}Group", role.name),
                bool_text(granted(&entity, &resource, &action, &role.group)),
            );
        }
    }
    out.insert(
        RUN_TIME_TABLE.to_string(),
        Table {
            data_shape: data_shape("RunTimePermissions_DS"),
            fields: run_time_fields(policy),
            rows,
        },
    );

    // Visibility: a row per entity of the project, by name.
    let mut visibility: Vec<Row> = Vec::new();
    let mut names: Vec<&ModelEntity> = loaded.entities.iter().collect();
    names.sort_by(|a, b| a.name().cmp(b.name()));
    for entity in names {
        let grants = effective(policy, entity, Kind::Visibility);
        let mut row = Row::from([
            ("Status".to_string(), "Live".to_string()),
            ("entityType".to_string(), entity.entity_type.clone()),
            ("entityName".to_string(), entity.name().to_string()),
        ]);
        for role in &policy.roles {
            let visible = role.org.as_ref().is_some_and(|org| {
                grants
                    .get(&Grant {
                        resource: String::new(),
                        action: "Visibility".to_string(),
                        principal: org.name.clone(),
                        principal_type: org.principal_type.clone(),
                    })
                    .copied()
                    .unwrap_or(false)
            });
            row.insert(format!("{}Org", role.name), bool_text(visible));
        }
        visibility.push(row);
    }
    out.insert(
        VISIBILITY_TABLE.to_string(),
        Table {
            data_shape: data_shape("VisibilityPermissions_DS"),
            fields: visibility_fields(policy),
            rows: visibility,
        },
    );
    out
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// One `FieldDefinition`, its attributes one step in.
fn field_xml(out: &mut String, field: &Field, indent: &str, eol: &str) {
    out.push_str(&format!("{indent}<FieldDefinition{eol}"));
    if let Some(primary) = field.primary_key {
        out.push_str(&format!("{indent} aspect.isPrimaryKey=\"{primary}\"{eol}"));
    }
    if let Some(default) = &field.default {
        out.push_str(&format!(
            "{indent} aspect.defaultValue=\"{}\"{eol}",
            escape(default)
        ));
    }
    out.push_str(&format!(
        "{indent} baseType=\"{}\"{eol}{indent} description=\"\"{eol}{indent} name=\"{}\"{eol}{indent} ordinal=\"{}\"></FieldDefinition>{eol}",
        escape(&field.base_type),
        escape(&field.name),
        field.ordinal
    ));
}

fn cdata(value: &str) -> String {
    String::from_utf8(scan::render_cdata(value.as_bytes())).unwrap_or_default()
}

/// A whole `ConfigurationTable`, from its opening tag to its closing tag.
fn table_xml(name: &str, table: &Table, indent: &str, eol: &str) -> String {
    let step = |n: usize| format!("{indent}{}", " ".repeat(4 * n));
    let mut out = format!(
        "<ConfigurationTable{eol}{indent} dataShapeName=\"{}\"{eol}{indent} description=\"\"{eol}{indent} isMultiRow=\"true\"{eol}{indent} name=\"{}\"{eol}{indent} ordinal=\"0\">{eol}",
        escape(&table.data_shape),
        escape(name)
    );
    out.push_str(&format!(
        "{}<DataShape>{eol}{}<FieldDefinitions>{eol}",
        step(1),
        step(2)
    ));
    for field in &table.fields {
        field_xml(&mut out, field, &step(3), eol);
    }
    out.push_str(&format!(
        "{}</FieldDefinitions>{eol}{}</DataShape>{eol}",
        step(2),
        step(1)
    ));
    out.push_str(&format!("{}<Rows>{eol}", step(1)));
    let booleans: BTreeSet<&str> = table
        .fields
        .iter()
        .filter(|f| f.base_type == "BOOLEAN")
        .map(|f| f.name.as_str())
        .collect();
    for row in &table.rows {
        out.push_str(&format!("{}<Row>{eol}", step(2)));
        for field in &table.fields {
            let Some(value) = row.get(&field.name) else {
                continue;
            };
            let cell = step(3);
            if booleans.contains(field.name.as_str()) {
                out.push_str(&format!("{cell}<{0}>{1}</{0}>{eol}", field.name, value));
            } else {
                out.push_str(&format!(
                    "{cell}<{0}>{eol}{cell}{1}{eol}{cell}{2}{eol}{cell}]]>{eol}{cell}</{0}>{eol}",
                    field.name,
                    "<![CDATA[",
                    cdata_body(value)
                ));
            }
        }
        out.push_str(&format!("{}</Row>{eol}", step(2)));
    }
    out.push_str(&format!(
        "{}</Rows>{eol}{indent}</ConfigurationTable>",
        step(1)
    ));
    out
}

/// A value inside `<![CDATA[` ... `]]>` written on its own line, split where it holds `]]>`.
fn cdata_body(value: &str) -> String {
    let rendered = cdata(value);
    rendered
        .strip_prefix("<![CDATA[")
        .and_then(|rest| rest.strip_suffix("]]>"))
        .unwrap_or(value)
        .to_string()
}

fn indent_before(src: &[u8], at: usize) -> String {
    let line_start = src[..at]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let before = &src[line_start..at];
    if before.iter().all(|b| *b == b' ' || *b == b'\t') {
        String::from_utf8_lossy(before).into_owned()
    } else {
        String::new()
    }
}

/// Replace the named tables of a helper Thing whose content differs. Same bytes when none does.
pub fn rewrite_tables(
    src: &[u8],
    wanted: &BTreeMap<String, Table>,
) -> Result<Vec<u8>, PermissionsError> {
    let entity = normalise::entity_of(src).map_err(|e| error(e.to_string()))?;
    let current = read_tables(&entity);
    let scanned = scan::scan(src).map_err(|e| error(e.to_string()))?;
    let tokens = &scanned.tokens;
    let root = crate::core::sidecar::entity_element(tokens, src)
        .ok_or_else(|| error("no entity element"))?;
    let eol = crate::core::workspace::line_ending(&String::from_utf8_lossy(src));
    let Some(&tables) = scan::child_tags(tokens, src, "ConfigurationTables", root).first() else {
        return Err(error("the helper Thing has no ConfigurationTables"));
    };
    let mut edits = Vec::new();
    for (name, table) in wanted {
        // Counted first: with two, the map read above holds only the last one.
        let found: Vec<usize> = scan::child_tags(tokens, src, "ConfigurationTable", tables)
            .into_iter()
            .filter(|&at| {
                scan::attribute(src, &tokens[at], "name")
                    .ok()
                    .flatten()
                    .is_some_and(|span| span.of(src) == name.as_bytes())
            })
            .collect();
        let [at] = found.as_slice() else {
            return Err(error(format!(
                "the helper Thing has {} {name} tables; it needs exactly one",
                found.len()
            )));
        };
        if current.get(name) == Some(table) {
            continue;
        }
        let span = scan::element_span(tokens, *at)
            .ok_or_else(|| error(format!("{name} is not closed")))?;
        let indent = indent_before(src, span.start);
        edits.push(Edit::new(
            span,
            table_xml(name, table, &indent, eol).into_bytes(),
        ));
    }
    if edits.is_empty() {
        return Ok(src.to_vec());
    }
    edits.sort_by_key(|edit| edit.span.start);
    let out = splice::splice(src, &edits).map_err(|e| error(e.to_string()))?;
    let back = read_tables(&normalise::entity_of(&out).map_err(|e| error(e.to_string()))?);
    for (name, table) in wanted {
        if back.get(name) != Some(table) {
            return Err(error(format!(
                "the rewritten {name} table does not read back as intended"
            )));
        }
    }
    Ok(out)
}

/// Make a DataShape's own `FieldDefinitions` exactly `fields`. Same bytes when they already are.
pub fn rewrite_fields(src: &[u8], fields: &[Field]) -> Result<Vec<u8>, PermissionsError> {
    let entity = normalise::entity_of(src).map_err(|e| error(e.to_string()))?;
    let current = child(&entity, "FieldDefinitions").map(fields_of);
    if current.as_deref() == Some(fields) {
        return Ok(src.to_vec());
    }
    let scanned = scan::scan(src).map_err(|e| error(e.to_string()))?;
    let tokens = &scanned.tokens;
    let root = crate::core::sidecar::entity_element(tokens, src)
        .ok_or_else(|| error("no entity element"))?;
    let eol = crate::core::workspace::line_ending(&String::from_utf8_lossy(src));
    let Some(&at) = scan::child_tags(tokens, src, "FieldDefinitions", root).first() else {
        return Err(error("the DataShape has no FieldDefinitions"));
    };
    let span =
        scan::element_span(tokens, at).ok_or_else(|| error("FieldDefinitions is not closed"))?;
    let indent = indent_before(src, span.start);
    let mut block = format!("<FieldDefinitions>{eol}");
    for field in fields {
        field_xml(&mut block, field, &format!("{indent}    "), eol);
    }
    block.push_str(&format!("{indent}</FieldDefinitions>"));
    let out = splice::splice(src, &[Edit::new(span, block.into_bytes())])
        .map_err(|e| error(e.to_string()))?;
    let back = normalise::entity_of(&out).map_err(|e| error(e.to_string()))?;
    if child(&back, "FieldDefinitions").map(fields_of).as_deref() != Some(fields) {
        return Err(error(
            "the rewritten FieldDefinitions do not read back as intended",
        ));
    }
    Ok(out)
}

/// What differs between two versions of a table, for a finding's details.
pub fn describe(name: &str, current: Option<&Table>, wanted: &Table) -> Vec<String> {
    let Some(current) = current else {
        return vec![format!("{name}: missing")];
    };
    let mut out = Vec::new();
    if current.fields != wanted.fields {
        let have: BTreeSet<&str> = current.fields.iter().map(|f| f.name.as_str()).collect();
        let want: BTreeSet<&str> = wanted.fields.iter().map(|f| f.name.as_str()).collect();
        let added: Vec<&&str> = want.difference(&have).collect();
        let dropped: Vec<&&str> = have.difference(&want).collect();
        out.push(format!(
            "{name}: columns differ (add {added:?}, drop {dropped:?}, or their definitions)"
        ));
    }
    let key = |row: &Row| {
        [
            row.get("entityName"),
            row.get("resource"),
            row.get("type"),
            row.get("displayName"),
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
    };
    let have: BTreeMap<String, &Row> = current.rows.iter().map(|r| (key(r), r)).collect();
    let want: BTreeMap<String, &Row> = wanted.rows.iter().map(|r| (key(r), r)).collect();
    for (k, row) in &want {
        match have.get(k) {
            None => out.push(format!("{name}: add row {k}")),
            Some(old) if old != row => {
                let changed: Vec<String> = row
                    .iter()
                    .filter(|(field, value)| old.get(*field) != Some(*value))
                    .map(|(field, value)| format!("{field}={value}"))
                    .collect();
                out.push(format!("{name}: row {k}: {}", changed.join(", ")));
            }
            Some(_) => {}
        }
    }
    for k in have.keys() {
        if !want.contains_key(k) {
            out.push(format!("{name}: drop row {k}"));
        }
    }
    if out.is_empty() {
        out.push(format!("{name}: row order differs"));
    }
    out
}

/// One helper file's new bytes, and what changed in it.
pub struct HelperEdit<'a> {
    pub entity: &'a ModelEntity,
    pub after: Vec<u8>,
    pub details: Vec<String>,
}

/// The helper Thing's tables and the two DataShapes' columns, written over `bytes_of` (each
/// file's bytes after any block change). Only files that change are returned.
pub fn edits<'a>(
    loaded: &'a Loaded,
    helper: &'a ModelEntity,
    bytes_of: &dyn Fn(&ModelEntity) -> Result<Vec<u8>, PermissionsError>,
) -> Result<Vec<HelperEdit<'a>>, PermissionsError> {
    let at = |entity: &ModelEntity, e: PermissionsError| error(format!("{}: {e}", entity.key()));
    let mut out = Vec::new();
    let before = bytes_of(helper)?;
    let current = read_tables(&normalise::entity_of(&before).map_err(|e| error(e.to_string()))?);
    let wanted = wanted_tables(loaded, &current);
    let after = rewrite_tables(&before, &wanted).map_err(|e| at(helper, e))?;
    if after != before {
        let details = wanted
            .iter()
            .filter(|(name, table)| current.get(*name) != Some(*table))
            .flat_map(|(name, table)| describe(name, current.get(name), table))
            .collect();
        out.push(HelperEdit {
            entity: helper,
            after,
            details,
        });
    }
    for (table, fields) in [
        (RUN_TIME_TABLE, run_time_fields(&loaded.policy)),
        (VISIBILITY_TABLE, visibility_fields(&loaded.policy)),
    ] {
        let shape = &wanted[table].data_shape;
        let Some(entity) = loaded
            .entities
            .iter()
            .find(|e| e.entity_type == "DataShape" && e.name() == shape)
        else {
            return Err(error(format!(
                "the project has no DataShape {shape} for the helper's {table}"
            )));
        };
        let before = bytes_of(entity)?;
        let after = rewrite_fields(&before, &fields).map_err(|e| at(entity, e))?;
        if after != before {
            out.push(HelperEdit {
                entity,
                after,
                details: vec![format!("{shape}: columns follow the roles of the policy")],
            });
        }
    }
    Ok(out)
}

/// Audit findings of helper mode: tables and DataShape columns the policy would change, and run-time
/// rows for services that no longer exist.
pub fn findings(
    loaded: &Loaded,
    helper: &ModelEntity,
) -> Result<Vec<super::audit::Finding>, PermissionsError> {
    use super::audit::{Finding, Severity};
    let mut out = Vec::new();
    let read = |entity: &ModelEntity| Ok(entity.bytes.to_vec());
    let bytes = helper.bytes.to_vec();
    let current = read_tables(&normalise::entity_of(&bytes).map_err(|e| error(e.to_string()))?);
    let planned = match edits(loaded, helper, &read) {
        Ok(planned) => planned,
        Err(why) => {
            out.push(Finding {
                severity: Severity::Error,
                code: "helper-unwritable",
                entity: Some(helper.key()),
                message: format!("the permission helper cannot be kept as the policy says: {why}"),
                details: Vec::new(),
            });
            return Ok(out);
        }
    };
    for (entity, details) in planned.into_iter().map(|edit| (edit.entity, edit.details)) {
        out.push(Finding {
            severity: Severity::Error,
            code: "helper-differs-from-policy",
            entity: Some(entity.key()),
            message: if entity.name() == helper.name() {
                "the permission helper's tables differ from the policy; `permissions apply` writes them".to_string()
            } else {
                "its columns differ from the policy's roles; `permissions apply` writes them".to_string()
            },
            details,
        });
    }
    let by_name: BTreeMap<&str, &ModelEntity> =
        loaded.entities.iter().map(|e| (e.name(), e)).collect();
    if let Some(table) = current.get(RUN_TIME_TABLE) {
        for row in &table.rows {
            let (Some(name), Some(resource), Some(action)) =
                (row.get("entityName"), row.get("resource"), row.get("type"))
            else {
                continue;
            };
            if action != "ServiceInvoke" || resource == "*" {
                continue;
            }
            // Only a ThingShape inherits nothing: a Thing's or a template's service can come from
            // a template outside the solution.
            if let Some(entity) = by_name.get(name.as_str()) {
                if entity.entity_type == "ThingShape" && !entity.services.contains(resource) {
                    out.push(Finding {
                        severity: Severity::Warning,
                        code: "helper-row-for-missing-service",
                        entity: Some(helper.key()),
                        message: format!(
                            "{RUN_TIME_TABLE} has a row for {name}.{resource}, which the entity does not define"
                        ),
                        details: Vec::new(),
                    });
                }
            }
        }
    }
    Ok(out)
}
