//! Offline service catalog derived from the entity model used by TypeScript generation.

use super::codes::{Coded, ErrorCode};
use super::config::Solution;
use super::index::inherit::{implements_shape, inheritance_chain, Inherits};
use super::index::Index;
use super::types::{self, Entity, Member, Service};
use super::types::{Parameter, Platform, TypedValue};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub struct Query<'a> {
    pub project: Option<&'a str>,
    /// An entity (full name or last dotted segment); when none has that name, `Entity.Service`
    /// or a service name only one entity declares, which narrows to that service.
    pub entity: Option<&'a str>,
    pub text: Option<&'a str>,
    /// Include each service's script. Only with `entity`: a whole solution's scripts are what
    /// the repository is for.
    pub body: bool,
}

#[derive(Debug, Serialize)]
pub struct Catalog {
    pub entities: Vec<CatalogEntity>,
    #[serde(skip)]
    pub skipped: Vec<String>,
}

impl Catalog {
    pub fn service_count(&self) -> usize {
        self.entities
            .iter()
            .map(|entity| entity.services.len())
            .sum()
    }
}

#[derive(Debug, Serialize)]
pub struct CatalogEntity {
    pub collection: String,
    pub name: String,
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inherits: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub implemented_by: Vec<String>,
    pub services: Vec<CatalogService>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CatalogService {
    pub name: String,
    pub parameters: Vec<CatalogParameter>,
    pub result: CatalogValue,
    pub description: String,
    pub from: String,
    pub has_script: bool,
    /// The script, when asked for and the service has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The collection and name of the entity that declares it, where its script lives.
    #[serde(skip)]
    pub(crate) declared_by: (String, String),
}

#[derive(Clone, Debug, Serialize)]
pub struct CatalogParameter {
    pub name: String,
    #[serde(rename = "baseType")]
    pub base_type: String,
    #[serde(rename = "dataShape", skip_serializing_if = "Option::is_none")]
    pub data_shape: Option<String>,
    pub(crate) required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) default: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CatalogValue {
    #[serde(rename = "baseType")]
    pub base_type: String,
    #[serde(rename = "dataShape", skip_serializing_if = "Option::is_none")]
    pub data_shape: Option<String>,
}

/// Build the complete inheritance view used by scoped refactors.
///
/// A Thing follows `thingTemplate`, templates follow `baseThingTemplate`, and each level adds its
/// implemented ThingShapes. `implemented_by` is transitive through those template chains. Unlike
/// [`build`], this includes entities with no services because configuration tables do not require
/// a service declaration. The second value lists entities the model could not read: a caller that
/// scopes a refactor by inheritance must refuse when it is not empty.
pub(crate) fn inheritance(solution: &Solution) -> (Vec<CatalogEntity>, Vec<String>) {
    let index = Index::build(solution);
    let mut entities: Vec<CatalogEntity> = index
        .entities()
        .filter(|(key, _)| {
            matches!(
                key.collection(),
                "Things" | "ThingTemplates" | "ThingShapes"
            )
        })
        .map(|(key, node)| CatalogEntity {
            collection: key.collection().to_string(),
            name: key.name().to_string(),
            project: node.project.clone(),
            extension: None,
            inherits: index.inheritance_names(key),
            implemented_by: if key.collection() == "ThingShapes" {
                index.implementers(key, None)
            } else {
                Vec::new()
            },
            services: Vec::new(),
        })
        .collect();
    entities.sort_by(|a, b| (&a.name, &a.collection).cmp(&(&b.name, &b.collection)));
    let skipped = index
        .unreadable()
        .iter()
        .map(|entry| {
            if entry.why.is_empty() {
                entry.what.clone()
            } else {
                format!("{}: {}", entry.what, entry.why)
            }
        })
        .collect();
    (entities, skipped)
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    UnknownEntity(String),
    #[error("{0}")]
    Ambiguous(String),
}

/// Build the service catalog without consulting the ThingWorx server.
pub fn build(solution: &Solution, query: Query<'_>) -> Result<Catalog, CatalogError> {
    if let Some(project) = query.project {
        if solution.project(project).is_none() {
            return Err(CatalogError::Invalid(format!(
                "this solution has no project named {project}"
            )));
        }
    }
    let (model, skipped) = types::load_model(solution);
    if query.body && query.entity.is_none() {
        return Err(CatalogError::Invalid(
            "--body needs an entity or an Entity.Service: the whole solution's scripts are in \
             the repository"
                .to_string(),
        ));
    }
    let mut pool: Vec<&Entity> = model
        .entities
        .iter()
        .filter(|entity| {
            query
                .project
                .is_none_or(|project| entity.project == project)
        })
        .collect();
    let mut only_service = None;
    if let Some(name) = query.entity {
        match resolve(&pool, name) {
            Ok(entity) => pool = vec![entity],
            // No entity has that name: it may be `Entity.Service`, or a service's own name.
            Err(CatalogError::UnknownEntity(_)) => {
                let (entity, service) = resolve_service(&pool, &model.entities, name)?;
                pool = vec![entity];
                only_service = Some(service);
            }
            Err(other) => return Err(other),
        }
    }

    let mut entities = Vec::new();
    let needle = query.text.map(str::to_lowercase);
    for entity in pool {
        let implemented_by = if entity.collection == "ThingShapes" {
            implementers(entity, &model.entities, query.project)
        } else {
            Vec::new()
        };
        let inherits = inheritance_names(entity, &model.entities);
        let mut services = if entity.collection == "ThingShapes" {
            own_services(entity, solution)
        } else {
            callable_services(entity, &model.entities, solution)
        };
        // A service matches by what it is, never by the entity it sits on: entity names share
        // the project's prefix, so a word of it would match every service. `<entity>` narrows
        // to an entity.
        if let Some(needle) = needle.as_deref() {
            services.retain(|service| service_matches(service, needle));
        }
        if let Some(only) = &only_service {
            services.retain(|service| &service.name == only);
        }
        if query.body {
            for service in &mut services {
                service.body = script_body(solution, &service.declared_by, &service.name);
            }
        }
        if services.is_empty() && (query.entity.is_none() || query.text.is_some()) {
            continue;
        }
        entities.push(CatalogEntity {
            collection: entity.collection.clone(),
            name: entity.name.clone(),
            project: entity.project.clone(),
            extension: entity.extension.clone(),
            inherits,
            implemented_by,
            services,
        });
    }
    entities.sort_by(|a, b| {
        (collection_rank(&a.collection), &a.name).cmp(&(collection_rank(&b.collection), &b.name))
    });
    Ok(Catalog { entities, skipped })
}

fn resolve<'a>(entities: &[&'a Entity], name: &str) -> Result<&'a Entity, CatalogError> {
    let exact: Vec<&Entity> = entities
        .iter()
        .copied()
        .filter(|entity| entity.name == name)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    if exact.len() > 1 {
        return Err(CatalogError::Ambiguous(format!(
            "entity {name} is ambiguous in this solution"
        )));
    }
    let suffix: Vec<&Entity> = entities
        .iter()
        .copied()
        .filter(|entity| {
            entity
                .name
                .rsplit('.')
                .next()
                .is_some_and(|part| part.eq_ignore_ascii_case(name))
        })
        .collect();
    match suffix.as_slice() {
        [] => Err(CatalogError::UnknownEntity(format!(
            "no entity named {name} in this solution"
        ))),
        [entity] => Ok(entity),
        many => Err(CatalogError::Ambiguous(format!(
            "{name} is ambiguous; it could be {}",
            many.iter()
                .map(|entity| entity.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// `Entity.Service` (the entity as [`resolve`] finds it, then a service it can call), or a
/// service name exactly one entity of the pool declares itself.
fn resolve_service<'a>(
    pool: &[&'a Entity],
    entities: &[Entity],
    name: &str,
) -> Result<(&'a Entity, String), CatalogError> {
    let declares = |entity: &Entity, service: &str| {
        entity
            .members
            .iter()
            .any(|member| matches!(member, Member::Service(own) if own.name == service))
    };
    if let Some((entity_name, service)) = name.rsplit_once('.') {
        // An entity part that names several entities is that, not a missing service.
        if let Err(CatalogError::Ambiguous(why)) = resolve(pool, entity_name) {
            return Err(CatalogError::Ambiguous(why));
        }
        if let Ok(entity) = resolve(pool, entity_name) {
            // A service the entity calls: its own, a shape's or a template's.
            let mut chain = vec![entity];
            let mut at = entity;
            while let Some(template) = at.template.as_deref().and_then(|template| {
                entities.iter().find(|candidate| {
                    candidate.collection == "ThingTemplates" && candidate.name == template
                })
            }) {
                if chain.iter().any(|seen| std::ptr::eq(*seen, template)) {
                    break;
                }
                chain.push(template);
                at = template;
            }
            let shapes = chain
                .iter()
                .flat_map(|item| &item.shapes)
                .filter_map(|shape| {
                    entities.iter().find(|candidate| {
                        candidate.collection == "ThingShapes" && &candidate.name == shape
                    })
                });
            if chain
                .iter()
                .copied()
                .chain(shapes)
                .any(|item| declares(item, service))
            {
                return Ok((entity, service.to_string()));
            }
            return Err(CatalogError::UnknownEntity(format!(
                "{} has no service {service}; `twaco catalog {}` lists what it has",
                entity.name, entity.name
            )));
        }
    }
    let declaring: Vec<&Entity> = pool
        .iter()
        .copied()
        .filter(|entity| declares(entity, name))
        .collect();
    match declaring.as_slice() {
        [] => Err(CatalogError::UnknownEntity(format!(
            "no entity or service named {name} in this solution"
        ))),
        [entity] => Ok((entity, name.to_string())),
        many => Err(CatalogError::Ambiguous(format!(
            "{name} is declared by more than one entity; name one: {}",
            many.iter()
                .map(|entity| format!("{}.{name}", entity.name))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// A service's script: its sidecar, or the script inside the declaring entity's XML when the
/// entity was never extracted.
fn script_body(
    solution: &Solution,
    declared_by: &(String, String),
    service: &str,
) -> Option<String> {
    let (collection, name) = declared_by;
    let sidecar = solution
        .src_root()
        .join(name)
        .join("services")
        .join(service)
        .join("script.js");
    if let Ok(script) = std::fs::read_to_string(sidecar) {
        return Some(script);
    }
    let file = super::workspace::entities(solution)
        .into_iter()
        .find(|file| &file.info.collection == collection && &file.info.name == name)?;
    let src = std::fs::read(&file.path).ok()?;
    super::sidecar::extract_services(&src)
        .ok()?
        .into_iter()
        .find(|sidecar| sidecar.name == service)
        .map(|sidecar| sidecar.script().to_string())
}

fn collection_rank(collection: &str) -> usize {
    ["Things", "ThingTemplates", "ThingShapes"]
        .iter()
        .position(|candidate| *candidate == collection)
        .unwrap_or(usize::MAX)
}

fn own_services(entity: &Entity, solution: &Solution) -> Vec<CatalogService> {
    let mut services: Vec<CatalogService> = entity
        .members
        .iter()
        .filter_map(|member| match member {
            Member::Service(service) => Some(catalog_service(
                service,
                "own",
                has_script(solution, entity, &service.name),
                entity,
            )),
            Member::Property(_) => None,
        })
        .collect();
    services.sort_by(|a, b| a.name.cmp(&b.name));
    services
}

fn callable_services(
    entity: &Entity,
    entities: &[Entity],
    solution: &Solution,
) -> Vec<CatalogService> {
    let find = |collection: &str, name: &str| {
        entities
            .iter()
            .find(|candidate| candidate.collection == collection && candidate.name == name)
    };
    let mut found = BTreeMap::<String, (&Service, &Entity)>::new();
    let mut sources = Vec::<&Entity>::new();
    let mut visited_templates = BTreeSet::new();
    let mut visited_shapes = BTreeSet::new();
    let mut current = Some(entity);
    while let Some(item) = current {
        sources.push(item);
        for member in &item.members {
            if let Member::Service(service) = member {
                found.entry(service.name.clone()).or_insert((service, item));
            }
        }
        for shape_name in &item.shapes {
            if visited_shapes.insert(shape_name.as_str()) {
                if let Some(shape) = find("ThingShapes", shape_name) {
                    sources.push(shape);
                    for member in &shape.members {
                        if let Member::Service(service) = member {
                            found
                                .entry(service.name.clone())
                                .or_insert((service, shape));
                        }
                    }
                }
            }
        }
        let Some(template_name) = item.template.as_deref() else {
            break;
        };
        if !visited_templates.insert(template_name) {
            break;
        }
        current = find("ThingTemplates", template_name);
    }
    found
        .into_iter()
        .map(|(_, (service, defined_by))| {
            let scripted = sources
                .iter()
                .any(|source| has_script(solution, source, &service.name));
            let from = if std::ptr::eq(entity, defined_by) {
                "own"
            } else {
                &defined_by.name
            };
            catalog_service(service, from, scripted, defined_by)
        })
        .collect()
}

/// Resolve one service through the same local inheritance order as the catalog. `complete` says
/// that every referenced template and shape (including GenericThing) was available locally or in
/// the optional platform cache, so a missing service is safe to diagnose.
pub(crate) fn lookup_service(
    entity: &Entity,
    entities: &[Entity],
    platform: Option<&Platform>,
    wanted: &str,
) -> ServiceLookup {
    let local = |collection: &str, name: &str| {
        entities
            .iter()
            .find(|candidate| candidate.collection == collection && candidate.name == name)
    };
    let mut complete = true;
    let mut visited_templates = BTreeSet::new();
    let mut visited_shapes = BTreeSet::new();
    let mut current = Some(entity);
    while let Some(item) = current {
        if let Some(service) = item.members.iter().find_map(|member| match member {
            Member::Service(service) if service.name == wanted => Some(service.clone()),
            _ => None,
        }) {
            return ServiceLookup {
                service: Some(service),
                complete,
            };
        }
        for shape_name in &item.shapes {
            if !visited_shapes.insert(shape_name.as_str()) {
                continue;
            }
            if let Some(shape) = local("ThingShapes", shape_name) {
                if let Some(service) = shape.members.iter().find_map(|member| match member {
                    Member::Service(service) if service.name == wanted => Some(service.clone()),
                    _ => None,
                }) {
                    return ServiceLookup {
                        service: Some(service),
                        complete,
                    };
                }
            } else if let Some(meta) = platform.and_then(|p| p.shapes.get(shape_name)) {
                if let Some(service) = meta.services.get(wanted) {
                    return ServiceLookup {
                        service: Some(platform_service(wanted, service)),
                        complete,
                    };
                }
            } else {
                complete = false;
            }
        }
        let Some(template_name) = item.template.as_deref() else {
            break;
        };
        if !visited_templates.insert(template_name) {
            break;
        }
        if let Some(template) = local("ThingTemplates", template_name) {
            current = Some(template);
        } else if let Some(meta) = platform.and_then(|p| p.templates.get(template_name)) {
            if let Some(service) = meta.services.get(wanted) {
                return ServiceLookup {
                    service: Some(platform_service(wanted, service)),
                    complete,
                };
            }
            current = None;
        } else {
            complete = false;
            current = None;
        }
    }
    // Every Thing has GenericThing's services, but only below everything else on its chain: a
    // template or shape twaco cannot see may declare the same service differently.
    match platform
        .filter(|_| complete)
        .and_then(|p| p.templates.get("GenericThing"))
    {
        Some(meta) => ServiceLookup {
            service: meta
                .services
                .get(wanted)
                .map(|service| platform_service(wanted, service)),
            complete,
        },
        None => ServiceLookup {
            service: None,
            complete: false,
        },
    }
}

/// What an ancestor holds for a service: its own implementation, and of which kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Implementation {
    None,
    Script,
    /// SQL, a route or any handler other than `Script`.
    Other,
}

/// The implementation an override of `wanted` would inherit, for `--from-parent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParentImplementation {
    /// The nearest implementing ancestor, `Collection/Name`, and the kind it implements.
    Found {
        collection: String,
        name: String,
        kind: Implementation,
    },
    /// A shape's implementation competes with another ancestor's; which applies is not known.
    Ambiguous(Vec<String>),
    /// An ancestor that may implement it is not in the repository.
    Unresolved(String),
    /// No ancestor in the repository implements it.
    Absent,
}

/// Walk the ancestors of `entity` in [`lookup_service`]'s order and find whose implementation
/// of `wanted` it inherits. An ancestor's own implementation is nearer than its shapes'; a shape's
/// implementation that another ancestor further up also provides is ambiguous, and an ancestor
/// outside the repository that may implement it stops the walk. `implements` reads an ancestor.
pub(crate) fn parent_implementation(
    entity: &Entity,
    entities: &[Entity],
    platform: Option<&Platform>,
    wanted: &str,
    implements: &dyn Fn(&Entity) -> Implementation,
) -> ParentImplementation {
    let local = |collection: &str, name: &str| {
        entities
            .iter()
            .find(|candidate| candidate.collection == collection && candidate.name == name)
    };
    // A platform ancestor whose metadata lacks the service cannot implement it.
    let may_implement = |meta: Option<&types::PlatformMeta>| {
        meta.is_none_or(|meta| meta.services.contains_key(wanted))
    };
    // (level, via a shape, the ancestor, its kind), in walk order.
    let mut hits: Vec<(usize, bool, &Entity, Implementation)> = Vec::new();
    let mut unresolved = None;
    let mut visited_templates = BTreeSet::new();
    let mut visited_shapes = BTreeSet::new();
    let mut current = Some(entity);
    let mut level = 0usize;
    'walk: while let Some(item) = current {
        if level > 0 {
            let kind = implements(item);
            if kind != Implementation::None {
                hits.push((level, false, item, kind));
            }
        }
        for shape_name in &item.shapes {
            if !visited_shapes.insert(shape_name.as_str()) {
                continue;
            }
            match local("ThingShapes", shape_name) {
                Some(shape) => {
                    let kind = implements(shape);
                    if kind != Implementation::None {
                        hits.push((level, true, shape, kind));
                    }
                }
                None if may_implement(platform.and_then(|p| p.shapes.get(shape_name))) => {
                    unresolved = Some(format!("ThingShapes/{shape_name}"));
                    break 'walk;
                }
                None => {}
            }
        }
        let Some(template_name) = item.template.as_deref() else {
            break;
        };
        if !visited_templates.insert(template_name) {
            break;
        }
        match local("ThingTemplates", template_name) {
            Some(template) => current = Some(template),
            None => {
                if may_implement(platform.and_then(|p| p.templates.get(template_name))) {
                    unresolved = Some(format!("ThingTemplates/{template_name}"));
                }
                break;
            }
        }
        level += 1;
    }
    let label = |entity: &Entity| format!("{}/{}", entity.collection, entity.name);
    let Some(&(first_level, via_shape, first, kind)) = hits.first() else {
        return match unresolved {
            Some(name) => ParentImplementation::Unresolved(name),
            None => ParentImplementation::Absent,
        };
    };
    // An ancestor's own implementation overrides its shapes', so it is recorded first at its
    // level; a shape's competes with any other implementation after it.
    let own_first = !via_shape && first_level > 0;
    if !own_first && (hits.len() > 1 || unresolved.is_some()) {
        let mut names: Vec<String> = hits.iter().map(|(_, _, entity, _)| label(entity)).collect();
        names.extend(unresolved);
        return ParentImplementation::Ambiguous(names);
    }
    ParentImplementation::Found {
        collection: first.collection.clone(),
        name: first.name.clone(),
        kind,
    }
}

/// The definition an inherited-service override may use. `Absent` means the complete ancestry
/// no longer declares it; `Unresolved` keeps callers from guessing through an external parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InheritedService {
    Defined(Service),
    Absent,
    Unresolved,
}

/// Resolve an override's inherited definition through repository source and the platform cache.
pub(crate) fn inherited_service(
    entity: &Entity,
    entities: &[Entity],
    platform: Option<&Platform>,
    wanted: &str,
) -> InheritedService {
    let lookup = lookup_service(entity, entities, platform, wanted);
    match lookup.service {
        Some(service) => InheritedService::Defined(service),
        None if lookup.complete => InheritedService::Absent,
        None => InheritedService::Unresolved,
    }
}

#[derive(Clone)]
pub(crate) struct ServiceLookup {
    pub(crate) service: Option<Service>,
    pub(crate) complete: bool,
}

fn platform_service(name: &str, service: &super::types::platform::PlatformService) -> Service {
    Service {
        name: name.to_string(),
        description: service.description.clone(),
        parameters: service
            .inputs
            .iter()
            .map(|(name, input)| Parameter {
                name: name.clone(),
                description: input.description.clone(),
                required: input.required,
                default: None,
                value: TypedValue {
                    base_type: input.base_type.clone(),
                    data_shape: input.data_shape.clone(),
                },
            })
            .collect(),
        result: TypedValue {
            base_type: service.result.base_type.clone(),
            data_shape: service.result.data_shape.clone(),
        },
    }
}

fn has_script(solution: &Solution, entity: &Entity, service: &str) -> bool {
    entity.script_services.contains(service)
        || solution
            .src_root()
            .join(&entity.name)
            .join("services")
            .join(service)
            .join("script.js")
            .is_file()
}

fn catalog_service(
    service: &Service,
    from: &str,
    has_script: bool,
    declared_by: &Entity,
) -> CatalogService {
    CatalogService {
        name: service.name.clone(),
        parameters: service
            .parameters
            .iter()
            .map(|parameter| CatalogParameter {
                name: parameter.name.clone(),
                base_type: parameter.value.base_type.clone(),
                data_shape: parameter.value.data_shape.clone(),
                required: parameter.required,
                default: parameter.default.clone(),
            })
            .collect(),
        result: CatalogValue {
            base_type: service.result.base_type.clone(),
            data_shape: service.result.data_shape.clone(),
        },
        description: service.description.clone(),
        from: from.to_string(),
        has_script,
        body: None,
        declared_by: (declared_by.collection.clone(), declared_by.name.clone()),
    }
}

fn service_matches(service: &CatalogService, needle: &str) -> bool {
    service.name.to_lowercase().contains(needle)
        || service.description.to_lowercase().contains(needle)
        || service
            .parameters
            .iter()
            .any(|parameter| parameter.name.to_lowercase().contains(needle))
}

/// What the shared inheritance walk reads of a model entity.
fn inherits(entity: &Entity) -> Inherits<'_> {
    Inherits {
        collection: &entity.collection,
        template: entity.template.as_deref(),
        shapes: &entity.shapes,
    }
}

fn template_in<'a>(entities: &'a [Entity], name: &str) -> Option<Inherits<'a>> {
    entities
        .iter()
        .find(|candidate| candidate.collection == "ThingTemplates" && candidate.name == name)
        .map(inherits)
}

pub(crate) fn inheritance_names(entity: &Entity, entities: &[Entity]) -> Vec<String> {
    inheritance_chain(inherits(entity), |name| template_in(entities, name))
}

pub(crate) fn implementers(
    shape: &Entity,
    entities: &[Entity],
    project: Option<&str>,
) -> Vec<String> {
    let mut out: Vec<String> = entities
        .iter()
        .filter(|entity| {
            entity.collection != "ThingShapes"
                && project.is_none_or(|project| entity.project == project)
        })
        .filter(|entity| {
            implements_shape(inherits(entity), &shape.name, |name| {
                template_in(entities, name)
            })
        })
        .map(|entity| entity.name.clone())
        .collect();
    out.sort();
    out
}

impl Coded for CatalogError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::UnknownEntity(_) => ErrorCode::UnknownEntity,
            Self::Ambiguous(_) => ErrorCode::Ambiguous,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::platform::PlatformMeta;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn node(collection: &str, name: &str, template: Option<&str>, shapes: &[&str]) -> Entity {
        Entity {
            name: name.to_string(),
            collection: collection.to_string(),
            project: "P".to_string(),
            extension: None,
            template: template.map(str::to_string),
            shapes: shapes.iter().map(|shape| shape.to_string()).collect(),
            members: Vec::new(),
            events: Vec::new(),
            script_services: BTreeSet::new(),
        }
    }

    fn resolve(
        entities: &[Entity],
        platform: Option<&Platform>,
        implementing: &[(&str, Implementation)],
    ) -> ParentImplementation {
        let implements = |entity: &Entity| {
            implementing
                .iter()
                .find(|(name, _)| *name == entity.name)
                .map_or(Implementation::None, |(_, kind)| *kind)
        };
        parent_implementation(&entities[0], entities, platform, "S", &implements)
    }

    fn found(collection: &str, name: &str, kind: Implementation) -> ParentImplementation {
        ParentImplementation::Found {
            collection: collection.into(),
            name: name.into(),
            kind,
        }
    }

    #[test]
    fn the_nearest_own_implementation_wins_and_a_shapes_competes() {
        let entities = [
            node("Things", "T", Some("Mid_TT"), &[]),
            node("ThingTemplates", "Mid_TT", Some("Base_TT"), &["Shape_TS"]),
            node("ThingTemplates", "Base_TT", None, &[]),
            node("ThingShapes", "Shape_TS", None, &[]),
        ];
        use Implementation::{Other, Script};
        // A nearer template's override beats the grandparent's implementation.
        assert_eq!(
            resolve(&entities, None, &[("Mid_TT", Script), ("Base_TT", Script)]),
            found("ThingTemplates", "Mid_TT", Script)
        );
        // An entity's own implementation overrides its shape's.
        assert_eq!(
            resolve(&entities, None, &[("Mid_TT", Other), ("Shape_TS", Script)]),
            found("ThingTemplates", "Mid_TT", Other)
        );
        // The nearest is not a script: the walk does not look past it for one.
        assert_eq!(
            resolve(&entities, None, &[("Base_TT", Script), ("Mid_TT", Other)]),
            found("ThingTemplates", "Mid_TT", Other)
        );
        // A shape's implementation against the base template's: which applies is not settled.
        assert_eq!(
            resolve(
                &entities,
                None,
                &[("Shape_TS", Script), ("Base_TT", Script)]
            ),
            ParentImplementation::Ambiguous(vec![
                "ThingShapes/Shape_TS".into(),
                "ThingTemplates/Base_TT".into()
            ])
        );
        assert_eq!(
            resolve(&entities, None, &[("Shape_TS", Script)]),
            found("ThingShapes", "Shape_TS", Script)
        );
        assert_eq!(resolve(&entities, None, &[]), ParentImplementation::Absent);
    }

    #[test]
    fn an_ancestor_outside_the_repository_stops_the_walk_unless_the_platform_rules_it_out() {
        let entities = [
            node("Things", "T", Some("Mid_TT"), &[]),
            node("ThingTemplates", "Mid_TT", Some("Vendor_TT"), &[]),
        ];
        assert_eq!(
            resolve(&entities, None, &[]),
            ParentImplementation::Unresolved("ThingTemplates/Vendor_TT".into())
        );
        let mut platform = Platform::default();
        platform
            .templates
            .insert("Vendor_TT".into(), PlatformMeta::default());
        assert_eq!(
            resolve(&entities, Some(&platform), &[]),
            ParentImplementation::Absent
        );
        // Found nearer than the unknown ancestor: it does not matter.
        assert_eq!(
            resolve(&entities, None, &[("Mid_TT", Implementation::Script)]),
            found("ThingTemplates", "Mid_TT", Implementation::Script)
        );
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-catalog-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        for collection in ["ThingShapes", "ThingTemplates", "Things"] {
            std::fs::create_dir_all(root.join(collection)).unwrap();
        }
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(root.join("ThingShapes/P.Shape.xml"), entity("ThingShapes", "ThingShape", "P.Shape", "", "<ServiceDefinitions><ServiceDefinition name=\"ShapeService\" description=\"from the shape\"><ParameterDefinitions><FieldDefinition name=\"rows\" baseType=\"INFOTABLE\" aspect.dataShape=\"P.Row\" aspect.isRequired=\"true\" aspect.defaultValue=\"[]\"/></ParameterDefinitions><ResultType baseType=\"BOOLEAN\"/></ServiceDefinition></ServiceDefinitions>" )).unwrap();
        std::fs::write(root.join("ThingTemplates/P.Template.xml"), entity("ThingTemplates", "ThingTemplate", "P.Template", " baseThingTemplate=\"GenericThing\"", "<ImplementedShapes><ImplementedShape name=\"P.Shape\"/></ImplementedShapes><ThingShape><ServiceDefinitions><ServiceDefinition name=\"TemplateService\"><ResultType baseType=\"STRING\"/></ServiceDefinition></ServiceDefinitions></ThingShape>" )).unwrap();
        std::fs::write(root.join("Things/P.Thing.xml"), entity("Things", "Thing", "P.Thing", " thingTemplate=\"P.Template\"", "<ThingShape><ServiceDefinitions><ServiceDefinition name=\"OwnService\" description=\"dashboard work\"><ResultType baseType=\"NUMBER\"/></ServiceDefinition></ServiceDefinitions><ServiceImplementations><ServiceImplementation name=\"OwnService\"><HandlerDefinition name=\"Script\"/><ConfigurationTables><ConfigurationTable><DataShape><FieldDefinitions/></DataShape><Rows><Row><code>return 1;</code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape>" )).unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution)
    }

    fn entity(collection: &str, tag: &str, name: &str, attributes: &str, body: &str) -> String {
        format!("<Entities><{collection}><{tag} name=\"{name}\" projectName=\"P\"{attributes}>{body}</{tag}></{collection}></Entities>")
    }

    #[test]
    fn event_definitions_do_not_change_catalog_output() {
        let (_dir, root, solution) = fixture();
        let before = serde_json::to_value(build(&solution, Query::default()).unwrap()).unwrap();
        let path = root.join("ThingShapes/P.Shape.xml");
        let xml = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, xml.replace("</ThingShape>", "<EventDefinitions><EventDefinition name=\"Changed\" dataShape=\"Acme.Rows\"/></EventDefinitions></ThingShape>")).unwrap();
        let after = serde_json::to_value(build(&solution, Query::default()).unwrap()).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn inherited_service_distinguishes_defined_absent_and_unresolved() {
        let (_dir, _, solution) = fixture();
        let (model, _) = types::load_model(&solution);
        let entity = model
            .entities
            .iter()
            .find(|entity| entity.name == "P.Thing")
            .unwrap();
        assert!(matches!(
            inherited_service(entity, &model.entities, None, "TemplateService"),
            InheritedService::Defined(_)
        ));
        assert!(matches!(
            inherited_service(entity, &model.entities, None, "Gone"),
            InheritedService::Unresolved
        ));
        let platform = Platform {
            templates: BTreeMap::from([("GenericThing".to_string(), PlatformMeta::default())]),
            ..Platform::default()
        };
        assert!(matches!(
            inherited_service(entity, &model.entities, Some(&platform), "Gone"),
            InheritedService::Absent
        ));
    }

    #[test]
    fn inheritance_implementers_filters_and_unknown_names() {
        let (_dir, _, solution) = fixture();
        let thing = build(
            &solution,
            Query {
                entity: Some("P.Thing"),
                ..Query::default()
            },
        )
        .unwrap();
        let services = &thing.entities[0].services;
        assert_eq!(
            services
                .iter()
                .map(|service| (service.name.as_str(), service.from.as_str()))
                .collect::<Vec<_>>(),
            [
                ("OwnService", "own"),
                ("ShapeService", "P.Shape"),
                ("TemplateService", "P.Template")
            ]
        );
        assert!(services[0].has_script);
        assert_eq!(services[1].parameters[0].default.as_deref(), Some("[]"));
        assert_eq!(
            thing.entities[0].inherits,
            ["P.Template", "P.Shape", "GenericThing"]
        );

        let shape = build(
            &solution,
            Query {
                entity: Some("P.Shape"),
                ..Query::default()
            },
        )
        .unwrap();
        assert_eq!(shape.entities[0].implemented_by, ["P.Template", "P.Thing"]);
        assert_eq!(shape.entities[0].services.len(), 1);

        let searched = build(
            &solution,
            Query {
                text: Some("DASHBOARD"),
                ..Query::default()
            },
        )
        .unwrap();
        assert_eq!(searched.service_count(), 1);
        assert_eq!(searched.entities[0].services[0].name, "OwnService");
        assert!(build(
            &solution,
            Query {
                project: Some("Nope"),
                ..Query::default()
            }
        )
        .is_err());
        assert!(build(
            &solution,
            Query {
                entity: Some("Missing"),
                ..Query::default()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("no entity or service named Missing"));
    }

    fn one(solution: &Solution, entity: &str, body: bool) -> Result<Catalog, CatalogError> {
        build(
            solution,
            Query {
                entity: Some(entity),
                body,
                ..Query::default()
            },
        )
    }

    fn names(catalog: &Catalog) -> Vec<(String, String)> {
        catalog
            .entities
            .iter()
            .flat_map(|entity| {
                entity
                    .services
                    .iter()
                    .map(|service| (entity.name.clone(), service.name.clone()))
            })
            .collect()
    }

    #[test]
    fn an_entity_service_or_a_service_one_entity_declares_narrows_to_it() {
        let (_dir, _, solution) = fixture();
        let pair = |entity: &str, service: &str| vec![(entity.to_string(), service.to_string())];
        assert_eq!(
            names(&one(&solution, "P.Thing.OwnService", false).unwrap()),
            pair("P.Thing", "OwnService")
        );
        // A service the Thing reaches through its template and shape, by the entity's last segment.
        assert_eq!(
            names(&one(&solution, "Thing.ShapeService", false).unwrap()),
            pair("P.Thing", "ShapeService")
        );
        // A bare name: the one entity that declares it, not every entity that can call it.
        assert_eq!(
            names(&one(&solution, "TemplateService", false).unwrap()),
            pair("P.Template", "TemplateService")
        );
        // An entity name still means the entity.
        assert_eq!(names(&one(&solution, "Thing", false).unwrap()).len(), 3);
        let unknown = one(&solution, "P.Thing.Nope", false).unwrap_err();
        assert!(
            unknown.to_string().contains("P.Thing has no service Nope"),
            "{unknown}"
        );
        assert!(matches!(
            one(&solution, "Nope", false),
            Err(CatalogError::UnknownEntity(_))
        ));
    }

    #[test]
    fn a_service_declared_twice_is_ambiguous_and_names_each() {
        let (_dir, root, solution) = fixture();
        std::fs::write(
            root.join("Things/P.Other.xml"),
            entity("Things", "Thing", "P.Other", "", "<ThingShape><ServiceDefinitions><ServiceDefinition name=\"OwnService\"><ResultType baseType=\"NUMBER\"/></ServiceDefinition></ServiceDefinitions></ThingShape>"),
        )
        .unwrap();
        let error = one(&solution, "OwnService", false).unwrap_err();
        assert!(matches!(error, CatalogError::Ambiguous(_)));
        assert!(
            error
                .to_string()
                .contains("P.Other.OwnService, P.Thing.OwnService"),
            "{error}"
        );
        // An entity part two entities end in is ambiguous, not a missing service.
        std::fs::write(
            root.join("Things/Q.Thing.xml"),
            entity(
                "Things",
                "Thing",
                "Q.Thing",
                "",
                "<ThingShape><ServiceDefinitions/></ThingShape>",
            ),
        )
        .unwrap();
        let error = one(&solution, "Thing.OwnService", false).unwrap_err();
        assert!(matches!(error, CatalogError::Ambiguous(_)), "{error}");
    }

    #[test]
    fn body_reads_the_sidecar_or_the_xml_of_the_declaring_entity() {
        let (_dir, root, solution) = fixture();
        // No sidecar: the script inside the XML.
        let from_xml = one(&solution, "P.Thing.OwnService", true).unwrap();
        assert_eq!(
            from_xml.entities[0].services[0].body.as_deref(),
            Some("return 1;")
        );
        // A sidecar wins.
        std::fs::create_dir_all(root.join("src/P.Thing/services/OwnService")).unwrap();
        std::fs::write(
            root.join("src/P.Thing/services/OwnService/script.js"),
            "return 2;",
        )
        .unwrap();
        let from_sidecar = one(&solution, "P.Thing.OwnService", true).unwrap();
        assert_eq!(
            from_sidecar.entities[0].services[0].body.as_deref(),
            Some("return 2;")
        );
        // An inherited service with no script anywhere has no body.
        let inherited = one(&solution, "P.Thing.TemplateService", true).unwrap();
        assert_eq!(inherited.entities[0].services[0].body, None);
        assert!(build(
            &solution,
            Query {
                body: true,
                ..Query::default()
            }
        )
        .is_err());
    }
}
