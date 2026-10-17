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
mod end;
mod event;
mod new;
mod on;
mod parameter;
mod recipes;

pub(crate) use add::function_property;
pub use add::{
    add_contained_plan_json, add_plan_json, plan_add, plan_add_contained, AddContainedPlan,
    AddContainedRequest, AddPlan, AddRequest, ContainedParameter,
};
use bind::{bind_in, data_call, existing_data_call};
pub use bind::{plan_bind, BindPlan, BindRequest};
use end::{
    allowed, area, base, details, menu_service, property, resolve_end, resolve_service,
    source_property_type, target_property_type, End,
};
pub use event::{plan_event, EventRequest};
pub use new::{
    apply_new, new_plan_json, parse_visibility, plan_new, template_slots, validate_template,
    NewPlan, NewRequest, DEFAULT_THEME,
};
use on::on_in;
pub use on::{plan_on, OnPlan, OnRequest};
pub use parameter::{
    apply_parameter, parameter_plan_json, plan_parameter, ParameterAction, ParameterDefault,
    ParameterPlan, ParameterRequest,
};
use parameter::{
    cache_entry, contained_parameters, default_value, ensure_cached, navigation_definition,
    parse_new_parameter, set_content_parameter, splice_parameter_definition, validate_parameter,
    ParameterDefinition,
};
pub use recipes::{
    plan_recipe, recipe_plan_json, Recipe, RecipeArgs, RecipePlan, RecipeRequest, RECIPES,
};

#[derive(Debug)]
pub struct SidecarEdit {
    pub content_path: PathBuf,
    pub content_before: Vec<u8>,
    pub content_after: Vec<u8>,
    pub css_path: PathBuf,
    /// The stylesheet is retained while content changes.
    pub css: String,
}

#[derive(Debug)]
pub struct EditPlan {
    /// The command, as the transaction journal names it (`mashup add`, `mashup bind`).
    pub command: &'static str,
    pub mashup: String,
    pub entity_path: PathBuf,
    pub entity_before: Vec<u8>,
    pub entity_after: Vec<u8>,
    /// `None` means this mashup has no sidecars and only its entity is changed.
    pub sidecars: Option<SidecarEdit>,
}

struct ExistingMashup {
    mashup: String,
    entity_path: PathBuf,
    entity_before: Vec<u8>,
    assets: mashup::Assets,
    sidecars: Option<(PathBuf, Vec<u8>, PathBuf)>,
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
fn load_existing(solution: &Solution, mashup_name: &str) -> Result<ExistingMashup, AuthoringError> {
    let entity = workspace::entities(solution)
        .into_iter()
        .find(|entity| entity.info.collection == "Mashups" && entity.info.name == mashup_name)
        .ok_or_else(|| {
            AuthoringError::Unknown(format!("no mashup {mashup_name} in the solution"))
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
            (
                assets,
                Some((content_path, content_before, directory.join("custom.css"))),
            )
        }
        None => (
            mashup::extract(&entity_before).map_err(AuthoringError::Mashup)?,
            None,
        ),
    };
    Ok(ExistingMashup {
        mashup: mashup_name.to_string(),
        entity_path: entity.path,
        entity_before,
        assets,
        sidecars,
    })
}

/// The loaded mashup's content as an editable document; a content twaco cannot edit names its mashup.
fn parse_loaded(loaded: &ExistingMashup) -> Result<MashupDocument, AuthoringError> {
    mashup::edit::parse(&loaded.assets.content).map_err(|error| match error {
        EditError::Invalid(message) => {
            AuthoringError::Edit(EditError::Invalid(format!("{}: {message}", loaded.mashup)))
        }
        other => AuthoringError::Edit(other),
    })
}

/// Put the changed content back through sync and prove that the entity holds precisely that content.
fn finish_edit(
    loaded: ExistingMashup,
    document: MashupDocument,
    command: &'static str,
) -> Result<EditPlan, AuthoringError> {
    let content = document.to_content();
    let assets_after = mashup::Assets {
        content: content.clone(),
        css: loaded.assets.css.clone(),
    };
    let (entity_after, _) =
        mashup::sync(&loaded.entity_before, &assets_after).map_err(AuthoringError::Mashup)?;
    if mashup::extract(&entity_after)
        .map_err(AuthoringError::Mashup)?
        .content
        != content
    {
        return Err(AuthoringError::Invalid(format!(
            "{}: mashup content did not round-trip through the entity file",
            loaded.mashup
        )));
    }
    let sidecars = loaded
        .sidecars
        .map(|(content_path, content_before, css_path)| SidecarEdit {
            content_path,
            content_before,
            content_after: content.into_bytes(),
            css_path,
            css: loaded.assets.css,
        });
    Ok(EditPlan {
        command,
        mashup: loaded.mashup,
        entity_path: loaded.entity_path,
        entity_before: loaded.entity_before,
        entity_after,
        sidecars,
    })
}

struct AuthoringContext<'a> {
    parameters: &'a [crate::core::mashup::describe::MashupParameter],
    model: &'a types::Model,
    platform: Option<&'a types::Platform>,
}

fn insert(map: &mut Map<String, Value>, key: &str, value: Value) {
    mashup::edit::insert_sorted(map, key, value);
}

/// Apply an existing-mashup edit as one transaction.
pub fn apply_edit(
    solution: &Solution,
    plan: &EditPlan,
    lock: &WorkspaceLock,
) -> Result<(), AuthoringError> {
    let mut transaction = Transaction::new(&solution.root, plan.command);
    transaction
        .replace_file(
            &plan.entity_path,
            &plan.entity_before,
            plan.entity_after.clone(),
        )
        .map_err(AuthoringError::Write)?;
    if let Some(sidecars) = &plan.sidecars {
        transaction
            .replace_file(
                &sidecars.content_path,
                &sidecars.content_before,
                sidecars.content_after.clone(),
            )
            .map_err(AuthoringError::Write)?;
    }
    transaction.apply(lock).map_err(AuthoringError::Write)?;
    Ok(())
}

#[cfg(test)]
mod tests;
