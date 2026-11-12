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
    /// Right of the anchor, at the far end of its row: the row splits into a start group and an
    /// end group, as the design kits lay out a label and its value.
    End,
}

impl Side {
    pub(crate) fn axis(self) -> &'static str {
        match self {
            Self::Left | Self::Right | Self::End => "row",
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
    /// In a footer row at the bottom of a column (`--in <column> --bottom`), as the kits place a
    /// filter panel's Apply and Clear.
    Bottom(String),
    Beside {
        anchor: String,
        side: Side,
    },
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
    /// Remove the widget at `path`, then the empty containers twaco made around it; returns the
    /// ids of those containers, so the caller can say they went too.
    pub(crate) fn remove_widget(&mut self, path: &[usize]) -> Result<Vec<String>, EditError> {
        let Some((&index, parent)) = path.split_last() else {
            return Err(EditError::Refused(
                "the mashup root cannot be removed".into(),
            ));
        };
        children_mut(self.at_mut(parent)).remove(index);
        let (parent, tidied) = self.remove_empty_generated_wrappers(parent.to_vec());
        let around = widget_id(self.at(&parent)).to_string();
        self.reflow_at(&parent, &Reflow::Edit { around: &around });
        Ok(tidied)
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
            Placement::Bottom(target) => self.place_at_bottom(widget, target),
            Placement::Beside {
                anchor,
                side: Side::End,
            } => {
                // A filling widget or a container gets wrappers of its own when something goes
                // beside it, so there is no row to split; refused before anything moves. The row
                // checks in split_at_end need the widget placed, so a refusal there puts the
                // document back as it was.
                let found = self.find(anchor)?;
                let node = self.at(&found.path);
                if type_of(node) == Some("flexcontainer") || fills(node) {
                    return Err(EditError::Refused(format!(
                        "--end: {} is a container or fills its row; --end goes beside a fixed widget, such as a label",
                        found.id
                    )));
                }
                let before = self.clone();
                self.place_beside(widget, anchor, Side::Right)
                    .and_then(|placed| self.split_at_end(placed))
                    .inspect_err(|_| *self = before)
            }
            Placement::Beside { anchor, side } => self.place_beside(widget, anchor, *side),
        }?;
        if widget.type_id == "flexcontainer" {
            // A container someone asked for is a panel or card of their own, not a wrapper
            // twaco made: it gets a name the wrapper rules never take for theirs, and stacks its
            // content, as the kits' cards and panels do.
            if let Ok(found) = self.find(&placed.id) {
                let number = placed.id.rsplit('-').next().unwrap_or_default().to_string();
                self.set_property(
                    &found.path,
                    "DisplayName",
                    Value::from(format!("panel-{number}")),
                );
                self.set_property(&found.path, "flex-direction", Value::from("column"));
            }
        }
        self.keep_footers_last();
        self.fix_rows(&placed);
        self.inherit_backgrounds(&placed);
        Ok(placed)
    }

    /// A footer is its column's last row whatever was placed beside it: a row put below it moves
    /// above it.
    fn keep_footers_last(&mut self) {
        let footers: Vec<WidgetPath> = self
            .widgets()
            .into_iter()
            .filter(|widget| is_footer(self.at(&widget.path)))
            .map(|widget| widget.path)
            .collect();
        // Deepest first, so moving one never shifts the path of another still to move.
        for path in footers.into_iter().rev() {
            let Some((&index, parent)) = path.split_last() else {
                continue;
            };
            let siblings = children_mut(self.at_mut(parent));
            if index + 1 < siblings.len() {
                let footer = siblings.remove(index);
                siblings.push(footer);
            }
        }
    }

    /// A row twaco made inside a container with a background of its own (a template's grey
    /// filter column, a styled panel) shows that background, as the kits' rows do with
    /// `background: inherit`; otherwise the theme paints each row white over it.
    fn inherit_backgrounds(&mut self, placed: &Placed) {
        for id in &placed.containers {
            let Ok(found) = self.find(id) else {
                continue;
            };
            if self.style_background(id).is_some() {
                continue;
            }
            let painted = (0..found.path.len()).rev().any(|depth| {
                let ancestor = widget_id(self.at(&found.path[..depth])).to_string();
                self.style_background(&ancestor)
                    .is_some_and(|background| background != "inherit")
            });
            if !painted {
                continue;
            }
            let properties = self.root_properties_mut();
            if !properties.contains_key("StyleProperties") {
                insert_sorted(
                    properties,
                    "StyleProperties",
                    serde_json::json!({"data": {}, "meta": {"type": "WidgetStyler", "version": "1.0"}}),
                );
            }
            if let Some(data) = properties["StyleProperties"]
                .get_mut("data")
                .and_then(Value::as_object_mut)
            {
                insert_sorted(
                    data,
                    id,
                    serde_json::json!({"parts": {"$": {"$": {"background": "inherit"}}}, "widget": "flex-container"}),
                );
            }
        }
    }

    /// The background a widget's own style entry gives it, if any.
    fn style_background(&self, id: &str) -> Option<String> {
        self.root_properties()
            .get("StyleProperties")?
            .get("data")?
            .get(id)?
            .get("parts")?
            .get("$")?
            .get("$")?
            .get("background")?
            .as_str()
            .filter(|background| !background.is_empty())
            .map(str::to_string)
    }

    /// Re-measure the rows around a widget after one of its properties changes.  This is kept
    /// separate from placement because a property such as `LabelType` changes a label's rendered
    /// height after Composer's initial drop layout has been made.
    /// Only rows that are already fixed grow, and nothing is made to fill: an edit never
    /// reshapes a layout it did not make.
    pub(crate) fn reflow_for(&mut self, id: &str) {
        if let Ok(widget) = self.find(id) {
            self.reflow_at(&widget.path, &Reflow::Edit { around: id });
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
            Reflow::Edit { .. } => None,
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
        let owns = |row: &Value| match scope {
            Reflow::Placement { placed, touched } => {
                touched.iter().any(|id| id == widget_id(row)) || contains(row, placed)
            }
            Reflow::Edit { .. } => false,
        };
        // The row just before the placement's own, when twaco made it, is the one that grew as
        // the column's last row; a row added after it makes it a fixed row again.
        let placed_row = rows.iter().position(|row| match scope {
            Reflow::Placement { placed, .. } => contains(row, placed),
            Reflow::Edit { .. } => false,
        });
        for (index, row) in rows.iter().enumerate() {
            if type_of(row) != Some("flexcontainer") {
                continue;
            }
            // A footer keeps its controls at the column's foot: in a column of controls it is
            // the row that grows, its controls at its bottom; beside a grid or a chart it is
            // fixed to them, so what fills keeps the spare height.
            if is_footer(row) {
                // twaco's own row, re-sized on every reflow: removing the grid beside it lets it
                // grow again.
                let row_path = [column_path.as_slice(), &[index]].concat();
                let beside_filler = rows
                    .iter()
                    .enumerate()
                    .any(|(other, sibling)| other != index && holds_filler(sibling));
                if beside_filler {
                    if let Some(tallest) = fixed_row_height(row, false) {
                        self.fix_row(&row_path, tallest, true);
                    }
                } else if row["Properties"]["flex-size"] == "fixed" {
                    self.unfix_row(&row_path);
                }
                continue;
            }
            // A split label/value row in a card takes its natural height, as the kits' rows do.
            // Beside a row that fills (a grid, a chart) it would share the spare height, so a
            // split toolbar there is fixed to its controls, as the kits' toolbars are.
            if is_split_row(row)
                && !rows
                    .iter()
                    .enumerate()
                    .any(|(other, sibling)| other != index && holds_filler(sibling))
            {
                continue;
            }
            // A row of cards made of label/value rows takes at least its cards' height and no
            // share of the column's spare height, as the kits' card rows (`range`) do; what
            // fills (a chart, a grid) keeps the rest.
            if let Some(tallest) = card_row_height(row) {
                if matches!(scope, Reflow::Placement { .. }) || contains_around(row, scope) {
                    let row_path = [column_path.as_slice(), &[index]].concat();
                    let exact = matches!(scope, Reflow::Placement { .. });
                    self.range_row(&row_path, tallest, exact);
                }
                continue;
            }
            // A panel or card someone added stacks rows and takes its content's height; fixing
            // it while it holds only its title would clip every row added later.
            if row["Properties"]["flex-direction"] == "column" && !is_generated_container(row) {
                continue;
            }
            // Only a placement measures a stack of rows: an edit never re-derives a layout's
            // geometry from it.
            let stack = matches!(scope, Reflow::Placement { .. });
            let Some(tallest) = fixed_row_height(row, stack) else {
                continue;
            };
            let owned = owns(row)
                || (placed_row == Some(index + 1)
                    && is_generated_container(row)
                    && row["Properties"]["flex-size"] == "default");
            // An edit only grows the row holding what changed.
            if let Reflow::Edit { around } = scope {
                if !contains(row, around) {
                    continue;
                }
            }
            let row_path = [column_path.as_slice(), &[index]].concat();
            self.fix_row(&row_path, tallest, owned);
        }

        // Runtime gives every row's bounding box `margin: auto`, so a column of only fixed rows
        // spreads them out over its height. Real mashups always let one row grow; with no data
        // widget to fill, the last control row twaco made is the one, its controls at its top.
        // In a panel too: a panel measured into a fixed row has no spare height for its last row
        // to take, and one placed straight into a growing column must not spread its rows.
        if matches!(scope, Reflow::Placement { .. }) {
            let rows = children(self.at(&column_path)).to_vec();
            let all_fixed = !rows.is_empty()
                && rows.iter().all(|row| {
                    type_of(row) == Some("flexcontainer")
                        && row["Properties"]["flex-size"] == "fixed"
                });
            if let (true, Some(last)) = (all_fixed, rows.last()) {
                if owns(last) && fixed_content_height(last, false).is_some() {
                    let last_path = [column_path.as_slice(), &[rows.len() - 1]].concat();
                    self.unfix_row(&last_path);
                }
            }
        }
    }

    /// Give a row a `range` size whose minimum is `tallest` plus a row's margin. A placement
    /// sets it to what its cards measure now (a row is fixed until `--end` splits it, so an
    /// earlier measure can be too tall); an edit only grows it. A row someone fixed is left alone.
    fn range_row(&mut self, path: &[usize], tallest: f64, exact: bool) {
        let node = self.at(path).clone();
        let properties = &node["Properties"];
        if properties["flex-size"] == "fixed" {
            return;
        }
        let height = (tallest + ROW_MARGIN).ceil();
        let current = properties["flex-min-height"]
            .as_str()
            .and_then(|text| text.strip_suffix("px"))
            .and_then(|number| number.parse::<f64>().ok());
        if properties["flex-size"] == "range"
            && current.is_some_and(|current| current == height || (!exact && current > height))
        {
            return;
        }
        self.set_property(path, "flex-size", Value::from("range"));
        self.set_property(path, "flex-min-height", Value::from(format!("{height}px")));
        self.set_property(path, "flex-max-height", Value::from(""));
        self.set_property(
            path,
            "iconClass",
            Value::from("widgets-flexcontainer-fixed"),
        );
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

    /// Whether a binding, an event or a data section names the widget `id`: such a container
    /// is not an empty wrapper to tidy away, whatever its name.
    fn is_named_outside_layout(&self, id: &str) -> bool {
        let quoted = Value::from(id).to_string();
        ["Data", "DataBindings", "Events"]
            .iter()
            .any(|section| self.value()[*section].to_string().contains(&quoted))
    }

    fn remove_empty_generated_wrappers(
        &mut self,
        mut path: WidgetPath,
    ) -> (WidgetPath, Vec<String>) {
        let mut tidied = Vec::new();
        while !path.is_empty()
            && children(self.at(&path)).is_empty()
            && is_generated_container(self.at(&path))
            && !self.is_named_outside_layout(widget_id(self.at(&path)))
        {
            tidied.push(widget_id(self.at(&path)).to_string());
            let (index, parent) = path.split_last().expect("non-root has a parent");
            children_mut(self.at_mut(parent)).remove(*index);
            path = parent.to_vec();
        }
        (path, tidied)
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
        let target_empty = target_children.is_empty();
        let wrap_column_for_filler = fills_alone(widget)
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
        // A container cannot share a container with bare widgets, so they move into rows of
        // their own first, as they do when a grid joins them.
        let wrap_leaves =
            wrap_column_for_filler || (holds_leaves && widget.type_id == "flexcontainer");
        if wrap_leaves {
            for (index, child) in target_children.into_iter().enumerate() {
                if type_of(&child) != Some("flexcontainer") {
                    let wrapper = self.wrap(child, &mut placed);
                    self.replace_child(&target.path, index, wrapper);
                }
            }
        }
        let node = if wrap_new {
            self.wrap(new, &mut placed)
        } else {
            new
        };
        // A footer stays the column's last row: what is added later goes above it.
        let count = children(self.at(&target.path)).len();
        if count > 0 && is_footer(&children(self.at(&target.path))[count - 1]) {
            self.insert_child(&target.path, count - 1, node);
        } else {
            self.push_child(&target.path, node);
        }
        Ok(placed)
    }

    fn place_at_bottom(&mut self, widget: &Widget, target: &str) -> Result<Placed, EditError> {
        let target = self.find(target)?;
        if target.type_id != "flexcontainer"
            || self.at(&target.path)["Properties"]["flex-direction"] != "column"
        {
            return Err(EditError::Refused(format!(
                "--bottom: {} is not a column; --bottom puts a widget at the foot of a column",
                target.id
            )));
        }
        if let Some(footer) = children(self.at(&target.path))
            .iter()
            .find(|row| is_footer(row))
        {
            let beside = children(footer).last().map_or_else(
                || widget_id(footer).to_string(),
                |w| widget_id(w).to_string(),
            );
            return Err(EditError::Refused(format!(
                "--bottom: {} already has a footer row; put the widget beside what is in it (--right-of {beside})",
                target.id
            )));
        }
        if widget.type_id == "flexcontainer" || fills_alone(widget) {
            return Err(EditError::Refused(
                "--bottom takes a control such as a button; a container or a filling widget goes --in the column".into(),
            ));
        }
        // Rows go beside rows: bare widgets already in the column move into rows of their own.
        let mut placed = Placed::default();
        let existing = children(self.at(&target.path)).to_vec();
        for (index, child) in existing.into_iter().enumerate() {
            if type_of(&child) != Some("flexcontainer") {
                let wrapper = self.wrap(child, &mut placed);
                self.replace_child(&target.path, index, wrapper);
            }
        }
        let number = self.allocate();
        placed.id = format!("{}-{number}", widget.type_id);
        let new = new_widget(widget, number, false);
        let mut footer = self.wrap(new, &mut placed);
        let footer_number = widget_id(&footer)
            .rsplit('-')
            .next()
            .unwrap_or_default()
            .to_string();
        let properties = properties_mut(&mut footer);
        insert_sorted(
            properties,
            "DisplayName",
            Value::from(format!("container-footer-{footer_number}")),
        );
        insert_sorted(properties, "flex-direction", Value::from("row"));
        insert_sorted(properties, "align-items", Value::from("flex-end"));
        insert_sorted(properties, "justify-content", Value::from("flex-start"));
        self.push_child(&target.path, footer);
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
                || fixed_content_height(node, false).is_none()
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
        let filler = fills_alone(widget) && !new_is_container;
        let axis_matches = direction.as_deref() == Some(side.axis());
        // A lone widget's container turns to the new direction, except a panel someone added:
        // it keeps stacking its content, and the pair gets a row of its own inside it.
        let may_turn = parent_children.len() == 1 && !is_panel(parent);
        let sibling_possible = !fills(self.at(&anchor.path))
            && (!filler || (anchor_is_container && axis_matches))
            && (axis_matches || may_turn);

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

    /// Turn a container just placed between rows into the kits' separator: a 1px fixed row with
    /// nothing in it (its colour is a root style entry). Reflow measures no height for an empty
    /// row, so it stays 1px.
    pub(crate) fn make_separator(&mut self, id: &str) -> Result<(), EditError> {
        let path = self.find(id)?.path;
        let number = id.rsplit('-').next().unwrap_or_default().to_string();
        for (key, value) in [
            ("DisplayName", format!("separator-{number}")),
            ("flex-direction", "row".to_string()),
            ("flex-size", "fixed".to_string()),
            ("flex-min-height", "1px".to_string()),
            ("flex-max-height", "1px".to_string()),
            ("iconClass", "widgets-flexcontainer-fixed".to_string()),
        ] {
            self.set_property(&path, key, Value::from(value));
        }
        Ok(())
    }

    /// Split the row the new widget joined into a start group (what was there) and an end group
    /// (the new widget), spread apart and centred, as the kits' label/value rows are. The row
    /// keeps its natural height: reflow leaves a split row alone.
    fn split_at_end(&mut self, mut placed: Placed) -> Result<Placed, EditError> {
        let new = self.find(&placed.id)?;
        let refuse = |why: &str| Err(EditError::Refused(format!("--end: {why}")));
        let Some((&index, parent_path)) = new.path.split_last() else {
            return refuse("the widget has no row");
        };
        let parent_path = parent_path.to_vec();
        let row = self.at(&parent_path).clone();
        if !is_generated_container(&row) || row["Properties"]["flex-direction"] != "row" {
            return refuse("the anchor is not in a row twaco made; place it --right-of instead");
        }
        if let Some((_, grand)) = parent_path.split_last() {
            if !grand.is_empty() && is_split_row(self.at(grand)) {
                return refuse(
                    "the row is split already; place it --right-of a widget of its end group",
                );
            }
        }
        let items = children(&row).to_vec();
        if index + 1 != items.len() || items.len() < 2 {
            return refuse("the anchor must be the last widget of its row");
        }
        let group = |document: &mut Self, justify: &str, members: &[Value]| {
            let mut group = new_widget(
                widgets::catalog()
                    .by_type("flexcontainer")
                    .expect("the catalogue has flexcontainer"),
                document.allocate(),
                true,
            );
            set_value_property(&mut group, "flex-direction", Value::from("row"));
            set_value_property(&mut group, "justify-content", Value::from(justify));
            set_value_property(&mut group, "align-items", Value::from("center"));
            for member in members {
                push_value_child(&mut group, member.clone());
            }
            group
        };
        let start = group(self, "flex-start", &items[..index]);
        let end = group(self, "flex-end", &items[index..]);
        placed.containers.push(widget_id(&start).to_string());
        placed.containers.push(widget_id(&end).to_string());
        let node = self.at_mut(&parent_path);
        children_mut(node).clear();
        children_mut(node).push(start);
        children_mut(node).push(end);
        self.set_property(
            &parent_path,
            "justify-content",
            Value::from("space-between"),
        );
        self.set_property(&parent_path, "align-items", Value::from("center"));
        self.unfix_row(&parent_path);
        Ok(placed)
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

        // Two plain widgets side by side in a panel: one row inside the panel holds both, as a
        // kit toolbar does, and the panel keeps stacking its content.
        if is_panel(self.at(parent_path))
            && !anchor_is_container
            && !new_is_container
            && !fills(&anchor_value)
            && !fills_alone(widget)
        {
            let wrapper_number = self.allocate();
            let mut row = new_widget(
                widgets::catalog()
                    .by_type("flexcontainer")
                    .expect("the catalogue has flexcontainer"),
                wrapper_number,
                true,
            );
            placed.containers.push(widget_id(&row).to_string());
            set_value_property(&mut row, "flex-direction", Value::from(side.axis()));
            let new = new_widget(widget, number, false);
            let (first, second) = if side.before() {
                (new, anchor_value)
            } else {
                (anchor_value, new)
            };
            push_value_child(&mut row, first);
            push_value_child(&mut row, second);
            self.replace_child(parent_path, anchor_index, row);
            // A container cannot share the panel with bare widgets: the others get rows too.
            let siblings = children(self.at(parent_path)).to_vec();
            for (index, child) in siblings.into_iter().enumerate() {
                if index != anchor_index && type_of(&child) != Some("flexcontainer") {
                    placed.wrapped.push(widget_id(&child).to_string());
                    let wrapped = self.wrap(child, &mut placed);
                    self.replace_child(parent_path, index, wrapped);
                }
            }
            return placed;
        }

        // The anchor alone in its container (a widget that fills it): that container becomes the
        // row or column itself, holding the anchor and the new widget each in a container of
        // their own, rather than gaining one more level that holds a single container.
        if children(self.at(parent_path)).len() == 1 && !is_panel(self.at(parent_path)) {
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
            Value::from(fills_alone(widget) && alone),
        );
        if fills_alone(widget) && alone {
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
    /// `around` is the widget edited, or the container a removed widget left.
    Edit { around: &'a str },
}

fn contains(widget: &Value, id: &str) -> bool {
    widget_id(widget) == id || children(widget).iter().any(|child| contains(child, id))
}

/// Whether a widget fills a container it is alone in. The catalogue marks a shape resizable, but
/// the kits use shapes as fixed marks (a 12px status dot beside a label), never as fillers.
fn fills_alone(widget: &Widget) -> bool {
    widget.auto_resize && widget.type_id != "shape"
}

fn can_fill(widget: &Value) -> bool {
    type_of(widget)
        .and_then(|type_id| widgets::catalog().by_type(type_id))
        .is_some_and(fills_alone)
}

fn row_fills(row: &Value) -> bool {
    fills(row) || children(row).iter().any(fills)
}

/// A footer row `--bottom` made: it stays the last row of its column. The name is twaco's own
/// (Composer numbers its containers), so a row someone called `footer-actions` is never taken
/// for one; as a generated container it is tidied away when its last widget goes.
fn is_footer(widget: &Value) -> bool {
    type_of(widget) == Some("flexcontainer")
        && widget["Properties"]["DisplayName"]
            .as_str()
            .is_some_and(|name| name.starts_with("container-footer-"))
}

/// Whether a widget that fills sits anywhere inside `widget`, or is it.
fn holds_filler(widget: &Value) -> bool {
    fills(widget) || children(widget).iter().any(holds_filler)
}

/// A fixed row may contain fixed leaves directly, or the per-widget wrappers made for a toolbar.
/// `stack` measures a nested column of several rows as their sum; without it such a column is
/// left unmeasured.
fn fixed_row_height(row: &Value, stack: bool) -> Option<f64> {
    let heights = children(row)
        .iter()
        .map(|child| fixed_content_height(child, stack))
        .collect::<Option<Vec<_>>>()?;
    (!heights.is_empty()).then(|| heights.into_iter().fold(0.0, f64::max))
}

fn fixed_content_height(widget: &Value, stack: bool) -> Option<f64> {
    if type_of(widget) != Some("flexcontainer") {
        return (!fills(widget)).then(|| content_height(widget)).flatten();
    }
    // A column stacks its children, and their margins and gaps are not known here: leave a
    // nested column of several widgets unmeasured rather than clip it to its tallest child.
    if widget["Properties"]["flex-direction"] == "column" && children(widget).len() > 1 {
        if !stack {
            return None;
        }
        // Rows fixed to a known height stack to their sum, the last one counted by its content
        // when it is the growing control row a column keeps; anything else is not measured.
        let rows = children(widget);
        return rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                fixed_row_px(row).or_else(|| {
                    (index + 1 == rows.len() && row["Properties"]["flex-size"] == "default")
                        .then(|| fixed_row_height(row, false))
                        .flatten()
                        .map(|tallest| (tallest + ROW_MARGIN).ceil())
                })
            })
            .collect::<Option<Vec<f64>>>()
            .map(|heights| heights.into_iter().sum());
    }
    let heights = children(widget)
        .iter()
        .map(|child| fixed_content_height(child, stack))
        .collect::<Option<Vec<_>>>()?;
    (!heights.is_empty()).then(|| heights.into_iter().fold(0.0, f64::max))
}

/// The height of a row fixed to a pixel size, as twaco or Composer fixed it.
fn fixed_row_px(row: &Value) -> Option<f64> {
    let properties = &row["Properties"];
    if type_of(row) != Some("flexcontainer") || properties["flex-size"] != "fixed" {
        return None;
    }
    properties["flex-min-height"]
        .as_str()?
        .strip_suffix("px")?
        .parse()
        .ok()
}

/// A container someone added with `mashup add container`: a card or panel of their own.
fn is_panel(widget: &Value) -> bool {
    type_of(widget) == Some("flexcontainer")
        && widget["Properties"]["DisplayName"]
            .as_str()
            .is_some_and(|name| name.starts_with("panel-"))
}

/// Whether an edit's widget is inside `row`; a placement reflow is not limited this way.
fn contains_around(row: &Value, scope: &Reflow) -> bool {
    match scope {
        Reflow::Edit { around } => contains(row, around),
        Reflow::Placement { .. } => true,
    }
}

/// How tall a label/value row renders: its tallest widget, each as measured on 10.2 (a
/// `label` 19px, a `sub-header` 27px, a `header` 33px) or by its own `Height`, with its
/// vertical margins. `None` when a widget's height is not known.
fn natural_height(widget: &Value) -> Option<f64> {
    if type_of(widget) == Some("flexcontainer") {
        let heights = children(widget)
            .iter()
            .map(natural_height)
            .collect::<Option<Vec<_>>>()?;
        return Some(heights.into_iter().fold(0.0, f64::max));
    }
    let properties = &widget["Properties"];
    let own = properties["Height"].as_f64();
    let height = match (own, type_of(widget)?) {
        (Some(own), _) => own,
        (None, "ptcslabel") => match properties["LabelType"].as_str() {
            Some("header") => 33.0,
            Some("sub-header") => 27.0,
            Some("label" | "body") | None => 19.0,
            Some(_) => return None,
        },
        _ => return None,
    };
    Some(height + vertical_margin(properties["Margin"].as_str().unwrap_or("")))
}

/// The height of a card someone added that holds label/value rows: its fixed rows and
/// separators at their size, its split rows at their natural height. `None` for any other
/// panel, so ordinary panels keep growing as rows are added to them.
fn card_height(panel: &Value) -> Option<f64> {
    if !is_panel(panel) || !children(panel).iter().any(is_split_row) {
        return None;
    }
    children(panel)
        .iter()
        .map(|row| {
            if is_split_row(row) {
                natural_height(row)
            } else {
                fixed_row_px(row)
            }
        })
        .sum()
}

/// The tallest card of a generated row of cards, when every child is such a card.
fn card_row_height(row: &Value) -> Option<f64> {
    if !is_generated_container(row) || row["Properties"]["flex-direction"] != "row" {
        return None;
    }
    let heights = children(row)
        .iter()
        .map(card_height)
        .collect::<Option<Vec<_>>>()?;
    (!heights.is_empty()).then(|| heights.into_iter().fold(0.0, f64::max))
}

/// A row `--end` split: a generated row spreading two generated groups apart.
fn is_split_row(widget: &Value) -> bool {
    is_generated_container(widget)
        && widget["Properties"]["flex-direction"] == "row"
        && widget["Properties"]["justify-content"] == "space-between"
        && children(widget).len() == 2
        && children(widget).iter().all(is_generated_container)
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
    fn an_end_placement_refused_by_its_row_checks_leaves_the_document_as_it_was() {
        // A row someone named is not one twaco made: --end places, then refuses to split it.
        let mut document = document(
            &container(
                "flexcontainer-2",
                "page",
                "column",
                &container(
                    "flexcontainer-3",
                    "Toolbar",
                    "row",
                    &format!(
                        "{},{}",
                        widget("ptcslabel", "ptcslabel-4", "first", ""),
                        widget("ptcslabel", "ptcslabel-5", "second", "")
                    ),
                ),
            ),
            6,
        );
        let before = document.clone();
        let refused = document.place(
            catalogue_widget("ptcslabel"),
            &Placement::Beside {
                anchor: "ptcslabel-5".into(),
                side: Side::End,
            },
        );
        assert!(
            matches!(&refused, Err(EditError::Refused(why)) if why.contains("--end")),
            "{refused:?}"
        );
        assert_eq!(document, before);
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
    fn placing_in_a_leaf_is_refused_and_a_container_moves_leaves_into_rows() {
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
        let panel = document
            .place(
                catalogue_widget("flexcontainer"),
                &Placement::In("flexcontainer-2".into()),
            )
            .unwrap();
        let kids = children(document.at(&document.find("flexcontainer-2").unwrap().path)).to_vec();
        assert_eq!(kids.len(), 3);
        assert!(kids.iter().all(|kid| type_of(kid) == Some("flexcontainer")));
        assert_eq!(child_ids(&document, widget_id(&kids[0])), ["ptcslabel-3"]);
        // The container someone asked for is a column panel, never taken for a wrapper.
        let panel = document.at(&document.find(&panel.id).unwrap().path).clone();
        assert_eq!(panel["Properties"]["flex-direction"], "column");
        assert!(panel["Properties"]["DisplayName"]
            .as_str()
            .unwrap()
            .starts_with("panel-"));
        assert!(!is_generated_container(&panel));
    }

    #[test]
    fn a_panel_keeps_growing_as_rows_are_added_to_it() {
        let column = container("flexcontainer-2", "content", "column", "");
        let mut document = document(&column, 2);
        let panel = document
            .place(
                catalogue_widget("flexcontainer"),
                &Placement::In("flexcontainer-2".into()),
            )
            .unwrap()
            .id;
        let title = document
            .place(catalogue_widget("ptcslabel"), &Placement::In(panel.clone()))
            .unwrap()
            .id;
        document
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::Beside {
                    anchor: title,
                    side: Side::Below,
                },
            )
            .unwrap();
        let found = document.find(&panel).unwrap();
        assert_ne!(document.at(&found.path)["Properties"]["flex-size"], "fixed");
    }

    #[test]
    fn a_panel_keeps_its_column_when_its_lone_widget_gets_a_neighbour() {
        let column = container("flexcontainer-2", "content", "column", "");
        let mut document = document(&column, 2);
        let panel = document
            .place(
                catalogue_widget("flexcontainer"),
                &Placement::In("flexcontainer-2".into()),
            )
            .unwrap()
            .id;
        let first = document
            .place(
                catalogue_widget("ptcsdatepicker"),
                &Placement::In(panel.clone()),
            )
            .unwrap()
            .id;
        let second = document
            .place(
                catalogue_widget("ptcsdatepicker"),
                &Placement::Beside {
                    anchor: first.clone(),
                    side: Side::Right,
                },
            )
            .unwrap()
            .id;
        let found = document.find(&panel).unwrap();
        assert_eq!(
            document.at(&found.path)["Properties"]["flex-direction"],
            "column"
        );
        let row_of = |id: &str| {
            let path = document.find(id).unwrap().path;
            widget_id(document.at(&path[..path.len() - 1])).to_string()
        };
        assert_eq!(row_of(&first), row_of(&second));
        assert_ne!(row_of(&first), panel);
    }

    #[test]
    fn a_neighbour_in_a_panel_of_several_widgets_leaves_no_bare_widget_beside_a_row() {
        let first = widget("ptcslabel", "ptcslabel-4", "A", "");
        let second = widget("ptcslabel", "ptcslabel-5", "B", "");
        let panel = container(
            "flexcontainer-3",
            "panel-3",
            "column",
            &format!("{first},{second}"),
        );
        let column = container("flexcontainer-2", "content", "column", &panel);
        let mut document = document(&column, 5);
        document
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::Beside {
                    anchor: "ptcslabel-4".into(),
                    side: Side::Right,
                },
            )
            .unwrap();
        let panel = document.find("flexcontainer-3").unwrap();
        assert!(children(document.at(&panel.path))
            .iter()
            .all(|child| type_of(child) == Some("flexcontainer")));
        assert!(document.problems().is_empty(), "{:?}", document.problems());
    }

    #[test]
    fn a_panel_in_a_growing_column_keeps_a_growing_last_row() {
        let label = widget("ptcslabel", "ptcslabel-4", "Title", "");
        let panel = container("flexcontainer-3", "panel-3", "column", &label);
        let column = container("flexcontainer-2", "content", "column", &panel);
        let mut document = document(&column, 4);
        document
            .place(
                catalogue_widget("ptcslabel"),
                &Placement::Beside {
                    anchor: "ptcslabel-4".into(),
                    side: Side::Below,
                },
            )
            .unwrap();
        let panel = document.find("flexcontainer-3").unwrap();
        let sizes: Vec<String> = children(document.at(&panel.path))
            .iter()
            .map(|row| {
                row["Properties"]["flex-size"]
                    .as_str()
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        assert_eq!(
            sizes,
            ["fixed", "default"],
            "Runtime would spread an all-fixed panel"
        );
    }

    #[test]
    fn an_edit_inside_a_composer_stack_of_fixed_rows_changes_no_size() {
        let label = |id: &str| {
            format!(
                r#"{{"Properties":{{"DisplayName":"{id}","Height":20,"Id":"{id}","Type":"ptcslabel"}},"Widgets":[]}}"#
            )
        };
        let top = fixed(
            &container(
                "flexcontainer-5",
                "container-5",
                "row",
                &label("ptcslabel-6"),
            ),
            "80px",
        );
        let bottom = fixed(
            &container(
                "flexcontainer-7",
                "container-7",
                "row",
                &label("ptcslabel-8"),
            ),
            "80px",
        );
        let stack = container(
            "flexcontainer-4",
            "container-4",
            "column",
            &format!("{top},{bottom}"),
        );
        let outer = fixed(
            &container("flexcontainer-3", "container-3", "row", &stack),
            "160px",
        );
        let column = container("flexcontainer-2", "content", "column", &outer);
        let mut document = document(&column, 8);
        let before = document.clone();
        let found = document.find("ptcslabel-6").unwrap();
        document.set_property(&found.path, "LabelText", Value::from("Changed"));
        document.reflow_for("ptcslabel-6");
        let outer = document.find("flexcontainer-3").unwrap();
        assert_eq!(
            document.at(&outer.path)["Properties"],
            before.at(&before.find("flexcontainer-3").unwrap().path)["Properties"]
        );
    }

    #[test]
    fn a_shape_is_a_fixed_mark_not_a_filler() {
        let column = container("flexcontainer-2", "content", "column", "");
        let mut document = document(&column, 2);
        let shape = document
            .place(
                catalogue_widget("shape"),
                &Placement::In("flexcontainer-2".into()),
            )
            .unwrap();
        let found = document.find(&shape.id).unwrap();
        assert_eq!(
            document.at(&found.path)["Properties"]["ResponsiveLayout"],
            false
        );
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

    fn fixed(container: &str, height: &str) -> String {
        container.replacen(
            r#""Type":"flexcontainer""#,
            &format!(
                r#""Type":"flexcontainer","flex-size":"fixed","flex-min-height":"{height}","flex-max-height":"{height}""#
            ),
            1,
        )
    }

    #[test]
    fn removing_a_widget_keeps_an_empty_container_a_binding_names() {
        let button = widget("ptcsbutton", "ptcsbutton-4", "Clear", "");
        let row = container("flexcontainer-3", "container-3", "row", &button);
        let column = container("flexcontainer-2", "content", "column", &row);
        let mut document = document(&column, 4);
        document
            .bindings_mut()
            .push(json!({"Id": "b", "TargetId": "flexcontainer-3", "TargetArea": "UI"}));
        let button = document.find("ptcsbutton-4").unwrap();
        document.remove_widget(&button.path).unwrap();
        assert!(document.find("flexcontainer-3").is_ok());
    }

    #[test]
    fn an_edit_leaves_a_fixed_composer_sibling_row_alone() {
        let title = widget("ptcslabel", "ptcslabel-3", "Title", "");
        let header = container("flexcontainer-3", "Header", "row", &title);
        let note = widget("ptcslabel", "ptcslabel-4", "Note", "");
        let footer = fixed(
            &container("flexcontainer-5", "Footer", "row", &note),
            "40px",
        );
        let column = container(
            "flexcontainer-2",
            "content",
            "column",
            &format!("{header},{footer}"),
        );
        let mut document = document(&column, 5);
        let before = document.clone();
        let title = document.find("ptcslabel-3").unwrap();
        document.set_property(&title.path, "LabelText", Value::from("Orders"));
        document.reflow_for("ptcslabel-3");
        let footer = document.find("flexcontainer-5").unwrap();
        assert_eq!(
            document.at(&footer.path),
            before.at(&before.find("flexcontainer-5").unwrap().path)
        );
    }

    #[test]
    fn a_placement_leaves_an_unrelated_default_named_row_alone() {
        let label = widget("ptcslabel", "ptcslabel-4", "Note", "");
        let other = container("flexcontainer-3", "container-3", "row", &label);
        let button = widget("ptcsbutton", "ptcsbutton-6", "Go", "");
        let toolbar = container("flexcontainer-5", "Toolbar", "row", &button);
        let grid = filling_widget("ptcsgrid", "ptcsgrid-8", "Orders");
        let body = container("flexcontainer-7", "Body", "row", &grid);
        let column = container(
            "flexcontainer-2",
            "content",
            "column",
            &format!("{other},{toolbar},{body}"),
        );
        let mut document = document(&column, 8);
        let before = document.clone();
        document
            .place(
                catalogue_widget("ptcsbutton"),
                &Placement::In("flexcontainer-5".into()),
            )
            .unwrap();
        let other = document.find("flexcontainer-3").unwrap();
        assert_eq!(
            document.at(&other.path),
            before.at(&before.find("flexcontainer-3").unwrap().path)
        );
    }

    #[test]
    fn a_nested_column_of_controls_is_not_clipped_to_its_tallest_child() {
        let first = widget("ptcstextfield", "ptcstextfield-5", "A", "");
        let second = widget("ptcstextfield", "ptcstextfield-6", "B", "");
        let stack = container(
            "flexcontainer-4",
            "Stack",
            "column",
            &format!("{first},{second}"),
        );
        let row = container("flexcontainer-3", "Fields", "row", &stack);
        let grid = filling_widget("ptcsgrid", "ptcsgrid-8", "Orders");
        let body = container("flexcontainer-7", "Body", "row", &grid);
        let column = container(
            "flexcontainer-2",
            "content",
            "column",
            &format!("{row},{body}"),
        );
        let mut document = document(&column, 8);
        document
            .place(
                catalogue_widget("ptcsbutton"),
                &Placement::In("flexcontainer-4".into()),
            )
            .unwrap();
        let row = document.find("flexcontainer-3").unwrap();
        assert_ne!(document.at(&row.path)["Properties"]["flex-size"], "fixed");
    }

    #[test]
    fn removing_a_widget_drops_its_empty_generated_cell() {
        let button = widget("ptcsbutton", "ptcsbutton-5", "Clear", "");
        let cell = container("flexcontainer-4", "container-4", "row", &button);
        let row = container("flexcontainer-3", "container-3", "row", &cell);
        let column = container("flexcontainer-2", "content", "column", &row);
        let mut document = document(&column, 5);
        let button = document.find("ptcsbutton-5").unwrap();
        let tidied = document.remove_widget(&button.path).unwrap();
        assert_eq!(tidied, ["flexcontainer-4", "flexcontainer-3"]);
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

    /// A card as the kits draw it: a panel, a title, then one dot-label-value row.
    fn card_with_row(document: &mut MashupDocument) -> (String, String, String) {
        let panel = document
            .place(
                widgets::catalog().by_type("flexcontainer").unwrap(),
                &Placement::In("flexcontainer-2".into()),
            )
            .unwrap()
            .id;
        let title = document
            .place(
                widgets::catalog().by_type("ptcslabel").unwrap(),
                &Placement::In(panel.clone()),
            )
            .unwrap()
            .id;
        let dot = document
            .place(
                widgets::catalog().by_type("shape").unwrap(),
                &Placement::Beside {
                    anchor: title,
                    side: Side::Below,
                },
            )
            .unwrap()
            .id;
        let name = document
            .place(
                widgets::catalog().by_type("ptcslabel").unwrap(),
                &Placement::Beside {
                    anchor: dot.clone(),
                    side: Side::Right,
                },
            )
            .unwrap()
            .id;
        (panel, dot, name)
    }

    fn parent_of<'a>(document: &'a MashupDocument, id: &str) -> &'a Value {
        let path = document.find(id).unwrap().path;
        document.at(&path[..path.len() - 1])
    }

    #[test]
    fn end_splits_the_row_into_a_start_and_an_end_group_of_natural_height() {
        let mut document = parse(crate::core::mashup::templates::BLANK).unwrap();
        let (panel, dot, name) = card_with_row(&mut document);
        let value = document
            .place(
                widgets::catalog().by_type("ptcslabel").unwrap(),
                &Placement::Beside {
                    anchor: name.clone(),
                    side: Side::End,
                },
            )
            .unwrap();
        assert!(value.containers.len() >= 2, "{value:?}");
        let start = parent_of(&document, &dot).clone();
        let end = parent_of(&document, &value.id).clone();
        assert_eq!(
            children(&start)
                .iter()
                .map(|w| widget_id(w).to_string())
                .collect::<Vec<_>>(),
            [dot.clone(), name.clone()]
        );
        assert_eq!(start["Properties"]["justify-content"], "flex-start");
        assert_eq!(end["Properties"]["justify-content"], "flex-end");
        assert_eq!(start["Properties"]["align-items"], "center");
        let row = parent_of(&document, widget_id(&start)).clone();
        assert!(is_split_row(&row), "{row}");
        assert_eq!(row["Properties"]["align-items"], "center");
        assert_eq!(row["Properties"]["flex-size"], "default");
        assert_eq!(row["Properties"]["flex-min-height"], "");
        // The row stays natural when the card grows below it.
        document
            .place(
                widgets::catalog().by_type("ptcslabel").unwrap(),
                &Placement::Beside {
                    anchor: dot,
                    side: Side::Below,
                },
            )
            .unwrap();
        let row_path = document.find(widget_id(&row)).unwrap().path;
        assert_eq!(document.at(&row_path)["Properties"]["flex-size"], "default");
        assert!(document.find(&panel).is_ok());
    }

    /// A list page's toolbar split by `--end` above a grid is fixed to its buttons, as the kits'
    /// toolbars are; left at its natural size it took half the page's height beside the grid.
    #[test]
    fn a_split_toolbar_above_a_grid_is_fixed_to_its_buttons() {
        let catalog = widgets::catalog();
        let place = |document: &mut MashupDocument, widget: &str, placement: Placement| {
            document
                .place(catalog.resolve(widget).unwrap(), &placement)
                .unwrap()
                .id
        };
        // Toolbar first, then the grid below it, and the other way round.
        for grid_first in [false, true] {
            let mut document = parse(crate::core::mashup::templates::BLANK).unwrap();
            let title = place(&mut document, "label", Placement::In("center".into()));
            let (view, grid);
            if grid_first {
                grid = place(&mut document, "grid", Placement::In("center".into()));
                view = place(
                    &mut document,
                    "button",
                    Placement::Beside {
                        anchor: grid.clone(),
                        side: Side::Above,
                    },
                );
            } else {
                view = place(
                    &mut document,
                    "button",
                    Placement::Beside {
                        anchor: title,
                        side: Side::Below,
                    },
                );
                grid = place(
                    &mut document,
                    "grid",
                    Placement::Beside {
                        anchor: view.clone(),
                        side: Side::Below,
                    },
                );
            }
            place(
                &mut document,
                "button",
                Placement::Beside {
                    anchor: view.clone(),
                    side: Side::End,
                },
            );
            let start = parent_of(&document, &view).clone();
            let row = parent_of(&document, widget_id(&start)).clone();
            assert!(is_split_row(&row), "{row}");
            assert_eq!(
                row["Properties"]["flex-size"], "fixed",
                "grid first: {grid_first}"
            );
            let grid_path = document.find(&grid).unwrap().path;
            assert_eq!(
                document.at(&grid_path)["Properties"]["ResponsiveLayout"],
                true
            );
        }
    }

    /// `--bottom` keeps a filter panel's buttons at its foot, as the kits do: a footer row that
    /// grows with its controls at its bottom, stays last as widgets are added, and is fixed to
    /// its controls beside a grid.
    #[test]
    fn bottom_puts_buttons_in_a_footer_that_stays_last() {
        let catalog = widgets::catalog();
        let place = |document: &mut MashupDocument, widget: &str, placement: Placement| {
            document.place(catalog.resolve(widget).unwrap(), &placement)
        };
        let mut document = parse(crate::core::mashup::templates::BLANK).unwrap();
        place(&mut document, "dropdown", Placement::In("center".into())).unwrap();
        let apply = place(&mut document, "button", Placement::Bottom("center".into()))
            .unwrap()
            .id;
        let clear = place(
            &mut document,
            "button",
            Placement::Beside {
                anchor: apply.clone(),
                side: Side::Right,
            },
        )
        .unwrap()
        .id;
        let later = place(&mut document, "dropdown", Placement::In("center".into()))
            .unwrap()
            .id;
        let center = document.find("center").unwrap();
        let rows = children(document.at(&center.path)).to_vec();
        let footer = rows.last().unwrap();
        assert!(is_footer(footer), "{footer}");
        assert!(
            contains(footer, &apply) && contains(footer, &clear),
            "{footer}"
        );
        assert!(!contains(footer, &later));
        assert_eq!(footer["Properties"]["align-items"], "flex-end");
        assert_ne!(
            footer["Properties"]["flex-size"], "fixed",
            "it is the row that grows"
        );
        for row in &rows[..rows.len() - 1] {
            assert_eq!(row["Properties"]["flex-size"], "fixed", "{row}");
        }
        // A second footer is refused, naming where the widget can go instead.
        let refused = place(&mut document, "button", Placement::Bottom("center".into()));
        assert!(
            matches!(&refused, Err(EditError::Refused(why)) if why.contains("already has a footer")),
            "{refused:?}"
        );
        // Not a column, a filling widget: refused.
        let footer_id = widget_id(footer).to_string();
        assert!(place(&mut document, "button", Placement::Bottom(footer_id)).is_err());
        let mut other = parse(crate::core::mashup::templates::BLANK).unwrap();
        assert!(place(&mut other, "grid", Placement::Bottom("center".into())).is_err());

        // Beside a grid the footer is fixed, so the grid keeps the spare height.
        let mut list = parse(crate::core::mashup::templates::BLANK).unwrap();
        let grid = place(&mut list, "grid", Placement::In("center".into()))
            .unwrap()
            .id;
        place(&mut list, "button", Placement::Bottom("center".into())).unwrap();
        let center = list.find("center").unwrap();
        let footer = children(list.at(&center.path)).last().unwrap().clone();
        assert!(is_footer(&footer));
        assert_eq!(footer["Properties"]["flex-size"], "fixed");
        let grid_path = list.find(&grid).unwrap().path;
        assert_eq!(list.at(&grid_path)["Properties"]["ResponsiveLayout"], true);
    }

    /// The footer is twaco's own row: one someone named `footer-actions` keeps its size; a row
    /// placed below the footer goes above it; removing the grid lets it grow again; removing its
    /// last widget tidies it away, so `--bottom` works again.
    #[test]
    fn a_footer_stays_last_follows_removals_and_never_takes_another_row() {
        let catalog = widgets::catalog();
        let place = |document: &mut MashupDocument, widget: &str, placement: Placement| {
            document
                .place(catalog.resolve(widget).unwrap(), &placement)
                .unwrap()
                .id
        };
        let footer_of = |document: &MashupDocument| {
            let center = document.find("center").unwrap();
            children(document.at(&center.path))
                .iter()
                .find(|row| is_footer(row))
                .cloned()
        };

        // Someone's own fixed `footer-actions` row in a column of controls keeps its height.
        let mut document = parse(crate::core::mashup::templates::BLANK).unwrap();
        let theirs = place(
            &mut document,
            "flexcontainer",
            Placement::In("center".into()),
        );
        let found = document.find(&theirs).unwrap();
        for (key, value) in [
            ("DisplayName", "footer-actions"),
            ("flex-size", "fixed"),
            ("flex-min-height", "60px"),
            ("flex-max-height", "60px"),
        ] {
            document.set_property(&found.path, key, Value::from(value));
        }
        place(&mut document, "dropdown", Placement::In("center".into()));
        let found = document.find(&theirs).unwrap();
        assert_eq!(document.at(&found.path)["Properties"]["flex-size"], "fixed");
        assert!(footer_of(&document).is_none());

        // A row placed below the footer's button moves above the footer.
        let mut document = parse(crate::core::mashup::templates::BLANK).unwrap();
        let grid = place(&mut document, "grid", Placement::In("center".into()));
        let apply = place(&mut document, "button", Placement::Bottom("center".into()));
        let below = place(
            &mut document,
            "label",
            Placement::Beside {
                anchor: apply.clone(),
                side: Side::Below,
            },
        );
        let center = document.find("center").unwrap();
        let rows = children(document.at(&center.path)).to_vec();
        assert!(is_footer(rows.last().unwrap()), "{rows:?}");
        assert!(!contains(rows.last().unwrap(), &below));
        assert_eq!(
            footer_of(&document).unwrap()["Properties"]["flex-size"],
            "fixed"
        );

        // With the grid gone, the footer grows again, its button at the column's foot.
        let grid_path = document.find(&grid).unwrap().path;
        document.remove_widget(&grid_path).unwrap();
        assert_ne!(
            footer_of(&document).unwrap()["Properties"]["flex-size"],
            "fixed"
        );

        // Its last widget gone, the footer goes too, and a new one can be made.
        let apply_path = document.find(&apply).unwrap().path;
        let tidied = document.remove_widget(&apply_path).unwrap();
        assert!(footer_of(&document).is_none(), "tidied {tidied:?}");
        place(&mut document, "button", Placement::Bottom("center".into()));
        assert!(footer_of(&document).is_some());
    }

    #[test]
    fn end_refuses_a_row_it_cannot_split() {
        let mut document = parse(crate::core::mashup::templates::BLANK).unwrap();
        let (_, dot, name) = card_with_row(&mut document);
        // Not the last widget of its row.
        assert!(matches!(
            document.place(
                widgets::catalog().by_type("ptcslabel").unwrap(),
                &Placement::Beside {
                    anchor: dot,
                    side: Side::End,
                },
            ),
            Err(EditError::Refused(_))
        ));
        let value = document
            .place(
                widgets::catalog().by_type("ptcslabel").unwrap(),
                &Placement::Beside {
                    anchor: name,
                    side: Side::End,
                },
            )
            .unwrap()
            .id;
        // Split already.
        assert!(matches!(
            document.place(
                widgets::catalog().by_type("ptcslabel").unwrap(),
                &Placement::Beside {
                    anchor: value,
                    side: Side::End,
                },
            ),
            Err(EditError::Refused(_))
        ));
    }

    #[test]
    fn a_separator_is_an_empty_fixed_one_pixel_row_reflow_leaves_alone() {
        let mut document = parse(crate::core::mashup::templates::BLANK).unwrap();
        let (panel, dot, _) = card_with_row(&mut document);
        let separator = document
            .place(
                widgets::catalog().by_type("flexcontainer").unwrap(),
                &Placement::Beside {
                    anchor: dot.clone(),
                    side: Side::Below,
                },
            )
            .unwrap()
            .id;
        document.make_separator(&separator).unwrap();
        let found = document.find(&separator).unwrap();
        // It sits in the card, between rows, not inside the dot's row.
        assert_eq!(widget_id(parent_of(&document, &separator)), panel);
        let properties = &document.at(&found.path)["Properties"];
        assert_eq!(properties["flex-size"], "fixed");
        assert_eq!(properties["flex-min-height"], "1px");
        assert_eq!(properties["flex-max-height"], "1px");
        assert_eq!(properties["flex-direction"], "row");
        assert!(children(document.at(&found.path)).is_empty());
        // A row added below it does not re-measure it.
        document
            .place(
                widgets::catalog().by_type("ptcslabel").unwrap(),
                &Placement::Beside {
                    anchor: separator.clone(),
                    side: Side::Below,
                },
            )
            .unwrap();
        let found = document.find(&separator).unwrap();
        assert_eq!(
            document.at(&found.path)["Properties"]["flex-min-height"],
            "1px"
        );
    }
}
