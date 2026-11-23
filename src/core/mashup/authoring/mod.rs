//! Creating and changing mashups in the repository, as Composer would.
//!
//! A new mashup's entity file is filled from the 10.2 template and its content written by
//! `mashup::sync`, so it is byte for byte what a sync of the same content would write. Edits to an
//! existing mashup change its `content.json` and go back through the same sync, which replaces
//! only the content section; nothing else in the entity file is re-serialised.

use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::lock::WorkspaceLock;
use crate::core::mashup;
use crate::core::mashup::edit::{EditError, MashupDocument};
use crate::core::transaction::{Transaction, TransactionError};
use crate::core::{types, workspace};
use serde_json::{Map, Value};
use std::path::PathBuf;

mod add;
mod bind;
mod collection;
mod end;
mod event;
mod gap;
mod move_widget;
mod new;
mod on;
mod parameter;
mod recipes;
mod remove;
pub(crate) mod session;
mod set;
mod source;
pub(crate) mod state_format;

pub use add::{add_contained_plan_json, add_plan_json, AddContainedRequest, AddRequest};
pub(crate) use add::{
    function_property, plan_add, plan_add_contained, property_value, AddContainedPlan, AddPlan,
};
pub use bind::BindRequest;
use bind::{bind_in, data_call, existing_data_call};
pub(crate) use bind::{plan_bind, BindPlan};
pub use collection::{
    collection_add_plan_json, CollectionAddRequest, CollectionPreset, DEFAULT_ACROSS,
};
pub(crate) use collection::{plan_collection_add, CollectionAddPlan};
use end::{
    allowed, area, base, details, menu_service, property, resolve_end, resolve_service,
    source_property_type, target_property_type, End,
};
pub(crate) use event::plan_event;
pub use event::EventRequest;
pub(crate) use gap::plan_gap;
pub use gap::{gap_plan_json, GapPlan, GapRequest};
pub use move_widget::{describe_placement, move_plan_json, MoveRequest};
pub(crate) use move_widget::{plan_move, MovePlan};
pub(crate) use new::{apply_new, parse_visibility, plan_new, NewPlan};
pub use new::{copy_summary_lines, new_plan_json, template_slots, NewRequest};
#[cfg(test)]
pub(crate) use new::{validate_template, DEFAULT_THEME};
use on::on_in;
pub use on::OnRequest;
pub(crate) use on::{plan_on, OnPlan};
pub(crate) use parameter::validate_expression_type;
pub(crate) use parameter::{apply_parameter, plan_parameter, root_default, ParameterPlan};
use parameter::{
    cache_entry, contained_parameters, default_value, ensure_cached, navigation_definition,
    parse_new_parameter, set_content_parameter, splice_parameter_definition, validate_parameter,
    ParameterDefinition,
};
pub use parameter::{parameter_plan_json, ParameterAction, ParameterDefault, ParameterRequest};
#[cfg(test)]
pub(crate) use recipes::recipe_transaction;
pub(crate) use recipes::{apply_recipe, plan_recipe, RecipePlan};
pub(crate) use recipes::{
    combo_props, grid_cell_props, grid_row_props, input_variant_styles, sparkline_props,
    sparkline_styles,
};
pub use recipes::{recipe_plan_json, Recipe, RecipeArgs, RecipeRequest, RECIPES};
pub(crate) use remove::{plan_off, plan_remove, plan_unbind, OffPlan, RemovePlan, UnbindPlan};
pub use remove::{OffRequest, RemoveRequest, UnbindRequest};
#[cfg(test)]
pub(crate) use set::colour;
pub(crate) use set::{plan_set, SetPlan};
pub use set::{set_plan_json, SetRequest};
pub use source::SourceInfo;
pub(crate) use source::{
    list_sources, plan_source_add, plan_source_remove, plan_source_retarget, SourceAddPlan,
    SourceRemovePlan, SourceRetargetPlan,
};
pub use source::{SourceAddRequest, SourceRemoveRequest, SourceRetargetRequest};
pub(crate) use state_format::plan_state_format_set;
pub(crate) use state_format::read_definition as read_state_definition;
pub use state_format::StateFormatSetRequest;

#[derive(Debug)]
pub struct SidecarEdit {
    pub content_path: PathBuf,
    pub(crate) content_before: Vec<u8>,
    pub(crate) content_after: Vec<u8>,
    pub(crate) css_path: PathBuf,
    pub(crate) css_before: Vec<u8>,
    pub(crate) css_after: Vec<u8>,
}

#[derive(Debug)]
pub struct EditPlan {
    /// The command, as the transaction journal names it (`mashup add`, `mashup bind`).
    pub(crate) command: &'static str,
    pub mashup: String,
    pub entity_path: PathBuf,
    pub(crate) entity_before: Vec<u8>,
    pub(crate) entity_after: Vec<u8>,
    /// `None` means this mashup has no sidecars and only its entity is changed.
    pub sidecars: Option<SidecarEdit>,
}

impl EditPlan {
    pub(crate) fn unchanged(&self) -> bool {
        // Sidecar writes preserve an existing byte-order mark.
        let same_sidecar = |before: &[u8], after: &[u8]| {
            before == after || before.strip_prefix(b"\xEF\xBB\xBF") == Some(after)
        };
        self.entity_before == self.entity_after
            && self.sidecars.as_ref().is_none_or(|sidecars| {
                same_sidecar(&sidecars.content_before, &sidecars.content_after)
                    && mashup::same_css_bytes(&sidecars.css_before, &sidecars.css_after)
            })
    }
}

pub(crate) struct ExistingMashup {
    mashup: String,
    entity_path: PathBuf,
    pub(crate) entity_before: Vec<u8>,
    pub(crate) assets: mashup::Assets,
    pub(crate) sidecars: Option<(PathBuf, Vec<u8>, PathBuf, Vec<u8>)>,
    pub(crate) deterministic: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthoringError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unknown(String),
    #[error("{0}")]
    Exists(String),
    #[error("{0}")]
    Ambiguous(String),
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Edit(EditError),
    #[error("{0}")]
    Mashup(mashup::MashupError),
    #[error("{0}")]
    Unreadable(String),
    #[error("nothing was written: {0}")]
    Write(TransactionError),
}

impl Coded for AuthoringError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Unknown(_) => ErrorCode::UnknownEntity,
            Self::Exists(_) => ErrorCode::AlreadyExists,
            Self::Ambiguous(_) => ErrorCode::Ambiguous,
            Self::Refused(_) => ErrorCode::GuardRefused,
            Self::Edit(error) => error.code(),
            Self::Mashup(_) => ErrorCode::InvalidData,
            Self::Unreadable(_) => ErrorCode::InvalidData,
            Self::Write(error) => error.code(),
        }
    }
}

/// Load one existing mashup, refusing a managed sidecar that is not its entity's current content.
pub(crate) fn load_existing(
    solution: &Solution,
    mashup_name: &str,
) -> Result<ExistingMashup, AuthoringError> {
    let found = workspace::entities(solution);
    let entity = found
        .iter()
        .find(|entity| entity.info.collection == "Mashups" && entity.info.name == mashup_name)
        .cloned()
        .ok_or_else(|| {
            AuthoringError::Unknown(format!(
                "no mashup {mashup_name} in the solution{}",
                workspace::short_name_hint(
                    found
                        .iter()
                        .filter(|entity| entity.info.collection == "Mashups")
                        .map(|entity| entity.info.name.as_str()),
                    mashup_name
                )
            ))
        })?;
    let entity_before = std::fs::read(&entity.path).map_err(|error| {
        AuthoringError::Unreadable(format!("{}: {error}", entity.path.display()))
    })?;
    let directory = workspace::mashup_dir(solution, &entity);
    let sidecar_assets = workspace::read_mashup(&directory)
        .map_err(|error| AuthoringError::Unreadable(error.to_string()))?;
    let (assets, sidecars) = match sidecar_assets {
        Some(assets) => {
            let (_, changes) =
                mashup::sync(&entity_before, &assets).map_err(AuthoringError::Mashup)?;
            if !changes.is_empty() {
                return Err(AuthoringError::Refused(format!(
                    "{mashup_name}: content.json and the entity file disagree; run `twaco sync` (keeps the sidecars) or `twaco extract` (keeps the entity file) first"
                )));
            }
            let content_path = directory.join("content.json");
            let content_before = std::fs::read(&content_path).map_err(|error| {
                AuthoringError::Unreadable(format!("{}: {error}", content_path.display()))
            })?;
            let css_path = directory.join("custom.css");
            let css_before = std::fs::read(&css_path).map_err(|error| {
                AuthoringError::Unreadable(format!("{}: {error}", css_path.display()))
            })?;
            (
                assets,
                Some((content_path, content_before, css_path, css_before)),
            )
        }
        None => (
            mashup::extract(&entity_before).map_err(AuthoringError::Mashup)?,
            None,
        ),
    };
    Ok(ExistingMashup {
        deterministic: false,
        mashup: mashup_name.to_string(),
        entity_path: entity.path,
        entity_before,
        assets,
        sidecars,
    })
}

/// The loaded mashup's content as an editable document; a content twaco cannot edit names its mashup.
pub(crate) fn parse_loaded(loaded: &ExistingMashup) -> Result<MashupDocument, AuthoringError> {
    let mut document =
        mashup::edit::parse(&loaded.assets.content).map_err(|error| match error {
            EditError::Invalid(message) => {
                AuthoringError::Edit(EditError::Invalid(format!("{}: {message}", loaded.mashup)))
            }
            other => AuthoringError::Edit(other),
        })?;
    mashup::layout::clear_page_fillers(&mut document);
    if loaded.deterministic {
        document.set_uuid_generator(crate::core::mashup::page::ids::uuid);
    }
    Ok(document)
}

/// Put the changed content back through sync and prove that the entity holds precisely that content.
fn finish_edit(
    loaded: ExistingMashup,
    document: MashupDocument,
    command: &'static str,
) -> Result<EditPlan, AuthoringError> {
    finish_edit_with_css(loaded, document, command, None)
}

/// Put changed content and, when requested, the stylesheet back through sync.
pub(crate) fn finish_edit_with_css(
    loaded: ExistingMashup,
    mut document: MashupDocument,
    command: &'static str,
    css: Option<String>,
) -> Result<EditPlan, AuthoringError> {
    // Page sessions finish layout once all nodes and explicit sizes are present.
    if !loaded.deterministic {
        mashup::layout::finish_page(&mut document);
    }
    let content = document.to_content();
    let assets_after = mashup::Assets {
        content: content.clone(),
        css: css.unwrap_or_else(|| loaded.assets.css.clone()),
    };
    let (entity_after, _) =
        mashup::sync(&loaded.entity_before, &assets_after).map_err(AuthoringError::Mashup)?;
    let extracted = mashup::extract(&entity_after)
        .map_err(AuthoringError::Mashup)?
        .content;
    if !content_round_trips(&content, &extracted, loaded.sidecars.is_some()) {
        return Err(AuthoringError::Invalid(format!(
            "{}: mashup content did not round-trip through the entity file",
            loaded.mashup
        )));
    }
    let sidecars =
        loaded.sidecars.map(
            |(content_path, content_before, css_path, css_before)| SidecarEdit {
                content_path,
                content_before,
                content_after: content.into_bytes(),
                css_path,
                css_before,
                css_after: assets_after.css.into_bytes(),
            },
        );
    Ok(EditPlan {
        command,
        mashup: loaded.mashup,
        entity_path: loaded.entity_path,
        entity_before: loaded.entity_before,
        entity_after,
        sidecars,
    })
}

// custom.css owns the generated top-level stylesheet, including legacy stale content keys.
fn content_round_trips(content: &str, extracted: &str, managed_css: bool) -> bool {
    content_difference(content, extracted, managed_css).is_none()
}

fn content_difference(
    content: &str,
    extracted: &str,
    managed_css: bool,
) -> Option<(String, String, String)> {
    fn first(a: &Value, b: &Value, path: String) -> Option<(String, String, String)> {
        if a == b {
            return None;
        }
        match (a, b) {
            (Value::Object(a), Value::Object(b)) => {
                for key in a.keys().chain(b.keys()) {
                    let next = format!("{path}.{key}");
                    match (a.get(key), b.get(key)) {
                        (Some(a), Some(b)) => {
                            if let Some(diff) = first(a, b, next) {
                                return Some(diff);
                            }
                        }
                        (a, b) => {
                            return Some((
                                next,
                                a.map_or("<absent>".into(), Value::to_string),
                                b.map_or("<absent>".into(), Value::to_string),
                            ))
                        }
                    }
                }
            }
            (Value::Array(a), Value::Array(b)) => {
                for at in 0..a.len().max(b.len()) {
                    let next = format!("{path}[{at}]");
                    match (a.get(at), b.get(at)) {
                        (Some(a), Some(b)) => {
                            if let Some(diff) = first(a, b, next) {
                                return Some(diff);
                            }
                        }
                        (a, b) => {
                            return Some((
                                next,
                                a.map_or("<absent>".into(), Value::to_string),
                                b.map_or("<absent>".into(), Value::to_string),
                            ))
                        }
                    }
                }
            }
            _ => {}
        }
        Some((path, a.to_string(), b.to_string()))
    }
    let clean = |text: &str| {
        let mut value: Value = serde_json::from_str(text).expect("checked JSON");
        if managed_css {
            value
                .as_object_mut()
                .expect("checked object")
                .remove("CustomMashupCss");
        }
        value
    };
    first(&clean(content), &clean(extracted), "content.json:$".into())
}

struct AuthoringContext<'a> {
    parameters: &'a [crate::core::mashup::describe::MashupParameter],
    model: &'a types::Model,
    platform: Option<&'a types::Platform>,
}

fn insert(map: &mut Map<String, Value>, key: &str, value: Value) {
    mashup::edit::insert_sorted(map, key, value);
}

/// Build one transaction so composite recipes can include generated artifacts.
pub(crate) fn edit_transaction<'a>(
    solution: &'a Solution,
    plan: &EditPlan,
) -> Result<Transaction<'a>, AuthoringError> {
    let mut transaction = Transaction::new(&solution.root, plan.command);
    transaction
        .replace_file(
            &plan.entity_path,
            &plan.entity_before,
            plan.entity_after.clone(),
        )
        .map_err(AuthoringError::Write)?;
    if let Some(sidecars) = &plan.sidecars {
        if sidecars.content_before != sidecars.content_after {
            transaction
                .replace_file(
                    &sidecars.content_path,
                    &sidecars.content_before,
                    // An edited content.json keeps the byte-order mark it had.
                    workspace::keeping_bom(&sidecars.content_path, sidecars.content_after.clone()),
                )
                .map_err(AuthoringError::Write)?;
        }
        if !mashup::same_css_bytes(&sidecars.css_before, &sidecars.css_after) {
            transaction
                .replace_file(
                    &sidecars.css_path,
                    &sidecars.css_before,
                    workspace::keeping_bom(&sidecars.css_path, sidecars.css_after.clone()),
                )
                .map_err(AuthoringError::Write)?;
        }
    }
    Ok(transaction)
}

pub(crate) fn apply_edit(
    solution: &Solution,
    plan: &EditPlan,
    lock: &WorkspaceLock,
) -> Result<(), AuthoringError> {
    edit_transaction(solution, plan)?
        .apply(lock)
        .map_err(AuthoringError::Write)?;
    Ok(())
}

#[cfg(test)]
mod tests;
