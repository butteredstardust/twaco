//! A building block's deploy step, run as the Solution Framework runs it.
//!
//! The framework's own deploy (`PTC.Base.Manager.InitializeSolution`) walks every installed
//! block; for each it checks the parameters with the entry point's
//! `ValidateComponentDeployConfigParams`, calls `DeployComponent({deploymentConfig})` unless the
//! entry point already says `Configured` (or the caller overrides that), and on success sets the
//! entry point's `componentDeploymentState` to `Configured`; an exception sets `Error`. This does
//! the same for one deployed block, after its project imported, so the Solution Framework page
//! agrees with what twaco deployed. It never calls `InitializeSolution`, which would deploy every
//! block on the server that is not configured.

use super::super::block;
use super::super::entity::key::ServiceTarget;
use super::super::server::ServerError;
use super::Remote;
use serde_json::{json, Value};
use std::time::Duration;

const STATE: &str = "componentDeploymentState";
const CONFIGURED: &str = "Configured";

/// What a building block's deploy step does, or did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockStep {
    pub project: String,
    pub entry_point: String,
    /// The entry point's state on the server before the deploy; `None` when it is not there yet.
    pub state: Option<String>,
    /// Whether `DeployComponent` runs: always unless the block is `Configured` and not redeployed.
    pub runs: bool,
    /// After an apply: it ran and the state is `Configured`.
    pub done: bool,
}

/// The entry point's `componentDeploymentState`, or `None` when the server has no such Thing.
pub(crate) fn state(remote: &dyn Remote, entry_point: &str) -> Result<Option<String>, ServerError> {
    match remote.call_service(
        &ServiceTarget::Thing(entry_point.to_string()),
        "GetPropertyValues",
        &json!({}),
        Duration::from_secs(60),
    ) {
        Ok(reply) => Ok(reply
            .as_ref()
            .and_then(|reply| reply.pointer(&format!("/rows/0/{STATE}")))
            .and_then(Value::as_str)
            .map(str::to_string)),
        Err(error) if error.is_not_found() => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn runs(state: Option<&str>, redeploy: bool) -> bool {
    redeploy || state != Some(CONFIGURED)
}

/// Run the step with `deployment_config` (placeholders resolved). `Ok(false)` when the block was
/// `Configured` and is left as it is.
pub(crate) fn run(
    remote: &dyn Remote,
    entry_point: &str,
    deployment_config: &Value,
    redeploy: bool,
) -> Result<bool, String> {
    let target = ServiceTarget::Thing(entry_point.to_string());
    let call = |service: &str, parameters: Value| {
        remote
            .call_service(&target, service, &parameters, Duration::from_secs(300))
            .map_err(|error| format!("{entry_point}.{service}: {error}"))
    };
    let current = state(remote, entry_point).map_err(|error| format!("{entry_point}: {error}"))?;
    if !runs(current.as_deref(), redeploy) {
        return Ok(false);
    }

    let schema = call("GetComponentDeployConfigurationParameters", json!({}))?.unwrap_or_default();
    let missing: Vec<String> = block::parse_parameters(&schema)
        .into_iter()
        .filter(|parameter| !parameter.optional)
        .filter(|parameter| {
            deployment_config
                .get(&parameter.name)
                .is_none_or(|value| value.is_null() || value == "")
        })
        .map(|parameter| {
            if parameter.description.is_empty() {
                parameter.name
            } else {
                format!("{} ({})", parameter.name, parameter.description)
            }
        })
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "{entry_point}.DeployComponent needs deploymentConfig {}; set them in [project.deploy] deploy_parameters",
            missing.join(", ")
        ));
    }
    let valid = call(
        "ValidateComponentDeployConfigParams",
        json!({ "jsonParams": deployment_config }),
    )?;
    if boolean(valid.as_ref()) != Some(true) {
        return Err(format!(
            "{entry_point}.ValidateComponentDeployConfigParams refused the deploy parameters"
        ));
    }

    match call(
        "DeployComponent",
        json!({ "deploymentConfig": deployment_config }),
    ) {
        Err(error) => {
            // As the framework does: a deploy that threw leaves the block in Error.
            let marked = set_state(remote, &target, "Error").is_ok();
            Err(format!(
                "{error}{}",
                if marked {
                    "; the entry point's state is Error"
                } else {
                    ""
                }
            ))
        }
        Ok(reply) if boolean(reply.as_ref()) != Some(true) => Err(format!(
            "{entry_point}.DeployComponent did not return true, so the block is not deployed; its state stays {}",
            current.as_deref().unwrap_or("Not Configured")
        )),
        Ok(_) => {
            set_state(remote, &target, CONFIGURED).map_err(|error| {
                format!("{entry_point}.DeployComponent succeeded, but recording {CONFIGURED} failed: {error}")
            })?;
            Ok(true)
        }
    }
}

/// A BOOLEAN service's answer: an infotable's `result`, or the value itself.
fn boolean(reply: Option<&Value>) -> Option<bool> {
    let reply = reply?;
    reply.pointer("/rows/0/result").unwrap_or(reply).as_bool()
}

fn set_state(remote: &dyn Remote, target: &ServiceTarget, state: &str) -> Result<(), ServerError> {
    let values = json!({
        "dataShape": { "fieldDefinitions": { STATE: { "name": STATE, "baseType": "STRING" } } },
        "rows": [{ STATE: state }],
    });
    remote
        .call_service(
            target,
            "SetPropertyValues",
            &json!({ "values": values }),
            Duration::from_secs(60),
        )
        .map(|_| ())
}
