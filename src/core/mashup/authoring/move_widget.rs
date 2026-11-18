//! `mashup move`: a widget and its subtree taken out of one place and laid out in another.

use super::add::ContainerContents;
use super::{finish_edit, load_existing, parse_loaded, AuthoringError, EditPlan};
use crate::core::config::Solution;
use crate::core::mashup::edit::{self, Placed, Placement, Side};
use crate::core::{extension_widgets, widgets};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct MoveRequest {
    pub mashup: String,
    pub widget: String,
    pub placement: Option<Placement>,
}

#[derive(Debug)]
pub struct MovePlan {
    pub mashup: String,
    /// `None` when the widget is already there.
    pub edit: Option<EditPlan>,
    pub widget: String,
    /// The widget's parent before the move.
    pub from: String,
    /// The placement, worded as the flags say it.
    pub to: String,
    /// Wrappers twaco made for the new position.
    pub placed: Placed,
    pub container_contents: Vec<ContainerContents>,
    /// Empty wrappers the old position left, removed as `mashup remove` removes them.
    pub removed: Vec<String>,
    /// Every widget id that moved: the widget and its descendants.
    pub moved: Vec<String>,
}

/// The placement as the flags that asked for it say it.
pub fn describe_placement(placement: &Placement) -> String {
    match placement {
        Placement::In(container) => format!("in {container}"),
        Placement::Bottom(column) => format!("at the bottom of {column}"),
        Placement::BelowInColumn(anchor) => format!("below {anchor} in its column"),
        Placement::Beside { anchor, side } => match side {
            Side::Right => format!("right of {anchor}"),
            Side::Left => format!("left of {anchor}"),
            Side::Above => format!("above {anchor}"),
            Side::Below => format!("below {anchor}"),
            Side::End => format!("at the end of {anchor}'s row"),
        },
    }
}

fn anchor_word(placement: &Placement) -> &str {
    match placement {
        Placement::In(word) | Placement::Bottom(word) | Placement::BelowInColumn(word) => word,
        Placement::Beside { anchor, .. } => anchor,
    }
}

pub(crate) fn plan_move(
    solution: &Solution,
    request: &MoveRequest,
) -> Result<MovePlan, AuthoringError> {
    let Some(placement) = &request.placement else {
        return Err(AuthoringError::Invalid(
            "say where: --in <container>, or --right-of/--left-of/--above/--below <widget>".into(),
        ));
    };
    let loaded = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&loaded)?;
    let found = document
        .find(&request.widget)
        .map_err(AuthoringError::Edit)?;
    let node = document.at(&found.path);
    if found.path.is_empty() {
        return Err(AuthoringError::Refused(
            "the mashup root cannot be moved".into(),
        ));
    }
    if found.path.len() == 1 {
        return Err(AuthoringError::Refused(
            if found.type_id == "flexcontainer" {
                format!(
                    "{} is the mashup's body container; move what is inside it",
                    found.id
                )
            } else {
                format!(
                "{} is a function widget, which sits beside the layout and has no position to move to",
                found.id
            )
            },
        ));
    }
    if edit::is_spacer(node) {
        return Err(AuthoringError::Refused(format!(
            "{} is a spacer twaco made; run gap to change it",
            found.id
        )));
    }
    let anchor = document
        .find(anchor_word(placement))
        .map_err(AuthoringError::Edit)?;
    if anchor.path.starts_with(&found.path) {
        return Err(AuthoringError::Refused(if anchor.id == found.id {
            format!("{} cannot be placed relative to itself", found.id)
        } else {
            format!(
                "{} is inside {}, which is being moved; choose a target outside it",
                anchor.id, found.id
            )
        }));
    }
    if !matches!(placement, Placement::In(_))
        && document.is_sole_wrapper_of(&found.path, &anchor.path)
    {
        return Err(AuthoringError::Refused(format!(
            "{} is {}'s own wrapper; anchor on a widget outside it",
            anchor.id, found.id
        )));
    }
    let moved = document
        .widgets()
        .into_iter()
        .filter(|widget| widget.path.starts_with(&found.path))
        .map(|widget| widget.id)
        .collect::<Vec<_>>();
    let to = describe_placement(placement);
    let (parent_path, _) = found.path.split_at(found.path.len() - 1);
    let ancestors = (0..parent_path.len() + 1)
        .rev()
        .map(|depth| {
            document
                .at(&parent_path[..depth])
                .pointer("/Properties/Id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect::<Vec<_>>();
    let mut from = ancestors[0].clone();
    if document.is_placed_at(&found.path, placement) {
        return Ok(MovePlan {
            mashup: request.mashup.clone(),
            edit: None,
            widget: found.id,
            from,
            to,
            placed: Placed::default(),
            container_contents: Vec::new(),
            removed: Vec::new(),
            moved,
        });
    }
    let defined;
    let widget = match widgets::catalog().by_type(&found.type_id) {
        Some(widget) => widget,
        None => match extension_widgets::find(solution, &found.type_id).0 {
            Some(found) => {
                defined = found;
                &defined
            }
            None => {
                return Err(AuthoringError::Refused(format!(
                    "{} is a {}, which twaco has no layout rules for",
                    found.id, found.type_id
                )));
            }
        },
    };
    document.reserve_ids();
    let (subtree, removed) = document
        .detach_widget(&found.path)
        .map_err(AuthoringError::Edit)?;
    if let Some(survivor) = ancestors.iter().find(|id| !removed.contains(id)) {
        from = survivor.clone();
    }
    let placed = document
        .place_existing(widget, subtree, placement)
        .map_err(AuthoringError::Edit)?;
    document.fit_placed_width(&placed.id);
    document.reflow_for(&placed.id);
    let container_contents = placed
        .containers
        .iter()
        .filter_map(|id| document.find_exact(id).ok())
        .map(|container| ContainerContents {
            id: container.id,
            holds: document.at(&container.path)["Widgets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|child| !edit::is_spacer(child))
                .filter_map(|child| child["Properties"]["Id"].as_str().map(str::to_string))
                .collect(),
        })
        .collect();
    let edit = finish_edit(loaded, document, "mashup move")?;
    Ok(MovePlan {
        mashup: request.mashup.clone(),
        edit: Some(edit),
        widget: found.id,
        from,
        to,
        placed,
        container_contents,
        removed,
        moved,
    })
}

/// The move plan as `--json` and MCP report it.
pub fn move_plan_json(solution: &Solution, plan: &MovePlan, applied: bool) -> Value {
    let relative = |path: &Path| solution.relative(path);
    let mut files = Vec::new();
    if let Some(edit) = &plan.edit {
        files.push(relative(&edit.entity_path));
        if let Some(sidecars) = &edit.sidecars {
            files.push(relative(&sidecars.content_path));
        }
    }
    json!({
        "applied": applied,
        "mashup": plan.mashup,
        "id": plan.widget,
        "moved": plan.moved,
        "from": plan.from,
        "to": plan.to,
        "changed": plan.edit.is_some(),
        "containers": plan.placed.containers,
        "container_contents": plan.container_contents,
        "flipped": plan.placed.flipped,
        "wrapped": plan.placed.wrapped,
        "removed": plan.removed,
        "files": files,
    })
}
