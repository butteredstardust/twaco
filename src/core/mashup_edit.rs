//! Editing a mashup's content document: the `content.json` a mashup extracts to.
//!
//! The document stays a `serde_json::Value` and only the parts an edit touches are changed, so
//! everything twaco does not model survives exactly. An export writes every object's keys in
//! ASCII order, so a new object is built sorted and a new key goes where sorting puts it (unless
//! the object was not sorted to begin with): a mashup exported again after a deploy then comes
//! back as twaco wrote it.
//!
//! Widgets are a tree under `UI`: each has `Properties` (with `Type`, `Id`, `DisplayName`) and
//! may have `Widgets`. A responsive container (`flexcontainer`) holds either containers or other
//! widgets, never both. New ids are `<type>-<n>`, `n` one more than the root's `id_index` and
//! every id already used, and the root's `id_index` moves on to `n`.

use crate::core::codes::{Coded, ErrorCode};
use crate::core::widgets::{self, Widget};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// The root widget's id in every mashup Composer makes.
pub const ROOT_ID: &str = "mashup-root";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EditError {
    /// The document is not a mashup twaco can safely edit.
    #[error("{0}")]
    Invalid(String),
    #[error("no widget in the mashup is {0}")]
    NoWidget(String),
    #[error("{word} could be any of {}; name one by its id", .ids.join(", "))]
    Ambiguous { word: String, ids: Vec<String> },
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Exists(String),
}

impl Coded for EditError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidData,
            Self::NoWidget(_) => ErrorCode::UnknownEntity,
            Self::Ambiguous { .. } => ErrorCode::Ambiguous,
            Self::Refused(_) => ErrorCode::GuardRefused,
            Self::Exists(_) => ErrorCode::AlreadyExists,
        }
    }
}

/// Where a widget is: the indexes of `Widgets` arrays from `UI` down. The root is `[]`.
pub type WidgetPath = Vec<usize>;

/// One widget as the editor sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetRef {
    pub id: String,
    pub type_id: String,
    pub display_name: String,
    pub path: WidgetPath,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The widget it is about, if it is about one.
    pub widget: Option<String>,
    pub message: String,
}

/// A mashup content document that has passed the checks an edit relies on.
#[derive(Clone, Debug, PartialEq)]
pub struct MashupDocument {
    value: Value,
}

/// Parse and check a `content.json`.
pub fn parse(content: &str) -> Result<MashupDocument, EditError> {
    let value: Value = serde_json::from_str(content)
        .map_err(|e| EditError::Invalid(format!("content.json will not parse: {e}")))?;
    MashupDocument::new(value)
}

impl MashupDocument {
    pub fn new(value: Value) -> Result<Self, EditError> {
        let document = Self { value };
        document.check()?;
        Ok(document)
    }

    pub fn value(&self) -> &Value {
        &self.value
    }

    /// The `content.json` text, formatted as extraction writes it.
    pub fn to_content(&self) -> String {
        crate::core::mashup::to_sidecar(&self.value)
    }

    fn check(&self) -> Result<(), EditError> {
        let root = self
            .value
            .as_object()
            .ok_or_else(|| EditError::Invalid("the mashup content is not a JSON object".into()))?;
        let ui = root
            .get("UI")
            .ok_or_else(|| EditError::Invalid("the mashup content has no UI".into()))?;
        let mut seen = BTreeSet::new();
        check_widget(ui, "UI", &mut seen)?;
        let properties = &ui["Properties"];
        if properties.get("Id").and_then(Value::as_str) != Some(ROOT_ID) {
            return Err(EditError::Invalid(format!(
                "the root widget's Id is not {ROOT_ID}"
            )));
        }
        if let Some(index) = properties.get("id_index") {
            if !index.is_u64() {
                return Err(EditError::Invalid(format!(
                    "the root's id_index is {index}, not a whole number"
                )));
            }
        }
        for section in ["DataBindings", "Events"] {
            if root.get(section).is_some_and(|v| !v.is_array()) {
                return Err(EditError::Invalid(format!("{section} is not a list")));
            }
        }
        if root.get("Data").is_some_and(|v| !v.is_object()) {
            return Err(EditError::Invalid("Data is not an object".into()));
        }
        Ok(())
    }

    /// Every widget, root first, depth first in document order.
    pub fn widgets(&self) -> Vec<WidgetRef> {
        let mut out = Vec::new();
        collect(&self.value["UI"], &mut Vec::new(), &mut out);
        out
    }

    /// The widget a person names: an exact `Id`, else a unique `DisplayName`, else the only
    /// widget of a type the catalogue resolves the word to.
    pub fn find(&self, word: &str) -> Result<WidgetRef, EditError> {
        let all = self.widgets();
        if let Some(found) = all.iter().find(|w| w.id == word) {
            return Ok(found.clone());
        }
        let one = |matches: Vec<&WidgetRef>| -> Option<Result<WidgetRef, EditError>> {
            match matches.as_slice() {
                [] => None,
                [only] => Some(Ok((*only).clone())),
                many => Some(Err(EditError::Ambiguous {
                    word: word.to_string(),
                    ids: many.iter().map(|w| w.id.clone()).collect(),
                })),
            }
        };
        if let Some(found) = one(all.iter().filter(|w| w.display_name == word).collect()) {
            return found;
        }
        if let Ok(widget) = widgets::catalog().resolve(word) {
            if let Some(found) = one(all.iter().filter(|w| w.type_id == widget.type_id).collect()) {
                return found;
            }
        }
        Err(EditError::NoWidget(word.to_string()))
    }

    /// The widget object at `path`.
    pub fn at(&self, path: &[usize]) -> &Value {
        let mut node = &self.value["UI"];
        for &index in path {
            node = &node["Widgets"][index];
        }
        node
    }

    /// The next free number for a new widget id, which the root's `id_index` then records.
    pub fn allocate(&mut self) -> u64 {
        let used = self
            .widgets()
            .iter()
            .filter_map(|w| number_of(&w.id))
            .max()
            .unwrap_or(0);
        let index = self.value["UI"]["Properties"]
            .get("id_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let next = used.max(index) + 1;
        let properties = self.value["UI"]["Properties"]
            .as_object_mut()
            .expect("checked: the root has Properties");
        insert_sorted(properties, "id_index", Value::from(next));
        next
    }

    /// Structural problems: duplicate ids are refused by `parse`, so these are what a document
    /// can still get wrong, such as a responsive container holding both containers and widgets.
    pub fn problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        for widget in self.widgets() {
            if widget.type_id != "flexcontainer" {
                continue;
            }
            let children = children(self.at(&widget.path));
            let containers = children
                .iter()
                .filter(|child| type_of(child) == Some("flexcontainer"))
                .count();
            if containers > 0 && containers < children.len() {
                out.push(Problem {
                    widget: Some(widget.id.clone()),
                    message: format!(
                        "{} holds both containers and widgets; a responsive container holds one or the other",
                        widget.id
                    ),
                });
            }
        }
        out
    }
}

/// A widget as Composer writes it when the widget is dropped: the 10.2 defaults, what Composer
/// adds on every drop, then its identity. Keys are sorted, as an export writes them.
pub fn new_widget(widget: &Widget, number: u64) -> Value {
    let catalog = widgets::catalog();
    let mut properties: Map<String, Value> = widget.defaults.clone();
    let dropped = if widget.type_id == "flexcontainer" {
        &catalog.dropped.flexcontainer
    } else {
        &catalog.dropped.leaf
    };
    for (key, value) in dropped {
        properties.insert(key.clone(), value.clone());
    }
    let id = format!("{}-{number}", widget.type_id);
    let display = format!("{}-{number}", widget.display_name);
    properties.insert("DisplayName".into(), Value::from(display));
    properties.insert("Id".into(), Value::from(id));
    properties.insert("LastContainer".into(), Value::from(false));
    properties.insert("Type".into(), Value::from(widget.type_id.clone()));
    properties.insert(
        "__TypeDisplayName".into(),
        Value::from(widget.type_display_name.clone()),
    );
    let mut out = Map::new();
    out.insert("Properties".into(), Value::Object(sorted(properties)));
    out.insert("Widgets".into(), Value::Array(Vec::new()));
    Value::Object(out)
}

/// Insert `key`, keeping a sorted object sorted; an object that was not sorted gets it at the end.
pub fn insert_sorted(object: &mut Map<String, Value>, key: &str, value: Value) {
    if object.contains_key(key) {
        object.insert(key.to_string(), value);
        return;
    }
    let keys: Vec<&String> = object.keys().collect();
    let was_sorted = keys.windows(2).all(|pair| pair[0] < pair[1]);
    if !was_sorted {
        object.insert(key.to_string(), value);
        return;
    }
    let at = keys.partition_point(|existing| existing.as_str() < key);
    let mut rebuilt = Map::new();
    let mut pending = Some(value);
    for (index, (existing, child)) in std::mem::take(object).into_iter().enumerate() {
        if index == at {
            rebuilt.insert(key.to_string(), pending.take().expect("inserted once"));
        }
        rebuilt.insert(existing, child);
    }
    if let Some(value) = pending {
        rebuilt.insert(key.to_string(), value);
    }
    *object = rebuilt;
}

fn sorted(object: Map<String, Value>) -> Map<String, Value> {
    let mut entries: Vec<(String, Value)> = object.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.into_iter().collect()
}

fn check_widget(widget: &Value, at: &str, seen: &mut BTreeSet<String>) -> Result<(), EditError> {
    let properties = widget
        .get("Properties")
        .and_then(Value::as_object)
        .ok_or_else(|| EditError::Invalid(format!("{at} has no Properties object")))?;
    let id = properties
        .get("Id")
        .and_then(Value::as_str)
        .ok_or_else(|| EditError::Invalid(format!("a widget under {at} has no Id")))?;
    if properties.get("Type").and_then(Value::as_str).is_none() {
        return Err(EditError::Invalid(format!("widget {id} has no Type")));
    }
    if !seen.insert(id.to_string()) {
        return Err(EditError::Invalid(format!(
            "two widgets have the Id {id}; Composer would not open this mashup"
        )));
    }
    match widget.get("Widgets") {
        None => Ok(()),
        Some(Value::Array(children)) => children
            .iter()
            .try_for_each(|child| check_widget(child, id, seen)),
        Some(_) => Err(EditError::Invalid(format!(
            "widget {id} has Widgets that are not a list"
        ))),
    }
}

fn collect(widget: &Value, path: &mut WidgetPath, out: &mut Vec<WidgetRef>) {
    let properties = &widget["Properties"];
    let text = |key: &str| properties[key].as_str().unwrap_or_default().to_string();
    out.push(WidgetRef {
        id: text("Id"),
        type_id: text("Type"),
        display_name: text("DisplayName"),
        path: path.clone(),
    });
    for (index, child) in children(widget).iter().enumerate() {
        path.push(index);
        collect(child, path, out);
        path.pop();
    }
}

pub(crate) fn children(widget: &Value) -> &[Value] {
    widget
        .get("Widgets")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

pub(crate) fn type_of(widget: &Value) -> Option<&str> {
    widget["Properties"]["Type"].as_str()
}

/// The `<n>` of an id that ends in `-<n>`.
fn number_of(id: &str) -> Option<u64> {
    id.rsplit_once('-')?.1.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLANK: &str = include_str!("mashup_templates/blank.content.json");

    fn document(widgets: &str, id_index: u64) -> MashupDocument {
        parse(&format!(
            r#"{{"UI":{{"Properties":{{"Id":"mashup-root","Type":"mashup","id_index":{id_index}}},"Widgets":[{widgets}]}},"Data":{{}},"DataBindings":[],"Events":[]}}"#
        ))
        .unwrap()
    }

    fn widget(type_id: &str, id: &str, display: &str, children: &str) -> String {
        format!(
            r#"{{"Properties":{{"DisplayName":"{display}","Id":"{id}","Type":"{type_id}"}},"Widgets":[{children}]}}"#
        )
    }

    #[test]
    fn the_blank_template_is_an_editable_mashup_with_a_center_container() {
        let document = parse(BLANK).unwrap();
        let center = document.find("center").unwrap();
        assert_eq!(
            (center.type_id.as_str(), center.id.as_str()),
            ("flexcontainer", "flexcontainer-2")
        );
        assert!(document.problems().is_empty());
        // Round trip: what the editor writes is what extraction would write.
        let assets = crate::core::mashup::assets_from_payload(BLANK).unwrap();
        assert_eq!(document.to_content(), assets.content);
    }

    #[test]
    fn the_root_must_be_composers_and_ids_must_be_unique() {
        let not_root = parse(r#"{"UI":{"Properties":{"Id":"x","Type":"mashup"}}}"#);
        assert!(matches!(not_root, Err(EditError::Invalid(m)) if m.contains("mashup-root")));
        let twice = parse(&format!(
            r#"{{"UI":{{"Properties":{{"Id":"mashup-root","Type":"mashup"}},"Widgets":[{},{}]}}}}"#,
            widget("ptcslabel", "ptcslabel-3", "a", ""),
            widget("ptcslabel", "ptcslabel-3", "b", "")
        ));
        let error = twice.unwrap_err();
        assert!(error.to_string().contains("ptcslabel-3"), "{error}");
        assert_eq!(error.code(), ErrorCode::InvalidData);
        let bad_index =
            parse(r#"{"UI":{"Properties":{"Id":"mashup-root","Type":"mashup","id_index":"7"}}}"#);
        assert!(bad_index.is_err());
    }

    #[test]
    fn a_widget_is_found_by_id_display_name_or_the_only_one_of_its_type() {
        let document = document(
            &[
                widget("ptcsgrid", "ptcsgrid-4", "orders", ""),
                widget("ptcslabel", "ptcslabel-5", "ptcs-label-5", ""),
                widget("ptcslabel", "ptcslabel-6", "ptcs-label-6", ""),
            ]
            .join(","),
            6,
        );
        assert_eq!(document.find("ptcsgrid-4").unwrap().id, "ptcsgrid-4");
        assert_eq!(document.find("orders").unwrap().id, "ptcsgrid-4");
        assert_eq!(document.find("grid").unwrap().id, "ptcsgrid-4");
        assert_eq!(
            document.find("label").unwrap_err(),
            EditError::Ambiguous {
                word: "label".into(),
                ids: vec!["ptcslabel-5".into(), "ptcslabel-6".into()]
            }
        );
        assert_eq!(
            document.find("chart").unwrap_err(),
            EditError::NoWidget("chart".into())
        );
    }

    #[test]
    fn a_new_id_is_past_every_used_one_and_the_index() {
        let mut labelled = document(&widget("ptcslabel", "ptcslabel-9", "x", ""), 4);
        assert_eq!(labelled.allocate(), 10);
        assert_eq!(labelled.value()["UI"]["Properties"]["id_index"], 10);
        // A deleted widget's number is never handed out again: the index remembers it.
        assert_eq!(document("", 12).allocate(), 13);
    }

    #[test]
    fn a_new_widget_is_written_as_composer_drops_it_with_sorted_keys() {
        let button = widgets::catalog().by_type("ptcsbutton").unwrap();
        let made = new_widget(button, 7);
        let properties = made["Properties"].as_object().unwrap();
        assert_eq!(properties["Id"], "ptcsbutton-7");
        assert_eq!(properties["DisplayName"], "ptcs-button-7");
        assert_eq!(properties["__TypeDisplayName"], "Button");
        assert_eq!(properties["Label"], "[[Button]]");
        assert_eq!(properties["LastContainer"], false);
        assert_eq!(properties["UseTheme"], true);
        let keys: Vec<&String> = properties.keys().collect();
        assert!(keys.windows(2).all(|p| p[0] < p[1]), "{keys:?}");
        let container = new_widget(widgets::catalog().by_type("flexcontainer").unwrap(), 8);
        assert_eq!(container["Properties"]["DisplayName"], "container-8");
        assert_eq!(
            container["Properties"]["iconClass"],
            "widgets-flexcontainer"
        );
    }

    #[test]
    fn a_key_goes_where_sorting_puts_it_unless_the_object_was_unsorted() {
        let mut object: Map<String, Value> =
            serde_json::from_str(r#"{"A":1,"C":3,"b":4}"#).unwrap();
        insert_sorted(&mut object, "B", Value::from(2));
        assert_eq!(object.keys().collect::<Vec<_>>(), ["A", "B", "C", "b"]);
        insert_sorted(&mut object, "z", Value::from(5));
        assert_eq!(object.keys().next_back().unwrap(), "z");
        let mut unsorted: Map<String, Value> = serde_json::from_str(r#"{"Z":1,"A":2}"#).unwrap();
        insert_sorted(&mut unsorted, "M", Value::from(3));
        assert_eq!(unsorted.keys().collect::<Vec<_>>(), ["Z", "A", "M"]);
    }

    #[test]
    fn a_container_holding_both_containers_and_widgets_is_a_problem() {
        let mixed = widget(
            "flexcontainer",
            "flexcontainer-2",
            "center",
            &[
                widget("flexcontainer", "flexcontainer-3", "container-3", ""),
                widget("ptcslabel", "ptcslabel-4", "ptcs-label-4", ""),
            ]
            .join(","),
        );
        let problems = document(&mixed, 4).problems();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].widget.as_deref(), Some("flexcontainer-2"));
    }
}
