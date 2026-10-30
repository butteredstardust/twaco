use super::super::config::Solution;
use super::super::entity::key::ServiceTarget;
use super::super::lock::{self, WorkspaceLock};
use super::super::progress::{self, Progress};
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

    /// A file the web application serves; `None` where this remote cannot serve files.
    fn web_file(&self, _path: &str) -> Result<Option<Vec<u8>>, ServerError> {
        Ok(None)
    }
}

impl Remote for Client {
    fn web_file(&self, path: &str) -> Result<Option<Vec<u8>>, ServerError> {
        self.fetch_web_file(path).map(Some)
    }

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
pub(crate) struct Platform {
    #[serde(default)]
    pub(crate) templates: BTreeMap<String, PlatformMeta>,
    #[serde(default)]
    pub(crate) shapes: BTreeMap<String, PlatformMeta>,
    #[serde(default)]
    pub(crate) resources: BTreeMap<String, PlatformMeta>,
    /// The mashup builder's code of every extension installed on the server, as the Builder
    /// loads it: where `mashup add` finds the widgets those extensions define.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) extension_widgets: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlatformMeta {
    #[serde(default)]
    pub(crate) services: BTreeMap<String, PlatformService>,
    #[serde(default)]
    pub(crate) properties: BTreeMap<String, PlatformProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlatformService {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) inputs: BTreeMap<String, PlatformInput>,
    pub(crate) result: PlatformValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlatformInput {
    #[serde(rename = "baseType")]
    pub(crate) base_type: String,
    #[serde(rename = "dataShape", default, skip_serializing_if = "Option::is_none")]
    pub(crate) data_shape: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlatformValue {
    #[serde(rename = "baseType")]
    pub(crate) base_type: String,
    #[serde(rename = "dataShape", default, skip_serializing_if = "Option::is_none")]
    pub(crate) data_shape: Option<String>,
}

/// Read the optional platform cache. A malformed or unreadable cache deliberately behaves as no
/// cache: callers must leave platform-dependent facts unchecked rather than guess.
pub(crate) fn cached(solution: &Solution) -> Option<Platform> {
    std::fs::read(solution.root.join(".twaco/platform.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

/// Add one ThingTemplate or ThingShape the solution does not define to the platform cache, as
/// `types --platform` would have, when the cache lacks it; other entries stay. `Ok(false)` when
/// the server has no such entity.
/// Add one template or shape to the platform cache. The cache is the workspace's: writing it
/// needs the workspace lock, which `_held` proves the caller holds.
pub(crate) fn cache_one(
    remote: &dyn Remote,
    solution: &Solution,
    collection: &str,
    name: &str,
    _held: &WorkspaceLock,
) -> Result<bool, TypesError> {
    lock::reject_unlocked_write_links(solution)?;
    let mut platform = cached(solution).unwrap_or_default();
    let (destination, service) = match collection {
        "ThingTemplates" => (&mut platform.templates, "GetInstanceMetadataAsJSON"),
        "ThingShapes" => (&mut platform.shapes, "GetInstanceMetadataAsJSON"),
        _ => return Ok(false),
    };
    if destination.contains_key(name) {
        return Ok(true);
    }
    let mut skipped = Vec::new();
    fetch_one(remote, collection, name, service, destination, &mut skipped)?;
    if !skipped.is_empty() {
        return Ok(false);
    }
    let text =
        serde_json::to_string_pretty(&platform).expect("platform cache structs are JSON encodable");
    workspace::write_lf_if_changed(&solution.root.join(".twaco/platform.json"), &text)?;
    Ok(true)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlatformProperty {
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

/// [`fetch_platform_with_progress`] without a progress reporter.
#[cfg(test)]
pub(crate) fn fetch_platform(
    remote: &dyn Remote,
    solution: &Solution,
    held: &WorkspaceLock,
) -> Result<PlatformOutcome, TypesError> {
    fetch_platform_with_progress(remote, solution, &progress::NONE, held)
}

/// Fetch a complete new cache before replacing the old one, then regenerate declarations from it.
///
/// It reports one step per template, shape and resource fetched.
pub(crate) fn fetch_platform_with_progress(
    remote: &dyn Remote,
    solution: &Solution,
    progress: &dyn Progress,
    held: &WorkspaceLock,
) -> Result<PlatformOutcome, TypesError> {
    lock::reject_unlocked_write_links(solution)?;
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
    match extension_widget_code(remote) {
        Ok(code) => platform.extension_widgets = code.unwrap_or_default(),
        Err(why) => skipped.push(format!("extension widgets: {why}")),
    }

    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&platform).expect("platform cache structs are JSON encodable")
    );
    let cache = (solution.root.join(".twaco/platform.json"), text);
    let types = write_model(
        solution,
        &model,
        Some(&platform),
        model_skipped,
        held,
        std::slice::from_ref(&cache),
    )?;
    Ok(PlatformOutcome {
        templates: platform.templates.len(),
        shapes: platform.shapes.len(),
        resources: platform.resources.len(),
        skipped,
        types,
    })
}

/// The combined mashup-builder code of the server's extensions: `Builder/index.html` names it in
/// `COMBINED_EXT_JS`, a file whose name changes with every extension installed.
fn extension_widget_code(remote: &dyn Remote) -> Result<Option<String>, String> {
    let Some(page) = remote
        .web_file("Builder/index.html")
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let page = String::from_utf8_lossy(&page);
    let marker = "COMBINED_EXT_JS='";
    let Some(start) = page.find(marker).map(|at| at + marker.len()) else {
        return Err("Builder/index.html names no COMBINED_EXT_JS".to_string());
    };
    let Some(length) = page[start..].find('\'') else {
        return Err("Builder/index.html: COMBINED_EXT_JS is not closed".to_string());
    };
    let file = &page[start..start + length];
    if !is_bundle_name(file) {
        return Err(format!(
            "Builder/index.html names {file:?} as COMBINED_EXT_JS"
        ));
    }
    let code = remote
        .web_file(&format!("Builder/{file}"))
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    String::from_utf8(code)
        .map(Some)
        .map_err(|_| format!("Builder/{file} is not UTF-8"))
}

/// `js/<name>.js` in plain letters, digits, `.`, `_` and `-`: the request carries the
/// credentials, so no escape or traversal of any spelling may steer it elsewhere.
fn is_bundle_name(file: &str) -> bool {
    file.strip_prefix("js/")
        .and_then(|name| name.strip_suffix(".js"))
        .is_some_and(|stem| {
            !stem.is_empty()
                && !stem.contains("..")
                && stem
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
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

#[cfg(test)]
mod name_tests {
    #[test]
    fn only_a_plain_bundle_name_is_fetched() {
        assert!(super::is_bundle_name(
            "js/CombinedExtensions.20261028_074036.js"
        ));
        for bad in [
            "../x.js",
            "js/../x.js",
            "js/%2e%2e/x.js",
            "js/a/b.js",
            "/js/x.js",
            "js/x.css",
            "js/.js",
            "http://h/js/x.js",
            "js/x.js?y=1",
        ] {
            assert!(!super::is_bundle_name(bad), "{bad}");
        }
    }
}
