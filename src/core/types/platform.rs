use super::super::config::Solution;
use super::super::entity_key::ServiceTarget;
use super::super::progress::{self, Progress, NONE};
use super::super::server::{Client, ServerError};
use super::super::workspace;
use super::model::{load_model, Member, Parameter, Property, Service, TypedValue};
use super::write::write_model;
use super::{PlatformOutcome, TypesError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const PLATFORM_TIMEOUT: Duration = Duration::from_secs(120);

/// The read-only calls needed to build the platform cache. Kept small for fake-server tests.
pub trait Remote {
    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
    ) -> Result<Option<Value>, ServerError>;
}

impl Remote for Client {
    fn call(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &Value,
    ) -> Result<Option<Value>, ServerError> {
        self.call_service(target, service, parameters, PLATFORM_TIMEOUT)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Platform {
    #[serde(default)]
    pub(super) templates: BTreeMap<String, PlatformMeta>,
    #[serde(default)]
    pub(super) shapes: BTreeMap<String, PlatformMeta>,
    #[serde(default)]
    pub(super) resources: BTreeMap<String, PlatformMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PlatformMeta {
    #[serde(default)]
    pub(super) services: BTreeMap<String, PlatformService>,
    #[serde(default)]
    pub(super) properties: BTreeMap<String, PlatformProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PlatformService {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(default)]
    inputs: BTreeMap<String, PlatformInput>,
    result: PlatformValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PlatformInput {
    #[serde(rename = "baseType")]
    base_type: String,
    #[serde(rename = "dataShape", default, skip_serializing_if = "Option::is_none")]
    data_shape: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PlatformValue {
    #[serde(rename = "baseType")]
    base_type: String,
    #[serde(rename = "dataShape", default, skip_serializing_if = "Option::is_none")]
    data_shape: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PlatformProperty {
    #[serde(rename = "baseType")]
    pub(super) base_type: String,
    #[serde(rename = "dataShape", default, skip_serializing_if = "Option::is_none")]
    pub(super) data_shape: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) description: String,
}

fn is_false(value: &bool) -> bool {
    !value
}

/// Fetch a complete new cache before replacing the old one, then regenerate declarations from it.
pub fn fetch_platform(
    remote: &dyn Remote,
    solution: &Solution,
) -> Result<PlatformOutcome, TypesError> {
    fetch_platform_with_progress(remote, solution, &NONE)
}

/// Like [`fetch_platform`], and report one step per template, shape and resource fetched.
pub fn fetch_platform_with_progress(
    remote: &dyn Remote,
    solution: &Solution,
    progress: &dyn Progress,
) -> Result<PlatformOutcome, TypesError> {
    let (model, model_skipped) = load_model(solution);
    let local_templates: BTreeSet<&str> = model
        .entities
        .iter()
        .filter(|entity| entity.collection == "ThingTemplates")
        .map(|entity| entity.name.as_str())
        .collect();
    let local_shapes: BTreeSet<&str> = model
        .entities
        .iter()
        .filter(|entity| entity.collection == "ThingShapes")
        .map(|entity| entity.name.as_str())
        .collect();
    let mut templates = BTreeSet::from(["GenericThing".to_string()]);
    let mut shapes = BTreeSet::new();
    for entity in &model.entities {
        if let Some(template) = &entity.template {
            if !local_templates.contains(template.as_str()) {
                templates.insert(template.clone());
            }
        }
        for shape in &entity.shapes {
            if !local_shapes.contains(shape.as_str()) {
                shapes.insert(shape.clone());
            }
        }
    }

    let mut platform = Platform::default();
    let mut skipped = Vec::new();
    let template_phase =
        progress::phase(progress, "fetching templates", Some(templates.len() as u64));
    for name in templates {
        progress.message(&name);
        fetch_one(
            remote,
            "ThingTemplates",
            &name,
            "GetInstanceMetadataAsJSON",
            &mut platform.templates,
            &mut skipped,
        )?;
        progress.advance(1);
    }
    drop(template_phase);
    let shape_phase = progress::phase(progress, "fetching shapes", Some(shapes.len() as u64));
    for name in shapes {
        progress.message(&name);
        fetch_one(
            remote,
            "ThingShapes",
            &name,
            "GetInstanceMetadataAsJSON",
            &mut platform.shapes,
            &mut skipped,
        )?;
        progress.advance(1);
    }
    drop(shape_phase);

    let listing_phase = progress::phase(progress, "listing resources", Some(1));
    let listing = required_reply(
        remote.call(
            &ServiceTarget::platform("Resources", "EntityServices"),
            "GetEntityList",
            &json!({ "type": "Resource", "maxItems": 1000 }),
        )?,
        "Resources/EntityServices.GetEntityList",
    )?;
    let resource_names = listing
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            TypesError::Platform(
                "Resources/EntityServices.GetEntityList returned no rows array".to_string(),
            )
        })?
        .iter()
        .map(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    TypesError::Platform(
                        "Resources/EntityServices.GetEntityList returned a row without a name"
                            .to_string(),
                    )
                })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    progress.advance(1);
    drop(listing_phase);
    let resource_phase = progress::phase(
        progress,
        "fetching resources",
        Some(resource_names.len() as u64),
    );
    for name in resource_names {
        progress.message(&name);
        fetch_one(
            remote,
            "Resources",
            &name,
            "GetMetadataAsJSON",
            &mut platform.resources,
            &mut skipped,
        )?;
        progress.advance(1);
    }
    drop(resource_phase);

    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&platform).expect("platform cache structs are JSON encodable")
    );
    workspace::write_lf_if_changed(&solution.root.join(".twaco/platform.json"), &text)?;
    let types = write_model(solution, &model, Some(&platform), model_skipped)?;
    Ok(PlatformOutcome {
        templates: platform.templates.len(),
        shapes: platform.shapes.len(),
        resources: platform.resources.len(),
        skipped,
        types,
    })
}

fn fetch_one(
    remote: &dyn Remote,
    collection: &str,
    name: &str,
    service: &str,
    destination: &mut BTreeMap<String, PlatformMeta>,
    skipped: &mut Vec<String>,
) -> Result<(), TypesError> {
    let target = ServiceTarget::entity(collection, name).map_err(ServerError::from)?;
    match remote.call(&target, service, &json!({})) {
        Ok(reply) => {
            let reply = required_reply(reply, &format!("{collection}/{name}.{service}"))?;
            destination.insert(name.to_string(), trim_metadata(&reply)?);
            Ok(())
        }
        Err(error) if error.is_not_found() => {
            skipped.push(format!("{collection}/{name}: {error}"));
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn required_reply(reply: Option<Value>, call: &str) -> Result<Value, TypesError> {
    reply.ok_or_else(|| TypesError::Platform(format!("{call} returned an empty body")))
}

pub(super) fn trim_metadata(value: &Value) -> Result<PlatformMeta, TypesError> {
    let object =
        |value: Option<&Value>, what: &str| -> Result<BTreeMap<String, Value>, TypesError> {
            match value {
                None | Some(Value::Null) => Ok(BTreeMap::new()),
                Some(Value::Object(map)) => {
                    Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                }
                Some(_) => Err(TypesError::Platform(format!(
                    "platform metadata {what} is not an object"
                ))),
            }
        };
    let mut services = BTreeMap::new();
    for (name, raw) in object(value.get("serviceDefinitions"), "serviceDefinitions")? {
        let inputs = object(
            raw.pointer("/Inputs/fieldDefinitions"),
            "service Inputs.fieldDefinitions",
        )?
        .into_iter()
        .map(|(name, field)| {
            let aspects = field.get("aspects");
            Ok((
                name,
                PlatformInput {
                    base_type: required_string(&field, "baseType")?,
                    data_shape: optional_string(aspects.and_then(|v| v.get("dataShape"))),
                    required: aspects
                        .and_then(|v| v.get("isRequired"))
                        .is_some_and(|v| v.as_bool() == Some(true) || v.as_str() == Some("true")),
                    description: optional_string(field.get("description")).unwrap_or_default(),
                },
            ))
        })
        .collect::<Result<_, TypesError>>()?;
        let outputs = raw.get("Outputs").unwrap_or(&Value::Null);
        services.insert(
            name,
            PlatformService {
                description: optional_string(raw.get("description")).unwrap_or_default(),
                inputs,
                result: PlatformValue {
                    base_type: required_string(outputs, "baseType")?,
                    data_shape: optional_string(outputs.get("dataShape")),
                },
            },
        );
    }
    let mut properties = BTreeMap::new();
    for (name, raw) in object(value.get("propertyDefinitions"), "propertyDefinitions")? {
        properties.insert(
            name,
            PlatformProperty {
                base_type: required_string(&raw, "baseType")?,
                data_shape: optional_string(raw.pointer("/aspects/dataShape")),
                description: optional_string(raw.get("description")).unwrap_or_default(),
            },
        );
    }
    Ok(PlatformMeta {
        services,
        properties,
    })
}

fn required_string(value: &Value, field: &str) -> Result<String, TypesError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| TypesError::Platform(format!("platform metadata has no string {field}")))
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn merge_platform_meta(members: &mut BTreeMap<String, Member>, meta: &PlatformMeta) {
    for (name, property) in &meta.properties {
        members.entry(name.clone()).or_insert_with(|| {
            Member::Property(Property {
                name: name.clone(),
                description: property.description.clone(),
                value: TypedValue {
                    base_type: property.base_type.clone(),
                    data_shape: property.data_shape.clone(),
                },
            })
        });
    }
    for (name, service) in &meta.services {
        members.entry(name.clone()).or_insert_with(|| {
            Member::Service(Service {
                name: name.clone(),
                description: service.description.clone(),
                parameters: service
                    .inputs
                    .iter()
                    .map(|(name, input)| Parameter {
                        name: name.clone(),
                        description: input.description.clone(),
                        value: TypedValue {
                            base_type: input.base_type.clone(),
                            data_shape: input.data_shape.clone(),
                        },
                        required: input.required,
                        default: None,
                    })
                    .collect(),
                result: TypedValue {
                    base_type: service.result.base_type.clone(),
                    data_shape: service.result.data_shape.clone(),
                },
            })
        });
    }
}
