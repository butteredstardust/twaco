use super::super::config::Solution;
use super::super::datashape;
use super::super::scan::{self, ScanError, Token};
use super::super::sidecar;
use super::super::workspace;
use super::write::ENTITY_COLLECTIONS;
use std::collections::BTreeSet;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypedValue {
    pub(crate) base_type: String,
    pub(crate) data_shape: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Parameter {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) value: TypedValue,
    pub(crate) required: bool,
    pub(crate) default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Service {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) parameters: Vec<Parameter>,
    pub(crate) result: TypedValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Property {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) value: TypedValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Member {
    Service(Service),
    Property(Property),
}

impl Member {
    pub(crate) fn name(&self) -> &str {
        match self {
            Member::Service(value) => &value.name,
            Member::Property(value) => &value.name,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Entity {
    pub(crate) name: String,
    pub(crate) collection: String,
    pub(crate) project: String,
    pub(crate) template: Option<String>,
    pub(crate) shapes: Vec<String>,
    pub(crate) members: Vec<Member>,
    pub(crate) script_services: BTreeSet<String>,
}

impl Entity {
    /// `("service" | "property", name)` for each member this entity declares itself.
    pub(crate) fn member_list(&self) -> Vec<(&'static str, &str)> {
        self.members
            .iter()
            .map(|member| match member {
                Member::Service(service) => ("service", service.name.as_str()),
                Member::Property(property) => ("property", property.name.as_str()),
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DataShape {
    pub(crate) name: String,
    pub(crate) fields: Vec<datashape::Field>,
}

#[derive(Debug, Default)]
pub(crate) struct Model {
    pub(crate) entities: Vec<Entity>,
    pub(crate) data_shapes: Vec<DataShape>,
}

pub(crate) fn load_model(solution: &Solution) -> (Model, Vec<String>) {
    let discovered = workspace::discover(solution);
    let mut model = Model::default();
    let mut skipped = discovered.unreadable;
    for file in discovered.entities {
        if file.info.collection != "DataShapes"
            && !ENTITY_COLLECTIONS.contains(&file.info.collection.as_str())
        {
            continue;
        }
        let bytes = match std::fs::read(&file.path) {
            Ok(bytes) => bytes,
            Err(error) => {
                skipped.push(format!("{}: {error}", file.path.display()));
                continue;
            }
        };
        match parse_document(&bytes, &file.info.collection, &file.info.name) {
            Ok(Parsed::Entity(mut entity)) => {
                entity.project = file.found_under;
                model.entities.push(entity);
            }
            Ok(Parsed::DataShape(shape)) => model.data_shapes.push(shape),
            Err(error) => skipped.push(format!("{}: {error}", file.path.display())),
        }
    }
    model
        .entities
        .sort_by(|a, b| (&a.name, &a.collection).cmp(&(&b.name, &b.collection)));
    model.data_shapes.sort_by(|a, b| a.name.cmp(&b.name));
    (model, skipped)
}

pub(super) fn script_declares(script: &str, name: &str) -> bool {
    script.lines().any(|line| {
        ["let", "const", "var", "function"].iter().any(|keyword| {
            let Some(after_keyword) = line.strip_prefix(keyword) else {
                return false;
            };
            let Some(first) = after_keyword.chars().next() else {
                return false;
            };
            if !first.is_whitespace() {
                return false;
            }
            let after_name = after_keyword.trim_start().strip_prefix(name);
            after_name.is_some_and(|rest| {
                let is_word = |character: char| character.is_alphanumeric() || character == '_';
                name.chars().last().is_some_and(is_word) != rest.chars().next().is_some_and(is_word)
            })
        })
    })
}

pub(super) enum Parsed {
    Entity(Entity),
    DataShape(DataShape),
}

pub(super) fn parse_document(
    src: &[u8],
    collection: &str,
    name: &str,
) -> Result<Parsed, ParseError> {
    let tokens = scan::tokenize(src).map_err(ParseError::from)?;
    let entity_at = sidecar::entity_element(&tokens, src)
        .ok_or_else(|| ParseError("not a ThingWorx entity export".to_string()))?;
    let name = attribute(&tokens[entity_at], src, "name")?
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| name.to_string());
    if collection == "DataShapes" {
        return datashape::extract(src)
            .map(|fields| Parsed::DataShape(DataShape { name, fields }))
            .map_err(|error| ParseError(error.to_string()));
    }
    let host = sidecar::member_host_of(&tokens, src)
        .ok_or_else(|| ParseError("entity has no member host".to_string()))?;
    let template_attribute = if collection == "Things" {
        "thingTemplate"
    } else {
        "baseThingTemplate"
    };
    let template =
        attribute(&tokens[entity_at], src, template_attribute)?.filter(|value| !value.is_empty());
    let mut shapes = Vec::new();
    // Things and templates export this beside their nested local ThingShape. Shape entities
    // keep their member sections directly, and accepting both locations also handles older
    // source-control layouts without descending into unrelated metadata.
    let parents = if entity_at == host {
        vec![entity_at]
    } else {
        vec![entity_at, host]
    };
    for parent in parents {
        if let Some(&section) = scan::child_tags(&tokens, src, "ImplementedShapes", parent).first()
        {
            for at in scan::child_tags(&tokens, src, "ImplementedShape", section) {
                if let Some(shape) =
                    attribute(&tokens[at], src, "name")?.filter(|value| !value.is_empty())
                {
                    shapes.push(shape);
                }
            }
        }
    }

    let mut members = Vec::new();
    let services = sidecar::named_children_of(
        &tokens,
        src,
        host,
        "ServiceDefinitions",
        "ServiceDefinition",
    )
    .map_err(|error| ParseError(error.to_string()))?;
    for (service_name, at) in services {
        members.push(Member::Service(parse_service(
            &tokens,
            src,
            at,
            service_name,
        )?));
    }
    let properties = sidecar::named_children_of(
        &tokens,
        src,
        host,
        "PropertyDefinitions",
        "PropertyDefinition",
    )
    .map_err(|error| ParseError(error.to_string()))?;
    for (property_name, at) in properties {
        members.push(Member::Property(Property {
            name: property_name,
            description: attribute(&tokens[at], src, "description")?.unwrap_or_default(),
            value: typed_value(&tokens[at], src)?,
        }));
    }
    members.sort_by(|a, b| a.name().cmp(b.name()));
    let mut script_services = BTreeSet::new();
    if let Some(&section) = scan::child_tags(&tokens, src, "ServiceImplementations", host).first() {
        for implementation in scan::child_tags(&tokens, src, "ServiceImplementation", section) {
            if sidecar::handler_of(&tokens, src, implementation)
                .map_err(|error| ParseError(error.to_string()))?
                == "Script"
                && sidecar::code_element_of(&tokens, src, implementation).is_some()
            {
                if let Some(name) =
                    attribute(&tokens[implementation], src, "name")?.filter(|name| !name.is_empty())
                {
                    script_services.insert(name);
                }
            }
        }
    }
    Ok(Parsed::Entity(Entity {
        name,
        collection: collection.to_string(),
        project: String::new(),
        template,
        shapes,
        members,
        script_services,
    }))
}

fn parse_service(
    tokens: &[Token],
    src: &[u8],
    at: usize,
    name: String,
) -> Result<Service, ParseError> {
    let mut parameters = Vec::new();
    if let Some(&section) = scan::child_tags(tokens, src, "ParameterDefinitions", at).first() {
        for parameter_at in scan::child_tags(tokens, src, "FieldDefinition", section) {
            parameters.push(Parameter {
                name: attribute(&tokens[parameter_at], src, "name")?.unwrap_or_default(),
                description: attribute(&tokens[parameter_at], src, "description")?
                    .unwrap_or_default(),
                value: typed_value(&tokens[parameter_at], src)?,
                required: attribute(&tokens[parameter_at], src, "aspect.isRequired")?.as_deref()
                    == Some("true"),
                default: attribute(&tokens[parameter_at], src, "aspect.defaultValue")?,
            });
        }
    }
    parameters.sort_by(|a, b| a.name.cmp(&b.name));
    let result = scan::child_tags(tokens, src, "ResultType", at)
        .first()
        .map(|&result_at| typed_value(&tokens[result_at], src))
        .transpose()?
        .unwrap_or(TypedValue {
            base_type: "NOTHING".to_string(),
            data_shape: None,
        });
    Ok(Service {
        name,
        description: attribute(&tokens[at], src, "description")?.unwrap_or_default(),
        parameters,
        result,
    })
}

fn typed_value(tag: &Token, src: &[u8]) -> Result<TypedValue, ParseError> {
    Ok(TypedValue {
        base_type: attribute(tag, src, "baseType")?.unwrap_or_default(),
        data_shape: attribute(tag, src, "aspect.dataShape")?.filter(|value| !value.is_empty()),
    })
}

fn attribute(tag: &Token, src: &[u8], name: &str) -> Result<Option<String>, ParseError> {
    scan::attribute(src, tag, name)
        .map(|value| {
            value.map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(src))))
        })
        .map_err(ParseError::from)
}

#[derive(Debug)]
pub(super) struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<ScanError> for ParseError {
    fn from(value: ScanError) -> Self {
        Self(value.to_string())
    }
}
