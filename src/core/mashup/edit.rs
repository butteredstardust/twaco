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
pub(crate) const ROOT_ID: &str = "mashup-root";

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
pub(crate) type WidgetPath = Vec<usize>;

/// One widget as the editor sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetRef {
    pub id: String,
    pub type_id: String,
    pub display_name: String,
    pub(crate) path: WidgetPath,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    /// The widget it is about, if it is about one.
    pub(crate) widget: Option<String>,
    pub(crate) message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Above,
    Below,
}

impl Side {
    pub(crate) fn axis(self) -> &'static str {
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

/// What a fixed row adds to its tallest widget: real rows of 34px fields are 45-49px, of 37px
/// labels 47px.
const ROW_MARGIN: f64 = 12.0;

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
pub(crate) struct MashupDocument {
    value: Value,
}

/// Parse and check a `content.json`.
pub(crate) fn parse(content: &str) -> Result<MashupDocument, EditError> {
    let value: Value = serde_json::from_str(content)
        .map_err(|e| EditError::Invalid(format!("content.json will not parse: {e}")))?;
    MashupDocument::new(value)
}

impl MashupDocument {
    pub(crate) fn new(value: Value) -> Result<Self, EditError> {
        let document = Self { value };
        document.check()?;
        Ok(document)
    }

    pub(crate) fn value(&self) -> &Value {
        &self.value
    }

    /// The editable root.  Callers must retain Composer's ordering conventions with
    /// [`insert_sorted`].
    pub(crate) fn value_mut(&mut self) -> &mut Value {
        &mut self.value
    }

    /// The root object. [`Self::check`] established it, so this cannot fail.
    pub(crate) fn root_mut(&mut self) -> &mut Map<String, Value> {
        self.value
            .as_object_mut()
            .expect("checked: the content is an object")
    }

    /// `DataBindings`, created empty if the mashup has none.
    pub(crate) fn bindings_mut(&mut self) -> &mut Vec<Value> {
        self.list_mut("DataBindings")
    }

    /// `Events`, created empty if the mashup has none.
    pub(crate) fn events_mut(&mut self) -> &mut Vec<Value> {
        self.list_mut("Events")
    }

    /// `Data`, created empty (in Composer's key order) if the mashup has none.
    pub(crate) fn data_mut(&mut self) -> &mut Map<String, Value> {
        let root = self.root_mut();
        if !root.contains_key("Data") {
            insert_sorted(root, "Data", Value::Object(Map::new()));
        }
        root["Data"]
            .as_object_mut()
            .expect("checked: Data is an object")
    }

    fn list_mut(&mut self, section: &str) -> &mut Vec<Value> {
        self.root_mut()
            .entry(section)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("checked: DataBindings and Events are lists")
    }

    /// The `content.json` text, formatted as extraction writes it.
    pub(crate) fn to_content(&self) -> String {
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
    pub(crate) fn widgets(&self) -> Vec<WidgetRef> {
        let mut out = Vec::new();
        collect(&self.value["UI"], &mut Vec::new(), &mut out);
        out
    }

    /// The widget a person names: an exact `Id`, else a unique `DisplayName`, else the only
    /// widget of a type the catalogue resolves the word to, else the only widget whose type a
    /// catalogue search for the word finds ("the chart").
    pub(crate) fn find(&self, word: &str) -> Result<WidgetRef, EditError> {
        self.find_widget(word, true)
    }

    /// As [`Self::find`] without the catalogue search, for wiring, where a loose guess could
    /// bind the wrong widget.
    pub(crate) fn find_exact(&self, word: &str) -> Result<WidgetRef, EditError> {
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
    pub(crate) fn at(&self, path: &[usize]) -> &Value {
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

    /// Remove one non-root widget, including its descendants. Callers must first account for
    /// every reference to its subtree; this only changes the structural tree.
    pub(crate) fn remove_widget(&mut self, path: &[usize]) -> Result<(), EditError> {
        let Some((&index, parent)) = path.split_last() else {
            return Err(EditError::Refused(
                "the mashup root cannot be removed".into(),
            ));
        };
        children_mut(self.at_mut(parent)).remove(index);
        let parent = self.remove_empty_generated_wrappers(parent.to_vec());
        self.reflow_at(&parent, &Reflow::Edit);
        Ok(())
    }

    /// Append `widget` as the last child of the widget at `parent`.
    pub(crate) fn push_child(&mut self, parent: &[usize], widget: Value) {
        self.insert_child(parent, children(self.at(parent)).len(), widget);
    }

    fn insert_child(&mut self, parent: &[usize], index: usize, widget: Value) {
        children_mut(self.at_mut(parent)).insert(index, widget);
    }

    fn replace_child(&mut self, parent: &[usize], index: usize, widget: Value) {
        children_mut(self.at_mut(parent))[index] = widget;
    }

    /// Set one widget property, preserving Composer's key ordering convention.
    pub(crate) fn set_property(&mut self, path: &[usize], key: &str, value: Value) {
        insert_sorted(properties_mut(self.at_mut(path)), key, value);
    }

    /// The root widget's properties.
    pub(crate) fn root_properties_mut(&mut self) -> &mut Map<String, Value> {
        properties_mut(&mut self.value["UI"])
    }

    /// The root widget's properties.
    pub(crate) fn root_properties(&self) -> &Map<String, Value> {
        self.value["UI"]["Properties"]
            .as_object()
            .expect("checked: UI.Properties is an object")
    }

    /// Set a top-level key of the document, such as `mashupType`.
    pub(crate) fn set_top(&mut self, key: &str, value: Value) {
        insert_sorted(self.root_mut(), key, value);
    }

    /// The next free number for a new widget id, which the root's `id_index` then records.
    pub(crate) fn allocate(&mut self) -> u64 {
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
        insert_sorted(self.root_properties_mut(), "id_index", Value::from(next));
        next
    }

    /// Structural problems: duplicate ids are refused by `parse`, so these are what a document
    /// can still get wrong, such as a responsive container holding both containers and widgets.
    pub(crate) fn problems(&self) -> Vec<Problem> {
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
    pub(crate) fn place(
        &mut self,
        widget: &Widget,
        placement: &Placement,
    ) -> Result<Placed, EditError> {
        let placed = match placement {
            Placement::In(target) => self.place_in(widget, target),
            Placement::Beside { anchor, side } => self.place_beside(widget, anchor, *side),
        }?;
        self.fix_rows(&placed);
        Ok(placed)
    }

    /// Re-measure the rows around a widget after one of its properties changes.  This is kept
    /// separate from placement because a property such as `LabelType` changes a label's rendered
    /// height after Composer's initial drop layout has been made.
    /// Only rows that are already fixed grow, and nothing is made to fill: an edit never
    /// reshapes a layout it did not make.
    pub(crate) fn reflow_for(&mut self, id: &str) {
        if let Ok(widget) = self.find(id) {
            self.reflow_at(&widget.path, &Reflow::Edit);
        }
    }

    /// Give the rows this placement made or added to the height of their widgets, as real
    /// mashups do (a header or a filter bar is a fixed row; what fills grows): a container in a
    /// column that holds only fixed-size widgets becomes a fixed row of the tallest one's height
    /// plus a margin. Only containers the placement created or put the new widget in are
    /// touched, and a row only ever grows, so a size someone chose survives.
    fn fix_rows(&mut self, placed: &Placed) {
        let mut touched = placed.containers.clone();
        touched.push(placed.id.clone());
        let scope = Reflow::Placement {
            placed: &placed.id,
            touched: &touched,
        };
        for id in &touched {
            if let Ok(widget) = self.find(id) {
                self.reflow_at(&widget.path, &scope);
            }
        }
    }

    /// Reflow the columns enclosing `path`.  A placement can touch several nested wrappers; each
    /// enclosing column owns its rows, while a row's own size remains guarded below.
    fn reflow_at(&mut self, path: &[usize], scope: &Reflow) {
        let mut columns = Vec::new();
        for depth in 0..=path.len() {
            let candidate = &path[..depth];
            let node = self.at(candidate);
            if type_of(node) == Some("flexcontainer")
                && node["Properties"]["flex-direction"] == "column"
            {
                columns.push(widget_id(node).to_string());
            }
        }
        for column in columns {
            self.reflow_column(&column, scope);
        }
    }

    /// Give one column one growing data row and fixed-height control rows. Only the widget just
    /// placed may become the filler, and only rows the placement made or put it in become fixed;
    /// any other row only grows if it is fixed already, so a Composer layout is not normalised.
    fn reflow_column(&mut self, id: &str, scope: &Reflow) {
        let Ok(column) = self.find(id) else {
            return;
        };
        let column_path = column.path;
        let rows = children(self.at(&column_path)).to_vec();
        let growing = rows.iter().position(row_fills);
        let filler = match scope {
            Reflow::Placement { placed, .. } => Some(*placed),
            Reflow::Edit => None,
        };
        let is_filler = |widget: &Value| filler == Some(widget_id(widget)) && can_fill(widget);
        let candidate = rows.iter().position(|row| {
            type_of(row) == Some("flexcontainer")
                && children(row).len() == 1
                && is_filler(&children(row)[0])
        });
        if growing.is_none() {
            if let Some(index) = candidate {
                let row_path = [column_path.as_slice(), &[index]].concat();
                self.make_row_fill(&row_path);
            } else if rows.len() == 1 && is_filler(&rows[0]) {
                let leaf_path = [column_path.as_slice(), &[0]].concat();
                self.make_leaf_fill(&leaf_path);
            }
        }

        // Re-read after possibly turning a lone data widget into the column's growing row.
        let rows = children(self.at(&column_path)).to_vec();
        for (index, row) in rows.iter().enumerate() {
            if type_of(row) != Some("flexcontainer") {
                continue;
            }
            let Some(tallest) = fixed_row_height(row) else {
                continue;
            };
            let owned = match scope {
                // A control row twaco generated is its own to size again, so the row that
                // grew as the last one becomes fixed when a row is added after it.
                Reflow::Placement { placed, touched } => {
                    touched.iter().any(|id| id == widget_id(row))
                        || contains(row, placed)
                        || is_generated_container(row)
                }
                Reflow::Edit => false,
            };
            let row_path = [column_path.as_slice(), &[index]].concat();
            self.fix_row(&row_path, tallest, owned);
        }

        // Runtime gives every row's bounding box `margin: auto`, so a column of only fixed rows
        // spreads them out over its height. Real mashups always let one row grow; with no data
        // widget to fill, the last control row twaco made is the one, its controls at its top.
        if matches!(scope, Reflow::Placement { .. }) {
            let rows = children(self.at(&column_path)).to_vec();
            let all_fixed = !rows.is_empty()
                && rows.iter().all(|row| {
                    type_of(row) == Some("flexcontainer")
                        && row["Properties"]["flex-size"] == "fixed"
                });
            if let (true, Some(last)) = (all_fixed, rows.last()) {
                if is_generated_container(last) && fixed_content_height(last).is_some() {
                    let last_path = [column_path.as_slice(), &[rows.len() - 1]].concat();
                    self.unfix_row(&last_path);
                }
            }
        }
    }

    fn unfix_row(&mut self, path: &[usize]) {
        self.set_property(path, "flex-size", Value::from("default"));
        self.set_property(path, "flex-min-height", Value::from(""));
        self.set_property(path, "flex-max-height", Value::from(""));
        self.set_property(path, "iconClass", Value::from("widgets-flexcontainer"));
    }

    fn make_row_fill(&mut self, path: &[usize]) {
        let leaf_path = [path, &[0]].concat();
        self.make_leaf_fill(&leaf_path);
        // A generated wrapper is twaco's row, so it may return to Composer's growing default.
        if is_generated_container(self.at(path)) {
            self.unfix_row(path);
        }
    }

    fn make_leaf_fill(&mut self, path: &[usize]) {
        self.set_property(path, "ResponsiveLayout", Value::Bool(true));
        properties_mut(self.at_mut(path)).remove("Width");
        properties_mut(self.at_mut(path)).remove("Height");
    }

    fn fix_row(&mut self, path: &[usize], tallest: f64, owned: bool) {
        let node = self.at(path).clone();
        let height = (tallest + ROW_MARGIN).ceil();
        let current = node["Properties"]["flex-min-height"]
            .as_str()
            .and_then(|text| text.strip_suffix("px"))
            .and_then(|number| number.parse::<f64>().ok());
        let is_fixed = node["Properties"]["flex-size"] == "fixed";
        if is_fixed && current.is_some_and(|current| current >= height) {
            return;
        }
        if !owned {
            // Someone else's fixed row only grows to fit; nothing else about it changes.
            if is_fixed {
                let size = Value::from(format!("{height}px"));
                self.set_property(path, "flex-min-height", size.clone());
                self.set_property(path, "flex-max-height", size);
            }
            return;
        }
        // Only a growing container (no size, or Composer's default) becomes a fixed row.
        let size = &node["Properties"]["flex-size"];
        if !is_fixed && !(size.is_null() || size == "default") {
            return;
        }
        if node["Properties"]["justify-content"].is_null() {
            self.set_property(path, "justify-content", Value::from("flex-start"));
        }
        let size = Value::from(format!("{height}px"));
        self.set_property(path, "flex-size", Value::from("fixed"));
        self.set_property(path, "flex-min-height", size.clone());
        self.set_property(path, "flex-max-height", size);
        self.set_property(
            path,
            "iconClass",
            Value::from("widgets-flexcontainer-fixed"),
        );
    }

    fn remove_empty_generated_wrappers(&mut self, mut path: WidgetPath) -> WidgetPath {
        while !path.is_empty()
            && children(self.at(&path)).is_empty()
            && is_generated_container(self.at(&path))
        {
            let (index, parent) = path.split_last().expect("non-root has a parent");
            children_mut(self.at_mut(parent)).remove(*index);
            path = parent.to_vec();
        }
        path
    }

    fn place_in(&mut self, widget: &Widget, target: &str) -> Result<Placed, EditError> {
        let target = self.find(target)?;
        if target.type_id != "flexcontainer" {
            return Err(EditError::Refused(format!(
                "{} is a {}, not a responsive container; place the widget beside it instead",
                target.id, target.type_id
            )));
        }
        let target_children = children(self.at(&target.path)).to_vec();
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
        let wrap_column_for_filler = widget.auto_resize
            && self.at(&target.path)["Properties"]["flex-direction"] == "column"
            && !target_empty
            && !holds_containers
            && !target_children.iter().any(fills);
        if target_children.len() == 1 && fills(&target_children[0]) {
            let child_id = widget_id(&target_children[0]);
            return Err(EditError::Refused(format!(
                "{child_id} fills {}; place the widget beside it (--right-of, --below, ...)",
                target.id
            )));
        }

        let number = self.allocate();
        let id = format!("{}-{number}", widget.type_id);
        let wrap_new =
            (holds_containers || wrap_column_for_filler) && widget.type_id != "flexcontainer";
        let new = new_widget(widget, number, target_empty || wrap_new);
        let mut placed = Placed {
            id,
            ..Placed::default()
        };
        if widget.type_id == "flexcontainer" {
            placed.containers.push(placed.id.clone());
        }
        if wrap_column_for_filler {
            for (index, child) in target_children.into_iter().enumerate() {
                let wrapper = self.wrap(child, &mut placed);
                self.replace_child(&target.path, index, wrapper);
            }
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
        let mut anchor = self.find(anchor_word)?;
        if anchor.path.is_empty() {
            return Err(EditError::Refused(format!(
                "{} is the page's top container; place the widget inside it",
                anchor.id
            )));
        }
        // A control row twaco made running the other way is part of the anchor, not the place to
        // put a sibling: below a field in its row, or below a toolbar button, means a new row in
        // the enclosing column, not turning the row into a column. A named container, or a cell
        // holding a widget that fills (a grid beside a chart), stops this.
        while let Some((_, parent)) = anchor.path.split_last() {
            if parent.is_empty() {
                break;
            }
            let node = self.at(parent);
            if !is_generated_container(node)
                || node["Properties"]["flex-direction"] == side.axis()
                || fixed_content_height(node).is_none()
            {
                break;
            }
            let id = widget_id(node).to_string();
            anchor = self.find(&id)?;
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
        let anchor_is_container = anchor.type_id == "flexcontainer";
        let new_is_container = widget.type_id == "flexcontainer";
        // A widget that can fill gets a container of its own, where it fills, rather than a
        // fixed size among siblings. Next to a container in a run of the right direction it is
        // wrapped as a sibling row or cell, which gives it that container.
        let filler = widget.auto_resize && !new_is_container;
        let axis_matches = direction.as_deref() == Some(side.axis());
        let sibling_possible = !fills(self.at(&anchor.path))
            && (!filler || (anchor_is_container && axis_matches))
            && (axis_matches || parent_children.len() == 1);

        if sibling_possible && (anchor_is_container || !new_is_container) {
            let number = self.allocate();
            let id = format!("{}-{number}", widget.type_id);
            let rows_in_column = side.axis() == "column"
                && direction.as_deref() == Some("column")
                && !anchor_is_container;
            let wrap_new = (anchor_is_container || rows_in_column) && !new_is_container;
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
            if rows_in_column {
                let old_children = children(self.at(&parent_path)).to_vec();
                for (index, child) in old_children.into_iter().enumerate() {
                    if type_of(&child) != Some("flexcontainer") {
                        let wrapper = self.wrap(child, &mut placed);
                        self.replace_child(&parent_path, index, wrapper);
                    }
                }
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
pub(crate) fn new_widget(widget: &Widget, number: u64, alone: bool) -> Value {
    let catalog = widgets::catalog();
    let mut properties: Map<String, Value> = widget.defaults.clone();
    let dropped = if widget.type_id == "flexcontainer" {
        Some(&catalog.dropped.flexcontainer)
    } else if widget.has_contract() && !widget.function && !widget.extension && !widget.legacy {
        Some(&catalog.dropped.ptcs)
    } else {
        None
    };
    for (key, value) in dropped.into_iter().flatten() {
        properties.insert(key.clone(), value.clone());
    }
    if widget.type_id == "ptcsgrid" {
        // Composer drops a grid titled "[[Grid]]" ("Grid Title"); the design kits clear it on
        // 17 of 18 grids and title the page with a label instead.
        properties.insert("Label".into(), Value::from(""));
    }
    if widget.type_id == "ptcscollection" {
        // Composer drops a collection centring its cells, which leaves a band above them; the
        // design kits align 29 of 32 collections to the top.
        properties.insert("ContentAlignment".into(), Value::from("top"));
    }
    if widget.type_id != "flexcontainer" {
        properties.insert(
            "ResponsiveLayout".into(),
            Value::from(widget.auto_resize && alone),
        );
        if widget.auto_resize && alone {
            // Responsive widgets fill their row; Composer does not retain the drop dimensions.
            properties.remove("Width");
            properties.remove("Height");
        }
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
pub(crate) fn insert_sorted(object: &mut Map<String, Value>, key: &str, value: Value) {
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

/// The `Properties` of a widget: one the document check passed, or one [`new_widget`] built.
pub(crate) fn properties_mut(widget: &mut Value) -> &mut Map<String, Value> {
    widget["Properties"]
        .as_object_mut()
        .expect("a checked or newly built widget has a Properties object")
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

/// How tall a fixed-size widget renders, with its vertical margins: its own `Height` when
/// Composer saved one, or what the 10.2 widget is measured at in real mashups (an input is a
/// 34px field, with a 22px label above it when it has one). `None` for a widget whose height is
/// not known, so its row is left as it is.
fn content_height(widget: &Value) -> Option<f64> {
    let properties = &widget["Properties"];
    let own = properties["Height"].as_f64();
    let measured = || -> Option<f64> {
        let labelled = properties["Label"]
            .as_str()
            .is_some_and(|label| !label.is_empty());
        Some(match type_of(widget)? {
            "ptcslabel" => match properties["LabelType"].as_str() {
                Some("large-header" | "header") => 45.0,
                _ => 36.0,
            },
            "ptcsbutton" | "ptcsicon" | "ptcsdivider" | "ptcstogglebutton" | "ptcspagination" => {
                34.0
            }
            "ptcslink" => 37.0,
            "ptcsimage" => 68.0,
            "ptcscheckbox" | "ptcsradio" => 24.0,
            "ptcstextfield" | "ptcsdropdown" | "ptcsdatepicker" | "ptcscombobox"
            | "ptcsnumericentry" => {
                if labelled {
                    56.0
                } else {
                    34.0
                }
            }
            _ => return None,
        })
    };
    let height = own.or_else(measured)?;
    Some(height + vertical_margin(properties["Margin"].as_str().unwrap_or("")))
}

/// The top plus bottom of a CSS margin shorthand in pixels: `5`, `0 10`, `0 10 15` or
/// `0 10 15 10`.
fn vertical_margin(margin: &str) -> f64 {
    let sides: Vec<f64> = margin
        .split_whitespace()
        .map(|side| side.trim_end_matches("px").parse().unwrap_or(0.0))
        .collect();
    match sides.as_slice() {
        [all] => all * 2.0,
        [vertical, _] => vertical * 2.0,
        [top, _, bottom] | [top, _, bottom, _] => top + bottom,
        _ => 0.0,
    }
}

fn fills(widget: &Value) -> bool {
    type_of(widget) != Some("flexcontainer")
        && widget["Properties"]["ResponsiveLayout"] == Value::Bool(true)
}

/// How far a reflow may reshape: a placement owns what it made, an edit owns nothing.
enum Reflow<'a> {
    Placement {
        placed: &'a str,
        touched: &'a [String],
    },
    Edit,
}

fn contains(widget: &Value, id: &str) -> bool {
    widget_id(widget) == id || children(widget).iter().any(|child| contains(child, id))
}

fn can_fill(widget: &Value) -> bool {
    type_of(widget)
        .and_then(|type_id| widgets::catalog().by_type(type_id))
        .is_some_and(|widget| widget.auto_resize)
}

fn row_fills(row: &Value) -> bool {
    fills(row) || children(row).iter().any(fills)
}

/// A fixed row may contain fixed leaves directly, or the per-widget wrappers made for a toolbar.
fn fixed_row_height(row: &Value) -> Option<f64> {
    let heights = children(row)
        .iter()
        .map(fixed_content_height)
        .collect::<Option<Vec<_>>>()?;
    (!heights.is_empty()).then(|| heights.into_iter().fold(0.0, f64::max))
}

fn fixed_content_height(widget: &Value) -> Option<f64> {
    if type_of(widget) != Some("flexcontainer") {
        return (!fills(widget)).then(|| content_height(widget)).flatten();
    }
    let heights = children(widget)
        .iter()
        .map(fixed_content_height)
        .collect::<Option<Vec<_>>>()?;
    (!heights.is_empty()).then(|| heights.into_iter().fold(0.0, f64::max))
}

/// Containers twaco created have Composer's generated display name, unlike named template slots.
fn is_generated_container(widget: &Value) -> bool {
    type_of(widget) == Some("flexcontainer")
        && widget["Properties"]["DisplayName"]
            .as_str()
            .is_some_and(|name| name.starts_with("container-"))
}

fn push_value_child(parent: &mut Value, child: Value) {
    children_mut(parent).push(child);
}

/// A widget's `Widgets`, created empty (in Composer's key order) if it has none.
fn children_mut(widget: &mut Value) -> &mut Vec<Value> {
    let object = widget
        .as_object_mut()
        .expect("a checked or newly built widget is an object");
    if !object.contains_key("Widgets") {
        insert_sorted(object, "Widgets", Value::Array(Vec::new()));
    }
    object["Widgets"]
        .as_array_mut()
        .expect("checked: Widgets is a list")
}

fn set_value_property(widget: &mut Value, key: &str, value: Value) {
    insert_sorted(properties_mut(widget), key, value);
}

/// The number an id ends in: `3` for `ptcslabel-3`, `7` for `legacy7`.
fn number_of(id: &str) -> Option<u64> {
    let digits = id.len() - id.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    id[id.len() - digits..].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BLANK: &str = include_str!("templates/blank.content.json");

    fn document(widgets: &str, id_index: u64) -> MashupDocument {
        parse(&format!(
            r#"{{"UI":{{"Properties":{{"Id":"mashup-root","Type":"mashup","id_index":{id_index}}},"Widgets":[{widgets}]}},"Data":{{}},"DataBindings":[],"Events":[]}}"#
        ))
        .unwrap()
    }

    #[test]
    fn a_checked_document_creates_missing_sections_and_refuses_misshapen_ones() {
        let mut bare = parse(
            r#"{"UI":{"Properties":{"Id":"mashup-root","Type":"mashup"}},"mashupType":"mashup"}"#,
        )
        .unwrap();
        bare.data_mut().insert("Things_T".into(), json!({}));
        bare.bindings_mut().push(json!({"Id": "b"}));
        bare.events_mut().push(json!({"Id": "e"}));
        let root = bare.value().as_object().unwrap();
        assert_eq!(root["DataBindings"], json!([{"Id": "b"}]));
        assert_eq!(root["Events"], json!([{"Id": "e"}]));
        // Data goes in Composer's key order among sorted keys; the lists are appended.
        assert_eq!(
            root.keys().collect::<Vec<_>>(),
            ["Data", "UI", "mashupType", "DataBindings", "Events"]
        );
        for (content, why) in [
            (r#""DataBindings":{}"#, "DataBindings is not a list"),
            (r#""Events":"x""#, "Events is not a list"),
            (r#""Data":[]"#, "Data is not an object"),
        ] {
            let text = format!(
                r#"{{"UI":{{"Properties":{{"Id":"mashup-root","Type":"mashup"}}}},{content}}}"#
            );
            assert!(
                parse(&text).unwrap_err().to_string().contains(why),
                "{content}"
            );
        }
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
            ["flexcontainer-8", "flexcontainer-6", "flexcontainer-7"]
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
    fn a_widget_that_can_fill_gets_a_container_of_its_own_beside_fixed_widgets() {
        let column = container(
            "flexcontainer-2",
            "center",
            "column",
            r#"{"Properties":{"Id":"ptcstextfield-3","Type":"ptcstextfield","ResponsiveLayout":false},"Widgets":[]},
               {"Properties":{"Id":"ptcsbutton-4","Type":"ptcsbutton","ResponsiveLayout":false},"Widgets":[]}"#,
        );
        let mut document = document(&column, 4);
        let placed = document
            .place(
                catalogue_widget("ptcsgrid"),
                &Placement::Beside {
                    anchor: "ptcstextfield-3".into(),
                    side: Side::Below,
                },
            )
            .unwrap();
        let grid = document.find(&placed.id).unwrap();
        let parent = &grid.path[..grid.path.len() - 1];
        assert_eq!(
            children(document.at(parent)).len(),
            1,
            "alone in its container"
        );
        assert_eq!(
            document.at(&grid.path)["Properties"]["ResponsiveLayout"],
            true
        );
        assert!(document.problems().is_empty());
    }

    #[test]
    fn rows_of_fixed_widgets_in_a_column_get_their_widgets_height() {
        // A labelled field (56) with a 5px margin all round, plus the row margin: 78px.
        let column = container("flexcontainer-2", "center", "column", "");
        let mut document = document(&column, 2);
        let field = document
            .place(
                catalogue_widget("ptcstextfield"),
                &Placement::In("flexcontainer-2".into()),
            )
            .unwrap();
        let path = document.find(&field.id).unwrap().path;
        document.set_property(&path, "Label", Value::from("Filter"));
        let grid = document
            .place(
                catalogue_widget("ptcsgrid"),
                &Placement::Beside {
                    anchor: field.id.clone(),
                    side: Side::Below,
                },
            )
            .unwrap();
        let row = document.find(&field.id).unwrap().path;
        let row = document.at(&row[..row.len() - 1])["Properties"].clone();
        assert_eq!(row["flex-size"], "fixed");
        assert_eq!(row["flex-min-height"], "78px");
        assert_eq!(row["flex-max-height"], "78px");
        assert_eq!(row["iconClass"], "widgets-flexcontainer-fixed");
        // The grid's container grows.
        let filled = document.find(&grid.id).unwrap().path;
        let filled = &document.at(&filled[..filled.len() - 1])["Properties"];
        assert_ne!(filled["flex-size"], "fixed");
        // A row someone made taller keeps its height when a smaller widget joins it.
        let row_path = {
            let found = document.find(&field.id).unwrap().path;
            found[..found.len() - 1].to_vec()
        };
        document.set_property(&row_path, "flex-min-height", Value::from("120px"));
        document.set_property(&row_path, "flex-max-height", Value::from("120px"));
        document
            .place(
                catalogue_widget("ptcsbutton"),
                &Placement::Beside {
                    anchor: field.id.clone(),
                    side: Side::Right,
                },
            )
            .unwrap();
        assert_eq!(
            document.at(&row_path)["Properties"]["flex-min-height"],
            "120px"
        );
    }

    #[test]
    fn side_by_side_buttons_share_one_row_as_the_kit_does() {
        let button = widget("ptcsbutton", "ptcsbutton-4", "View Details", "");
        let row = container("flexcontainer-3", "container-3", "row", &button);
        let column = container("flexcontainer-2", "content", "column", &row);
        let mut document = document(&column, 4);
        let placed = document
            .place(
                catalogue_widget("ptcsbutton"),
                &Placement::Beside {
                    anchor: "ptcsbutton-4".into(),
                    side: Side::Right,
                },
            )
            .unwrap();
        let row = document.find("flexcontainer-3").unwrap();
        let row = document.at(&row.path);
        // The column's only row: it grows, so Runtime does not centre it.
        assert_eq!(row["Properties"]["flex-size"], "default");
        assert_eq!(row["Properties"]["justify-content"], "flex-start");
        // Direct children, no per-button containers: Runtime gives a container's box
        // `margin: auto`, which would spread the buttons across the row.
        assert_eq!(
            child_ids(&document, "flexcontainer-3"),
            ["ptcsbutton-4".to_string(), placed.id.clone()]
        );
        assert!(document.find(&placed.id).is_ok());
    }

    #[test]
    fn a_grid_in_a_column_with_controls_fills_while_the_controls_are_fixed() {
        let column = container("flexcontainer-2", "content", "column", "");
        let mut document = document(&column, 2);
        let grid = document
            .place(
                catalogue_widget("ptcsgrid"),
                &Placement::In("flexcontainer-2".into()),
            )
            .unwrap();
        let label = document
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::Beside {
                    anchor: grid.id.clone(),
                    side: Side::Below,
                },
            )
            .unwrap();
        let grid = document.find(&grid.id).unwrap();
        let grid_row = &grid.path[..grid.path.len() - 1];
        assert_eq!(
            document.at(&grid.path)["Properties"]["ResponsiveLayout"],
            true
        );
        assert!(document.at(&grid.path)["Properties"].get("Width").is_none());
        assert!(document.at(&grid.path)["Properties"]
            .get("Height")
            .is_none());
        assert_ne!(document.at(grid_row)["Properties"]["flex-size"], "fixed");
        let label = document.find(&label.id).unwrap();
        let label_row = &label.path[..label.path.len() - 1];
        assert_eq!(document.at(label_row)["Properties"]["flex-size"], "fixed");
    }

    #[test]
    fn a_grid_added_in_a_column_with_siblings_gets_the_only_growing_row() {
        let column = container("flexcontainer-2", "content", "column", "");
        let mut document = document(&column, 2);
        let title = document
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::In("flexcontainer-2".into()),
            )
            .unwrap();
        let grid = document
            .place(
                catalogue_widget("ptcsgrid"),
                &Placement::In("flexcontainer-2".into()),
            )
            .unwrap();
        let content = document.find("flexcontainer-2").unwrap();
        let rows = children(document.at(&content.path));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.iter().filter(|row| row_fills(row)).count(), 1);
        let title = document.find(&title.id).unwrap();
        let title_row = &title.path[..title.path.len() - 1];
        assert_eq!(document.at(title_row)["Properties"]["flex-size"], "fixed");
        let grid = document.find(&grid.id).unwrap();
        let grid_row = &grid.path[..grid.path.len() - 1];
        assert_ne!(document.at(grid_row)["Properties"]["flex-size"], "fixed");
        assert_eq!(
            document.at(&grid.path)["Properties"]["ResponsiveLayout"],
            true
        );
    }

    #[test]
    fn reflowing_a_label_after_its_type_changes_grows_its_fixed_row() {
        let label = widget("ptcslabel", "ptcslabel-4", "label", "");
        // As a placement leaves it: a fixed row of the label's height.
        let row = container("flexcontainer-3", "container-3", "row", &label).replacen(
            r#""flex-direction":"row""#,
            r#""flex-direction":"row","flex-size":"fixed","flex-min-height":"48px","flex-max-height":"48px""#,
            1,
        );
        let column = container("flexcontainer-2", "content", "column", &row);
        let mut document = document(&column, 4);
        document.reflow_for("ptcslabel-4");
        let row = document.find("flexcontainer-3").unwrap();
        assert_eq!(
            document.at(&row.path)["Properties"]["flex-min-height"],
            "48px"
        );
        let label = document.find("ptcslabel-4").unwrap();
        document.set_property(&label.path, "LabelType", Value::from("large-header"));
        document.reflow_for("ptcslabel-4");
        assert_eq!(
            document.at(&row.path)["Properties"]["flex-min-height"],
            "57px"
        );
    }

    #[test]
    fn a_property_edit_leaves_an_unrelated_composer_layout_alone() {
        let label = widget("ptcslabel", "ptcslabel-4", "Title", "");
        let header = container("flexcontainer-3", "Header", "row", &label);
        let grid = r#"{"Properties":{"DisplayName":"Orders","Height":400,"Id":"ptcsgrid-6","Type":"ptcsgrid","Width":600},"Widgets":[]}"#;
        let body = container("flexcontainer-5", "Body", "row", grid);
        let column = container(
            "flexcontainer-2",
            "content",
            "column",
            &format!("{header},{body}"),
        );
        let mut document = document(&column, 6);
        let label = document.find("ptcslabel-4").unwrap();
        document.set_property(&label.path, "LabelText", Value::from("Orders"));
        document.reflow_for("ptcslabel-4");
        let grid = document.find("ptcsgrid-6").unwrap();
        assert_eq!(
            document.at(&grid.path)["Properties"]["Width"],
            600,
            "grid lost its size"
        );
        let header = document.find("flexcontainer-3").unwrap();
        assert!(
            document.at(&header.path)["Properties"]
                .get("flex-size")
                .is_none(),
            "header row resized"
        );
    }

    #[test]
    fn below_a_control_in_its_row_adds_a_row_to_the_column_not_a_staircase() {
        let catalog = widgets::catalog();
        let column = container("flexcontainer-2", "content", "column", "");
        let mut document = document(&column, 2);
        let place = |document: &mut MashupDocument, name: &str, placement: Placement| {
            document
                .place(catalog.resolve(name).unwrap(), &placement)
                .unwrap()
                .id
        };
        let below = |anchor: &str| Placement::Beside {
            anchor: anchor.into(),
            side: Side::Below,
        };
        let title = place(
            &mut document,
            "label",
            Placement::In("flexcontainer-2".into()),
        );
        let field = place(&mut document, "text-field", below(&title));
        let button = place(&mut document, "button", below(&field));
        let other = place(
            &mut document,
            "button",
            Placement::Beside {
                anchor: button.clone(),
                side: Side::Right,
            },
        );
        let grid = place(&mut document, "grid", below(&button));

        // Each widget is one row of the column, in order; the two buttons share one toolbar row.
        let rows = child_ids(&document, "flexcontainer-2");
        let tree: Vec<String> = document
            .widgets()
            .iter()
            .map(|w| {
                format!(
                    "{}{} {}",
                    "  ".repeat(w.path.len()),
                    w.id,
                    document.at(&w.path)["Properties"]["flex-direction"]
                        .as_str()
                        .unwrap_or("")
                )
            })
            .collect();
        assert_eq!(
            rows.len(),
            4,
            "{}",
            tree.join(
                "
"
            )
        );
        let row_of = |document: &MashupDocument, id: &str| {
            let path = document.find(id).unwrap().path;
            widget_id(document.at(&path[..2])).to_string()
        };
        assert_eq!(
            [&title, &field, &button, &grid].map(|id| row_of(&document, id)),
            [&rows[0], &rows[1], &rows[2], &rows[3]].map(String::clone)
        );
        assert_eq!(row_of(&document, &other), rows[2]);
        let grid = document.find(&grid).unwrap();
        assert_eq!(
            document.at(&grid.path)["Properties"]["ResponsiveLayout"],
            true
        );
    }

    #[test]
    fn a_column_of_controls_fixes_every_row_but_the_last_which_grows() {
        let catalog = widgets::catalog();
        let column = container("flexcontainer-2", "content", "column", "");
        let mut document = document(&column, 2);
        let mut anchor = None;
        for _ in 0..3 {
            let placement = match &anchor {
                None => Placement::In("flexcontainer-2".into()),
                Some(id) => Placement::Beside {
                    anchor: String::clone(id),
                    side: Side::Below,
                },
            };
            anchor = Some(
                document
                    .place(catalog.resolve("text-field").unwrap(), &placement)
                    .unwrap()
                    .id,
            );
        }
        let content = document.find("flexcontainer-2").unwrap();
        let sizes: Vec<String> = children(document.at(&content.path))
            .iter()
            .map(|row| {
                row["Properties"]["flex-size"]
                    .as_str()
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        assert_eq!(sizes, ["fixed", "fixed", "default"]);
    }

    #[test]
    fn removing_a_widget_drops_its_empty_generated_cell() {
        let button = widget("ptcsbutton", "ptcsbutton-5", "Clear", "");
        let cell = container("flexcontainer-4", "container-4", "row", &button);
        let row = container("flexcontainer-3", "container-3", "row", &cell);
        let column = container("flexcontainer-2", "content", "column", &row);
        let mut document = document(&column, 5);
        let button = document.find("ptcsbutton-5").unwrap();
        document.remove_widget(&button.path).unwrap();
        assert!(document.find("flexcontainer-4").is_err());
        assert!(document.find("flexcontainer-3").is_err());
    }

    #[test]
    fn a_row_with_a_widget_of_unknown_height_is_left_growing() {
        let column = container("flexcontainer-2", "center", "column", "");
        let mut document = document(&column, 2);
        let first = document
            .place(
                catalogue_widget("ptcsslider"),
                &Placement::In("flexcontainer-2".into()),
            )
            .unwrap();
        document
            .place(
                catalogue_widget("ptcschartbar"),
                &Placement::Beside {
                    anchor: first.id.clone(),
                    side: Side::Below,
                },
            )
            .unwrap();
        let path = document.find(&first.id).unwrap().path;
        let row = &document.at(&path[..path.len() - 1])["Properties"];
        assert_ne!(row["flex-size"], "fixed");
    }

    #[test]
    fn vertical_margins_read_the_css_shorthand() {
        assert_eq!(vertical_margin("5"), 10.0);
        assert_eq!(vertical_margin("0 10"), 0.0);
        assert_eq!(vertical_margin("0 10 15"), 15.0);
        assert_eq!(vertical_margin("20 0 0 0"), 20.0);
        assert_eq!(vertical_margin(""), 0.0);
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
        let mut document = parse(crate::core::mashup::templates::BLANK).unwrap();
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
