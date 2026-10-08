//! Offline service catalog derived from the entity model used by TypeScript generation.

use super::config::Solution;
use super::index::inherit::{implements_shape, inheritance_chain, Inherits};
use super::index::Index;
use super::types::{self, Entity, Member, Service};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub struct Query<'a> {
    pub project: Option<&'a str>,
    pub entity: Option<&'a str>,
    pub text: Option<&'a str>,
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
}

#[derive(Clone, Debug, Serialize)]
pub struct CatalogParameter {
    pub name: String,
    #[serde(rename = "baseType")]
    pub base_type: String,
    #[serde(rename = "dataShape", skip_serializing_if = "Option::is_none")]
    pub data_shape: Option<String>,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
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
pub fn inheritance(solution: &Solution) -> (Vec<CatalogEntity>, Vec<String>) {
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
    let mut pool: Vec<&Entity> = model
        .entities
        .iter()
        .filter(|entity| {
            query
                .project
                .is_none_or(|project| entity.project == project)
        })
        .collect();
    if let Some(name) = query.entity {
        pool = vec![resolve(&pool, name)?];
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
        if services.is_empty() && (query.entity.is_none() || query.text.is_some()) {
            continue;
        }
        entities.push(CatalogEntity {
            collection: entity.collection.clone(),
            name: entity.name.clone(),
            project: entity.project.clone(),
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
            catalog_service(service, from, scripted)
        })
        .collect()
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

fn catalog_service(service: &Service, from: &str, has_script: bool) -> CatalogService {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> (PathBuf, Solution) {
        let nonce = crate::test_nonce();
        let root =
            std::env::temp_dir().join(format!("twaco-catalog-{}-{nonce}", std::process::id()));
        for collection in ["ThingShapes", "ThingTemplates", "Things"] {
            std::fs::create_dir_all(root.join(collection)).unwrap();
        }
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(root.join("ThingShapes/P.Shape.xml"), entity("ThingShapes", "ThingShape", "P.Shape", "", "<ServiceDefinitions><ServiceDefinition name=\"ShapeService\" description=\"from the shape\"><ParameterDefinitions><FieldDefinition name=\"rows\" baseType=\"INFOTABLE\" aspect.dataShape=\"P.Row\" aspect.isRequired=\"true\" aspect.defaultValue=\"[]\"/></ParameterDefinitions><ResultType baseType=\"BOOLEAN\"/></ServiceDefinition></ServiceDefinitions>" )).unwrap();
        std::fs::write(root.join("ThingTemplates/P.Template.xml"), entity("ThingTemplates", "ThingTemplate", "P.Template", " baseThingTemplate=\"GenericThing\"", "<ImplementedShapes><ImplementedShape name=\"P.Shape\"/></ImplementedShapes><ThingShape><ServiceDefinitions><ServiceDefinition name=\"TemplateService\"><ResultType baseType=\"STRING\"/></ServiceDefinition></ServiceDefinitions></ThingShape>" )).unwrap();
        std::fs::write(root.join("Things/P.Thing.xml"), entity("Things", "Thing", "P.Thing", " thingTemplate=\"P.Template\"", "<ThingShape><ServiceDefinitions><ServiceDefinition name=\"OwnService\" description=\"dashboard work\"><ResultType baseType=\"NUMBER\"/></ServiceDefinition></ServiceDefinitions><ServiceImplementations><ServiceImplementation name=\"OwnService\"><HandlerDefinition name=\"Script\"/><ConfigurationTables><ConfigurationTable><DataShape><FieldDefinitions/></DataShape><Rows><Row><code>return 1;</code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape>" )).unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root, solution)
    }

    fn entity(collection: &str, tag: &str, name: &str, attributes: &str, body: &str) -> String {
        format!("<Entities><{collection}><{tag} name=\"{name}\" projectName=\"P\"{attributes}>{body}</{tag}></{collection}></Entities>")
    }

    #[test]
    fn inheritance_implementers_filters_and_unknown_names() {
        let (root, solution) = fixture();
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
        .contains("no entity named Missing"));
        let _ = std::fs::remove_dir_all(root);
    }
}
