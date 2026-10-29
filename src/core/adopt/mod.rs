//! Comparing a designer's Composer export with the repository, before anything is adopted.
//!
//! A Composer export may contain stale service bodies, so importing it wholesale can revert
//! repository changes. It also uses exporter-specific layout and includes live metadata and
//! values, making a plain diff too noisy to isolate meaningful changes.
//!
//! So the report answers two questions, in this order:
//!
//! 1. **What would the export revert?** Every service body in the export is compared with its
//!    sidecar, or with the entity XML where there is none.
//! 2. **What would it change?** Entity by entity and node by node, with the two exporters'
//!    formatting settled, regenerated binding ids counted rather than listed, and the solution's
//!    `[adopt] ignore_paths` left out.
//!
//! This is the generic engine; applying an export is project policy. The comparison follows
//! ElementTree-compatible text semantics: an element's text is what precedes its first child,
//! CDATA included and comments invisible.

use super::codes::{Coded, ErrorCode};
use std::collections::BTreeSet;
use std::path::PathBuf;

mod apply;
mod base;
mod classify;
mod compare;
mod flatten;
mod read;
mod retype;

#[cfg(test)]
mod apply_tests;
#[cfg(test)]
mod base_tests;
#[cfg(test)]
mod classify_tests;
#[cfg(test)]
mod retype_tests;

use crate::core::basetype::Mapped;
pub(crate) use apply::{apply, is_file_name};
pub(crate) use base::{check_handoff, handoffs, record_handoff};
pub use base::{suggest as base_suggestions, suggestions_json, Handoff, Side, NO_BASE_NOTE};
pub use classify::{Change, Kind};
pub(crate) use compare::{compare_with, CompareOptions};
pub(crate) use flatten::{row_key, service_bodies};
pub(crate) use read::{
    export_entities, export_entities_bytes, raw_entity_bytes, replace_entity_bytes,
    repository_entities,
};

/// Elements the platform rewrites on its own: whoever exported the file, an icon nobody sets,
/// and two history tables that grow on every save.
const NOISE_TAGS: [&str; 4] = ["ChangeHistory", "ConfigurationChanges", "Owner", "avatar"];
/// Attributes with the same problem. `sourceType`/`source` record which import last wrote the
/// entity, so they differ by construction between two servers.
const NOISE_ATTRS: [&str; 5] = [
    "lastModifiedDate",
    "owner",
    "sourceType",
    "source",
    "creationDate",
];
/// A configuration table row identifies itself by one of these children, in this order. Keying
/// on the value rather than the position stops an inserted row from shifting every path after it.
const ROW_KEY_TAGS: [&str; 4] = ["UID", "Name", "name", "id"];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityRef {
    pub collection: String,
    pub name: String,
}

impl EntityRef {
    pub fn path(&self) -> String {
        format!("{}/{}", self.collection, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub path: String,
    pub export: Option<String>,
    pub repo: Option<String>,
}

/// A base type (an XML `baseType` attribute or a mashup JSON `BaseType`) that differs, present
/// on both sides.
fn is_type_change(difference: &Difference) -> bool {
    let path = difference.path.as_str();
    // A contract's type only: a field, parameter, property or result definition in the XML, or
    // a mashup's data sources and bindings in its JSON, not any widget property of that name.
    let definition = ["FieldDefinition", "ResultType", "PropertyDefinition"]
        .iter()
        .any(|tag| path.contains(tag));
    let data = path.contains("#json.Data.") || path.contains("#json.DataBindings[");
    let typed = (path.ends_with("@baseType") && definition)
        || ((path.ends_with(".BaseType") || path.ends_with(".baseType")) && data);
    match (&difference.export, &difference.repo) {
        (Some(export), Some(repo)) => typed && export != repo,
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    New,
    Changed,
    Identical,
}

#[derive(Debug, Clone)]
pub struct EntityReport {
    pub entity: EntityRef,
    /// The export's own `projectName`, so a multi-project drop is attributed per project.
    pub project: String,
    pub(crate) status: Status,
    pub differences: Vec<Difference>,
    pub volatile_ids: usize,
    pub ignored: usize,
    /// Whether this is a designer-owned collection or backend work.
    pub kind: Kind,
    /// What the three-way comparison says about importing the export.
    pub change: Change,
}

#[derive(Debug, Clone)]
pub struct ServiceReport {
    pub entity: String,
    pub service: String,
    pub source: PathBuf,
    /// Generated here; the repository is authoritative and the difference expected.
    pub generated: bool,
    /// False when compared with the entity XML because the service has no sidecar.
    pub sidecar: bool,
    /// What the three-way comparison says about this service body.
    pub change: Change,
}

#[derive(Debug, Default)]
pub struct Report {
    /// The base used for the three-way comparison, when one was found.
    pub base: Option<String>,
    /// Advice about resolving this comparison, presented by both front ends.
    pub notes: Vec<String>,
    /// The parsed base is retained for apply's frame-level merge.  It is intentionally not a
    /// presentation field: callers present `base`, above, rather than an XML tree.
    base_side: Option<Side>,
    pub services: Vec<ServiceReport>,
    pub entities: Vec<EntityReport>,
    /// Exported services with nothing here to compare with.
    pub unmatched_services: Vec<String>,
    /// In the repository, in a collection the export carries, but not in the export.
    pub absent: Vec<EntityRef>,
    pub retyped_arguments: Vec<RetypedArgument>,
    pub retyped_defaults: Vec<RetypedDefault>,
}

/// A literal argument a script passes to a service parameter the export retypes.
#[derive(Debug, Clone)]
pub struct RetypedArgument {
    pub service_entity: String,
    pub service: String,
    pub parameter: String,
    pub from: String,
    pub to: String,
    pub caller: String,
    pub file: PathBuf,
    pub line: usize,
    pub literal: String,
    pub(crate) mapped: Mapped,
}

impl RetypedArgument {
    pub fn mapped_source(&self) -> Option<&str> {
        match &self.mapped {
            Mapped::Rewritten(text) => Some(text),
            _ => None,
        }
    }
    pub fn unmappable_why(&self) -> Option<&'static str> {
        match self.mapped {
            Mapped::Unmappable(why) => Some(why),
            _ => None,
        }
    }
}

/// A retyped parameter's default value that the new base type does not take.
#[derive(Debug, Clone)]
pub struct RetypedDefault {
    pub entity: String,
    pub service: String,
    pub parameter: String,
    pub to: String,
    pub default: String,
    pub why: &'static str,
}

impl Report {
    /// Differences that retype something both sides have: a mashup parameter, a binding, a
    /// service parameter or a field. Whoever passes the old type now sends a value the other
    /// side does not expect, so these are shown whatever else changed.
    pub fn type_changes(&self) -> Vec<(&EntityRef, &Difference)> {
        self.with_status(Status::Changed)
            .flat_map(|entry| {
                entry
                    .differences
                    .iter()
                    .filter(|difference| is_type_change(difference))
                    .map(move |difference| (&entry.entity, difference))
            })
            .collect()
    }

    pub fn with_status(&self, status: Status) -> impl Iterator<Item = &EntityReport> {
        self.entities
            .iter()
            .filter(move |entity| entity.status == status)
    }

    /// Non-generated services for which importing the export would undo or conflict with
    /// repository work. `Theirs`, `Added` and `Same` are not reverts.
    pub fn reverts(&self) -> impl Iterator<Item = &ServiceReport> {
        self.services.iter().filter(|service| {
            !service.generated
                && matches!(
                    service.change,
                    Change::Stale | Change::Conflict | Change::Unknown
                )
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdoptError {
    #[error("cannot read export {}: {why}", .path.display())]
    Export { path: PathBuf, why: String },
    #[error("cannot read {}: {why}", .path.display())]
    Repository { path: PathBuf, why: String },
    #[error("cannot use base {base}: {why}")]
    Base { base: String, why: String },
    #[error("handoff {name} already exists")]
    AlreadyExists { name: String },
    /// The writes, made as one transaction, did not happen.
    #[error("nothing was adopted: {0}")]
    Write(super::transaction::TransactionError),
    #[error("--take names nothing in this report: {0}")]
    Take(String),
}

/// Which side an explicit adopt resolution selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TakeSide {
    Theirs,
    Ours,
}

/// An explicit resolution for an entity (`Thing`) or service (`Thing.Run`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Take {
    pub side: TakeSide,
    pub target: String,
}

/// The take target that resolves every UI conflict and unknown in favour of the export.
pub(crate) const TAKE_ALL_UI: &str = "ui";
/// What `apply` did: one line per step, in the report's order, and any declaration refresh.
pub struct ApplyOutcome {
    pub lines: Vec<String>,
    pub types: super::types::Refresh,
    /// The retyped service parameters this apply adopted, as `(entity, service, parameter)`: only
    /// their arguments and defaults were mapped, or are left unmapped.
    pub retypes: BTreeSet<(String, String, String)>,
}

impl ApplyOutcome {
    /// The report's retyped arguments that this apply adopted but could not map.
    pub fn unmapped_arguments<'a>(
        &'a self,
        report: &'a Report,
    ) -> impl Iterator<Item = &'a RetypedArgument> {
        report.retyped_arguments.iter().filter(|argument| {
            argument.unmappable_why().is_some()
                && self.retypes.contains(&(
                    argument.service_entity.clone(),
                    argument.service.clone(),
                    argument.parameter.clone(),
                ))
        })
    }

    /// The report's retyped defaults that this apply adopted but the new type does not take.
    pub fn unmapped_defaults<'a>(
        &'a self,
        report: &'a Report,
    ) -> impl Iterator<Item = &'a RetypedDefault> {
        report.retyped_defaults.iter().filter(|default| {
            self.retypes.contains(&(
                default.entity.clone(),
                default.service.clone(),
                default.parameter.clone(),
            ))
        })
    }
}
impl Coded for AdoptError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Export { .. }
            | Self::Repository { .. }
            | Self::Base { .. }
            | Self::AlreadyExists { .. } => ErrorCode::IoError,
            Self::Take(_) => ErrorCode::InvalidArguments,
            Self::Write(error) => error.code(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::apply::{is_base64, media_content, render_parameter_definitions};

    use super::compare::compare;
    use super::flatten::{canonical, et_itertext, et_text, flatten, is_volatile_id};

    use crate::core::config::Solution;
    use crate::core::lock::WorkspaceLock;
    use crate::core::normalise::{self, Element, Node};

    use std::path::Path;
    #[test]
    fn a_base_type_changed_on_both_sides_is_a_type_change() {
        let change = |path: &str, export: Option<&str>, repo: Option<&str>| Difference {
            path: path.into(),
            export: export.map(Into::into),
            repo: repo.map(Into::into),
        };
        let entry = |collection: &str, differences: Vec<Difference>| EntityReport {
            entity: EntityRef {
                collection: collection.into(),
                name: "Acme.Board_MU".into(),
            },
            project: "Acme".into(),
            status: Status::Changed,
            differences,
            volatile_ids: 0,
            ignored: 0,
            kind: classify::Kind::Ui,
            change: classify::Change::Theirs,
        };
        let report = Report {
            entities: vec![entry(
                "Mashups",
                vec![
                    // As a Composer export retyped a mashup parameter and its binding.
                    change("/ParameterDefinitions/FieldDefinition[cardUid]@baseType", Some("STRING"), Some("LONG")),
                    change("/mashupContent#json.Data.Things_Acme.Manager.Services[0].Parameters[0].BaseType", Some("STRING"), Some("LONG")),
                    change("/ParameterDefinitions/FieldDefinition[cardUid]@description", Some("b"), Some("a")),
                    change("/ParameterDefinitions/FieldDefinition[added]@baseType", Some("STRING"), None),
                    // A widget property that happens to be called BaseType is no contract.
                    change("/mashupContent#json.UI.Widgets[0].Properties.BaseType", Some("NUMBER"), Some("STRING")),
                ],
            )],
            ..Report::default()
        };
        let found: Vec<&str> = report
            .type_changes()
            .iter()
            .map(|(_, difference)| difference.path.as_str())
            .collect();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found
            .iter()
            .all(|path| path.to_lowercase().ends_with("basetype")));
    }

    use super::*;

    fn element(src: &str) -> Element {
        match normalise::parse_document(src.as_bytes())
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
        {
            Node::Element(e) => e,
            _ => panic!("not an element"),
        }
    }

    #[test]
    fn canonical_settles_whitespace_and_json_key_order() {
        assert_eq!(canonical("  a \n\t b  "), "a b");
        assert_eq!(
            canonical("{\"b\": 1,\n \"a\": [1, 2]}"),
            "{\"a\": [1, 2], \"b\": 1}"
        );
        assert_eq!(canonical("{not json"), "{not json");
        assert_eq!(
            canonical("x\u{1f}y"),
            "x y",
            "Unicode separators are whitespace"
        );
    }

    #[test]
    fn element_text_is_what_precedes_the_first_child_cdata_included() {
        let e = element("<a> one <![CDATA[two]]><!-- c --> three<b>x</b>tail</a>");
        assert_eq!(et_text(&e).unwrap(), " one two three");
        let mut texts = Vec::new();
        et_itertext(&e, &mut texts);
        assert_eq!(texts, [" one two three", "x", "tail"]);
    }

    #[test]
    fn paths_use_names_and_row_keys_not_positions() {
        let e = element(
            "<Thing name=\"T\" lastModifiedDate=\"x\"><Owner name=\"me\"/><ConfigurationTables>\
             <ConfigurationTable name=\"CT\"><Rows>\
             <Row><UID>2</UID><v>b</v></Row><Row><UID>1</UID><v>a</v></Row>\
             </Rows></ConfigurationTable></ConfigurationTables><x>1</x><x>2</x></Thing>",
        );
        let flat = flatten(&e);
        assert_eq!(flat.get("@name").map(String::as_str), Some("T"));
        assert!(!flat
            .keys()
            .any(|k| k.contains("lastModifiedDate") || k.contains("Owner")));
        assert_eq!(
            flat.get("/ConfigurationTables/ConfigurationTable[CT]/Rows/Row[1]/v#text")
                .map(String::as_str),
            Some("a")
        );
        assert_eq!(flat.get("/x[2]#text").map(String::as_str), Some("2"));
    }

    #[test]
    fn mashup_json_is_compared_leaf_by_leaf() {
        let e = element("<Mashup><mashupContent><![CDATA[{\"UI\": {\"Properties\": {\"Width\": 10, \"Id\": \"a\"}, \"Empty\": {}}}]]></mashupContent></Mashup>");
        let flat = flatten(&e);
        assert_eq!(
            flat.get("/mashupContent#json.UI.Properties.Width")
                .map(String::as_str),
            Some("10")
        );
        assert_eq!(
            flat.get("/mashupContent#json.UI.Properties.Id")
                .map(String::as_str),
            Some("\"a\"")
        );
        assert!(
            !flat.keys().any(|k| k.contains("Empty")),
            "an empty object contributes nothing"
        );
    }

    #[test]
    fn regenerated_ids_are_recognised_and_nothing_else() {
        assert!(is_volatile_id("/mashupContent#json.Events[3].Id"));
        assert!(is_volatile_id("/mashupContent#json.UI.DataBindings[12].Id"));
        assert!(!is_volatile_id("/mashupContent#json.Events[3].Name"));
        assert!(!is_volatile_id("/mashupContent#json.MyEvents[3].Id"));
        assert!(is_volatile_id(
            "/mashupContent#json.DataBindings[GetOrders>grid-1~0a1b2c3d].Id"
        ));
        assert!(!is_volatile_id("/mashupContent#json.Events[].Id"));
    }

    fn mashup(content: serde_json::Value) -> std::collections::BTreeMap<String, String> {
        let xml = format!(
            "<Mashup name=\"P.M\"><mashupContent><![CDATA[{content}]]></mashupContent></Mashup>"
        );
        flatten(&element(&xml))
    }

    fn differing(
        left: &std::collections::BTreeMap<String, String>,
        right: &std::collections::BTreeMap<String, String>,
    ) -> Vec<String> {
        left.keys()
            .chain(right.keys())
            .filter(|path| left.get(*path) != right.get(*path))
            .filter(|path| !is_volatile_id(path))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    #[test]
    fn bindings_and_events_are_a_set_known_by_what_they_connect() {
        let binding = |id: &str, target: &str| {
            serde_json::json!({"Id": id, "SourceArea": "Data", "SourceId": "GetOrders",
                "TargetArea": "UI", "TargetId": target,
                "PropertyMaps": [{"SourceProperty": "", "TargetProperty": "Data"}]})
        };
        let event = |id: &str, handler: &str| {
            serde_json::json!({"Id": id, "EventTriggerArea": "UI", "EventTriggerId": "button-1",
                "EventTriggerEvent": "Clicked", "EventHandlerArea": "Data",
                "EventHandlerId": "Things_P.T", "EventHandlerService": handler})
        };
        let before = mashup(serde_json::json!({
            "DataBindings": [binding("b1", "grid-1"), binding("b2", "grid-2")],
            "Events": [event("e1", "Save"), event("e2", "Load")],
        }));
        // Composer saved again: new ids, another order, nothing else.
        let resaved = mashup(serde_json::json!({
            "DataBindings": [binding("b9", "grid-2"), binding("b8", "grid-1")],
            "Events": [event("e9", "Load"), event("e8", "Save")],
        }));
        assert_eq!(differing(&before, &resaved), Vec::<String>::new());

        // One binding now feeds another widget: that binding, and only it, is out and in.
        let retargeted = mashup(serde_json::json!({
            "DataBindings": [binding("b1", "grid-1"), binding("b2", "grid-3")],
            "Events": [event("e1", "Save"), event("e2", "Load")],
        }));
        let changed = differing(&before, &retargeted);
        assert!(!changed.is_empty());
        assert!(
            changed
                .iter()
                .all(|path| path.contains("GetOrders>grid-2") || path.contains("GetOrders>grid-3")),
            "{changed:?}"
        );
    }

    #[test]
    fn widgets_are_known_by_id_and_their_order_is_one_change() {
        let widget = |id: &str, width: u32| serde_json::json!({"Properties": {"Id": id, "Width": width}, "Widgets": []});
        let ui = |widgets: Vec<serde_json::Value>| {
            mashup(serde_json::json!({"UI": {"Properties": {"Id": "root"}, "Widgets": widgets}}))
        };
        let before = ui(vec![widget("a", 1), widget("b", 2), widget("c", 3)]);

        let moved = ui(vec![widget("c", 3), widget("a", 1), widget("b", 2)]);
        assert_eq!(
            differing(&before, &moved),
            ["/mashupContent#json.UI.Widgets#order"]
        );

        let resized = ui(vec![widget("a", 1), widget("b", 5), widget("c", 3)]);
        assert_eq!(
            differing(&before, &resized),
            ["/mashupContent#json.UI.Widgets[b].Properties.Width"]
        );
    }

    #[test]
    fn permissions_are_known_by_resource_and_principals_by_type_and_name() {
        let thing = |permissions: &str| {
            let xml = format!("<Thing name=\"P.T\"><RunTimePermissions>{permissions}</RunTimePermissions></Thing>");
            flatten(&element(&xml))
        };
        let block = |resource: &str, principals: &str| {
            format!("<Permissions resourceName=\"{resource}\"><ServiceInvoke>{principals}</ServiceInvoke></Permissions>")
        };
        let group = "<Principal name=\"P.Users\" type=\"Group\" isPermitted=\"true\"/>";
        let organization =
            "<Principal name=\"P.Users\" type=\"Organization\" isPermitted=\"true\"/>";
        let before = thing(&format!(
            "{}{}",
            block("A", &format!("{group}{organization}")),
            block("B", group)
        ));

        let reordered = thing(&format!(
            "{}{}",
            block("B", group),
            block("A", &format!("{organization}{group}"))
        ));
        assert_eq!(differing(&before, &reordered), Vec::<String>::new());

        // A Group and an Organization of one name are two principals: dropping one is a change.
        let without_organization = thing(&format!("{}{}", block("A", group), block("B", group)));
        let changed = differing(&before, &without_organization);
        assert!(!changed.is_empty());
        assert!(
            changed
                .iter()
                .all(|path| path.contains("Organization:P.Users")),
            "{changed:?}"
        );
    }

    #[test]
    fn a_name_that_looks_numbered_never_hides_a_sibling() {
        let xml = "<Thing name=\"P.T\"><Tags><Tag name=\"x#2\" vocabulary=\"a\"/><Tag name=\"x\" vocabulary=\"b\"/><Tag name=\"x\" vocabulary=\"c\"/></Tags></Thing>";
        let flat = flatten(&element(xml));
        let vocabularies: std::collections::BTreeSet<_> = flat
            .iter()
            .filter(|(path, _)| path.ends_with("@vocabulary"))
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(vocabularies, ["a", "b", "c"].into());

        let widget = |id: Option<&str>, width: u32| match id {
            Some(id) => serde_json::json!({"Properties": {"Id": id, "Width": width}}),
            None => serde_json::json!({"Properties": {"Width": width}}),
        };
        let flat = mashup(serde_json::json!({"UI": {"Properties": {"Id": "root"},
            "Widgets": [widget(None, 1), widget(Some("#0"), 2)]}}));
        let widths = flat
            .iter()
            .filter(|(path, _)| path.ends_with(".Properties.Width"))
            .count();
        assert_eq!(widths, 2, "{flat:?}");
    }

    #[test]
    fn a_permissions_block_is_known_by_its_resource_even_with_a_name() {
        let xml = |first: &str, second: &str| {
            format!("<Thing name=\"P.T\"><RunTimePermissions><Permissions name=\"shared\" resourceName=\"{first}\"/><Permissions name=\"shared\" resourceName=\"{second}\"/></RunTimePermissions></Thing>")
        };
        assert_eq!(
            flatten(&element(&xml("A", "B"))),
            flatten(&element(&xml("B", "A")))
        );
    }

    #[test]
    fn two_siblings_with_one_name_are_both_kept() {
        let xml = "<Thing name=\"P.T\"><Tags><Tag name=\"x\" vocabulary=\"a\"/><Tag name=\"x\" vocabulary=\"b\"/></Tags></Thing>";
        let flat = flatten(&element(xml));
        assert_eq!(
            flat.get("/Tags/Tag[x]@vocabulary").map(String::as_str),
            Some("a")
        );
        assert_eq!(
            flat.get("/Tags/Tag[x#2]@vocabulary").map(String::as_str),
            Some("b")
        );
    }

    #[test]
    fn parameter_definitions_render_one_attribute_per_line_escaped() {
        let e = element(
            "<Mashup><ParameterDefinitions><FieldDefinition name=\"p\" description=\"a &lt;b&gt; &amp; &quot;c&quot;\"/>\
             </ParameterDefinitions></Mashup>",
        );
        assert_eq!(
            render_parameter_definitions(&e).unwrap(),
            "            <ParameterDefinitions>\n                <FieldDefinition\n                 \
             description=\"a &lt;b&gt; &amp; &quot;c&quot;\"\n                 name=\"p\"></FieldDefinition>\n            \
             </ParameterDefinitions>"
        );
        assert_eq!(
            render_parameter_definitions(&element("<Mashup/>")).unwrap(),
            "            <ParameterDefinitions></ParameterDefinitions>"
        );
    }

    fn locked(solution: &Solution) -> WorkspaceLock {
        crate::core::lock::acquire(&solution.root, "test", &[]).unwrap()
    }

    /// A one-project solution with one mashup to take the shape from, and an export of `mashups`
    /// (name, mashupContent) next to it.
    fn adopt_case(mashups: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, Solution, PathBuf) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-adopt-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("Mashups")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join("Mashups/P.A.xml"),
            "<Entities>\n    <Mashups>\n        <Mashup\n         aspect.isFlex=\"false\"\n         \
             name=\"P.A\"\n         projectName=\"P\">\n            <ParameterDefinitions>\
             </ParameterDefinitions>\n            <mashupContent><![CDATA[{\"UI\":{}}]]></mashupContent>\n\
             \x20       </Mashup>\n    </Mashups>\n</Entities>\n",
        )
        .unwrap();
        let mut export = String::from("<Entities><Mashups>");
        export.push_str("<Mashup name=\"P.A\" projectName=\"P\"><mashupContent><![CDATA[{\"UI\":{}}]]></mashupContent></Mashup>");
        for (name, content) in mashups {
            export.push_str(&format!(
                "<Mashup name=\"{}\" projectName=\"P\"><mashupContent><![CDATA[{content}]]></mashupContent></Mashup>",
                name.replace('&', "&amp;")
            ));
        }
        export.push_str("</Mashups></Entities>");
        let export_path = root.join("export.xml");
        std::fs::write(&export_path, export).unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution, export_path)
    }

    fn files_under(root: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path())
                } else {
                    found.push(entry.path())
                }
            }
        }
        found.sort();
        found
    }

    #[test]
    fn a_write_that_cannot_happen_leaves_none_of_the_others_behind() {
        let (_dir, root, solution, export) = adopt_case(&[("P.B", "{\"UI\":{\"x\":1}}")]);
        // A file where the new mashup's sidecar folder must go: its entity file is written
        // first, and used to stay when the sidecars then failed.
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/P.B"), "in the way").unwrap();
        let report = compare(&solution, &export, &[]).unwrap();
        let outside_twaco = |root: &Path| -> Vec<(PathBuf, Vec<u8>)> {
            files_under(root)
                .into_iter()
                .filter(|path| !path.starts_with(root.join(".twaco")))
                .map(|path| {
                    let bytes = std::fs::read(&path).unwrap();
                    (path, bytes)
                })
                .collect()
        };
        let before = outside_twaco(&root);
        let error = apply(&solution, &export, &report, &[], &locked(&solution))
            .err()
            .expect("the sidecar folder cannot be made");
        // Where it fails depends on the platform: Windows reads `src/P.B/mashup/content.json` as
        // missing, so the write is planned and the transaction undoes it; Unix says a file is in
        // the way (ENOTDIR) while planning. Either way nothing may be left behind.
        match &error {
            AdoptError::Write(_) => assert!(
                error.to_string().starts_with("nothing was adopted"),
                "{error}"
            ),
            AdoptError::Repository { .. } => {}
            other => panic!("unexpected {other}"),
        }
        assert_eq!(outside_twaco(&root), before, "every change was undone");
        assert!(!root.join("Mashups/P.B.xml").exists());
    }

    #[test]
    fn a_new_mashup_is_created_under_its_own_name_escaped_and_only_there() {
        let (_dir, root, solution, export) = adopt_case(&[("P.B&C", "{\"UI\":{\"x\":1}}")]);
        std::fs::create_dir_all(root.join(".twaco/types")).unwrap();
        let report = compare(&solution, &export, &[]).unwrap();
        let outcome = apply(&solution, &export, &report, &[], &locked(&solution)).unwrap();
        assert!(outcome.types.files_written.is_some());
        let lines: Vec<String> = outcome
            .lines
            .into_iter()
            .filter(|l| !l.contains("P.A"))
            .collect();
        assert_eq!(lines, ["created  Mashups/P.B&C.xml", "sidecar  P.B&C"]);
        let text = std::fs::read_to_string(root.join("Mashups/P.B&C.xml")).unwrap();
        assert!(text.contains("\n         name=\"P.B&amp;C\"\n"), "{text}");
        assert!(root.join("src/P.B&C/mashup/content.json").is_file());
    }

    #[test]
    fn with_no_base_an_entity_as_last_deployed_is_named_in_a_note() {
        let (_dir, root, solution, export) = adopt_case(&[]);
        // The export's P.A differs from ours, and no base says who changed it.
        let text = std::fs::read_to_string(&export).unwrap().replacen(
            "{\"UI\":{}}",
            "{\"UI\":{\"x\":2}}",
            1,
        );
        std::fs::write(&export, text).unwrap();
        let notes = |solution: &Solution| compare(solution, &export, &[]).unwrap().notes;
        assert_eq!(notes(&solution).len(), 1, "only the no-base note");

        let ours =
            crate::core::normalise::hash(&std::fs::read(root.join("Mashups/P.A.xml")).unwrap())
                .unwrap();
        let mut baseline = crate::core::baseline::Baseline::default();
        baseline.set("Mashups", "P.A", ours, "v8:server".into());
        baseline.write(&root, "default").unwrap();
        let notes = notes(&solution);
        assert!(
            notes.iter().any(|note| note.starts_with(
                "1 of the conflicting entities are as last deployed to, or recorded against"
            ) && note.contains("Mashups/P.A")),
            "{notes:?}"
        );
    }

    #[test]
    fn a_new_mashup_that_would_share_a_name_or_sidecars_with_another_is_refused() {
        // Another letter case of a mashup the repository has, and a name whose sidecar folder
        // is already there: creating either would overwrite another mashup's sidecars.
        for (name, clash) in [("p.a", "Mashups/P.A.xml"), ("P.B", "src/P.B/mashup")] {
            let (_dir, root, solution, export) = adopt_case(&[(name, "{\"UI\":{\"x\":1}}")]);
            std::fs::create_dir_all(root.join("src/P.B/mashup")).unwrap();
            std::fs::write(root.join("src/P.B/mashup/content.json"), "{\"UI\":{}}").unwrap();
            let lock = locked(&solution);
            let before = files_under(&root);
            let report = compare(&solution, &export, &[]).unwrap();
            let error = apply(&solution, &export, &report, &[], &lock)
                .err()
                .expect("refused");
            let text = error.to_string().replace('\\', "/");
            assert!(
                text.contains(&format!("new mashup {name} would share {clash}")),
                "{text}"
            );
            assert_eq!(files_under(&root), before, "nothing was adopted");
        }
    }

    #[test]
    fn an_export_that_cannot_be_adopted_whole_changes_nothing() {
        for bad in [
            ("P.C", "not json"),
            ("../escape", "{\"UI\":{}}"),
            ("CON", "{\"UI\":{}}"),
        ] {
            let (_dir, root, solution, export) = adopt_case(&[("P.B", "{\"UI\":{\"x\":1}}"), bad]);
            let lock = locked(&solution);
            let before = files_under(&root);
            let report = compare(&solution, &export, &[]).unwrap();
            assert!(
                apply(&solution, &export, &report, &[], &lock).is_err(),
                "{bad:?}"
            );
            assert_eq!(files_under(&root), before, "{bad:?} wrote something");
            assert!(!root.parent().unwrap().join("escape").exists());
        }
    }

    #[test]
    fn a_new_mashup_without_content_is_skipped_not_given_another_ones() {
        let (_dir, root, solution, export) = adopt_case(&[("P.D", "  ")]);
        let report = compare(&solution, &export, &[]).unwrap();
        let lines: Vec<String> = apply(&solution, &export, &report, &[], &locked(&solution))
            .unwrap()
            .lines
            .into_iter()
            .filter(|l| !l.contains("P.A"))
            .collect();
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with("skipped  Mashups/P.D: the export holds no mashup content"),
            "{lines:?}"
        );
        assert!(!root.join("Mashups/P.D.xml").exists());
    }

    #[test]
    fn lines_come_in_the_reports_order() {
        let (_dir, _, solution, export) =
            adopt_case(&[("P.B", "{\"UI\":{\"x\":1}}"), ("P.C", " ")]);
        let report = compare(&solution, &export, &[]).unwrap();
        let lines: Vec<String> = apply(&solution, &export, &report, &[], &locked(&solution))
            .unwrap()
            .lines
            .into_iter()
            .filter(|l| !l.contains("P.A"))
            .collect();
        let kinds: Vec<&str> = lines
            .iter()
            .map(|l| l.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(kinds, ["created", "sidecar", "skipped"], "{lines:?}");
    }

    #[test]
    fn names_and_images_are_checked_before_use() {
        for good in ["P.Mashup_1", "Acme.Dash-Board.X", "P.B&C", "Console"] {
            assert!(is_file_name(good), "{good}");
        }
        for bad in [
            "", ".", "..", "../x", "a/b", "a\\b", "C:x", "x.", "x ", "NUL", "com1.txt", "a\u{1}b",
        ] {
            assert!(!is_file_name(bad), "{bad:?}");
        }
        assert!(is_base64("QUJD") && is_base64("QUI=") && is_base64("QQ=="));
        assert!(!is_base64("QUJ") && !is_base64("Q===") && !is_base64("QU!D"));
    }

    #[test]
    fn media_content_is_rewrapped_and_an_unchanged_file_is_not_written() {
        let dir_guard = tempfile::Builder::new()
            .prefix("twaco-media-")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("M.xml");
        std::fs::write(&path, "<a>\n        <content>\n            <![CDATA[\n            old\n            ]]>\n        </content>\n</a>").unwrap();
        let encoded = "A".repeat(80);
        let text = media_content(&path, &encoded)
            .unwrap()
            .expect("new content changes the file");
        let expected_body = format!("            {}\n            {}", "A".repeat(76), "AAAA");
        assert!(
            text.contains(&format!(
                "<content>\n            <![CDATA[\n{expected_body}\n            ]]>"
            )),
            "{text}"
        );
        std::fs::write(&path, &text).unwrap();
        assert_eq!(
            media_content(&path, &encoded).unwrap(),
            None,
            "the same content is not rewritten"
        );
    }

    #[test]
    fn a_service_body_is_the_longest_text_that_is_not_editor_state() {
        let e = element(
            "<Thing><ThingShape><ServiceImplementations><ServiceImplementation name=\"S\"><ConfigurationTables>\
             <ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[result = 1;]]></code>\
             <editor><![CDATA[{\"viewState\": {\"cursor\": \"far longer than the script itself\"}}]]></editor>\
             </Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation>\
             </ServiceImplementations></ThingShape></Thing>",
        );
        let bodies = service_bodies(&e);
        assert_eq!(bodies.get("S").map(String::as_str), Some("result = 1;"));
    }
}
