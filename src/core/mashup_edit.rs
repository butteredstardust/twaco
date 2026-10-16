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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Above,
    Below,
}

impl Side {
    pub fn axis(self) -> &'static str {
        match self {
            Self::Left | Self::Right => "row",
            Self::Above | Self::Below => "column",
        }
    }

    fn before(self) -> bool {
        matches!(self, Self::Left | Self::Above)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placement {
    In(String),
    Beside { anchor: String, side: Side },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Placed {
    /// The id of the new widget.
    pub id: String,
    /// The containers made to hold widgets during the placement, in creation order.
    pub containers: Vec<String>,
    /// A container whose direction changed to make a sibling insertion possible.
    pub flipped: Option<String>,
    /// Existing leaf widgets that moved into wrappers.
    pub wrapped: Vec<String>,
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

    /// The editable root.  Callers must retain Composer's ordering conventions with
    /// [`insert_sorted`].
    pub(crate) fn value_mut(&mut self) -> &mut Value {
        &mut self.value
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
    /// widget of a type the catalogue resolves the word to, else the only widget whose type a
    /// catalogue search for the word finds ("the chart").
    pub fn find(&self, word: &str) -> Result<WidgetRef, EditError> {
        self.find_widget(word, true)
    }

    /// As [`Self::find`] without the catalogue search, for wiring, where a loose guess could
    /// bind the wrong widget.
    pub fn find_exact(&self, word: &str) -> Result<WidgetRef, EditError> {
        self.find_widget(word, false)
    }

    fn find_widget(&self, word: &str, search: bool) -> Result<WidgetRef, EditError> {
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
        // "the chart": the only widget whose type, alias or label has the word in it.
        let kinds: Vec<&str> = widgets::catalog()
            .search(word)
            .into_iter()
            .map(|widget| widget.type_id.as_str())
            .collect();
        if search && !word.trim().is_empty() {
            if let Some(found) = one(all
                .iter()
                .filter(|w| kinds.contains(&w.type_id.as_str()))
                .collect())
            {
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

    /// The mutable widget object at `path`.
    pub(crate) fn at_mut(&mut self, path: &[usize]) -> &mut Value {
        let mut node = &mut self.value["UI"];
        for &index in path {
            node = &mut node["Widgets"][index];
        }
        node
    }

    /// Append `widget` as the last child of the widget at `parent`.
    pub fn push_child(&mut self, parent: &[usize], widget: Value) {
        self.insert_child(parent, children(self.at(parent)).len(), widget);
    }

    fn insert_child(&mut self, parent: &[usize], index: usize, widget: Value) {
        let object = self
            .at_mut(parent)
            .as_object_mut()
            .expect("checked: a widget is an object");
        if !object.contains_key("Widgets") {
            insert_sorted(object, "Widgets", Value::Array(Vec::new()));
        }
        object["Widgets"]
            .as_array_mut()
            .expect("checked: Widgets is a list")
            .insert(index, widget);
    }

    fn replace_child(&mut self, parent: &[usize], index: usize, widget: Value) {
        self.at_mut(parent)["Widgets"]
            .as_array_mut()
            .expect("checked: Widgets is a list")[index] = widget;
    }

    /// Set one widget property, preserving Composer's key ordering convention.
    pub fn set_property(&mut self, path: &[usize], key: &str, value: Value) {
        let properties = self.at_mut(path)["Properties"]
            .as_object_mut()
            .expect("checked: a widget has Properties");
        insert_sorted(properties, key, value);
    }

    /// The root widget's properties.
    pub fn root_properties_mut(&mut self) -> &mut Map<String, Value> {
        self.value["UI"]["Properties"]
            .as_object_mut()
            .expect("checked: the root has Properties")
    }

    /// Set a top-level key of the document, such as `mashupType`.
    pub fn set_top(&mut self, key: &str, value: Value) {
        let object = self.value.as_object_mut().expect("checked: an object");
        insert_sorted(object, key, value);
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

    /// Put a new widget in a responsive container or beside an existing widget.
    pub fn place(&mut self, widget: &Widget, placement: &Placement) -> Result<Placed, EditError> {
        match placement {
            Placement::In(target) => self.place_in(widget, target),
            Placement::Beside { anchor, side } => self.place_beside(widget, anchor, *side),
        }
    }

    fn place_in(&mut self, widget: &Widget, target: &str) -> Result<Placed, EditError> {
        let target = self.find(target)?;
        if target.type_id != "flexcontainer" {
            return Err(EditError::Refused(format!(
                "{} is a {}, not a responsive container; place the widget beside it instead",
                target.id, target.type_id
            )));
        }
        let target_children = children(self.at(&target.path));
        let holds_containers = target_children
            .iter()
            .any(|child| type_of(child) == Some("flexcontainer"));
        let holds_leaves = target_children
            .iter()
            .any(|child| type_of(child) != Some("flexcontainer"));
        if holds_leaves && widget.type_id == "flexcontainer" {
            return Err(EditError::Refused(format!(
                "{} holds widgets, so it cannot also hold a container",
                target.id
            )));
        }
        let target_empty = target_children.is_empty();
        if target_children.len() == 1 && fills(&target_children[0]) {
            let child_id = widget_id(&target_children[0]);
            return Err(EditError::Refused(format!(
                "{child_id} fills {}; place the widget beside it (--right-of, --below, ...)",
                target.id
            )));
        }

        let number = self.allocate();
        let id = format!("{}-{number}", widget.type_id);
        let wrap_new = holds_containers && widget.type_id != "flexcontainer";
        let new = new_widget(widget, number, target_empty || wrap_new);
        let mut placed = Placed {
            id,
            ..Placed::default()
        };
        if widget.type_id == "flexcontainer" {
            placed.containers.push(placed.id.clone());
        }
        if wrap_new {
            let wrapper = self.wrap(new, &mut placed);
            self.push_child(&target.path, wrapper);
        } else {
            self.push_child(&target.path, new);
        }
        Ok(placed)
    }

    fn place_beside(
        &mut self,
        widget: &Widget,
        anchor_word: &str,
        side: Side,
    ) -> Result<Placed, EditError> {
        let anchor = self.find(anchor_word)?;
        if anchor.path.is_empty() {
            return Err(EditError::Refused(format!(
                "{} is the page's top container; place the widget inside it",
                anchor.id
            )));
        }
        let mut parent_path = anchor.path.clone();
        let anchor_index = parent_path.pop().expect("checked: anchor is not root");
        if parent_path.is_empty() {
            return Err(EditError::Refused(format!(
                "{} is the page's top container; place the widget inside it",
                anchor.id
            )));
        }
        let parent = self.at(&parent_path);
        let parent_type = type_of(parent).unwrap_or_default().to_string();
        if parent_type != "flexcontainer" {
            return Err(EditError::Refused(format!(
                "{} is inside a {}, which twaco does not lay out",
                anchor.id, parent_type
            )));
        }
        let parent_id = widget_id(parent).to_string();
        let parent_children = children(parent);
        let direction = parent["Properties"]["flex-direction"]
            .as_str()
            .map(str::to_string);
        let sibling_possible = !fills(self.at(&anchor.path))
            && (direction.as_deref() == Some(side.axis()) || parent_children.len() == 1);
        let anchor_is_container = anchor.type_id == "flexcontainer";
        let new_is_container = widget.type_id == "flexcontainer";

        if sibling_possible && (anchor_is_container || !new_is_container) {
            let number = self.allocate();
            let id = format!("{}-{number}", widget.type_id);
            let wrap_new = anchor_is_container && !new_is_container;
            let new = new_widget(widget, number, wrap_new);
            let mut placed = Placed {
                id,
                ..Placed::default()
            };
            if new_is_container {
                placed.containers.push(placed.id.clone());
            }
            if direction.as_deref() != Some(side.axis()) {
                self.set_property(&parent_path, "flex-direction", Value::from(side.axis()));
                placed.flipped = Some(parent_id);
            }
            let inserted = if wrap_new {
                self.wrap(new, &mut placed)
            } else {
                new
            };
            let index = if side.before() {
                anchor_index
            } else {
                anchor_index + 1
            };
            self.insert_child(&parent_path, index, inserted);
            return Ok(placed);
        }

        Ok(self.split_beside(widget, &anchor, &parent_path, anchor_index, side))
    }

    fn split_beside(
        &mut self,
        widget: &Widget,
        anchor: &WidgetRef,
        parent_path: &[usize],
        anchor_index: usize,
        side: Side,
    ) -> Placed {
        let number = self.allocate();
        let id = format!("{}-{number}", widget.type_id);
        let anchor_value = self.at(&anchor.path).clone();
        let anchor_is_container = anchor.type_id == "flexcontainer";
        let new_is_container = widget.type_id == "flexcontainer";
        let new = new_widget(widget, number, !new_is_container);
        let mut placed = Placed {
            id,
            ..Placed::default()
        };
        if new_is_container {
            placed.containers.push(placed.id.clone());
        }

        // The anchor alone in its container (a widget that fills it): that container becomes the
        // row or column itself, holding the anchor and the new widget each in a container of
        // their own, rather than gaining one more level that holds a single container.
        if children(self.at(parent_path)).len() == 1 {
            let parent_id = widget_id(self.at(parent_path)).to_string();
            if self.at(parent_path)["Properties"]["flex-direction"].as_str() != Some(side.axis()) {
                self.set_property(parent_path, "flex-direction", Value::from(side.axis()));
                placed.flipped = Some(parent_id);
            }
            let anchor_child = if anchor_is_container {
                anchor_value
            } else {
                placed.wrapped.push(anchor.id.clone());
                self.wrap(anchor_value, &mut placed)
            };
            let new_child = if new_is_container {
                new
            } else {
                self.wrap(new, &mut placed)
            };
            let (first, second) = if side.before() {
                (new_child, anchor_child)
            } else {
                (anchor_child, new_child)
            };
            self.replace_child(parent_path, 0, first);
            self.insert_child(parent_path, 1, second);
            return placed;
        }

        let wrapper_number = self.allocate();
        let mut wrapper = new_widget(
            widgets::catalog()
                .by_type("flexcontainer")
                .expect("the catalogue has flexcontainer"),
            wrapper_number,
            true,
        );
        let wrapper_id = widget_id(&wrapper).to_string();
        placed.containers.push(wrapper_id);
        set_value_property(&mut wrapper, "flex-direction", Value::from(side.axis()));

        let anchor_child = if anchor_is_container {
            anchor_value
        } else {
            placed.wrapped.push(anchor.id.clone());
            self.wrap(anchor_value, &mut placed)
        };
        let new_child = if new_is_container {
            new
        } else {
            self.wrap(new, &mut placed)
        };
        if side.before() {
            push_value_child(&mut wrapper, new_child);
            push_value_child(&mut wrapper, anchor_child);
        } else {
            push_value_child(&mut wrapper, anchor_child);
            push_value_child(&mut wrapper, new_child);
        }

        if !anchor_is_container {
            let old_children = children(self.at(parent_path)).to_vec();
            for (index, child) in old_children.into_iter().enumerate() {
                if index != anchor_index && type_of(&child) != Some("flexcontainer") {
                    placed.wrapped.push(widget_id(&child).to_string());
                    let wrapped = self.wrap(child, &mut placed);
                    self.replace_child(parent_path, index, wrapped);
                }
            }
        }
        self.replace_child(parent_path, anchor_index, wrapper);
        placed
    }

    fn wrap(&mut self, child: Value, placed: &mut Placed) -> Value {
        let number = self.allocate();
        let mut wrapper = new_widget(
            widgets::catalog()
                .by_type("flexcontainer")
                .expect("the catalogue has flexcontainer"),
            number,
            true,
        );
        placed.containers.push(widget_id(&wrapper).to_string());
        push_value_child(&mut wrapper, child);
        wrapper
    }
}

/// A widget as Composer writes it when the widget is dropped: the 10.2 defaults, what Composer
/// adds on every drop, then its identity. Keys are sorted, as an export writes them.
///
/// `alone` says whether it will be the only widget in its container: Composer makes a widget
/// that can resize fill a container it is alone in (`ResponsiveLayout`), and gives every other
/// widget a fixed size.
pub fn new_widget(widget: &Widget, number: u64, alone: bool) -> Value {
    let catalog = widgets::catalog();
    let mut properties: Map<String, Value> = widget.defaults.clone();
    let dropped = if widget.type_id == "flexcontainer" {
        Some(&catalog.dropped.flexcontainer)
    } else if widget.has_contract() {
        Some(&catalog.dropped.ptcs)
    } else {
        None
    };
    for (key, value) in dropped.into_iter().flatten() {
        properties.insert(key.clone(), value.clone());
    }
    if widget.type_id != "flexcontainer" {
        properties.insert(
            "ResponsiveLayout".into(),
            Value::from(widget.auto_resize && alone),
        );
    }
    let display = if widget.numbered {
        format!("{}-{number}", widget.display_name)
    } else {
        widget.display_name.clone()
    };
    properties.insert("DisplayName".into(), Value::from(display));
    properties.insert(
        "Id".into(),
        Value::from(format!("{}-{number}", widget.type_id)),
    );
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

fn widget_id(widget: &Value) -> &str {
    widget["Properties"]["Id"]
        .as_str()
        .expect("checked: a widget has an Id")
}

fn fills(widget: &Value) -> bool {
    type_of(widget) != Some("flexcontainer")
        && widget["Properties"]["ResponsiveLayout"] == Value::Bool(true)
}

fn push_value_child(parent: &mut Value, child: Value) {
    let object = parent
        .as_object_mut()
        .expect("checked: a widget is an object");
    if !object.contains_key("Widgets") {
        insert_sorted(object, "Widgets", Value::Array(Vec::new()));
    }
    object["Widgets"]
        .as_array_mut()
        .expect("checked: Widgets is a list")
        .push(child);
}

fn set_value_property(widget: &mut Value, key: &str, value: Value) {
    let properties = widget["Properties"]
        .as_object_mut()
        .expect("checked: a widget has Properties");
    insert_sorted(properties, key, value);
}

/// The number an id ends in: `3` for `ptcslabel-3`, `7` for `legacy7`.
fn number_of(id: &str) -> Option<u64> {
    let digits = id.len() - id.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    id[id.len() - digits..].parse().ok()
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

    fn container(id: &str, display: &str, direction: &str, children: &str) -> String {
        format!(
            r#"{{"Properties":{{"DisplayName":"{display}","Id":"{id}","Type":"flexcontainer","flex-direction":"{direction}"}},"Widgets":[{children}]}}"#
        )
    }

    fn filling_widget(type_id: &str, id: &str, display: &str) -> String {
        format!(
            r#"{{"Properties":{{"DisplayName":"{display}","Id":"{id}","ResponsiveLayout":true,"Type":"{type_id}"}},"Widgets":[]}}"#
        )
    }

    fn child_ids(document: &MashupDocument, id: &str) -> Vec<String> {
        children(document.at(&document.find(id).unwrap().path))
            .iter()
            .map(|child| widget_id(child).to_string())
            .collect()
    }

    fn catalogue_widget(type_id: &str) -> &Widget {
        widgets::catalog().by_type(type_id).unwrap()
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
        // Any trailing number counts, with or without a dash.
        assert_eq!(
            document(&widget("ptcslabel", "legacy41", "x", ""), 4).allocate(),
            42
        );
    }

    #[test]
    fn a_new_widget_is_written_as_composer_drops_it_with_sorted_keys() {
        let button = widgets::catalog().by_type("ptcsbutton").unwrap();
        let made = new_widget(button, 7, true);
        let properties = made["Properties"].as_object().unwrap();
        assert_eq!(properties["Id"], "ptcsbutton-7");
        assert_eq!(properties["DisplayName"], "ptcs-button-7");
        assert_eq!(properties["__TypeDisplayName"], "Button");
        assert_eq!(properties["Label"], "[[Button]]");
        assert_eq!(properties["LastContainer"], false);
        assert_eq!(properties["UseTheme"], true);
        let keys: Vec<&String> = properties.keys().collect();
        assert!(keys.windows(2).all(|p| p[0] < p[1]), "{keys:?}");
        // A button cannot resize, so it never fills its container.
        assert_eq!(properties["ResponsiveLayout"], false);
        let container = new_widget(
            widgets::catalog().by_type("flexcontainer").unwrap(),
            8,
            true,
        );
        assert_eq!(container["Properties"]["DisplayName"], "container-8");
        assert_eq!(
            container["Properties"]["iconClass"],
            "widgets-flexcontainer"
        );
    }

    #[test]
    fn a_widget_that_can_resize_fills_its_container_only_when_alone_in_it() {
        let grid = widgets::catalog().by_type("ptcsgrid").unwrap();
        assert_eq!(
            new_widget(grid, 3, true)["Properties"]["ResponsiveLayout"],
            true
        );
        assert_eq!(
            new_widget(grid, 3, false)["Properties"]["ResponsiveLayout"],
            false
        );
        // The older containers are not themed, and the page container has one fixed name.
        let page = new_widget(
            widgets::catalog().by_type("pagemashupcontainer").unwrap(),
            4,
            true,
        );
        assert_eq!(page["Properties"]["DisplayName"], "Page Mashup");
        assert_eq!(page["Properties"]["Id"], "pagemashupcontainer-4");
        assert!(page["Properties"].get("UseTheme").is_none());
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

    #[test]
    fn placing_in_a_container_obeys_responsive_layout_and_the_filling_guard() {
        let mut document = parse(BLANK).unwrap();
        let grid = document
            .place(
                catalogue_widget("ptcsgrid"),
                &Placement::In("center".into()),
            )
            .unwrap();
        assert_eq!(grid.id, "ptcsgrid-3");
        assert_eq!(
            document.at(&document.find(&grid.id).unwrap().path)["Properties"]["ResponsiveLayout"],
            true
        );
        let mut labels = parse(BLANK).unwrap();
        let label = labels
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::In("center".into()),
            )
            .unwrap();
        assert_eq!(label.id, "ptcslabel-3");
        assert_eq!(
            labels.at(&labels.find(&label.id).unwrap().path)["Properties"]["ResponsiveLayout"],
            false
        );
        assert!(document.problems().is_empty());
        assert!(labels.problems().is_empty());

        let mut filled = parse(BLANK).unwrap();
        filled
            .place(
                catalogue_widget("ptcsgrid"),
                &Placement::In("center".into()),
            )
            .unwrap();
        let error = filled
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::In("center".into()),
            )
            .unwrap_err();
        assert!(
            matches!(error, EditError::Refused(message) if message.contains("fills flexcontainer-2"))
        );
    }

    #[test]
    fn placing_a_leaf_in_a_container_of_containers_wraps_the_leaf() {
        let inner = container("flexcontainer-3", "container-3", "row", "");
        let outer = container("flexcontainer-2", "outer", "column", &inner);
        let mut document = document(&outer, 3);
        let placed = document
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::In("flexcontainer-2".into()),
            )
            .unwrap();
        assert_eq!(placed.containers, ["flexcontainer-5"]);
        assert_eq!(
            child_ids(&document, "flexcontainer-2"),
            ["flexcontainer-3", "flexcontainer-5"]
        );
        assert_eq!(child_ids(&document, "flexcontainer-5"), ["ptcslabel-4"]);
        assert!(document.problems().is_empty());
    }

    #[test]
    fn placing_in_a_leaf_or_mixing_a_container_with_leaves_is_refused() {
        let leaf = widget("ptcsbutton", "ptcsbutton-3", "button", "");
        let mut leaf_document = document(&leaf, 3);
        let error = leaf_document
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::In("ptcsbutton-3".into()),
            )
            .unwrap_err();
        assert!(
            matches!(error, EditError::Refused(message) if message.contains("not a responsive container"))
        );

        let leaves = [
            widget("ptcslabel", "ptcslabel-3", "label-3", ""),
            widget("ptcslabel", "ptcslabel-4", "label-4", ""),
        ]
        .join(",");
        let target = container("flexcontainer-2", "target", "row", &leaves);
        let mut document = document(&target, 4);
        let error = document
            .place(
                catalogue_widget("flexcontainer"),
                &Placement::In("flexcontainer-2".into()),
            )
            .unwrap_err();
        assert!(matches!(error, EditError::Refused(message) if message.contains("holds widgets")));
    }

    #[test]
    fn matching_direction_inserts_beside_the_anchor_in_side_order() {
        let leaves = [
            widget("ptcslabel", "ptcslabel-3", "label-3", ""),
            widget("ptcslabel", "ptcslabel-4", "label-4", ""),
        ]
        .join(",");
        let row = container("flexcontainer-2", "row", "row", &leaves);
        let mut right = document(&row, 4);
        right
            .place(
                catalogue_widget("ptcsbutton"),
                &Placement::Beside {
                    anchor: "ptcslabel-3".into(),
                    side: Side::Right,
                },
            )
            .unwrap();
        assert_eq!(
            child_ids(&right, "flexcontainer-2"),
            ["ptcslabel-3", "ptcsbutton-5", "ptcslabel-4"]
        );
        assert!(right.problems().is_empty());

        let mut left = document(&row, 4);
        left.place(
            catalogue_widget("ptcsbutton"),
            &Placement::Beside {
                anchor: "ptcslabel-3".into(),
                side: Side::Left,
            },
        )
        .unwrap();
        assert_eq!(
            child_ids(&left, "flexcontainer-2"),
            ["ptcsbutton-5", "ptcslabel-3", "ptcslabel-4"]
        );

        let column = container("flexcontainer-2", "column", "column", &leaves);
        let mut above = document(&column, 4);
        above
            .place(
                catalogue_widget("ptcsbutton"),
                &Placement::Beside {
                    anchor: "ptcslabel-3".into(),
                    side: Side::Above,
                },
            )
            .unwrap();
        assert_eq!(
            child_ids(&above, "flexcontainer-2"),
            ["ptcsbutton-5", "ptcslabel-3", "ptcslabel-4"]
        );
    }

    #[test]
    fn one_child_container_flips_direction_for_a_sibling() {
        let leaf = widget("ptcslabel", "ptcslabel-3", "label-3", "");
        let column = container("flexcontainer-2", "column", "column", &leaf);
        let mut document = document(&column, 3);
        let placed = document
            .place(
                catalogue_widget("ptcsbutton"),
                &Placement::Beside {
                    anchor: "ptcslabel-3".into(),
                    side: Side::Right,
                },
            )
            .unwrap();
        assert_eq!(placed.flipped.as_deref(), Some("flexcontainer-2"));
        assert_eq!(
            document.at(&document.find("flexcontainer-2").unwrap().path)["Properties"]
                ["flex-direction"],
            "row"
        );
        assert_eq!(
            child_ids(&document, "flexcontainer-2"),
            ["ptcslabel-3", "ptcsbutton-4"]
        );
        assert!(document.problems().is_empty());
    }

    #[test]
    fn a_container_next_to_a_leaf_splits_even_when_the_direction_matches() {
        let leaves = [
            widget("ptcslabel", "ptcslabel-3", "label-3", ""),
            widget("ptcslabel", "ptcslabel-4", "label-4", ""),
        ]
        .join(",");
        let row = container("flexcontainer-2", "row", "row", &leaves);
        let mut document = document(&row, 4);
        let placed = document
            .place(
                catalogue_widget("flexcontainer"),
                &Placement::Beside {
                    anchor: "ptcslabel-3".into(),
                    side: Side::Right,
                },
            )
            .unwrap();
        assert_eq!(
            placed.containers,
            [
                "flexcontainer-5",
                "flexcontainer-6",
                "flexcontainer-7",
                "flexcontainer-8"
            ]
        );
        assert_eq!(
            child_ids(&document, "flexcontainer-2"),
            ["flexcontainer-6", "flexcontainer-8"]
        );
        assert_eq!(
            child_ids(&document, "flexcontainer-6"),
            ["flexcontainer-7", "flexcontainer-5"]
        );
        assert!(document.problems().is_empty());
    }

    #[test]
    fn splitting_beside_a_filling_chart_wraps_all_leaf_siblings() {
        let leaves = [
            filling_widget("ptcschartbar", "ptcschartbar-3", "chart"),
            widget("ptcslabel", "ptcslabel-4", "label", ""),
        ]
        .join(",");
        let column = container("flexcontainer-2", "column", "column", &leaves);
        let mut document = document(&column, 4);
        let placed = document
            .place(
                catalogue_widget("ptcsgrid"),
                &Placement::Beside {
                    anchor: "ptcschartbar-3".into(),
                    side: Side::Right,
                },
            )
            .unwrap();
        assert_eq!(placed.id, "ptcsgrid-5");
        assert_eq!(
            placed.containers,
            [
                "flexcontainer-6",
                "flexcontainer-7",
                "flexcontainer-8",
                "flexcontainer-9"
            ]
        );
        assert_eq!(placed.wrapped, ["ptcschartbar-3", "ptcslabel-4"]);
        assert_eq!(
            child_ids(&document, "flexcontainer-2"),
            ["flexcontainer-6", "flexcontainer-9"]
        );
        assert_eq!(
            document.at(&document.find("flexcontainer-6").unwrap().path)["Properties"]
                ["flex-direction"],
            "row"
        );
        assert_eq!(
            child_ids(&document, "flexcontainer-6"),
            ["flexcontainer-7", "flexcontainer-8"]
        );
        assert_eq!(child_ids(&document, "flexcontainer-7"), ["ptcschartbar-3"]);
        assert_eq!(child_ids(&document, "flexcontainer-8"), ["ptcsgrid-5"]);
        assert_eq!(
            document.at(&document.find("ptcschartbar-3").unwrap().path)["Properties"]
                ["ResponsiveLayout"],
            true
        );
        assert_eq!(
            document.at(&document.find("ptcsgrid-5").unwrap().path)["Properties"]
                ["ResponsiveLayout"],
            true
        );
        assert_eq!(document.value()["UI"]["Properties"]["id_index"], 9);
        assert_eq!(
            document
                .widgets()
                .iter()
                .map(|widget| &widget.id)
                .collect::<BTreeSet<_>>()
                .len(),
            document.widgets().len()
        );
        assert!(document.problems().is_empty());
    }

    #[test]
    fn direction_mismatch_with_siblings_splits_and_wraps_leaves() {
        let leaves = [
            widget("ptcslabel", "ptcslabel-3", "label-3", ""),
            widget("ptcslabel", "ptcslabel-4", "label-4", ""),
        ]
        .join(",");
        let column = container("flexcontainer-2", "column", "column", &leaves);
        let mut document = document(&column, 4);
        let placed = document
            .place(
                catalogue_widget("ptcsbutton"),
                &Placement::Beside {
                    anchor: "ptcslabel-3".into(),
                    side: Side::Right,
                },
            )
            .unwrap();
        assert_eq!(placed.wrapped, ["ptcslabel-3", "ptcslabel-4"]);
        assert_eq!(
            child_ids(&document, "flexcontainer-2"),
            ["flexcontainer-6", "flexcontainer-9"]
        );
        assert_eq!(
            child_ids(&document, "flexcontainer-6"),
            ["flexcontainer-7", "flexcontainer-8"]
        );
        assert!(document.problems().is_empty());
    }

    #[test]
    fn placing_below_a_container_splits_without_wrapping_its_siblings() {
        let children = [
            container("flexcontainer-3", "container-3", "column", ""),
            container("flexcontainer-4", "container-4", "column", ""),
        ]
        .join(",");
        let row = container("flexcontainer-2", "row", "row", &children);
        let mut document = document(&row, 4);
        let placed = document
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::Beside {
                    anchor: "flexcontainer-3".into(),
                    side: Side::Below,
                },
            )
            .unwrap();
        assert!(placed.wrapped.is_empty());
        assert_eq!(placed.containers, ["flexcontainer-6", "flexcontainer-7"]);
        assert_eq!(
            child_ids(&document, "flexcontainer-2"),
            ["flexcontainer-6", "flexcontainer-4"]
        );
        assert_eq!(
            child_ids(&document, "flexcontainer-6"),
            ["flexcontainer-3", "flexcontainer-7"]
        );
        assert_eq!(child_ids(&document, "flexcontainer-7"), ["ptcslabel-5"]);
        assert!(document.problems().is_empty());
    }

    #[test]
    fn left_of_and_above_put_the_new_widget_first_in_the_split() {
        let leaves = [
            filling_widget("ptcschartbar", "ptcschartbar-3", "chart"),
            widget("ptcslabel", "ptcslabel-4", "label", ""),
        ]
        .join(",");
        let column = container("flexcontainer-2", "column", "column", &leaves);
        let mut left = document(&column, 4);
        left.place(
            catalogue_widget("ptcsbutton"),
            &Placement::Beside {
                anchor: "ptcschartbar-3".into(),
                side: Side::Left,
            },
        )
        .unwrap();
        assert_eq!(
            child_ids(&left, "flexcontainer-2"),
            ["flexcontainer-6", "flexcontainer-9"]
        );
        assert_eq!(
            child_ids(&left, "flexcontainer-6"),
            ["flexcontainer-8", "flexcontainer-7"]
        );
        assert_eq!(child_ids(&left, "flexcontainer-8"), ["ptcsbutton-5"]);
        assert_eq!(child_ids(&left, "flexcontainer-7"), ["ptcschartbar-3"]);
        assert!(left.problems().is_empty());

        let children = [
            container("flexcontainer-3", "container-3", "column", ""),
            container("flexcontainer-4", "container-4", "column", ""),
        ]
        .join(",");
        let row = container("flexcontainer-2", "row", "row", &children);
        let mut above = document(&row, 4);
        above
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::Beside {
                    anchor: "flexcontainer-3".into(),
                    side: Side::Above,
                },
            )
            .unwrap();
        assert_eq!(
            child_ids(&above, "flexcontainer-2"),
            ["flexcontainer-6", "flexcontainer-4"]
        );
        assert_eq!(
            child_ids(&above, "flexcontainer-6"),
            ["flexcontainer-7", "flexcontainer-3"]
        );
        assert_eq!(
            above.at(&above.find("flexcontainer-6").unwrap().path)["Properties"]["flex-direction"],
            "column"
        );
        assert!(above.problems().is_empty());
    }

    #[test]
    fn top_unknown_and_ambiguous_anchors_are_refused() {
        let mut blank = parse(BLANK).unwrap();
        let error = blank
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::Beside {
                    anchor: "center".into(),
                    side: Side::Right,
                },
            )
            .unwrap_err();
        assert!(matches!(error, EditError::Refused(message) if message.contains("top container")));
        let error = blank
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::Beside {
                    anchor: "missing".into(),
                    side: Side::Right,
                },
            )
            .unwrap_err();
        assert_eq!(error, EditError::NoWidget("missing".into()));

        let labels = [
            widget("ptcslabel", "ptcslabel-3", "first", ""),
            widget("ptcslabel", "ptcslabel-4", "second", ""),
        ]
        .join(",");
        let mut ambiguous = document(&container("flexcontainer-2", "row", "row", &labels), 4);
        let error = ambiguous
            .place(
                catalogue_widget("ptcsbutton"),
                &Placement::Beside {
                    anchor: "label".into(),
                    side: Side::Right,
                },
            )
            .unwrap_err();
        assert!(matches!(error, EditError::Ambiguous { word, .. } if word == "label"));
    }
}

#[cfg(test)]
mod placement_walkthrough {
    use super::*;

    /// The user's example on the real blank template: a chart in center, a grid to its right, a
    /// button below the grid. Prints the tree so a person can read the result.
    #[test]
    fn a_grid_right_of_the_chart_then_a_button_below_the_grid() {
        let catalog = widgets::catalog();
        let mut document = parse(crate::core::mashup_templates::BLANK).unwrap();
        let chart = document
            .place(
                catalog.resolve("bar-chart").unwrap(),
                &Placement::In("center".into()),
            )
            .unwrap();
        let grid = document
            .place(
                catalog.resolve("grid").unwrap(),
                &Placement::Beside {
                    anchor: "chart".into(),
                    side: Side::Right,
                },
            )
            .unwrap();
        document
            .place(
                catalog.resolve("button").unwrap(),
                &Placement::Beside {
                    anchor: grid.id.clone(),
                    side: Side::Below,
                },
            )
            .unwrap();
        let tree: Vec<String> = document
            .widgets()
            .iter()
            .map(|w| {
                let p = &document.at(&w.path)["Properties"];
                format!(
                    "{}{} {} fills={}",
                    "  ".repeat(w.path.len()),
                    w.id,
                    p["flex-direction"].as_str().unwrap_or(""),
                    p["ResponsiveLayout"]
                )
            })
            .collect();
        assert_eq!(
            tree,
            [
                "mashup-root  fills=true",
                "  flexcontainer-2 row fills=true",
                "    flexcontainer-5 row fills=true",
                "      ptcschartbar-3  fills=true",
                "    flexcontainer-6 column fills=true",
                "      flexcontainer-8 row fills=true",
                "        ptcsgrid-4  fills=true",
                "      flexcontainer-9 row fills=true",
                "        ptcsbutton-7  fills=false",
            ],
            "center, alone with the chart, becomes the row; the grid's own container the column"
        );
        assert!(document.problems().is_empty());
        assert_eq!(chart.id, "ptcschartbar-3");
        let after = document.find("ptcschartbar-3").unwrap();
        assert_eq!(
            document.at(&after.path)["Properties"]["ResponsiveLayout"],
            true
        );
    }
}
