//! Semantic, ID-insensitive mashup comparison.
//!
//! Composer and twaco allocate widget ids and UUIDs freely. The comparator pairs widgets by
//! aligning sibling types through single wrapper levels, gives each pair one name, and drops
//! the UUIDs nothing refers to.
//! Sibling order is layout and is kept; bindings, events and calls are compared without order,
//! since ThingWorx does not depend on it.

use crate::core::config::Solution;
use crate::core::{mashup, workspace};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The semantic part of a mashup to report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Executable,
    Layout,
    Visual,
    All,
}

impl Tier {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "executable" => Some(Self::Executable),
            "layout" => Some(Self::Layout),
            "visual" => Some(Self::Visual),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Executable => "executable",
            Self::Layout => "layout",
            Self::Visual => "visual",
            Self::All => "all",
        }
    }

    fn includes(self, other: DifferenceTier) -> bool {
        matches!(self, Self::All)
            || matches!(
                (self, other),
                (Self::Executable, DifferenceTier::Executable)
                    | (Self::Layout, DifferenceTier::Layout)
                    | (Self::Visual, DifferenceTier::Visual)
            )
    }
}

impl Serialize for Tier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.name())
    }
}

/// A comparison result intended for both CLI JSON and MCP.
#[derive(Debug, Serialize)]
pub struct DiffResult {
    pub a: String,
    pub b: String,
    pub tier: Tier,
    pub equal: bool,
    pub summary: Summary,
    pub differences: Vec<Difference>,
}

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub executable: usize,
    pub layout: usize,
    pub visual: usize,
}

#[derive(Debug, Serialize)]
pub struct Difference {
    pub tier: DifferenceTier,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DifferenceTier {
    Executable,
    Layout,
    Visual,
}

impl DifferenceTier {
    fn name(self) -> &'static str {
        match self {
            Self::Executable => "executable",
            Self::Layout => "layout",
            Self::Visual => "visual",
        }
    }
}

impl Serialize for DifferenceTier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.name())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    #[error("no mashup {0} in the solution")]
    Unknown(String),
    #[error("{0}")]
    Unreadable(String),
}

impl crate::core::codes::Coded for DiffError {
    fn code(&self) -> crate::core::codes::ErrorCode {
        match self {
            Self::Unknown(_) => crate::core::codes::ErrorCode::UnknownEntity,
            Self::Unreadable(_) => crate::core::codes::ErrorCode::InvalidData,
        }
    }
}

// Composer allocator/display metadata and properties that have no effect in a responsive child.
// Composer editor state: the version that last saved the widget, the designer's lock and label,
// the icon its palette shows, which container it last split, its migration flag, and the base
// type it copies from a picker's bound display field, and the alignment it remembers.
pub(super) const IGNORED_PROPERTIES: &[&str] = &[
    "UpdatedAfterRefactor",
    "prev-align",
    "DisplayFieldBaseType",
    "Id",
    "id_index",
    "__TypeDisplayName",
    "_baseVersion",
    "isLocked",
    "DisplayName",
    "iconClass",
    "LastContainer",
];
/// A value Composer 10.2 saves that the widget catalogue has no default for.
fn composer_default(type_id: Option<&str>, key: &str, value: &Value) -> bool {
    match (type_id, key) {
        (Some("flexcontainer"), "Scrolling") => value == "auto-scroll",
        (Some("mashup"), "MinimizeDataStorage") => value == &Value::Bool(false),
        _ => false,
    }
}
// Numbered property families that only count up to their widget's count property: a chart keeps
// 100 series' and reference lines' settings whatever it shows.
const COUNTED_FAMILIES: &[(&str, &str)] = &[
    ("ReferenceLine", "NumberOfReferenceLines"),
    ("DataSeriesStyle", "NumberOfSeries"),
    ("DisplaySeries", "NumberOfSeries"),
    ("DataLabel", "NumberOfSeries"),
    ("DataField", "NumberOfSeries"),
    ("Series", "NumberOfSeries"),
    ("HideYAxis", "NumberOfYaxes"),
    ("YAxis", "NumberOfYaxes"),
];
const LAYOUT_PROPERTIES: &[&str] = &[
    "positioning",
    "overflow",
    "Margin",
    "MinWidth",
    "MinHeight",
    "MaxWidth",
    "MaxHeight",
    "LastContainer",
    "Top",
    "Left",
    "Width",
    "Height",
    "Z-index",
    "align-content",
    "align-items",
    "justify-content",
];

/// Compare two repository mashups, reading managed sidecars just as `mashup describe` does.
pub fn diff(solution: &Solution, a: &str, b: &str, tier: Tier) -> Result<DiffResult, DiffError> {
    let left = read(solution, a)?;
    let right = read(solution, b)?;
    Ok(compare(
        a,
        b,
        &left.0,
        &right.0,
        left.1.as_deref(),
        right.1.as_deref(),
        tier,
    ))
}

fn read(solution: &Solution, name: &str) -> Result<(Value, Option<String>), DiffError> {
    let entity = workspace::discover(solution)
        .entities
        .into_iter()
        .find(|file| file.info.collection == "Mashups" && file.info.name == name)
        .ok_or_else(|| DiffError::Unknown(name.to_string()))?;
    let path = entity.path.clone();
    let bytes = std::fs::read(&path)
        .map_err(|error| DiffError::Unreadable(format!("{}: {error}", path.display())))?;
    let assets = match workspace::read_mashup(&workspace::mashup_dir(solution, &entity))
        .map_err(|error| DiffError::Unreadable(error.to_string()))?
    {
        Some(assets) => {
            let (_, changes) = mashup::sync(&bytes, &assets)
                .map_err(|error| DiffError::Unreadable(error.to_string()))?;
            if !changes.is_empty() {
                return Err(DiffError::Unreadable(format!(
                    "{name}: content.json and the entity file disagree; run `twaco sync` (keeps the sidecars) or `twaco extract` (keeps the entity file) first"
                )));
            }
            assets
        }
        None => {
            mashup::extract(&bytes).map_err(|error| DiffError::Unreadable(error.to_string()))?
        }
    };
    let value = serde_json::from_str(&assets.content)
        .map_err(|error| DiffError::Unreadable(format!("{name}: {error}")))?;
    Ok((value, Some(assets.css)))
}

/// Compare parsed mashup content.  This is public within the crate for focused tests and adapters.
pub(crate) fn compare(
    a: &str,
    b: &str,
    left: &Value,
    right: &Value,
    left_css: Option<&str>,
    right_css: Option<&str>,
    selected: Tier,
) -> DiffResult {
    let (left, right) = normalise_pair(left, right);
    let mut differences = Vec::new();

    compare_optional(
        left.get("mashupType"),
        right.get("mashupType"),
        "mashupType",
        DifferenceTier::Executable,
        &mut differences,
    );
    compare_ui(&left, &right, &mut differences);
    compare_optional(
        left.get("Data"),
        right.get("Data"),
        "Data",
        DifferenceTier::Executable,
        &mut differences,
    );
    for key in ["DataBindings", "Events"] {
        compare_unordered(&left, &right, key, &mut differences);
    }
    compare_other_top_level(&left, &right, &mut differences);
    compare_css(left_css, right_css, &mut differences);

    let mut summary = Summary::default();
    for difference in &differences {
        match difference.tier {
            DifferenceTier::Executable => summary.executable += 1,
            DifferenceTier::Layout => summary.layout += 1,
            DifferenceTier::Visual => summary.visual += 1,
        }
    }
    let differences: Vec<_> = differences
        .into_iter()
        .filter(|difference| selected.includes(difference.tier))
        .collect();
    DiffResult {
        a: a.to_string(),
        b: b.to_string(),
        tier: selected,
        equal: differences.is_empty(),
        summary,
        differences,
    }
}

#[derive(Default)]
struct WidgetIds {
    names: BTreeMap<String, String>,
    wrappers: BTreeSet<String>,
}

/// Shared widget names, with single wrapper levels bypassed and unreferenced identities removed.
fn normalise_pair(left: &Value, right: &Value) -> (Value, Value) {
    let mut left_ids = WidgetIds::default();
    let mut right_ids = WidgetIds::default();
    let mut next = 1;
    match (left.get("UI"), right.get("UI")) {
        (Some(l), Some(r)) => pair_ids(l, r, true, &mut next, &mut left_ids, &mut right_ids),
        (Some(l), None) => unpaired_ids(l, "a", &mut left_ids.names),
        (None, Some(r)) => unpaired_ids(r, "b", &mut right_ids.names),
        (None, None) => {}
    }
    let mut left = left.clone();
    let mut right = right.clone();
    rewrite_widget_ids(&mut left, &left_ids);
    rewrite_widget_ids(&mut right, &right_ids);
    for value in [&mut left, &mut right] {
        strip_identity(value);
        if let Some(ui) = value.get_mut("UI") {
            key_parameter_defs(ui);
        }
    }
    (left, right)
}

/// `_currentParameterDefs` as an object keyed by `ParameterName`, so one added or renamed
/// parameter is one difference instead of shifting every later entry.
fn key_parameter_defs(widget: &mut Value) {
    if let Some(Value::Array(defs)) = widget.pointer_mut("/Properties/_currentParameterDefs") {
        let named = defs
            .iter()
            .all(|def| def.get("ParameterName").and_then(Value::as_str).is_some());
        if named {
            let keyed: Map<String, Value> = defs
                .iter()
                .map(|def| {
                    (
                        def["ParameterName"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        def.clone(),
                    )
                })
                .collect();
            widget["Properties"]["_currentParameterDefs"] = Value::Object(keyed);
        }
    }
    if let Some(Value::Array(children)) = widget.get_mut("Widgets") {
        for child in children {
            key_parameter_defs(child);
        }
    }
}

fn widget_id(widget: &Value) -> Option<&str> {
    widget.pointer("/Properties/Id").and_then(Value::as_str)
}

fn widget_type(widget: &Value) -> Option<&str> {
    widget.pointer("/Properties/Type").and_then(Value::as_str)
}

fn children(widget: &Value) -> &[Value] {
    widget
        .get("Widgets")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn pair_ids(
    left: &Value,
    right: &Value,
    root: bool,
    next: &mut usize,
    left_ids: &mut WidgetIds,
    right_ids: &mut WidgetIds,
) {
    if let Some(is_left) = node_wrapper_side(left, right) {
        let (wrapper, ids, side) = if is_left {
            (left, &mut *left_ids, "a")
        } else {
            (right, &mut *right_ids, "b")
        };
        if let Some(id) = widget_id(wrapper) {
            ids.names.insert(id.to_string(), format!("{side}:{id}"));
            ids.wrappers.insert(id.to_string());
        }
        if is_left {
            pair_ids(&children(left)[0], right, root, next, left_ids, right_ids);
        } else {
            pair_ids(left, &children(right)[0], root, next, left_ids, right_ids);
        }
        return;
    }
    let name = if root {
        mashup::edit::ROOT_ID.to_string()
    } else {
        let name = format!("w{next}");
        *next += 1;
        name
    };
    if let Some(id) = widget_id(left) {
        left_ids.names.insert(id.to_string(), name.clone());
    }
    if let Some(id) = widget_id(right) {
        right_ids.names.insert(id.to_string(), name);
    }
    let (left, right) = match wrapper_side(left, right) {
        Some(true) => {
            let wrapper = &children(left)[0];
            if let Some(id) = widget_id(wrapper) {
                left_ids.names.insert(id.to_string(), format!("a:{id}"));
                left_ids.wrappers.insert(id.to_string());
            }
            (wrapper, right)
        }
        Some(false) => {
            let wrapper = &children(right)[0];
            if let Some(id) = widget_id(wrapper) {
                right_ids.names.insert(id.to_string(), format!("b:{id}"));
                right_ids.wrappers.insert(id.to_string());
            }
            (left, wrapper)
        }
        None => (left, right),
    };
    let (lc, rc) = (children(left), children(right));
    for pair in align(lc, rc) {
        match pair {
            (Some(i), Some(j)) => pair_ids(&lc[i], &rc[j], false, next, left_ids, right_ids),
            (Some(i), None) => unpaired_ids(&lc[i], "a", &mut left_ids.names),
            (None, Some(j)) => unpaired_ids(&rc[j], "b", &mut right_ids.names),
            (None, None) => {}
        }
    }
}

fn unpaired_ids(widget: &Value, side: &str, ids: &mut BTreeMap<String, String>) {
    if let Some(id) = widget_id(widget) {
        ids.insert(id.to_string(), format!("{side}:{id}"));
    }
    for child in children(widget) {
        unpaired_ids(child, side, ids);
    }
}

/// Sibling index pairs in document order: both sides where the types align, one side otherwise.
fn align(left: &[Value], right: &[Value]) -> Vec<(Option<usize>, Option<usize>)> {
    let (n, m) = (left.len(), right.len());
    // lcs[i][j]: longest common type subsequence of left[i..] and right[j..].
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if widget_type(&left[i]) == widget_type(&right[j]) {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut pairs) = (0, 0, Vec::new());
    while i < n || j < m {
        if i < n && j < m && widget_type(&left[i]) == widget_type(&right[j]) {
            pairs.push((Some(i), Some(j)));
            i += 1;
            j += 1;
        } else if j == m || (i < n && lcs[i + 1][j] >= lcs[i][j + 1]) {
            pairs.push((Some(i), None));
            i += 1;
        } else {
            pairs.push((None, Some(j)));
            j += 1;
        }
    }
    pairs
}

fn tree_size(widget: &Value) -> usize {
    1 + children(widget).iter().map(tree_size).sum::<usize>()
}

fn shape_distance(left: &Value, right: &Value) -> usize {
    let (lc, rc) = (children(left), children(right));
    align(lc, rc)
        .into_iter()
        .map(|pair| match pair {
            (Some(i), Some(j)) => shape_distance(&lc[i], &rc[j]),
            (Some(i), None) => tree_size(&lc[i]),
            (None, Some(j)) => tree_size(&rc[j]),
            (None, None) => 0,
        })
        .sum()
}

fn same_widget(left: &Value, right: &Value) -> bool {
    let named = |widget: &Value| {
        widget
            .pointer("/Properties/DisplayName")
            .filter(|name| name.as_str().is_some_and(|name| !name.is_empty()))
            .cloned()
    };
    widget_type(left) == widget_type(right)
        && ((widget_id(left).is_some() && widget_id(left) == widget_id(right))
            || (named(left).is_some() && named(left) == named(right)))
}

fn better_wrapper_side(l: Option<usize>, r: Option<usize>, direct: usize) -> Option<bool> {
    let l = l.filter(|score| *score < direct);
    let r = r.filter(|score| *score < direct);
    match (l, r) {
        (Some(l), Some(r)) if l == r => None,
        (Some(l), Some(r)) => Some(l < r),
        (Some(_), None) => Some(true),
        (None, Some(_)) => Some(false),
        (None, None) => None,
    }
}

fn node_wrapper_side(left: &Value, right: &Value) -> Option<bool> {
    if widget_type(left) != Some("flexcontainer")
        || widget_type(right) != Some("flexcontainer")
        || same_widget(left, right)
    {
        return None;
    }
    let through = |wrapper: &Value, other: &Value| match children(wrapper) {
        [child] if widget_type(child) == Some("flexcontainer") => {
            Some(shape_distance(child, other))
        }
        _ => None,
    };
    better_wrapper_side(
        through(left, right),
        through(right, left),
        shape_distance(left, right),
    )
}

/// Look through a sole wrapper child only when its children fit the other parent better.
fn wrapper_side(left: &Value, right: &Value) -> Option<bool> {
    let container =
        |widget: &Value| matches!(widget_type(widget), Some("flexcontainer" | "mashup"));
    if !container(left) || !container(right) {
        return None;
    }
    if let ([l], [r]) = (children(left), children(right)) {
        if same_widget(l, r) {
            return None;
        }
    }
    let through = |wrapper: &Value, other: &Value| match children(wrapper) {
        [child] if widget_type(child) == Some("flexcontainer") && !children(child).is_empty() => {
            Some(shape_distance(child, other))
        }
        _ => None,
    };
    better_wrapper_side(
        through(left, right),
        through(right, left),
        shape_distance(left, right),
    )
}

fn rewrite_widget_ids(value: &mut Value, ids: &WidgetIds) {
    rewrite_widget_tree(value.get_mut("UI"), ids);
    for binding in value
        .get_mut("DataBindings")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if binding["SourceArea"].as_str() == Some("UI") {
            rewrite_field(binding, "SourceId", &ids.names);
        }
        if binding["TargetArea"].as_str() == Some("UI") {
            rewrite_field(binding, "TargetId", &ids.names);
        }
    }
    for event in value
        .get_mut("Events")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if event["EventTriggerArea"].as_str() == Some("UI") {
            rewrite_field(event, "EventTriggerId", &ids.names);
        }
        if event["EventHandlerArea"].as_str() == Some("UI") {
            rewrite_field(event, "EventHandlerId", &ids.names);
        }
    }
}

fn rewrite_widget_tree(value: Option<&mut Value>, ids: &WidgetIds) {
    let Some(value) = value else { return };
    // The root's StyleProperties keep each styled widget's parts under its id. Composer leaves
    // the entries of removed widgets behind; they style nothing.
    if let Some(Value::Object(styled)) = value.pointer_mut("/Properties/StyleProperties/data") {
        let renamed: Map<String, Value> = std::mem::take(styled)
            .into_iter()
            // Wrapper presence already accounts for its own styling.
            .filter(|(id, _)| !ids.wrappers.contains(id))
            .filter_map(|(id, parts)| ids.names.get(&id).map(|name| (name.clone(), parts)))
            .collect();
        *styled = renamed;
    }
    if let Some(properties) = value.get_mut("Properties") {
        rewrite_field(properties, "Id", &ids.names);
        rewrite_field(properties, "LastContainer", &ids.names);
    }
    for child in value
        .get_mut("Widgets")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        rewrite_widget_tree(Some(child), ids);
    }
}

fn rewrite_field(value: &mut Value, field: &str, ids: &BTreeMap<String, String>) {
    let Some(original) = value.get(field).and_then(Value::as_str) else {
        return;
    };
    if let Some(replacement) = ids.get(original) {
        value[field] = Value::String(replacement.clone());
    }
}

/// Removes the `Id` of data sections, calls, bindings and events: nothing in a mashup refers to
/// them, and Composer and twaco allocate them freely. Calls are put in name order, which
/// ThingWorx does not depend on.
fn strip_identity(value: &mut Value) {
    if let Some(data) = value.get_mut("Data").and_then(Value::as_object_mut) {
        for section in data.values_mut() {
            let Some(section) = section.as_object_mut() else {
                continue;
            };
            section.remove("Id");
            if let Some(Value::Array(calls)) = section.get_mut("Services") {
                for call in calls.iter_mut() {
                    if let Some(call) = call.as_object_mut() {
                        call.remove("Id");
                        // Composer writes caching switched off; twaco leaves it out. Same call.
                        if call.get("Caching").and_then(|caching| caching.get("cache"))
                            == Some(&Value::Bool(false))
                        {
                            call.remove("Caching");
                        }
                    }
                }
                calls.sort_by(|a, b| {
                    a.get("Name")
                        .and_then(Value::as_str)
                        .cmp(&b.get("Name").and_then(Value::as_str))
                });
            }
        }
    }
    for key in ["DataBindings", "Events"] {
        for item in value
            .get_mut(key)
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if let Some(item) = item.as_object_mut() {
                item.remove("Id");
                // Composer spellings of one binding: an empty member written or left out, a
                // SourceName repeating SourceId, and `property` or `Property`.
                item.retain(|key, value| value != "" && key != "SourceName");
                for map in item
                    .get_mut("PropertyMaps")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_object_mut)
                {
                    for kind in ["SourcePropertyType", "TargetPropertyType"] {
                        if let Some(Value::String(text)) = map.get_mut(kind) {
                            if text == "Property" {
                                *text = "property".into();
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The value with object keys sorted at every level, for an order-insensitive identity.
fn canonical(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();
                Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), sorted(&map[key])))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    sorted(value).to_string()
}

/// Pair wiring by endpoints, retaining duplicate multiplicity and changes to binding metadata.
fn compare_unordered(left: &Value, right: &Value, key: &str, differences: &mut Vec<Difference>) {
    let grouped = |document: &Value| {
        let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for item in document
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            groups
                .entry(wiring_key(item, key))
                .or_default()
                .push(item.clone());
        }
        for items in groups.values_mut() {
            items.sort_by_key(canonical);
        }
        groups
    };
    let mut left_groups = grouped(left);
    let mut right_groups = grouped(right);
    let keys: std::collections::BTreeSet<_> = left_groups
        .keys()
        .chain(right_groups.keys())
        .cloned()
        .collect();
    for identity in keys {
        let mut l = left_groups.remove(&identity).unwrap_or_default();
        let mut r = right_groups.remove(&identity).unwrap_or_default();
        l.retain(|item| match r.iter().position(|other| item == other) {
            Some(index) => {
                r.remove(index);
                false
            }
            None => true,
        });
        for index in 0..l.len().max(r.len()) {
            let (li, ri) = (l.get(index), r.get(index));
            let (item, document) = li
                .map(|item| (item, left))
                .or_else(|| ri.map(|item| (item, right)))
                .expect("unmatched wiring");
            push(
                DifferenceTier::Executable,
                format!("{key}: {}", wiring_description(item, document, key)),
                li.map(|item| readable_wiring(item, left, key)),
                ri.map(|item| readable_wiring(item, right, key)),
                differences,
            );
        }
    }
}

fn wiring_key(item: &Value, key: &str) -> String {
    let fields: &[&str] = if key == "DataBindings" {
        &[
            "SourceArea",
            "SourceId",
            "SourceSection",
            "SourceProperty",
            "TargetArea",
            "TargetId",
            "TargetSection",
            "TargetProperty",
        ]
    } else {
        &[
            "EventTriggerArea",
            "EventTriggerId",
            "EventTriggerSection",
            "EventTriggerEvent",
            "EventHandlerArea",
            "EventHandlerId",
            "EventHandlerSection",
            "EventHandlerService",
        ]
    };
    let mut identity: Vec<Value> = fields.iter().map(|field| item[*field].clone()).collect();
    if key == "DataBindings" {
        identity.push(Value::Array(
            item.get("PropertyMaps")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|map| {
                    Value::Array(vec![
                        map["SourceProperty"].clone(),
                        map["TargetProperty"].clone(),
                    ])
                })
                .collect(),
        ));
    }
    canonical(&Value::Array(identity))
}

fn widget_description(widget: &Value) -> String {
    let type_id = widget_type(widget).unwrap_or("unknown widget");
    match widget
        .pointer("/Properties/DisplayName")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
    {
        Some(name) => format!("{name} ({type_id})"),
        None => type_id.to_string(),
    }
}

fn find_widget<'a>(widget: &'a Value, id: &str) -> Option<&'a Value> {
    if widget_id(widget) == Some(id) {
        Some(widget)
    } else {
        children(widget)
            .iter()
            .find_map(|child| find_widget(child, id))
    }
}

fn endpoint(document: &Value, area: &Value, id: &Value, section: &Value, member: &Value) -> String {
    let (area, id, section) = (
        area.as_str().unwrap_or("unknown"),
        id.as_str().unwrap_or(""),
        section.as_str().unwrap_or(""),
    );
    let mut description = match area {
        "UI" => document
            .get("UI")
            .and_then(|ui| find_widget(ui, id))
            .map(widget_description)
            .unwrap_or_else(|| format!("missing widget {id}")),
        "Mashup" => "mashup".to_string(),
        "Data" => format!(
            "data:{section}{}{id}",
            if section.is_empty() { "" } else { "." }
        ),
        _ => format!("{area}:{id}"),
    };
    if area != "Data" && !section.is_empty() {
        description.push_str(&format!(".{section}"));
    }
    if let Some(member) = member.as_str() {
        if member.is_empty() {
            description.push_str(" (whole result)");
        } else {
            description.push_str(&format!(".{member}"));
        }
    }
    description
}

fn wiring_description(item: &Value, document: &Value, key: &str) -> String {
    if key == "DataBindings" {
        let map = item
            .get("PropertyMaps")
            .and_then(Value::as_array)
            .and_then(|maps| maps.first())
            .unwrap_or(item);
        let source = endpoint(
            document,
            &item["SourceArea"],
            &item["SourceId"],
            &item["SourceSection"],
            &map["SourceProperty"],
        );
        let target = endpoint(
            document,
            &item["TargetArea"],
            &item["TargetId"],
            &item["TargetSection"],
            &map["TargetProperty"],
        );
        format!("{source} -> {target}")
    } else {
        let trigger = endpoint(
            document,
            &item["EventTriggerArea"],
            &item["EventTriggerId"],
            &item["EventTriggerSection"],
            &item["EventTriggerEvent"],
        );
        let handler = endpoint(
            document,
            &item["EventHandlerArea"],
            &item["EventHandlerId"],
            &item["EventHandlerSection"],
            &item["EventHandlerService"],
        );
        format!("{trigger} -> {handler}")
    }
}

fn readable_wiring(item: &Value, document: &Value, key: &str) -> Value {
    let mut item = item.clone();
    let ends = if key == "DataBindings" {
        ["Source", "Target"]
    } else {
        ["EventTrigger", "EventHandler"]
    };
    for end in ends {
        if item[format!("{end}Area")] == "UI" {
            let field = format!("{end}Id");
            item[&field] = Value::String(endpoint(
                document,
                &item[&format!("{end}Area")],
                &item[&field],
                &Value::Null,
                &Value::Null,
            ));
        }
    }
    item
}

fn compare_ui(left: &Value, right: &Value, differences: &mut Vec<Difference>) {
    let left_ui = left.get("UI").unwrap_or(&Value::Null);
    let right_ui = right.get("UI").unwrap_or(&Value::Null);
    compare_properties(left_ui, right_ui, "UI", false, false, true, differences);
    compare_children(
        left_ui,
        right_ui,
        "UI",
        responsive(left_ui),
        responsive(right_ui),
        differences,
    );
}

fn compare_children(
    left: &Value,
    right: &Value,
    parent_path: &str,
    left_responsive: bool,
    right_responsive: bool,
    differences: &mut Vec<Difference>,
) {
    let (left, right, left_responsive, right_responsive) =
        if let Some(is_left) = wrapper_side(left, right) {
            let wrapper = &children(if is_left { left } else { right })[0];
            missing_wrapper(wrapper, parent_path, is_left, differences);
            if is_left {
                (wrapper, right, responsive(wrapper), right_responsive)
            } else {
                (left, wrapper, left_responsive, responsive(wrapper))
            }
        } else {
            (left, right, left_responsive, right_responsive)
        };
    let (lc, rc) = (children(left), children(right));
    for pair in align(lc, rc) {
        match pair {
            (Some(i), Some(j)) => compare_widget(
                &lc[i],
                &rc[j],
                parent_path,
                left_responsive,
                right_responsive,
                differences,
            ),
            (Some(i), None) => missing_widget(&lc[i], parent_path, true, differences),
            (None, Some(j)) => missing_widget(&rc[j], parent_path, false, differences),
            (None, None) => {}
        }
    }
}

fn missing_widget(
    widget: &Value,
    parent_path: &str,
    is_left: bool,
    differences: &mut Vec<Difference>,
) {
    let path = widget_path(parent_path, widget);
    let type_id = widget
        .pointer("/Properties/Type")
        .cloned()
        .unwrap_or(Value::Null);
    differences.push(Difference {
        tier: DifferenceTier::Layout,
        path,
        left: is_left.then_some(type_id.clone()),
        right: (!is_left).then_some(type_id),
    });
}

fn missing_wrapper(
    widget: &Value,
    parent_path: &str,
    is_left: bool,
    differences: &mut Vec<Difference>,
) {
    let action = if is_left { "removed" } else { "added" };
    let description = widget_description(widget);
    push(
        DifferenceTier::Layout,
        format!("{parent_path}/{action} wrapper {description}"),
        is_left.then(|| Value::String(description.clone())),
        (!is_left).then_some(Value::String(description)),
        differences,
    );
}

fn compare_widget(
    left: &Value,
    right: &Value,
    parent_path: &str,
    left_parent_responsive: bool,
    right_parent_responsive: bool,
    differences: &mut Vec<Difference>,
) {
    if let Some(is_left) = node_wrapper_side(left, right) {
        let wrapper = if is_left { left } else { right };
        missing_wrapper(wrapper, parent_path, is_left, differences);
        if is_left {
            compare_widget(
                &children(left)[0],
                right,
                parent_path,
                responsive(left),
                right_parent_responsive,
                differences,
            );
        } else {
            compare_widget(
                left,
                &children(right)[0],
                parent_path,
                left_parent_responsive,
                responsive(right),
                differences,
            );
        }
        return;
    }
    let path = widget_path(parent_path, left);
    // `align` pairs only widgets of one type; a changed type is a removal plus an addition.
    let function = widget_type(left)
        .and_then(|type_id| crate::core::widgets::catalog().by_type(type_id))
        .is_some_and(|widget| widget.function);
    compare_properties(
        left,
        right,
        &path,
        function,
        left_parent_responsive && right_parent_responsive,
        false,
        differences,
    );
    compare_children(
        left,
        right,
        &path,
        responsive(left),
        responsive(right),
        differences,
    );
}

fn responsive(widget: &Value) -> bool {
    widget.pointer("/Properties/ResponsiveLayout") == Some(&Value::Bool(true))
        && matches!(
            widget.pointer("/Properties/Type").and_then(Value::as_str),
            Some("flexcontainer" | "mashup")
        )
}

fn widget_path(parent: &str, widget: &Value) -> String {
    let id = widget
        .pointer("/Properties/Id")
        .and_then(Value::as_str)
        .unwrap_or("missing");
    let type_id = widget
        .pointer("/Properties/Type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    format!("{parent}/{id}({type_id})")
}

fn compare_properties(
    left: &Value,
    right: &Value,
    prefix: &str,
    function: bool,
    responsive_child: bool,
    root: bool,
    differences: &mut Vec<Difference>,
) {
    let empty = Map::new();
    let left = left
        .get("Properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let right = right
        .get("Properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let keys: std::collections::BTreeSet<_> = left.keys().chain(right.keys()).collect();
    for key in keys {
        if ignored(key, responsive_child)
            || inert(key, left, right)
            || default_on_one_side(key, left.get(key), right.get(key), left.get("Type"))
        {
            continue;
        }
        let tier = if function
            || (root && matches!(key.as_str(), "_currentParameterDefs" | "MashupParameters"))
        {
            DifferenceTier::Executable
        } else if layout_property(key) {
            DifferenceTier::Layout
        } else {
            DifferenceTier::Visual
        };
        compare_optional(
            left.get(key),
            right.get(key),
            &format!("{prefix}.Properties.{key}"),
            tier,
            differences,
        );
    }
}

fn ignored(key: &str, responsive_child: bool) -> bool {
    IGNORED_PROPERTIES.contains(&key)
        || (responsive_child && matches!(key, "Top" | "Left" | "Width" | "Height"))
}

/// A numbered family member past its count on both sides, as `ReferenceLine7Axis` with no
/// reference lines.
fn inert(key: &str, left: &Map<String, Value>, right: &Map<String, Value>) -> bool {
    COUNTED_FAMILIES.iter().any(|(prefix, count)| {
        let Some(rest) = key.strip_prefix(prefix) else {
            return false;
        };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let Ok(number) = digits.parse::<u64>() else {
            return false;
        };
        let shown = |side: &Map<String, Value>| side.get(*count).and_then(Value::as_u64);
        match (shown(left), shown(right)) {
            (None, None) => false,
            (l, r) => number > l.unwrap_or(0).max(r.unwrap_or(0)),
        }
    })
}

/// A property only one side writes, at its empty or catalogue default value: Composer and twaco
/// write different subsets of the defaults.
fn default_on_one_side(
    key: &str,
    left: Option<&Value>,
    right: Option<&Value>,
    type_id: Option<&Value>,
) -> bool {
    let value = match (left, right) {
        (Some(value), None) | (None, Some(value)) => value,
        _ => return false,
    };
    let empty = match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => {
            map.is_empty()
                // An empty widget styler.
                || (map.get("data").is_some_and(|data| data.as_object().is_some_and(Map::is_empty))
                    && map.keys().all(|key| key == "data" || key == "meta"))
        }
        _ => false,
    };
    let type_id = type_id.and_then(Value::as_str);
    empty
        || type_id
            .and_then(|type_id| crate::core::widgets::catalog().by_type(type_id))
            .and_then(|widget| widget.defaults.get(key))
            == Some(value)
        || composer_default(type_id, key, value)
}

fn layout_property(key: &str) -> bool {
    key.starts_with("flex-") || key.starts_with("Responsive") || LAYOUT_PROPERTIES.contains(&key)
}

fn compare_other_top_level(left: &Value, right: &Value, differences: &mut Vec<Difference>) {
    let empty = Map::new();
    let left = left.as_object().unwrap_or(&empty);
    let right = right.as_object().unwrap_or(&empty);
    let keys: std::collections::BTreeSet<_> = left.keys().chain(right.keys()).collect();
    for key in keys {
        if matches!(
            key.as_str(),
            "mashupType"
                | "_currentParameterDefs"
                | "MashupParameters"
                | "UI"
                | "Data"
                | "DataBindings"
                | "Events"
                | "CustomMashupCss"
        ) {
            continue;
        }
        compare_optional(
            left.get(key),
            right.get(key),
            key,
            DifferenceTier::Visual,
            differences,
        );
    }
}

fn compare_value(
    left: &Value,
    right: &Value,
    path: &str,
    tier: DifferenceTier,
    differences: &mut Vec<Difference>,
) {
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            let keys: std::collections::BTreeSet<_> = left.keys().chain(right.keys()).collect();
            for key in keys {
                let next = format!("{path}.{key}");
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => {
                        compare_value(left, right, &next, tier, differences)
                    }
                    (Some(left), None) => push(tier, next, Some(left.clone()), None, differences),
                    (None, Some(right)) => push(tier, next, None, Some(right.clone()), differences),
                    (None, None) => unreachable!(),
                }
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            for index in 0..left.len().max(right.len()) {
                let next = format!("{path}[{index}]");
                match (left.get(index), right.get(index)) {
                    (Some(left), Some(right)) => {
                        compare_value(left, right, &next, tier, differences)
                    }
                    (Some(left), None) => push(tier, next, Some(left.clone()), None, differences),
                    (None, Some(right)) => push(tier, next, None, Some(right.clone()), differences),
                    (None, None) => unreachable!(),
                }
            }
        }
        _ if left != right => push(
            tier,
            path.to_string(),
            (!left.is_null()).then_some(left.clone()),
            (!right.is_null()).then_some(right.clone()),
            differences,
        ),
        _ => {}
    }
}

fn compare_optional(
    left: Option<&Value>,
    right: Option<&Value>,
    path: &str,
    tier: DifferenceTier,
    differences: &mut Vec<Difference>,
) {
    match (left, right) {
        (Some(left), Some(right)) => compare_value(left, right, path, tier, differences),
        (Some(left), None) => push(tier, path.into(), Some(left.clone()), None, differences),
        (None, Some(right)) => push(tier, path.into(), None, Some(right.clone()), differences),
        (None, None) => {}
    }
}

fn push(
    tier: DifferenceTier,
    path: String,
    left: Option<Value>,
    right: Option<Value>,
    differences: &mut Vec<Difference>,
) {
    differences.push(Difference {
        tier,
        path,
        left,
        right,
    });
}

fn compare_css(left: Option<&str>, right: Option<&str>, differences: &mut Vec<Difference>) {
    let left = normal_css(left.unwrap_or_default());
    let right = normal_css(right.unwrap_or_default());
    if left == right {
        return;
    }
    let index = left
        .lines()
        .zip(right.lines())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| left.lines().count().min(right.lines().count()));
    push(
        DifferenceTier::Visual,
        "custom.css".into(),
        left.lines()
            .nth(index)
            .map(|line| Value::String(line.into())),
        right
            .lines()
            .nth(index)
            .map(|line| Value::String(line.into())),
        differences,
    );
}

fn normal_css(css: &str) -> String {
    css.replace("\r\n", "\n").trim_end_matches('\n').to_string()
}

/// The compact, human-readable form. JSON output never truncates values.
pub fn render(result: &DiffResult) -> String {
    if result.equal {
        return "same\n".to_string();
    }
    let count = result.differences.len();
    let mut out = format!(
        "{count} difference(s): executable {}, layout {}, visual {}\n",
        result.summary.executable, result.summary.layout, result.summary.visual
    );
    for difference in &result.differences {
        out.push_str(&format!(
            "{} {}: {} -> {}\n",
            difference.tier.name(),
            difference.path,
            display(difference.left.as_ref()),
            display(difference.right.as_ref())
        ));
    }
    out
}

fn display(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "(missing)".to_string();
    };
    let mut text = serde_json::to_string(value).expect("JSON values serialise");
    if text.chars().count() > 120 {
        text = format!("{}...", text.chars().take(117).collect::<String>());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mashup(widgets: Value) -> Value {
        json!({"mashupType":"mashup", "Data": {}, "DataBindings": [], "Events": [], "UI": {"Properties":{"Id":"mashup-root","Type":"mashup"}, "Widgets": widgets}})
    }
    fn widget(id: &str, type_id: &str, properties: Value, widgets: Value) -> Value {
        let mut properties = properties.as_object().unwrap().clone();
        properties.insert("Id".into(), json!(id));
        properties.insert("Type".into(), json!(type_id));
        json!({"Properties":properties,"Widgets":widgets})
    }

    #[test]
    fn ids_uuids_and_references_do_not_diff() {
        let left = json!({"mashupType":"mashup", "Data":{"Things_Acme.Manager":{"Id":"data-a","Services":[{"Id":"call-a","Target":"Get"}]}}, "DataBindings":[{"Id":"binding-a","SourceArea":"UI","SourceId":"button-a","TargetArea":"UI","TargetId":"label-a"}], "Events":[{"Id":"event-a","EventTriggerArea":"UI","EventTriggerId":"button-a","EventHandlerArea":"UI","EventHandlerId":"label-a"}], "UI":{"Properties":{"Id":"root-a","Type":"mashup","id_index":99},"Widgets":[widget("button-a","ptcsbutton",json!({}),json!([])),widget("label-a","ptcslabel",json!({}),json!([]))]}});
        let right = json!({"mashupType":"mashup", "Data":{"Things_Acme.Manager":{"Id":"data-b","Services":[{"Id":"call-b","Target":"Get"}]}}, "DataBindings":[{"Id":"binding-b","SourceArea":"UI","SourceId":"button-b","TargetArea":"UI","TargetId":"label-b"}], "Events":[{"Id":"event-b","EventTriggerArea":"UI","EventTriggerId":"button-b","EventHandlerArea":"UI","EventHandlerId":"label-b"}], "UI":{"Properties":{"Id":"root-b","Type":"mashup","id_index":2},"Widgets":[widget("button-b","ptcsbutton",json!({}),json!([])),widget("label-b","ptcslabel",json!({}),json!([]))]}});
        assert!(
            compare(
                "Acme.A_MU",
                "Acme.B_MU",
                &left,
                &right,
                None,
                None,
                Tier::All
            )
            .equal
        );
    }

    #[test]
    fn classifies_layout_visual_executable_and_css() {
        let left = mashup(json!([widget(
            "container-a",
            "flexcontainer",
            json!({"flex-grow": 1}),
            json!([
                widget(
                    "label-a",
                    "ptcslabel",
                    json!({"LabelText":"left"}),
                    json!([])
                ),
                widget(
                    "expression-a",
                    "expression2",
                    json!({"Expression":"1"}),
                    json!([])
                )
            ])
        )]));
        let right = mashup(json!([widget(
            "container-b",
            "flexcontainer",
            json!({"flex-grow": 2}),
            json!([
                widget(
                    "label-b",
                    "ptcslabel",
                    json!({"LabelText":"right"}),
                    json!([])
                ),
                widget(
                    "expression-b",
                    "expression2",
                    json!({"Expression":"2"}),
                    json!([])
                )
            ])
        )]));
        let result = compare(
            "a",
            "b",
            &left,
            &right,
            Some("a\r\n"),
            Some("b\n"),
            Tier::All,
        );
        assert_eq!(result.summary.layout, 1);
        assert_eq!(result.summary.visual, 2);
        assert_eq!(result.summary.executable, 1);
        assert!(result
            .differences
            .iter()
            .any(|difference| difference.path == "custom.css"));
    }

    #[test]
    fn missing_widget_is_one_layout_difference() {
        let left = mashup(json!([widget(
            "button-a",
            "ptcsbutton",
            json!({}),
            json!([])
        )]));
        let right = mashup(json!([]));
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        assert_eq!(result.differences.len(), 1);
        assert_eq!(result.differences[0].tier, DifferenceTier::Layout);
    }

    #[test]
    fn service_bindings_and_events_are_executable() {
        let mut left = mashup(json!([]));
        left["Data"] = json!({"Things_Acme.Manager":{"Id":"data-a","Services":[{"Id":"service-a","Target":"GetOrders"}]}});
        left["DataBindings"] =
            json!([{"Id":"binding-a","SourceArea":"Data","SourceId":"GetOrders"}]);
        left["Events"] = json!([{"Id":"event-a","EventHandlerArea":"Data","EventHandlerId":"Things_Acme.Manager","EventHandlerService":"GetOrders"}]);
        let mut right = left.clone();
        right["Data"]["Things_Acme.Manager"]["Services"][0]["Target"] = json!("GetOpenOrders");
        right["DataBindings"] = json!([]);
        right["Events"][0]["EventHandlerService"] = json!("Refresh");
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        // Target, the missing binding, and the changed event (unordered: one gone, one new).
        assert_eq!(result.summary.executable, 4);
        assert!(result
            .differences
            .iter()
            .all(|difference| difference.tier == DifferenceTier::Executable));
    }

    #[test]
    fn sibling_order_is_layout() {
        let left = mashup(json!([
            widget("button-a", "ptcsbutton", json!({}), json!([])),
            widget("label-a", "ptcslabel", json!({}), json!([]))
        ]));
        let right = mashup(json!([
            widget("label-b", "ptcslabel", json!({}), json!([])),
            widget("button-b", "ptcsbutton", json!({}), json!([]))
        ]));
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        // One sibling pairs; the other is missing on one side and extra on the other.
        let paths: Vec<_> = result.differences.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(
            paths,
            ["UI/a:button-a(ptcsbutton)", "UI/b:button-b(ptcsbutton)"]
        );
        assert_eq!(result.summary.layout, 2);
    }

    #[test]
    fn an_extra_container_does_not_shift_later_widgets_or_their_bindings() {
        let binding = |button: &str, label: &str| {
            json!({"Id": format!("{button}-{label}"), "SourceArea":"UI","SourceId":button,"SourceProperty":"Clicked",
                   "TargetArea":"UI","TargetId":label,"TargetProperty":"Text"})
        };
        let mut left = mashup(json!([
            widget("button-1", "ptcsbutton", json!({}), json!([])),
            widget("label-1", "ptcslabel", json!({"LabelText":"x"}), json!([]))
        ]));
        left["DataBindings"] = json!([binding("button-1", "label-1")]);
        let mut right = mashup(json!([
            widget("flexcontainer-9", "flexcontainer", json!({}), json!([])),
            widget("button-7", "ptcsbutton", json!({}), json!([])),
            widget("label-3", "ptcslabel", json!({"LabelText":"x"}), json!([]))
        ]));
        right["DataBindings"] = json!([binding("button-7", "label-3")]);
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        let paths: Vec<_> = result.differences.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, ["UI/b:flexcontainer-9(flexcontainer)"]);
    }

    #[test]
    fn binding_event_and_call_order_is_not_a_difference() {
        let section = |calls: Value| json!({"Things_Acme.Manager": {"Id": "s", "Services": calls}});
        let get = json!({"Id":"1","Name":"GetA","Target":"GetA"});
        let other = json!({"Id":"2","Name":"GetB","Target":"GetB"});
        let one = json!({"Id":"x","SourceArea":"Data","SourceId":"GetA","TargetArea":"Mashup","TargetId":"mashup-root","PropertyMaps":[{"SourceProperty":"a","TargetProperty":"p"}]});
        let two = json!({"Id":"y","SourceArea":"Data","SourceId":"GetB","TargetArea":"Mashup","TargetId":"mashup-root","PropertyMaps":[{"SourceProperty":"b","TargetProperty":"q"}]});
        let mut left = mashup(json!([]));
        left["Data"] = section(json!([get.clone(), other.clone()]));
        left["DataBindings"] = json!([one.clone(), two.clone()]);
        let mut right = mashup(json!([]));
        right["Data"] = section(json!([other, get]));
        right["DataBindings"] = json!([two, one]);
        assert!(compare("a", "b", &left, &right, None, None, Tier::All).equal);
    }

    #[test]
    fn parameters_compare_by_name() {
        let def = |name: &str, base: &str| json!({"ParameterName": name, "BaseType": base});
        let mut left = mashup(json!([]));
        left["UI"]["Properties"]["_currentParameterDefs"] =
            json!([def("Alpha", "STRING"), def("Beta", "STRING")]);
        let mut right = mashup(json!([]));
        right["UI"]["Properties"]["_currentParameterDefs"] = json!([
            def("Aardvark", "NUMBER"),
            def("Alpha", "STRING"),
            def("Beta", "STRING")
        ]);
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        let paths: Vec<_> = result.differences.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, ["UI.Properties._currentParameterDefs.Aardvark"]);
        assert_eq!(result.summary.executable, 1);
    }

    #[test]
    fn composer_editor_state_and_one_sided_defaults_are_not_differences() {
        let chart = |properties: Value| widget("chart", "ptcschartcombo", properties, json!([]));
        let mut left = mashup(json!([chart(json!({
            "_baseVersion": "10.2.0-b75", "isLocked": false, "DisplayName": "ptcs-chartcombo-11",
            "NumberOfReferenceLines": 0, "ReferenceLine7Axis": "yaxis1", "CustomClass": "",
            "DataSeriesStyle3_StateParameters": [], "NumberOfSeries": 2, "DataField2": "b"
        }))]));
        left["UI"]["Properties"]["StyleProperties"] =
            json!({"data": {}, "meta": {"type": "WidgetStyler", "version": "1.0"}});
        let right = mashup(json!([chart(json!({
            "_baseVersion": "10.0.0-b44", "DisplayName": "chart-3", "ReferenceLine7Axis": "xaxis",
            "NumberOfSeries": 2, "DataField2": "b", "DataField9": "ignored past the count"
        }))]));
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        assert!(result.equal, "{:?}", result.differences);
        // Within the count, a series setting is a real difference.
        let mut changed = right.clone();
        changed["UI"]["Widgets"][0]["Properties"]["DataField2"] = json!("c");
        let result = compare("a", "b", &left, &changed, None, None, Tier::All);
        let paths: Vec<_> = result.differences.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, ["UI/w1(ptcschartcombo).Properties.DataField2"]);
    }

    #[test]
    fn caching_switched_off_and_composer_saved_defaults_are_not_differences() {
        let call = |caching: Option<Value>| {
            let mut call = json!({"Name": "Get", "Target": "Get"});
            if let Some(caching) = caching {
                call["Caching"] = caching;
            }
            call
        };
        let mut left = mashup(json!([widget(
            "c",
            "flexcontainer",
            json!({"Scrolling": "auto-scroll"}),
            json!([])
        )]));
        left["UI"]["Properties"]["MinimizeDataStorage"] = json!(false);
        left["Data"] = json!({"Things_Acme.M": {"Services": [call(Some(json!({"cache": false, "cacheKeyParams": []})))]}});
        let mut right = mashup(json!([widget("d", "flexcontainer", json!({}), json!([]))]));
        right["Data"] = json!({"Things_Acme.M": {"Services": [call(None)]}});
        assert!(compare("a", "b", &left, &right, None, None, Tier::All).equal);
        // Caching switched on is a real difference.
        right["Data"]["Things_Acme.M"]["Services"][0]["Caching"] = json!({"cache": true});
        assert_eq!(
            compare("a", "b", &left, &right, None, None, Tier::All)
                .summary
                .executable,
            1
        );
    }

    #[test]
    fn composer_spellings_of_one_binding_are_the_same_binding() {
        let mut left = mashup(json!([]));
        left["DataBindings"] = json!([{"SourceArea": "Mashup", "SourceId": "mashup-root", "SourceSection": "",
            "PropertyMaps": [{"SourceProperty": "Manager", "SourcePropertyType": "property", "TargetProperty": "value"}],
            "TargetArea": "Data", "TargetId": "EntityName", "TargetSection": "S", "SourceName": "Get"}]);
        let mut right = mashup(json!([]));
        right["DataBindings"] = json!([{"SourceArea": "Mashup", "SourceDetails": "", "SourceId": "mashup-root",
            "SourceSection": "", "PropertyMaps": [{"SourceProperty": "Manager", "SourcePropertyType": "Property",
            "TargetProperty": "value"}], "TargetArea": "Data", "TargetId": "EntityName", "TargetSection": "S"}]);
        assert!(compare("a", "b", &left, &right, None, None, Tier::All).equal);
        right["DataBindings"][0]["PropertyMaps"][0]["TargetProperty"] = json!("other");
        assert_eq!(
            compare("a", "b", &left, &right, None, None, Tier::All)
                .summary
                .executable,
            2
        );
    }

    #[test]
    fn an_empty_source_property_is_the_whole_result_and_stays_a_difference() {
        let binding = |maps: Value| {
            json!({"SourceArea": "Data", "SourceId": "Get", "SourceSection": "Things_Acme.M",
                "TargetArea": "UI", "TargetId": "grid", "PropertyMaps": maps})
        };
        let mut left = mashup(json!([]));
        left["DataBindings"] = json!([binding(
            json!([{"SourceProperty": "", "TargetProperty": "Data"}])
        )]);
        let mut right = mashup(json!([]));
        right["DataBindings"] = json!([binding(json!([{"TargetProperty": "Data"}]))]);
        assert_eq!(
            compare("a", "b", &left, &right, None, None, Tier::All)
                .summary
                .executable,
            2
        );
    }

    #[test]
    fn a_responsive_root_ignores_its_childrens_coordinates_and_styles_follow_widget_pairs() {
        let mut left = mashup(json!([widget(
            "container-2",
            "flexcontainer",
            json!({"Top": 0, "Left": 0}),
            json!([])
        )]));
        left["UI"]["Properties"]["ResponsiveLayout"] = json!(true);
        left["UI"]["Properties"]["StyleProperties"] =
            json!({"data": {"container-2": {"widget": "flex-container"}}, "meta": {}});
        let mut right = mashup(json!([widget(
            "flexcontainer-9",
            "flexcontainer",
            json!({}),
            json!([])
        )]));
        right["UI"]["Properties"]["ResponsiveLayout"] = json!(true);
        right["UI"]["Properties"]["StyleProperties"] =
            json!({"data": {"flexcontainer-9": {"widget": "flex-container"}}, "meta": {}});
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        assert!(result.equal, "{:?}", result.differences);
    }

    #[test]
    fn a_duplicated_binding_is_one_difference() {
        let one = json!({"SourceArea":"Data","SourceId":"GetA","TargetArea":"Mashup","TargetId":"mashup-root"});
        let mut left = mashup(json!([]));
        left["DataBindings"] = json!([one.clone()]);
        let mut right = mashup(json!([]));
        right["DataBindings"] = json!([one.clone(), one]);
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        let paths: Vec<_> = result.differences.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, ["DataBindings: data:GetA -> mashup"]);
    }

    fn wired_labels(side: &str, count: usize) -> Value {
        let mut content = mashup(Value::Array(
            (0..count)
                .map(|i| {
                    widget(
                        &format!("{side}-label-{i}"),
                        "ptcslabel",
                        json!({"DisplayName": format!("Acme value {i}")}),
                        json!([]),
                    )
                })
                .collect(),
        ));
        content["DataBindings"] = Value::Array(
            (0..count)
                .map(|i| {
                    json!({"SourceArea": "Data", "SourceSection": "Things_Acme.Manager",
                        "SourceId": format!("GetValue{i}"), "TargetArea": "UI",
                        "TargetId": format!("{side}-label-{i}"),
                        "PropertyMaps": [{"SourceProperty": "result", "TargetProperty": "LabelText"}]})
                })
                .collect(),
        );
        content
    }

    #[test]
    fn ten_reordered_bindings_pair_by_both_endpoints() {
        let left = wired_labels("a", 10);
        let mut right = wired_labels("b", 10);
        right["DataBindings"].as_array_mut().unwrap().reverse();
        assert!(compare("a", "b", &left, &right, None, None, Tier::All).equal);
        right["DataBindings"].as_array_mut().unwrap().remove(4);
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        assert_eq!(result.summary.executable, 1);
        assert_eq!(result.differences.len(), 1);
        let report = render(&result);
        assert!(report.contains("Acme.Manager"), "{report}");
        assert!(report.contains("GetValue5"), "{report}");
        assert!(report.contains("Acme value 5 (ptcslabel)"), "{report}");
        assert!(report.contains("LabelText"), "{report}");
        assert!(!report.contains("a-label-5"), "{report}");
        assert!(result.differences[0].left.is_some());
        assert!(result.differences[0].right.is_none());
        let mut reordered = left.clone();
        reordered["DataBindings"].as_array_mut().unwrap().reverse();
        let same_loss = compare("a", "b", &reordered, &right, None, None, Tier::All);
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            serde_json::to_value(&same_loss).unwrap()
        );
    }

    #[test]
    fn two_services_into_one_label_keep_distinct_binding_keys() {
        let mut left = wired_labels("a", 1);
        let mut other = left["DataBindings"][0].clone();
        other["SourceId"] = json!("GetOtherValue");
        left["DataBindings"].as_array_mut().unwrap().push(other);
        let right = wired_labels("b", 1);
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        assert_eq!(result.summary.executable, 1);
        let report = render(&result);
        assert!(report.contains("GetOtherValue"), "{report}");
        assert!(report.contains("Acme value 0 (ptcslabel)"), "{report}");
    }

    #[test]
    fn reordered_events_pair_widget_and_data_endpoints() {
        let events = |side: &str| {
            json!([
                {"EventTriggerArea": "UI", "EventTriggerId": format!("{side}-label-0"),
                 "EventTriggerEvent": "Clicked", "EventHandlerArea": "Data",
                 "EventHandlerId": "Things_Acme.Manager", "EventHandlerService": "GetValue"},
                {"EventTriggerArea": "Data", "EventTriggerSection": "Things_Acme.Manager",
                 "EventTriggerId": "GetValue", "EventTriggerEvent": "ServiceInvokeCompleted",
                 "EventHandlerArea": "UI", "EventHandlerId": format!("{side}-label-1"),
                 "EventHandlerService": "Reset"}
            ])
        };
        let mut left = wired_labels("a", 2);
        left["Events"] = events("a");
        let mut right = wired_labels("b", 2);
        right["Events"] = events("b");
        right["Events"].as_array_mut().unwrap().reverse();
        assert!(compare("a", "b", &left, &right, None, None, Tier::All).equal);
        right["Events"].as_array_mut().unwrap().remove(0);
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        assert_eq!(result.summary.executable, 1);
        let report = render(&result);
        for endpoint in [
            "Acme.Manager",
            "GetValue",
            "ServiceInvokeCompleted",
            "Acme value 1 (ptcslabel)",
            "Reset",
        ] {
            assert!(report.contains(endpoint), "{report}");
        }
    }

    #[test]
    fn an_extra_root_wrapper_pairs_children_and_their_wiring_in_both_directions() {
        let wrap = |id: &str, name: &str, widgets: Value| {
            widget(
                id,
                "flexcontainer",
                json!({"DisplayName": name, "ResponsiveLayout": true}),
                widgets,
            )
        };
        let mut left = wired_labels("a", 2);
        left["Events"] = json!([{"EventTriggerArea": "UI", "EventTriggerId": "a-label-0",
            "EventTriggerEvent": "Clicked", "EventHandlerArea": "UI", "EventHandlerId": "a-label-1", "EventHandlerService": "Reset"}]);
        left["UI"]["Properties"]["StyleProperties"] =
            json!({"data": {"a-label-0": {"widget": "ptcslabel"}}, "meta": {}});
        left["UI"]["Widgets"] = json!([wrap(
            "a-content",
            "Acme content",
            left["UI"]["Widgets"].clone()
        )]);
        let mut right = wired_labels("b", 2);
        right["Events"] = json!([{"EventTriggerArea": "UI", "EventTriggerId": "b-label-0",
            "EventTriggerEvent": "Clicked", "EventHandlerArea": "UI", "EventHandlerId": "b-label-1", "EventHandlerService": "Reset"}]);
        right["UI"]["Properties"]["StyleProperties"] = json!({"data": {
            "b-label-0": {"widget": "ptcslabel"}, "b-wrapper": {"widget": "flex-container"}}, "meta": {}});
        right["UI"]["Widgets"] = json!([wrap(
            "b-wrapper",
            "Acme wrapper",
            json!([wrap(
                "b-content",
                "Acme content",
                right["UI"]["Widgets"].clone()
            )])
        )]);
        for (left, right, action) in [(&left, &right, "added"), (&right, &left, "removed")] {
            let result = compare("a", "b", left, right, None, None, Tier::All);
            assert_eq!(result.summary.layout, 1, "{:?}", result.differences);
            assert_eq!(result.summary.executable, 0, "{:?}", result.differences);
            assert_eq!(result.summary.visual, 0, "{:?}", result.differences);
            assert!(result.differences[0]
                .path
                .contains(&format!("{action} wrapper Acme wrapper")));
            let selected = compare("a", "b", left, right, None, None, Tier::Executable);
            assert!(selected.equal);
            assert_eq!(selected.summary.layout, 1);
        }
        right["UI"]["Widgets"][0]["Widgets"][0]["Widgets"][0]["Properties"]["LabelText"] =
            json!("Acme changed value");
        let changed = compare("a", "b", &left, &right, None, None, Tier::All);
        assert_eq!(changed.summary.layout, 1);
        assert_eq!(changed.summary.visual, 1);
        assert_eq!(changed.summary.executable, 0);
    }

    #[test]
    fn a_wrapper_directly_under_the_mashup_root_keeps_leaf_bindings_paired() {
        let left = wired_labels("a", 2);
        let mut right = wired_labels("b", 2);
        right["UI"]["Widgets"] = json!([widget(
            "b-wrapper",
            "flexcontainer",
            json!({"DisplayName": "Acme root wrapper"}),
            right["UI"]["Widgets"].clone()
        )]);
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        assert_eq!(result.summary.layout, 1, "{:?}", result.differences);
        assert_eq!(result.summary.executable, 0, "{:?}", result.differences);
        assert_eq!(result.summary.visual, 0, "{:?}", result.differences);
        assert_eq!(
            result.differences[0].path,
            "UI/added wrapper Acme root wrapper (flexcontainer)"
        );
    }

    #[test]
    fn an_inner_wrapper_keeps_the_existing_container_paired() {
        let mut left = wired_labels("a", 2);
        let mut right = wired_labels("b", 2);
        let container = |id: &str, widgets: Value| {
            widget(
                id,
                "flexcontainer",
                json!({"DisplayName": "Acme content", "Width": 275}),
                widgets,
            )
        };
        left["UI"]["Widgets"] = json!([container("a-content", left["UI"]["Widgets"].clone())]);
        right["UI"]["Widgets"] = json!([container(
            "b-content",
            json!([widget(
                "b-wrapper",
                "flexcontainer",
                json!({"DisplayName": "Acme inner wrapper"}),
                right["UI"]["Widgets"].clone()
            )])
        )]);
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        assert_eq!(result.summary.layout, 1, "{:?}", result.differences);
        assert_eq!(result.summary.executable, 0, "{:?}", result.differences);
        assert_eq!(result.summary.visual, 0, "{:?}", result.differences);
        assert_eq!(
            result.differences[0].path,
            "UI/w1(flexcontainer)/added wrapper Acme inner wrapper (flexcontainer)"
        );
    }

    #[test]
    fn a_wrapper_beside_a_sibling_pairs_the_retained_container_and_its_references() {
        let mut left = wired_labels("a", 2);
        let mut right = wired_labels("b", 2);
        for (content, side) in [(&mut left, "a"), (&mut right, "b")] {
            let container = widget(
                &format!("{side}-content"),
                "flexcontainer",
                json!({"DisplayName": "Acme content", "Width": 275}),
                content["UI"]["Widgets"].clone(),
            );
            content["UI"]["Widgets"] = json!([
                if side == "b" {
                    widget(
                        "b-wrapper",
                        "flexcontainer",
                        json!({"DisplayName": "Acme wrapper"}),
                        json!([container]),
                    )
                } else {
                    container
                },
                widget(
                    &format!("{side}-button"),
                    "ptcsbutton",
                    json!({}),
                    json!([])
                )
            ]);
            content["DataBindings"].as_array_mut().unwrap().push(json!({
                "SourceArea": "UI", "SourceId": format!("{side}-content"),
                "TargetArea": "UI", "TargetId": format!("{side}-label-0"),
                "PropertyMaps": [{"SourceProperty": "Width", "TargetProperty": "LabelText"}]
            }));
        }
        for (left, right, action) in [(&left, &right, "added"), (&right, &left, "removed")] {
            let result = compare("a", "b", left, right, None, None, Tier::All);
            assert_eq!(result.differences.len(), 1, "{:?}", result.differences);
            assert_eq!(result.summary.layout, 1);
            assert_eq!(
                result.differences[0].path,
                format!("UI/{action} wrapper Acme wrapper (flexcontainer)")
            );
        }
    }

    #[test]
    fn endpoint_metadata_changes_pair_once_and_duplicate_order_is_ignored() {
        let mut left = wired_labels("a", 1);
        left["DataBindings"][0]["PropertyMaps"][0]["TargetPropertyBaseType"] = json!("STRING");
        let mut right = wired_labels("b", 1);
        right["DataBindings"][0]["PropertyMaps"][0]["TargetPropertyBaseType"] = json!("NUMBER");
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        assert_eq!(result.summary.executable, 1);
        assert!(result.differences[0].left.is_some());
        assert!(result.differences[0].right.is_some());
        let mut other = left["DataBindings"][0].clone();
        other["PropertyMaps"][0]["TargetPropertyBaseType"] = json!("NUMBER");
        left["DataBindings"].as_array_mut().unwrap().push(other);
        right = left.clone();
        right["DataBindings"].as_array_mut().unwrap().reverse();
        assert!(compare("a", "b", &left, &right, None, None, Tier::All).equal);
    }

    #[test]
    fn selected_tier_keeps_full_summary() {
        let mut right = mashup(json!([]));
        right["mashupType"] = json!("master");
        let result = compare(
            "a",
            "b",
            &mashup(json!([])),
            &right,
            None,
            None,
            Tier::Visual,
        );
        assert!(result.equal);
        assert_eq!(result.summary.executable, 1);
    }

    #[test]
    fn responsive_children_ignore_coordinates() {
        let left = mashup(json!([widget(
            "flex-a",
            "flexcontainer",
            json!({"ResponsiveLayout":true}),
            json!([widget("label-a", "ptcslabel", json!({"Top":1}), json!([]))])
        )]));
        let right = mashup(json!([widget(
            "flex-b",
            "flexcontainer",
            json!({"ResponsiveLayout":true}),
            json!([widget("label-b", "ptcslabel", json!({"Top":2}), json!([]))])
        )]));
        assert!(compare("a", "b", &left, &right, None, None, Tier::All).equal);
    }

    #[test]
    fn static_children_keep_coordinates_as_layout() {
        let left = mashup(json!([widget(
            "container-a",
            "flexcontainer",
            json!({"ResponsiveLayout":false}),
            json!([widget("label-a", "ptcslabel", json!({"Top":1}), json!([]))])
        )]));
        let right = mashup(json!([widget(
            "container-b",
            "flexcontainer",
            json!({"ResponsiveLayout":false}),
            json!([widget("label-b", "ptcslabel", json!({"Top":2}), json!([]))])
        )]));
        let result = compare("a", "b", &left, &right, None, None, Tier::All);
        assert_eq!(result.summary.layout, 1);
        assert_eq!(
            result.differences[0].path,
            "UI/w1(flexcontainer)/w2(ptcslabel).Properties.Top"
        );
    }

    #[test]
    fn css_newlines_are_not_a_difference() {
        let content = mashup(json!([]));
        assert!(
            compare(
                "a",
                "b",
                &content,
                &content,
                Some(".acme { color: blue; }\r\n"),
                Some(".acme { color: blue; }\n"),
                Tier::All,
            )
            .equal
        );
    }
}
