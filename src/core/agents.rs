//! AI agent and MCP namespace references read from entity XML.

use super::scan::{self, Kind, Token};
use super::sidecar;

#[derive(Debug, Default)]
pub(crate) struct Namespace {
    pub(crate) tools: Vec<ServiceReference>,
    pub(crate) resources: Vec<Resource>,
    pub(crate) prompts: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ServiceReference {
    pub(crate) name: String,
    pub(crate) service: String,
    pub(crate) provider: String,
    pub(crate) provider_type: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Resource {
    pub(crate) name: String,
    pub(crate) service: Option<ServiceReference>,
    pub(crate) file: Option<FileReference>,
}

#[derive(Debug, Clone)]
pub(crate) struct FileReference {
    pub(crate) path: String,
    pub(crate) repository: String,
}

#[derive(Debug, Default)]
pub(crate) struct Agent {
    pub(crate) namespaces: Vec<NamespaceReference>,
    pub(crate) taxonomy: Vec<(String, String)>,
    pub(crate) resolvers: Vec<String>,
    pub(crate) skills: Vec<SkillReference>,
}

#[derive(Debug, Default)]
pub(crate) struct NamespaceReference {
    pub(crate) name: String,
    pub(crate) tools: Vec<String>,
    pub(crate) resources: Vec<String>,
    pub(crate) prompts: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct SkillReference {
    pub(crate) repository: String,
    pub(crate) path: String,
}

pub(crate) fn namespace(src: &[u8]) -> Result<Namespace, String> {
    let tokens = scan::tokenize(src).map_err(|error| error.to_string())?;
    sidecar::entity_element(&tokens, src).ok_or("not a ThingWorx entity export".to_string())?;
    let mut out = Namespace::default();
    for at in tags(&tokens, src, "MCPTool") {
        if let Some(reference) = service_reference(&tokens, src, at) {
            out.tools.push(reference);
        }
    }
    for at in tags(&tokens, src, "MCPResource") {
        let name = attribute(&tokens[at], src, "name").unwrap_or_default();
        let service = if attribute(&tokens[at], src, "type").as_deref() == Some("Service") {
            children(&tokens, src, "ServiceDefinition", at)
                .into_iter()
                .find_map(|child| {
                    service_reference(&tokens, src, child).map(|mut reference| {
                        reference.name = name.clone();
                        reference
                    })
                })
        } else {
            None
        };
        let file = if attribute(&tokens[at], src, "type").as_deref() == Some("File") {
            children(&tokens, src, "FileDefinition", at)
                .into_iter()
                .find_map(|child| {
                    let path = attribute(&tokens[child], src, "filePath")?;
                    let repository = attribute(&tokens[child], src, "fileRepository")?;
                    Some(FileReference { path, repository })
                })
        } else {
            None
        };
        out.resources.push(Resource {
            name,
            service,
            file,
        });
    }
    out.prompts = tags(&tokens, src, "MCPPrompt")
        .into_iter()
        .filter_map(|at| attribute(&tokens[at], src, "name"))
        .collect();
    Ok(out)
}

pub(crate) fn agent(src: &[u8]) -> Result<Agent, String> {
    let tokens = scan::tokenize(src).map_err(|error| error.to_string())?;
    sidecar::entity_element(&tokens, src).ok_or("not a ThingWorx entity export".to_string())?;
    let mut out = Agent::default();
    for at in tags(&tokens, src, "MCPNamespaceReference") {
        let mut reference = NamespaceReference {
            name: attribute(&tokens[at], src, "name").unwrap_or_default(),
            ..NamespaceReference::default()
        };
        reference.tools = named_children(&tokens, src, "Tool", at);
        reference.resources = named_children(&tokens, src, "Resource", at);
        reference.prompts = named_children(&tokens, src, "Prompt", at);
        out.namespaces.push(reference);
    }
    for row in tags(&tokens, src, "Row") {
        let kind = child_text(&tokens, src, "entityType", row);
        let name = child_text(&tokens, src, "entityTypeName", row);
        if let (Some(kind), Some(name)) = (kind, name) {
            if !kind.is_empty() && !name.is_empty() {
                out.taxonomy.push((kind, name));
            }
        }
        if let Some(name) =
            child_text(&tokens, src, "resolverThingName", row).filter(|name| !name.is_empty())
        {
            out.resolvers.push(name);
        }
    }
    for at in tags(&tokens, src, "SkillReference") {
        if let (Some(repository), Some(path)) = (
            attribute(&tokens[at], src, "repositoryName"),
            attribute(&tokens[at], src, "skillPath"),
        ) {
            out.skills.push(SkillReference { repository, path });
        }
    }
    Ok(out)
}

/// The collection a `serviceProviderType` or taxonomy `entityType` names, when it is one the
/// repository can hold.
pub(crate) fn collection(kind: &str) -> Option<&'static str> {
    match kind {
        "Thing" => Some("Things"),
        "ThingTemplate" => Some("ThingTemplates"),
        "ThingShape" => Some("ThingShapes"),
        _ => None,
    }
}

fn service_reference(tokens: &[Token], src: &[u8], at: usize) -> Option<ServiceReference> {
    Some(ServiceReference {
        name: attribute(&tokens[at], src, "name").unwrap_or_default(),
        service: attribute(&tokens[at], src, "serviceName")?,
        provider: attribute(&tokens[at], src, "serviceProvider")?,
        provider_type: attribute(&tokens[at], src, "serviceProviderType")?,
    })
}

fn attribute(token: &Token, src: &[u8], name: &str) -> Option<String> {
    scan::attribute(src, token, name)
        .ok()
        .flatten()
        .map(|value| scan::decode_entities(&String::from_utf8_lossy(value.of(src))))
}

fn tags(tokens: &[Token], src: &[u8], name: &str) -> Vec<usize> {
    tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| {
            matches!(token.kind, Kind::Start | Kind::Empty) && token.name.of(src) == name.as_bytes()
        })
        .map(|(at, _)| at)
        .collect()
}

fn children(tokens: &[Token], src: &[u8], name: &str, parent: usize) -> Vec<usize> {
    scan::tags_within(tokens, src, name, parent)
}

fn named_children(tokens: &[Token], src: &[u8], name: &str, parent: usize) -> Vec<String> {
    children(tokens, src, name, parent)
        .into_iter()
        .filter_map(|at| attribute(&tokens[at], src, "name"))
        .collect()
}

fn child_text(tokens: &[Token], src: &[u8], name: &str, parent: usize) -> Option<String> {
    let at = children(tokens, src, name, parent).into_iter().next()?;
    let end = scan::element_end_in(tokens, src, at)?;
    let mut text = String::new();
    for token in &tokens[at + 1..end] {
        match token.kind {
            Kind::Cdata => text.push_str(&String::from_utf8_lossy(token.inner.of(src))),
            Kind::Text => text.push_str(&scan::decode_entities(&String::from_utf8_lossy(
                token.span.of(src),
            ))),
            _ => {}
        }
    }
    Some(text.trim().to_string())
}
