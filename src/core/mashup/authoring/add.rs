//! `mashup add` and `mashup add-contained`: a widget, function or contained mashup placed in an
//! existing mashup.

use super::{
    cache_entry, contained_parameters, default_value, finish_edit, load_existing,
    navigation_definition, parse_loaded, validate_parameter, AuthoringError, EditPlan,
    ParameterDefinition,
};
use crate::core::config::Solution;
use crate::core::mashup::edit::{MashupDocument, Placement};
use crate::core::mashup::layout;
use crate::core::{extension_widgets, mashup, widgets};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, serde::Serialize)]
pub struct ContainerContents {
    pub id: String,
    pub holds: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TabSetOptions {
    pub labels: Option<Vec<String>>,
    pub style: Option<String>,
    pub width: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AddRequest {
    pub mashup: String,
    pub widget: String,
    /// `name:TYPE` or `name:TYPE=default` inputs of an expression or validator.
    pub inputs: Vec<String>,
    pub placement: Option<Placement>,
    pub label: Option<String>,
    pub properties: Vec<(String, String)>,
    pub tabs: TabSetOptions,
}

#[derive(Debug)]
pub struct AddPlan {
    pub edit: EditPlan,
    pub placed: mashup::edit::Placed,
    pub container_contents: Vec<ContainerContents>,
    pub container_tree: Vec<String>,
    /// The widget's type, such as `ptcsgrid`.
    pub widget: String,
    /// Composer's label for it, such as `Grid`.
    pub label: String,
    /// A function widget, which goes beside the layout.
    pub function: bool,
    pub notes: Vec<String>,
}

/// A contained mashup is a `mashupcontainer` whose public properties mirror the child's
/// parameters.  It deliberately has its own request rather than pretending that the catalogue
/// knows the child-specific contract.
#[derive(Clone, Debug)]
pub struct AddContainedRequest {
    pub mashup: String,
    pub child: String,
    pub placement: Option<Placement>,
    pub settings: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct AddContainedPlan {
    pub notes: Vec<String>,
    pub(crate) edit: EditPlan,
    pub placed: mashup::edit::Placed,
    pub child: String,
    pub parameters: Vec<ContainedParameter>,
    pub(crate) settings: Vec<String>,
}

#[derive(Debug)]
pub struct ContainedParameter {
    pub name: String,
    pub base_type: String,
    pub direction: String,
}

/// Add a function widget under the mashup root, beside the layout, as Composer does; an expression
/// or validator gets its `name:TYPE` inputs as bindable targets.
fn add_function_widget(
    document: &mut MashupDocument,
    widget: &widgets::Widget,
    request: &AddRequest,
) -> Result<mashup::edit::Placed, AuthoringError> {
    if request.placement.is_some() {
        return Err(AuthoringError::Invalid(format!(
            "{} is a function widget: it sits beside the layout, so leave out --in and --right-of/--left-of/--above/--below",
            widget.type_id
        )));
    }
    let takes_inputs = matches!(
        widget.type_id.as_str(),
        "expression" | "expression2" | "validator" | "validator2"
    );
    if !request.inputs.is_empty() && !takes_inputs {
        return Err(AuthoringError::Invalid(format!(
            "--input is for an expression or a validator, not {}",
            widget.type_id
        )));
    }
    let mut definitions = Vec::new();
    for text in &request.inputs {
        let (declaration, default) = match text.split_once('=') {
            Some((declaration, default)) => (declaration, Some(default)),
            None => (text.as_str(), None),
        };
        let (name, base_type) = declaration.split_once(':').ok_or_else(|| {
            AuthoringError::Invalid(format!("--input {text:?} must be name:TYPE"))
        })?;
        validate_parameter(name, base_type, "IN", Some(""))?;
        if definitions
            .iter()
            .any(|existing: &ParameterDefinition| existing.name == name)
        {
            return Err(AuthoringError::Invalid(format!(
                "input {name} is given twice"
            )));
        }
        let default = default
            .map(|value| {
                default_value(base_type, value).map_err(|error| {
                    AuthoringError::Invalid(format!(
                        "--input {text:?} has an invalid {base_type} default: {error}"
                    ))
                })
            })
            .transpose()?;
        definitions.push(ParameterDefinition {
            name: name.into(),
            base_type: base_type.into(),
            direction: "IN".into(),
            description: String::new(),
            default,
            data_shape: None,
            visible: true,
        });
    }
    let number = document.allocate();
    let mut value = mashup::edit::new_widget(widget, number, false);
    if takes_inputs {
        let entries = definitions
            .iter()
            .map(|def| navigation_definition(def, def.default.as_ref()))
            .collect();
        mashup::edit::insert_sorted(
            mashup::edit::properties_mut(&mut value),
            "_currentParameterDefs",
            Value::Array(entries),
        );
    }
    let id = value["Properties"]["Id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    document.push_child(&[], value);
    Ok(mashup::edit::Placed {
        id,
        ..mashup::edit::Placed::default()
    })
}

/// A function widget's property that its definition makes at run time: an expression's or
/// validator's inputs and output, an events router's inputs and output, a navigation function's
/// copy of its target's parameters. As (can be a source, can be a target, base type).
pub(crate) fn function_property(
    widget: &Value,
    type_id: &str,
    property: &str,
) -> Option<(bool, bool, String)> {
    let properties = &widget["Properties"];
    let input = || {
        properties["_currentParameterDefs"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["ParameterName"] == property)
            .map(|entry| {
                (
                    entry["isBindingSource"] == true,
                    entry["isBindingTarget"] != false,
                    entry["BaseType"].as_str().unwrap_or_default().to_string(),
                )
            })
    };
    match type_id {
        "expression" | "expression2" if property == "Output" => Some((
            true,
            false,
            properties["OutputBaseType"]
                .as_str()
                .unwrap_or("STRING")
                .to_string(),
        )),
        "expression" | "expression2" | "validator" | "validator2" => input(),
        "eventsrouter" => {
            let data_type = properties["DataType"]
                .as_str()
                .unwrap_or("STRING")
                .to_string();
            if property == "Output" {
                return Some((true, false, data_type));
            }
            let count = match &properties["NumberOfInputs"] {
                Value::String(text) => text.parse().unwrap_or(0),
                other => other.as_u64().unwrap_or(0),
            };
            let n: u64 = property.strip_prefix("Input")?.parse().ok()?;
            (1..=count).contains(&n).then_some((false, true, data_type))
        }
        "navigationfunction" => properties["MashupParameters"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|entry| entry["ParameterName"] == property)
            .map(|entry| {
                let direction = entry["ParmDef"]["aspects"]["bindingDirection"]
                    .as_str()
                    .unwrap_or("BOTH");
                (
                    direction != "IN",
                    direction != "OUT",
                    entry["BaseType"].as_str().unwrap_or_default().to_string(),
                )
            }),
        _ => None,
    }
}

fn validate_properties(request: &AddRequest) -> Result<(), AuthoringError> {
    for (_, value) in &request.properties {
        crate::core::repo::refuse_git_bash_value(&Value::String(value.clone()))
            .map_err(|error| AuthoringError::Invalid(error.to_string()))?;
    }
    Ok(())
}

/// Plan adding one catalogue widget to an existing mashup. Nothing is written.
pub(crate) fn plan_add(
    solution: &Solution,
    request: &AddRequest,
) -> Result<AddPlan, AuthoringError> {
    validate_properties(request)?;
    let loaded = load_existing(solution, &request.mashup)?;
    plan_add_loaded(solution, request, loaded, None)
}

pub(super) fn plan_add_loaded(
    solution: &Solution,
    request: &AddRequest,
    loaded: super::ExistingMashup,
    tokens: Option<&crate::core::mashup::tokens::Tokens>,
) -> Result<AddPlan, AuthoringError> {
    validate_properties(request)?;
    let mut document = parse_loaded(&loaded)?;
    // A platform widget first, then one of the solution's own extension projects defines.
    let defined;
    // A separator is the kits' 1px rule between rows: an empty container, not a widget.
    let separator = request.widget == "separator";
    let widget = if separator {
        widgets::catalog()
            .by_type("flexcontainer")
            .expect("the catalogue has flexcontainer")
    } else {
        match widgets::catalog().resolve_for_add(&request.widget) {
            Ok(widget) => widget,
            Err(error) => match extension_widgets::find(solution, &request.widget) {
                (Some(found), _) => {
                    defined = found;
                    &defined
                }
                (None, skipped) => {
                    let unread = if skipped.is_empty() {
                        String::new()
                    } else {
                        format!(" (unread: {})", skipped.join("; "))
                    };
                    return Err(AuthoringError::Invalid(format!(
                    "{error}; nor does an [[extension]] project of this solution define it, nor an extension `twaco types --platform` saw installed on the server{unread}"
                )));
                }
            },
        }
    };
    let TabSetOptions {
        labels,
        style,
        width,
    } = &request.tabs;
    let adjusted_request;
    let request = if labels.is_some() || style.is_some() || width.is_some() {
        if widget.type_id != "ptcstabset" {
            return Err(AuthoringError::Invalid(
                "--tabs, --tab-style and --tab-width are for ptcstabset".into(),
            ));
        }
        let mut adjusted = request.clone();
        if let Some(labels) = labels {
            if !(1..=16).contains(&labels.len())
                || labels.iter().any(|label| label.trim().is_empty())
            {
                return Err(AuthoringError::Invalid(
                    "--tabs needs 1 to 16 nonempty, comma-separated tab names".into(),
                ));
            }
            if adjusted.properties.iter().any(|(name, _)| {
                name == "NumberOfTabs"
                    || name == "Items"
                    || (name.starts_with("Tab") && name.ends_with("Name"))
            }) {
                return Err(AuthoringError::Invalid(
                    "--tabs already sets NumberOfTabs, Items and Tab<n>Name".into(),
                ));
            }
            adjusted
                .properties
                .push(("NumberOfTabs".into(), labels.len().to_string()));
            adjusted.properties.extend(
                labels
                    .iter()
                    .enumerate()
                    .map(|(i, label)| (format!("Tab{}Name", i + 1), label.trim().into())),
            );
        }
        for (name, value) in [("TabStyle", style), ("TabWidthConfig", width)] {
            if let Some(value) = value {
                if adjusted.properties.iter().any(|(key, _)| key == name) {
                    return Err(AuthoringError::Invalid(format!("give {name} only once")));
                }
                adjusted.properties.push((name.into(), value.clone()));
            }
        }
        adjusted_request = adjusted;
        &adjusted_request
    } else {
        request
    };
    let placed = if widget.function {
        add_function_widget(&mut document, widget, request)?
    } else {
        if !request.inputs.is_empty() {
            return Err(AuthoringError::Invalid(format!(
                "--input is for an expression or a validator, not {}",
                widget.type_id
            )));
        }
        let placement =
            match &request.placement {
                Some(placement) => placement.clone(),
                None if document.find("center").is_ok() => Placement::In("center".to_string()),
                None => return Err(AuthoringError::Invalid(
                    "say where: --in <container>, or --right-of/--left-of/--above/--below <widget>"
                        .to_string(),
                )),
            };
        if separator {
            let (Placement::Beside {
                anchor,
                side: mashup::edit::Side::Above | mashup::edit::Side::Below,
            }
            | Placement::BelowInColumn(anchor)) = &placement
            else {
                return Err(AuthoringError::Invalid(
                    "a separator goes between rows: --below or --above a widget".into(),
                ));
            };
            // A card is not a row: a rule below one card of a row of cards would wrap it.
            let found = document.find(anchor).map_err(AuthoringError::Edit)?;
            if document.at(&found.path)["Properties"]["DisplayName"]
                .as_str()
                .is_some_and(|name| name.starts_with("panel-"))
            {
                return Err(AuthoringError::Invalid(format!(
                    "{} is a card; place the separator --below a widget inside it",
                    found.id
                )));
            }
            // Its colour goes in the root style map, which must be Composer's shape.
            super::set::style_properties(document.root_properties(), &request.mashup)?;
        }
        // Placement measures the rows it touches, so it sees the widget as --prop leaves it (a
        // margin or a label type changes its height); the loop below still checks and sets each.
        let adjusted;
        let placing = if request.properties.is_empty() && request.label.is_none() {
            widget
        } else {
            let mut copy = widget.clone();
            if let Some(label) = &request.label {
                if let Some(property) = ["LabelText", "Label", "Text", "Title"]
                    .into_iter()
                    .find(|name| widget.defaults.contains_key(*name))
                {
                    copy.defaults
                        .insert(property.into(), Value::String(label.clone()));
                }
            }
            for (name, text) in &request.properties {
                // A catalogue default, or a contract property an extension declares without one.
                if copy.defaults.contains_key(name) || widget.property(name).is_some() {
                    if let Ok(value) = property_value(widget, name, Value::String(text.clone())) {
                        copy.defaults.insert(name.clone(), value);
                    }
                }
            }
            adjusted = copy;
            &adjusted
        };
        let placed = document
            .place(placing, &placement)
            .map_err(AuthoringError::Edit)?;
        if separator {
            let active;
            let tokens = match tokens {
                Some(tokens) => tokens,
                None => {
                    active = crate::core::mashup::tokens::Tokens::active(solution)
                        .map_err(AuthoringError::Invalid)?;
                    &active
                }
            };
            document
                .make_separator(&placed.id)
                .map_err(AuthoringError::Edit)?;
            super::set::copy_style_entry(
                &mut document,
                &placed.id,
                serde_json::json!({
                    "parts": {"$": {"$": {"background": tokens.colour("line").map_err(AuthoringError::Invalid)?}}},
                    "widget": "flex-container"
                }),
                None,
                &mut Vec::new(),
            );
        }
        placed
    };
    if let Some(label) = &request.label {
        let property = ["LabelText", "Label", "Text", "Title"]
            .into_iter()
            .find(|name| widget.defaults.contains_key(*name))
            .ok_or_else(|| {
                AuthoringError::Invalid(format!(
                    "{} has no label property; use --prop",
                    widget.type_id
                ))
            })?;
        let path = document
            .find(&placed.id)
            .map_err(AuthoringError::Edit)?
            .path;
        document.set_property(&path, property, Value::String(label.clone()));
    }
    let placed_path = document
        .find(&placed.id)
        .map_err(AuthoringError::Edit)?
        .path;
    let node = layout::context::Node::op("add", layout::Element::Widget)
        .widget(&widget.type_id)
        .fact(
            "in-filter-footer",
            layout::context::in_filter_footer(&document, &placed_path),
        )
        .fact(
            "narrow-column",
            widget.type_id == "ptcsdatepicker"
                && layout::context::narrow_column(&document, &placed_path),
        );
    let node = match request.label.as_deref() {
        Some(label) => node.label(label),
        None => node,
    };
    let decided = document.rules().evaluate(&node.context());
    for (key, value) in decided.patch() {
        document.set_property(&placed_path, key, value);
    }
    if document.rules().version() == "v1"
        && widget.type_id == "ptcsbutton"
        && layout::context::in_filter_footer(&document, &placed_path)
    {
        // AdminPage01 uses 120px buttons; the narrower 280px column needs 108px with its kit margins.
        let parent = &placed_path[..placed_path.len() - 1];
        if document.at(parent)["Widgets"]
            .as_array()
            .is_some_and(|rows| rows.len() > 1)
        {
            document.set_property(&placed_path, "Margin", Value::from("24 24 24 8"));
        }
    }
    if widget.type_id == "ptcskpidial"
        && request.label.is_none()
        && document.value()["twacoPageRules"] == "v1"
    {
        document.set_property(&placed_path, "Label", Value::String(String::new()));
    }
    let mut properties = request
        .properties
        .iter()
        .map(|(name, value)| (name.clone(), Value::String(value.clone())))
        .collect();
    let notes =
        super::set::menubar_properties(&document, &widget.type_id, &placed.id, &mut properties)?;

    let mut configured = document.at(&placed_path)["Properties"].clone();
    for (name, text) in &request.properties {
        configured[name] =
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()));
    }

    for (name, text) in &request.properties {
        if [
            "Id",
            "Type",
            "DisplayName",
            "__TypeDisplayName",
            "LastContainer",
            "ResponsiveLayout",
        ]
        .contains(&name.as_str())
        {
            return Err(AuthoringError::Invalid(format!("twaco sets {name}")));
        }
        if ["_currentParameterDefs", "MashupParameters"].contains(&name.as_str()) {
            return Err(AuthoringError::Refused(format!(
                "{name} is managed by mashup param / add-contained / the function-widget inputs; use those"
            )));
        }
        let value = match widget.defaults.get(name) {
            Some(_) => property_value(widget, name, Value::String(text.to_string()))?,
            // Extension definitions have historically accepted arbitrary JSON for an opaque
            // property. Keep that compatibility when their base type gives us no scalar shape.
            None if widget.extension && widget.property(name).is_some() => {
                let base = widget
                    .property(name)
                    .and_then(|contract| contract.base_type.as_deref());
                if matches!(
                    base,
                    Some(
                        "BOOLEAN"
                            | "NUMBER"
                            | "INTEGER"
                            | "OBJECT"
                            | "JSON"
                            | "STATEFORMATTING"
                            | "RENDERERWITHSTATE"
                    )
                ) {
                    property_value(widget, name, Value::String(text.to_string()))?
                } else {
                    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
                }
            }
            // Catalogue contracts without literal defaults still have a base type, including
            // structured formatter values Composer leaves unset on a dropped widget.
            None if widget.property(name).is_some() => {
                property_value(widget, name, Value::String(text.to_string()))?
            }
            None if widget.numbered_default(name).is_some() => {
                property_value(widget, name, Value::String(text.to_string()))?
            }
            None => {
                if let Some(contract) =
                    widgets::dynamic_property(&widget.type_id, &configured, name, false)
                {
                    let mut dynamic = widget.clone();
                    dynamic
                        .properties
                        .get_or_insert_default()
                        .insert(name.clone(), contract);
                    let value = property_value(&dynamic, name, Value::String(text.clone()))?;
                    document.set_property(&placed_path, name, value);
                    continue;
                }
                // A stale legacy name a mashup still carries (Composer renames it when it opens
                // the mashup): kept as written, never dropped.
                if widgets::migrated_property(&widget.type_id, name).is_some()
                    && widgets::migration_changes_values(&widget.type_id, name)
                {
                    let value =
                        serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()));
                    document.set_property(&placed_path, name, value);
                    continue;
                }
                return Err(AuthoringError::Invalid(
                    widget.no_property(&widget.type_id, name),
                ));
            }
        };
        if widget.property(name).is_some() && !property_editable(widget, name) {
            return Err(AuthoringError::Refused(format!("{name} is not editable")));
        }
        check_property_options(widget, name, &value, false)?;
        let path = document
            .find(&placed.id)
            .map_err(AuthoringError::Edit)?
            .path;
        document.set_property(&path, name, value);
    }
    if !notes.is_empty() {
        let path = document
            .find_exact(&placed.id)
            .map_err(AuthoringError::Edit)?
            .path;
        document.set_property(&path, "DataSource", json!("menuentity"));
    }
    if widget.type_id == "flexcontainer"
        && request.properties.iter().any(|(key, _)| sizes_layout(key))
    {
        let path = document
            .find_exact(&placed.id)
            .map_err(AuthoringError::Edit)?
            .path;
        document
            .at_mut(&path)
            .as_object_mut()
            .expect("widget")
            .insert("twacoLayoutAuthored".into(), Value::Bool(true));
    }
    let mut notes = notes;
    if !separator
        && widgets::catalog().resolve(&request.widget).is_err()
        && widgets::catalog().resolve_for_add(&request.widget).is_ok()
    {
        // An everyday word that is also a legacy type id: say which widget was used, because
        // a legacy one already on a page has other events.
        let events = widget
            .events
            .as_deref()
            .filter(|events| !events.is_empty())
            .map(|events| format!("; its events are {}", events.join(", ")))
            .unwrap_or_default();
        notes.push(format!(
            "{} -> {}: the legacy `{}` widget cannot be authored, so its modern counterpart was added; it has other events and properties than a legacy `{}` already on a page{events}",
            request.widget.trim(),
            widget.type_id,
            request.widget.trim().to_ascii_lowercase(),
            request.widget.trim().to_ascii_lowercase(),
        ));
    }
    for (name, value) in &request.properties {
        if let Some(note) = crate::core::repo::git_bash_path_note(&Value::String(value.clone())) {
            notes.push(format!("{name}: {note}"));
        }
    }
    let path = document
        .find_exact(&placed.id)
        .map_err(AuthoringError::Edit)?
        .path;
    let mut limit_properties = request
        .properties
        .iter()
        .map(|(name, text)| (name.clone(), Value::String(text.clone())))
        .collect();
    notes.extend(super::set::menubar_limit(
        solution,
        &document,
        &widget.type_id,
        &placed.id,
        &mut limit_properties,
    )?);
    for (name, value) in limit_properties {
        if name == "PrimaryNavMaxItems" {
            document.set_property(&path, &name, super::property_value(widget, &name, value)?);
        }
    }

    // A tab set shows one container per tab; without them Composer has nowhere to drop a tab's content.
    if widget.type_id == "ptcstabset" {
        let path = document
            .find_exact(&placed.id)
            .map_err(AuthoringError::Edit)?
            .path;
        let count = super::new::tab_count(&document.at(&path)["Properties"]["NumberOfTabs"]);
        let tabs = super::new::sync_tabs(&mut document, &placed.id, count)?;
        notes.push(format!(
            "{} tab container(s): {}; add a tab's content with --in <container>",
            tabs.len(),
            tabs.join(", ")
        ));
    }
    if let Some(row) = &placed.below_row {
        let found = document.find_exact(row).map_err(AuthoringError::Edit)?;
        notes.push(format!(
            "{} row {} ({row}); new content is a sibling row; fixed rows keep their height; use --in-column to stack within the panel",
            if matches!(&request.placement, Some(Placement::Beside { side: mashup::edit::Side::Above, .. })) { "above" } else { "below" },
            found.display_name
        ));
    }
    // Each add beside the same anchor lands immediately next to it, so a series reverses unless
    // it chains from the widget just added.
    match &request.placement {
        Some(Placement::Beside {
            anchor,
            side: mashup::edit::Side::Right,
        }) => notes.push(format!(
            "next: --right-of {} keeps order (--right-of {anchor} again would insert before it)",
            placed.id
        )),
        Some(Placement::Beside {
            anchor,
            side: mashup::edit::Side::Left,
        }) => notes.push(format!(
            "next: --left-of {} adds further left (--left-of {anchor} again keeps order)",
            placed.id
        )),
        _ => {}
    }
    document.fit_placed_width(&placed.id);
    document.reflow_for(&placed.id);
    let container_contents = placed
        .containers
        .iter()
        .map(|id| {
            let found = document.find_exact(id).expect("a new container exists");
            let holds = document.at(&found.path)["Widgets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|child| !mashup::edit::is_spacer(child))
                .filter_map(|child| child["Properties"]["Id"].as_str().map(str::to_string))
                .collect();
            ContainerContents {
                id: id.clone(),
                holds,
            }
        })
        .collect();
    let container_tree = match &request.placement {
        Some(Placement::Beside {
            anchor,
            side: mashup::edit::Side::Right | mashup::edit::Side::End,
        }) => {
            let anchor = document.find(anchor).map_err(AuthoringError::Edit)?;
            let new = document
                .find_exact(&placed.id)
                .map_err(AuthoringError::Edit)?;
            let shared = anchor
                .path
                .iter()
                .zip(&new.path)
                .take_while(|(a, b)| a == b)
                .count();
            let shared = shared
                .min(new.path.len().saturating_sub(1))
                .min(anchor.path.len().saturating_sub(1));
            mashup::describe::tree_lines(document.at(&new.path[..shared]))
        }
        _ => Vec::new(),
    };
    Ok(AddPlan {
        container_tree,
        container_contents,
        edit: finish_edit(loaded, document, "mashup add")?,
        placed,
        widget: widget.type_id.clone(),
        label: widget.type_display_name.clone(),
        function: widget.function,
        notes,
    })
}

/// Whether a property fixes a container's size, which twaco then leaves as it was set. The
/// direction and wrapping only say how its content runs: a row added with them still takes its
/// content's height.
fn sizes_layout(key: &str) -> bool {
    (key.starts_with("flex-") && !matches!(key, "flex-direction" | "flex-wrap"))
        || matches!(key, "Width" | "Height")
}

pub(super) fn contained_page_warning(child: &str, document: &MashupDocument) -> Option<String> {
    let page = document.widgets().iter().any(|widget| {
        widget.display_name == "page"
            && document.at(&widget.path)["twacoTemplateName"] != "tab-content"
    });
    page.then(|| format!("{child}: page template fills its own page and may scroll inside this tab; use --template tab-content for a responsive content mashup with one growing layout and no title or gutter, as in the kit, and check nested scrolling at runtime"))
}

/// Plan adding a child mashup as Composer's contained-mashup builder does.  The child is read
/// from the repository, never from a server, so the cache is deterministic and reviewable.
pub(crate) fn plan_add_contained(
    solution: &Solution,
    request: &AddContainedRequest,
) -> Result<AddContainedPlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    plan_add_contained_loaded(solution, request, loaded)
}

pub(super) fn plan_add_contained_loaded(
    solution: &Solution,
    request: &AddContainedRequest,
    loaded: super::ExistingMashup,
) -> Result<AddContainedPlan, AuthoringError> {
    if request.mashup == request.child {
        return Err(AuthoringError::Refused(
            "a mashup cannot contain itself".into(),
        ));
    }
    let child = load_existing(solution, &request.child)?;
    let child_document = parse_loaded(&child)?;
    if child_document.value()["mashupType"] == "targetmashup" {
        return Err(AuthoringError::Refused(
            "a master mashup is shown by a page container, not contained".into(),
        ));
    }
    let definitions = contained_parameters(&child, &child_document)?;
    let placement = match &request.placement {
        Some(placement) => placement.clone(),
        None if parse_loaded(&loaded)?.find("center").is_ok() => Placement::In("center".into()),
        None => {
            return Err(AuthoringError::Invalid(
                "say where: --in <container>, or --right-of/--left-of/--above/--below <widget>"
                    .into(),
            ))
        }
    };
    let mut document = parse_loaded(&loaded)?;
    let widget = widgets::catalog()
        .by_type("mashupcontainer")
        .expect("the embedded catalogue has mashupcontainer");
    let placed = document
        .place(widget, &placement)
        .map_err(AuthoringError::Edit)?;
    let path = document
        .find(&placed.id)
        .map_err(AuthoringError::Edit)?
        .path;
    document.set_property(&path, "Name", json!(request.child));
    let tab_parent = (1..path.len()).find_map(|depth| {
        let parent = &path[..depth];
        (document.at(&parent[..parent.len() - 1])["Properties"]["Type"] == "ptcstabset")
            .then(|| parent.to_vec())
    });
    let mut notes = Vec::new();
    if let Some(tab) = &tab_parent {
        let rules = document.rules();
        let in_tab =
            |element| layout::context::Node::op("add-contained", element).fact("in-tab", true);
        let tab_node = in_tab(layout::Element::Tab);
        for (key, value) in rules.evaluate(&tab_node.context()).patch() {
            document.set_property(tab, key, value);
        }
        let held = in_tab(layout::Element::Widget).widget("mashupcontainer");
        for (key, value) in rules.evaluate(&held.context()).patch() {
            document.set_property(&path, key, value);
        }
        notes.extend(contained_page_warning(&request.child, &child_document));
    }
    // Placement floored the row at Composer's 300px drop height; the child's own rows say how
    // tall it really is, so a KPI tile is neither clipped nor padded out when the page is short.
    if let Some(height) = child_document
        .natural_page_height()
        .filter(|_| tab_parent.is_none())
    {
        document.floor_contained_row(&placed.id, height, &placed.containers);
    }
    document.set_property(
        &path,
        "MashupParameters",
        Value::Array(
            definitions
                .iter()
                .map(|def| cache_entry(def, def.default.clone()))
                .collect(),
        ),
    );
    let visible = definitions
        .iter()
        .filter(|definition| definition.visible && definition.direction != "OUT")
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    let mut settings = Vec::new();
    for (name, text) in &request.settings {
        let definition = definitions
            .iter()
            .find(|definition| definition.name == *name)
            .ok_or_else(|| {
                AuthoringError::Invalid(format!(
                    "{} has no settable parameter {name}; settable parameters are {}",
                    request.child,
                    visible.join(", ")
                ))
            })?;
        if !definition.visible || definition.direction == "OUT" {
            return Err(AuthoringError::Invalid(format!(
                "{}.{name} is not a settable parameter; settable parameters are {}",
                request.child,
                visible.join(", ")
            )));
        }
        let value = default_value(&definition.base_type, text)?;
        document.set_property(&path, name, value.clone());
        let properties = mashup::edit::properties_mut(document.at_mut(&path));
        if let Some(entry) = properties["MashupParameters"]
            .as_array_mut()
            .and_then(|entries| {
                entries
                    .iter_mut()
                    .find(|entry| entry["ParameterName"] == *name)
            })
        {
            mashup::edit::insert_sorted(
                entry.as_object_mut().expect("cache entries are objects"),
                "DefaultValue",
                value,
            );
        }
        settings.push(name.clone());
    }
    Ok(AddContainedPlan {
        notes,
        edit: finish_edit(loaded, document, "mashup add-contained")?,
        placed,
        child: request.child.clone(),
        parameters: definitions
            .iter()
            .map(|definition| ContainedParameter {
                name: definition.name.clone(),
                base_type: definition.base_type.clone(),
                direction: definition.direction.clone(),
            })
            .collect(),
        settings,
    })
}

pub(crate) fn property_editable(widget: &widgets::Widget, name: &str) -> bool {
    // The catalogue marks Data non-editable; exported rich-text headers still carry literal HTML.
    (widget.type_id == "ptcsrte" && name == "Data")
        || widget
            .property(name)
            .is_none_or(|contract| contract.editable)
}

/// Composer select settings carry a finite list; force permits a platform-specific override.
pub(super) fn check_property_options(
    widget: &widgets::Widget,
    name: &str,
    value: &Value,
    force: bool,
) -> Result<(), AuthoringError> {
    if let Some(contract) = widget.property(name).filter(|_| !force) {
        let mut options = contract.allowed_values().peekable();
        if options.peek().is_some()
            && !options.any(|option| {
                option == value
                    || option
                        .as_str()
                        .zip(value.as_str())
                        .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
            })
        {
            let allowed = contract
                .allowed_values()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(AuthoringError::Invalid(format!(
                "{name} for {} must be one of {allowed}",
                widget.type_id
            )));
        }
    }
    Ok(())
}

/// Parse and check a value set by `--prop` or `mashup set`. Strings have CLI syntax; a non-string
/// value arrived from MCP and is already JSON.
pub(crate) fn property_value(
    widget: &widgets::Widget,
    name: &str,
    supplied: Value,
) -> Result<Value, AuthoringError> {
    let supplied = if widget.type_id == "ptcsrte" && name == "DefaultFontSize" {
        let text = supplied
            .as_str()
            .map_or_else(|| supplied.to_string(), str::to_string);
        match text.parse::<u32>() {
            Ok(size) => json!(format!("{size}px")),
            Err(_) => supplied,
        }
    } else {
        supplied
    };
    let value = checked_property_value(widget, name, supplied)?;
    let imagelink = widget
        .property(name)
        .is_some_and(|contract| contract.base_type.as_deref() == Some("IMAGELINK"));
    if let (true, Value::String(text)) = (imagelink, &value) {
        crate::core::cds_icons::check(text)
            .map_err(|error| AuthoringError::Invalid(format!("{name}: {error}")))?;
    }
    Ok(value)
}

fn checked_property_value(
    widget: &widgets::Widget,
    name: &str,
    supplied: Value,
) -> Result<Value, AuthoringError> {
    crate::core::repo::refuse_git_bash_value(&supplied)
        .map_err(|error| AuthoringError::Invalid(error.to_string()))?;
    let numbered = widget.numbered_default(name);
    let default = widget.defaults.get(name).or(numbered.as_ref());
    let contract = widget.property(name);
    let state_parameters = name
        .strip_suffix("_StateParameters")
        .is_some_and(|property| {
            widget.property(property).is_some_and(|contract| {
                matches!(
                    contract.base_type.as_deref(),
                    Some("STATEFORMATTING" | "RENDERERWITHSTATE")
                )
            })
        });
    let base = contract.and_then(|contract| contract.base_type.as_deref());
    let structured = default
        .is_some_and(|value| value.is_object() || value.is_array() || value.is_null())
        || matches!(
            base,
            Some("OBJECT" | "JSON" | "INFOTABLE" | "STATEFORMATTING" | "RENDERERWITHSTATE")
        )
        || state_parameters
        || (!widget.extension && name == "ColumnFormat" && widget.type_id.contains("grid"));
    let array = default.is_some_and(Value::is_array) || state_parameters;
    if structured {
        let value = match supplied {
            Value::String(text) if matches!(base, Some("RENDERERWITHSTATE")) => {
                match serde_json::from_str(&text) {
                    Ok(value @ Value::Object(_)) | Ok(value @ Value::String(_)) => value,
                    Ok(_) => return Err(shape_error(name, false)),
                    Err(_) => Value::String(text),
                }
            }
            Value::String(text) => serde_json::from_str(&text).map_err(|_| {
                AuthoringError::Invalid(format!(
                    "{name} is structured; --prop {name}=<JSON> needs valid JSON"
                ))
            })?,
            value => value,
        };
        if array && !value.is_array() {
            return Err(shape_error(name, true));
        }
        if !array
            && !value.is_object()
            && !(matches!(base, Some("RENDERERWITHSTATE")) && value.is_string())
        {
            return Err(shape_error(name, false));
        }
        return Ok(value);
    }
    let text = match supplied {
        Value::String(text) => text,
        value => return scalar_value(name, default, base, value),
    };
    // A style property names a StyleDefinition, or holds Composer's inline style object (a
    // kit's 12px status dot carries its colour that way): JSON object text is the inline form.
    if base == Some("STYLEDEFINITION") && text.trim_start().starts_with('{') {
        return match serde_json::from_str::<Value>(&text) {
            Ok(value @ Value::Object(_)) => Ok(value),
            _ => Err(AuthoringError::Invalid(format!(
                "{name} takes a StyleDefinition name or an inline style JSON object"
            ))),
        };
    }
    if contract.is_some_and(|contract| contract.css_size) && text.parse::<f64>().is_err() {
        return Ok(Value::String(text));
    }
    // The default's own type decides first; the contract's base type only when there is none.
    let boolean =
        matches!(default, Some(Value::Bool(_))) || (default.is_none() && base == Some("BOOLEAN"));
    let number = matches!(default, Some(Value::Number(_)))
        || (default.is_none() && matches!(base, Some("NUMBER" | "INTEGER")));
    match () {
        () if boolean => match text.as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(AuthoringError::Invalid(format!(
                "{name} must be true or false"
            ))),
        },
        () if number => {
            if let Ok(number) = text.parse::<i64>() {
                Ok(Value::from(number))
            } else if let Ok(number) = text.parse::<f64>() {
                serde_json::Number::from_f64(number)
                    .map(Value::Number)
                    .ok_or_else(|| AuthoringError::Invalid(format!("{name} must be a JSON number")))
            } else {
                Err(AuthoringError::Invalid(format!(
                    "{name} must be a JSON number"
                )))
            }
        }
        _ => Ok(Value::String(text)),
    }
}

fn scalar_value(
    name: &str,
    default: Option<&Value>,
    base: Option<&str>,
    value: Value,
) -> Result<Value, AuthoringError> {
    match (default, base, &value) {
        (Some(Value::Bool(_)), _, Value::Bool(_)) | (_, Some("BOOLEAN"), Value::Bool(_)) => {
            Ok(value)
        }
        (Some(Value::Number(_)), _, Value::Number(_))
        | (_, Some("NUMBER" | "INTEGER"), Value::Number(_)) => Ok(value),
        (Some(Value::String(_)), _, Value::String(_)) | (_, _, Value::String(_)) => Ok(value),
        _ => Err(AuthoringError::Invalid(format!(
            "{name} must be a JSON value of its property type"
        ))),
    }
}

fn shape_error(name: &str, array: bool) -> AuthoringError {
    AuthoringError::Invalid(format!(
        "{name} must be a JSON {}",
        if array { "array" } else { "object" }
    ))
}

/// The add plan as `--json` and MCP report it.
pub fn add_plan_json(solution: &Solution, plan: &AddPlan, applied: bool) -> Value {
    let relative = |path: &Path| solution.relative(path);
    let mut files = vec![relative(&plan.edit.entity_path)];
    if let Some(sidecars) = &plan.edit.sidecars {
        files.push(relative(&sidecars.content_path));
    }
    json!({
        "applied": applied,
        "mashup": plan.edit.mashup,
        "notes": plan.notes,
        "next_steps": [format!("{}see the result: twaco mashup describe {}", if applied { "" } else { "after --apply, " }, plan.edit.mashup)],
        "widget": plan.widget,
        "id": plan.placed.id,
        "containers": plan.placed.containers,
        "container_contents": plan.container_contents,
        "container_tree": plan.container_tree,
        "flipped": plan.placed.flipped,
        "wrapped": plan.placed.wrapped,
        "files": files,
    })
}

/// The contained-mashup plan as `--json` and MCP report it.
pub fn add_contained_plan_json(
    solution: &Solution,
    plan: &AddContainedPlan,
    applied: bool,
) -> Value {
    let relative = |path: &Path| solution.relative(path);
    let mut files = vec![relative(&plan.edit.entity_path)];
    if let Some(sidecars) = &plan.edit.sidecars {
        files.push(relative(&sidecars.content_path));
    }
    json!({
        "notes": plan.notes, "applied": applied, "mashup": plan.edit.mashup, "child": plan.child,
        "widget": "mashupcontainer", "id": plan.placed.id,
        "containers": plan.placed.containers, "flipped": plan.placed.flipped,
        "wrapped": plan.placed.wrapped, "parameters": plan.parameters.iter().map(|parameter| json!({
            "name": parameter.name, "base_type": parameter.base_type,
            "direction": parameter.direction, "set": plan.settings.contains(&parameter.name),
        })).collect::<Vec<_>>(), "files": files,
    })
}
