//! `new mcp-namespace` and `new ai-agent`: the ThingWorx 10.2 AI entities as repository files.
//!
//! The layout is the platform's 10.2 export of each, as a source-controlled solution keeps it.
//! What they name is checked against the repository the way the `ai references` gate checks it,
//! so a new namespace or agent starts out passing that gate.

use super::{cdata, common_tail, escape, footer, header, NewError, NewRequest};
use crate::core::agents_check::{declares_service, entity_map, implements_shape};
use crate::core::config::Solution;
use crate::core::server::encode_path_segment;
use crate::core::types::{self, Member, Model, Platform};
use std::collections::{BTreeMap, BTreeSet};

/// The tools a 10.2 agent draws from the platform's `AgentUtilities` namespace by default.
const AGENT_UTILITIES: &str = "AgentUtilities";
const AGENT_UTILITIES_TOOLS: &[&str] = &[
    "resolve_type",
    "resolve_entity",
    "list_entity_type",
    "invoke_service",
    "describe_entity_schema",
    "get_property_values",
    "query_property_history",
    "query_stream_data",
    "query_alert_summary_for_thing",
    "query_alert_history",
    "spotlight_search",
    "list_entities_by_type",
    "set_property_value",
    "acknowledge_alerts",
    "query_entities",
    "query_entities_by_taxonomy",
];

/// What the repository and the platform cache know, read once per request.
struct Known {
    model: Model,
    platform: Option<Platform>,
}

impl Known {
    fn read(solution: &Solution) -> Self {
        Known {
            model: types::load_model(solution).0,
            platform: types::cached_platform(solution),
        }
    }

    /// The collections of the repository's Things, templates and shapes named `name`.
    fn in_repository(&self, name: &str) -> Vec<&'static str> {
        ["Things", "ThingTemplates", "ThingShapes"]
            .into_iter()
            .filter(|collection| {
                self.model
                    .entities
                    .iter()
                    .any(|entity| entity.collection == *collection && entity.name == name)
            })
            .collect()
    }

    fn in_platform(&self, collection: &str, name: &str) -> bool {
        self.platform
            .as_ref()
            .is_some_and(|platform| match collection {
                "ThingTemplates" => platform.templates.contains_key(name),
                "ThingShapes" => platform.shapes.contains_key(name),
                _ => false,
            })
    }
}

fn collection_of(word: &str) -> Option<&'static str> {
    crate::core::agents::collection(word)
}

fn word_of(collection: &str) -> &'static str {
    match collection {
        "ThingTemplates" => "ThingTemplate",
        "ThingShapes" => "ThingShape",
        _ => "Thing",
    }
}

/// `name=Provider.Service`, the provider optionally prefixed `Thing:`, `ThingTemplate:` or
/// `ThingShape:`.
struct ServiceSpec {
    name: String,
    provider: String,
    provider_type: &'static str,
    service: String,
    description: String,
}

fn service_spec(known: &Known, flag: &str, spec: &str) -> Result<ServiceSpec, NewError> {
    let shape = || NewError::Invalid(format!("{flag} {spec:?} must be name=Provider.Service"));
    let (name, target) = spec.split_once('=').ok_or_else(shape)?;
    let (given_type, target) = match target.split_once(':') {
        Some((word, rest)) if collection_of(word).is_some() => (collection_of(word), rest),
        _ => (None, target),
    };
    let (provider, service) = target.rsplit_once('.').ok_or_else(shape)?;
    if name.is_empty() || provider.is_empty() || service.is_empty() {
        return Err(shape());
    }
    let found = known.in_repository(provider);
    let collection = match (given_type, found.as_slice()) {
        (Some(wanted), found) if found.is_empty() || found.contains(&wanted) => wanted,
        (Some(wanted), found) => {
            return Err(NewError::Invalid(format!(
                "{provider} is a {} in the repository, not a {}",
                word_of(found[0]),
                word_of(wanted)
            )))
        }
        (None, [one]) => one,
        (None, []) => ["ThingTemplates", "ThingShapes"]
            .into_iter()
            .find(|collection| known.in_platform(collection, provider))
            .ok_or_else(|| {
                NewError::Unknown(format!(
                    "no Thing, ThingTemplate or ThingShape {provider} in the solution or platform cache; \
                     write Thing:{provider}.{service} to name one outside it"
                ))
            })?,
        (None, _) => {
            return Err(NewError::Invalid(format!(
                "{provider} is more than one kind of entity; prefix it Thing:, ThingTemplate: or ThingShape:"
            )))
        }
    };
    let entities = entity_map(&known.model);
    let mut description = String::new();
    if let Some(entity) = entities.get(&(collection, provider)).copied() {
        if !declares_service(entity, service, &entities, &mut BTreeSet::new()) {
            return Err(NewError::Unknown(format!(
                "{} {provider} has no service {service}, nor does anything it inherits",
                word_of(collection)
            )));
        }
        description = own_description(entity, service, &entities).unwrap_or_default();
    }
    Ok(ServiceSpec {
        name: name.to_string(),
        provider: provider.to_string(),
        provider_type: word_of(collection),
        service: service.to_string(),
        description,
    })
}

/// The description of `service` where the entity or what it inherits declares it.
fn own_description(
    entity: &types::Entity,
    service: &str,
    entities: &BTreeMap<(&str, &str), &types::Entity>,
) -> Option<String> {
    let mut queue = vec![entity];
    let mut seen = BTreeSet::new();
    while let Some(entity) = queue.pop() {
        if !seen.insert((entity.collection.as_str(), entity.name.as_str())) {
            continue;
        }
        for member in &entity.members {
            if let Member::Service(found) = member {
                if found.name == service {
                    return Some(found.description.clone());
                }
            }
        }
        if let Some(parent) = entity
            .template
            .as_deref()
            .and_then(|name| entities.get(&("ThingTemplates", name)))
        {
            queue.push(parent);
        }
        for shape in &entity.shapes {
            if let Some(found) = entities.get(&("ThingShapes", shape.as_str())) {
                queue.push(found);
            }
        }
    }
    None
}

/// `name=Repository:/path`.
struct FileSpec {
    name: String,
    repository: String,
    path: String,
}

fn file_spec(spec: &str) -> Result<FileSpec, NewError> {
    let shape = || {
        NewError::Invalid(format!(
            "--file-resource {spec:?} must be name=Repository:/path"
        ))
    };
    let (name, target) = spec.split_once('=').ok_or_else(shape)?;
    let (repository, path) = target.split_once(':').ok_or_else(shape)?;
    let path = path.trim_start_matches('/');
    if name.is_empty() || repository.is_empty() || path.is_empty() {
        return Err(shape());
    }
    Ok(FileSpec {
        name: name.to_string(),
        repository: repository.to_string(),
        path: format!("/{path}"),
    })
}

fn mime_type(path: &str) -> &'static str {
    match path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
    {
        Some(extension) if extension == "md" => "text/markdown",
        Some(extension) if extension == "txt" => "text/plain",
        Some(extension) if extension == "json" => "application/json",
        Some(extension) if extension == "csv" => "text/csv",
        Some(extension) if extension == "html" || extension == "htm" => "text/html",
        Some(extension) if extension == "pdf" => "application/pdf",
        _ => "",
    }
}

/// The `{{argument}}` placeholders of a prompt, in order of first use.
fn placeholders(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find("}}") else {
            break;
        };
        let name = rest[..end].trim();
        if !name.is_empty() && !out.iter().any(|known| known == name) {
            out.push(name.to_string());
        }
        rest = &rest[end + 2..];
    }
    out
}

fn uri_segment(text: &str) -> String {
    encode_path_segment(text)
}

fn unique<'s>(kind: &str, names: impl IntoIterator<Item = &'s str>) -> Result<(), NewError> {
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(NewError::Invalid(format!(
                "{kind} {name} is listed more than once"
            )));
        }
    }
    Ok(())
}

/// The namespace file `new mcp-namespace` writes.
pub(super) fn namespace_xml(
    solution: &Solution,
    request: &NewRequest,
    project: &str,
) -> Result<String, NewError> {
    let known = Known::read(solution);
    let tools = request
        .tools
        .iter()
        .map(|spec| service_spec(&known, "--tool", spec))
        .collect::<Result<Vec<_>, _>>()?;
    let services = request
        .service_resources
        .iter()
        .map(|spec| service_spec(&known, "--service-resource", spec))
        .collect::<Result<Vec<_>, _>>()?;
    let files = request
        .file_resources
        .iter()
        .map(|spec| file_spec(spec))
        .collect::<Result<Vec<_>, _>>()?;
    let prompts = request
        .prompts
        .iter()
        .map(|spec| {
            spec.split_once('=')
                .filter(|(name, text)| !name.is_empty() && !text.is_empty())
                .ok_or_else(|| NewError::Invalid(format!("--prompt {spec:?} must be name=text")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut described = BTreeMap::new();
    for spec in &request.describe {
        let (name, text) = spec
            .split_once('=')
            .filter(|(name, text)| !name.is_empty() && !text.trim().is_empty())
            .ok_or_else(|| NewError::Invalid(format!("--describe {spec:?} must be name=text")))?;
        if described.insert(name, text.trim()).is_some() {
            return Err(NewError::Invalid(format!(
                "--describe {name} is given more than once"
            )));
        }
    }
    let defined: BTreeSet<&str> = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .chain(services.iter().map(|resource| resource.name.as_str()))
        .chain(files.iter().map(|file| file.name.as_str()))
        .chain(prompts.iter().map(|(name, _)| *name))
        .collect();
    if let Some(name) = described.keys().find(|name| !defined.contains(*name)) {
        return Err(NewError::Invalid(format!(
            "--describe {name} names no tool, resource or prompt of this namespace"
        )));
    }
    // The server refuses a tool, resource, prompt or prompt argument without a description.
    let description = |name: &str, default: &str| -> Result<String, NewError> {
        let text = described.get(name).copied().unwrap_or(default).trim();
        if text.is_empty() {
            return Err(NewError::Invalid(format!(
                "{name} has no description, which ThingWorx requires; give --describe {name}=text"
            )));
        }
        Ok(text.to_string())
    };
    if tools.is_empty() && services.is_empty() && files.is_empty() && prompts.is_empty() {
        return Err(NewError::Invalid(
            "mcp-namespace needs at least one --tool, --service-resource, --file-resource or --prompt"
                .into(),
        ));
    }
    unique("tool", tools.iter().map(|tool| tool.name.as_str()))?;
    unique(
        "resource",
        services
            .iter()
            .map(|resource| resource.name.as_str())
            .chain(files.iter().map(|file| file.name.as_str())),
    )?;
    unique("prompt", prompts.iter().map(|(name, _)| *name))?;
    for file in &files {
        if let Some(root) = solution.repositories_root() {
            let folder = root.join(&file.repository);
            if folder.is_dir() && !folder.join(file.path.trim_start_matches('/')).is_file() {
                return Err(NewError::Unknown(format!(
                    "{} has no {} in its [repositories] mirror; the import refuses a File resource whose file is missing",
                    file.repository, file.path
                )));
            }
        }
    }

    let namespace = uri_segment(&request.name);
    let mut body = common_tail(&[])?;
    if !tools.is_empty() {
        body.push_str("\n            <MCPTools>");
        for tool in &tools {
            body.push_str(&format!(
                "\n                <MCPTool\n                 _meta=\"{}\"\n                 description=\"{}\"\n                 name=\"{}\"\n                 serviceName=\"{}\"\n                 serviceProvider=\"{}\"\n                 serviceProviderType=\"{}\"\n                 title=\"\"></MCPTool>",
                escape(&serde_json::json!({ "serviceName": tool.service }).to_string())?,
                escape(&description(&tool.name, &tool.description)?)?,
                escape(&tool.name)?,
                escape(&tool.service)?,
                escape(&tool.provider)?,
                tool.provider_type
            ));
        }
        body.push_str("\n            </MCPTools>");
    }
    if !services.is_empty() || !files.is_empty() {
        body.push_str("\n            <MCPResources>");
        for resource in &services {
            let uri = format!(
                "twx://MCPNamespaces/{namespace}/Resources/{}/{}/Services/{}",
                resource.provider_type,
                uri_segment(&resource.provider),
                uri_segment(&resource.service)
            );
            body.push_str(&format!(
                "\n                <MCPResource\n                 _meta=\"{{}}\"\n                 annotation.audience=\"\"\n                 description=\"{}\"\n                 mimeType=\"\"\n                 name=\"{}\"\n                 title=\"\"\n                 type=\"Service\"\n                 uri=\"{}\">\n                    <ServiceDefinition\n                     serviceName=\"{}\"\n                     serviceProvider=\"{}\"\n                     serviceProviderType=\"{}\"></ServiceDefinition>\n                </MCPResource>",
                escape(&description(&resource.name, &resource.description)?)?,
                escape(&resource.name)?,
                escape(&uri)?,
                escape(&resource.service)?,
                escape(&resource.provider)?,
                resource.provider_type
            ));
        }
        for file in &files {
            let path: Vec<String> = file
                .path
                .trim_start_matches('/')
                .split('/')
                .map(uri_segment)
                .collect();
            let uri = format!(
                "twx://MCPNamespaces/{namespace}/Resources/{}/Filepath/{}",
                uri_segment(&file.repository),
                path.join("/")
            );
            body.push_str(&format!(
                "\n                <MCPResource\n                 _meta=\"{{}}\"\n                 annotation.audience=\"\"\n                 description=\"{}\"\n                 mimeType=\"{}\"\n                 name=\"{}\"\n                 title=\"\"\n                 type=\"File\"\n                 uri=\"{}\">\n                    <FileDefinition\n                     filePath=\"{}\"\n                     fileRepository=\"{}\"></FileDefinition>\n                </MCPResource>",
                escape(&description(
                    &file.name,
                    file.path.rsplit('/').next().unwrap_or_default()
                )?)?,
                mime_type(&file.path),
                escape(&file.name)?,
                escape(&uri)?,
                escape(&file.path)?,
                escape(&file.repository)?
            ));
        }
        body.push_str("\n            </MCPResources>");
    }
    if !prompts.is_empty() {
        body.push_str("\n            <MCPPrompts>");
        for (name, text) in &prompts {
            let content = serde_json::json!({ "text": text }).to_string();
            let arguments: String = placeholders(text)
                .iter()
                .map(|argument| {
                    Ok(format!(
                        "\n                        <MCPPromptArgument\n                         description=\"{}\"\n                         name=\"{}\"\n                         required=\"true\"\n                         title=\"\"></MCPPromptArgument>",
                        escape(argument)?,
                        escape(argument)?
                    ))
                })
                .collect::<Result<_, NewError>>()?;
            let arguments = if arguments.is_empty() {
                "\n                    <MCPPromptArguments></MCPPromptArguments>".to_string()
            } else {
                format!("\n                    <MCPPromptArguments>{arguments}\n                    </MCPPromptArguments>")
            };
            body.push_str(&format!(
                "\n                <MCPPrompt\n                 _meta=\"{{}}\"\n                 content=\"{}\"\n                 description=\"{}\"\n                 name=\"{}\"\n                 role=\"user\"\n                 title=\"\">{arguments}\n                </MCPPrompt>",
                escape(&content)?,
                escape(&description(name, text)?)?,
                escape(name)?
            ));
        }
        body.push_str("\n            </MCPPrompts>");
    }
    Ok(format!(
        "{}        <MCPNamespace\n         description=\"{}\"\n         documentationContent=\"\"\n         enabled=\"true\"\n         homeMashup=\"\"\n         name=\"{}\"\n         projectName=\"{}\"\n         tags=\"\">\n{body}\n        </MCPNamespace>\n{}",
        header("MCPNamespaces"),
        escape(&request.description)?,
        escape(&request.name)?,
        escape(project)?,
        footer("MCPNamespaces")
    ))
}

/// What an agent is allowed from one namespace, each name sorted into its kind.
struct Allowed {
    namespace: String,
    tools: Vec<String>,
    resources: Vec<String>,
    prompts: Vec<String>,
}

fn allowed(solution: &Solution, spec: &str) -> Result<Allowed, NewError> {
    let (namespace, items) = match spec.split_once('=') {
        Some((namespace, items)) => (namespace, Some(items)),
        None => (spec, None),
    };
    if namespace.is_empty() {
        return Err(NewError::Invalid(format!(
            "--namespace {spec:?} must be Namespace or Namespace=item,item"
        )));
    }
    let names: Option<Vec<&str>> = items.map(|items| {
        items
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .collect()
    });
    let mut out = Allowed {
        namespace: namespace.to_string(),
        tools: Vec::new(),
        resources: Vec::new(),
        prompts: Vec::new(),
    };
    let file = crate::core::workspace::entities(solution)
        .into_iter()
        .find(|entry| entry.info.collection == "MCPNamespaces" && entry.info.name == namespace);
    let Some(file) = file else {
        if namespace != AGENT_UTILITIES {
            return Err(NewError::Unknown(format!(
                "no MCPNamespace {namespace} in the solution; create it first with new mcp-namespace"
            )));
        }
        out.tools = match names {
            Some(names) => {
                for name in &names {
                    if !AGENT_UTILITIES_TOOLS.contains(name) {
                        return Err(NewError::Unknown(format!(
                            "{AGENT_UTILITIES} has no tool {name}; it has {}",
                            AGENT_UTILITIES_TOOLS.join(", ")
                        )));
                    }
                }
                names.into_iter().map(str::to_string).collect()
            }
            None => AGENT_UTILITIES_TOOLS
                .iter()
                .map(|name| name.to_string())
                .collect(),
        };
        return Ok(out);
    };
    let bytes = std::fs::read(&file.path)
        .map_err(|error| NewError::Invalid(format!("{}: {error}", file.path.display())))?;
    let defined = crate::core::agents::namespace(&bytes)
        .map_err(|error| NewError::Invalid(format!("{}: {error}", file.path.display())))?;
    let tools: Vec<&str> = defined
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    let resources: Vec<&str> = defined
        .resources
        .iter()
        .map(|resource| resource.name.as_str())
        .collect();
    let prompts: Vec<&str> = defined.prompts.iter().map(String::as_str).collect();
    match names {
        None => {
            out.tools = tools.iter().map(|name| name.to_string()).collect();
            out.resources = resources.iter().map(|name| name.to_string()).collect();
            out.prompts = prompts.iter().map(|name| name.to_string()).collect();
        }
        Some(names) => {
            for name in names {
                if tools.contains(&name) {
                    out.tools.push(name.to_string());
                } else if resources.contains(&name) {
                    out.resources.push(name.to_string());
                } else if prompts.contains(&name) {
                    out.prompts.push(name.to_string());
                } else {
                    return Err(NewError::Unknown(format!(
                        "MCPNamespace {namespace} has no tool, resource or prompt {name}"
                    )));
                }
            }
        }
    }
    Ok(out)
}

/// `Repository:/path`.
fn skill_spec(spec: &str) -> Result<(String, String), NewError> {
    spec.split_once(':')
        .filter(|(repository, path)| !repository.is_empty() && !path.trim_matches('/').is_empty())
        .map(|(repository, path)| {
            (
                repository.to_string(),
                format!("/{}", path.trim_matches('/')),
            )
        })
        .ok_or_else(|| NewError::Invalid(format!("--skill {spec:?} must be Repository:/path")))
}

/// `TemplateOrShape` or `TemplateOrShape=alias, alias`.
fn taxonomy_spec(known: &Known, spec: &str) -> Result<(&'static str, String, String), NewError> {
    let (name, aliases) = spec.split_once('=').unwrap_or((spec, ""));
    if name.is_empty() {
        return Err(NewError::Invalid(format!(
            "--taxonomy {spec:?} must be TemplateOrShape or TemplateOrShape=alias, alias"
        )));
    }
    let collection = known
        .in_repository(name)
        .into_iter()
        .find(|collection| *collection != "Things")
        .or_else(|| {
            ["ThingTemplates", "ThingShapes"]
                .into_iter()
                .find(|collection| known.in_platform(collection, name))
        })
        .ok_or_else(|| {
            NewError::Unknown(format!(
                "no ThingTemplate or ThingShape {name} in the solution or platform cache"
            ))
        })?;
    Ok((
        word_of(collection),
        name.to_string(),
        aliases.trim().to_string(),
    ))
}

const TAXONOMY_TABLE: &str = "                <ConfigurationTable\n                 dataShapeName=\"\"\n                 description=\"Taxonomy configuration for this agent: one row per resolvable type (ThingTemplate / ThingShape).\"\n                 isMultiRow=\"true\"\n                 name=\"TaxonomyConfiguration\"\n                 ordinal=\"1\">\n                    <DataShape>\n                        <FieldDefinitions>\n                            <FieldDefinition\n                             aspect.friendlyName=\"Aliases (comma-separated)\"\n                             baseType=\"STRING\"\n                             description=\"Comma-separated user-facing aliases/synonyms for this type (e.g. &quot;stacker, stacking robots&quot;).\"\n                             name=\"alias\"\n                             ordinal=\"3\"></FieldDefinition>\n                            <FieldDefinition\n                             aspect.defaultValue=\"ThingTemplate\"\n                             aspect.friendlyName=\"Entity Type\"\n                             aspect.selectOptions=\"ThingTemplate:ThingTemplate|ThingShape:ThingShape\"\n                             baseType=\"STRING\"\n                             description=\"Whether this entry resolves to a ThingTemplate or a ThingShape.\"\n                             name=\"entityType\"\n                             ordinal=\"1\"></FieldDefinition>\n                            <FieldDefinition\n                             aspect.friendlyName=\"Entity Type Name\"\n                             baseType=\"STRING\"\n                             description=\"The ThingTemplate / ThingShape name this entry resolves to (e.g. PTC.ROBO.Stacker-eu).\"\n                             name=\"entityTypeName\"\n                             ordinal=\"2\"></FieldDefinition>\n                            <FieldDefinition\n                             aspect.dataShape=\"TaxonomyIdentityProperty\"\n                             aspect.friendlyName=\"Identity Properties\"\n                             baseType=\"INFOTABLE\"\n                             description=\"Ordered identity properties used to match a specific instance during entity resolution.\"\n                             name=\"identityProperties\"\n                             ordinal=\"4\"></FieldDefinition>\n                        </FieldDefinitions>\n                    </DataShape>\n";

const HIERARCHY_TABLE: &str = "                <ConfigurationTable\n                 dataShapeName=\"\"\n                 description=\"Hierarchy configuration for this agent: HierarchyResolver ThingShape implementing Thing which intends to contain.\"\n                 isMultiRow=\"false\"\n                 name=\"HierarchyConfiguration\"\n                 ordinal=\"1\">\n                    <DataShape>\n                        <FieldDefinitions>\n                            <FieldDefinition\n                             aspect.thingShape=\"HierarchyResolver\"\n                             baseType=\"THINGNAME\"\n                             description=\"Thing with HierarchyResolver ThingShape.\"\n                             name=\"resolverThingName\"\n                             ordinal=\"1\"></FieldDefinition>\n                        </FieldDefinitions>\n                    </DataShape>\n";

fn taxonomy_row(kind: &str, name: &str, aliases: &str) -> String {
    format!(
        "\n                        <Row>\n                            <alias>{}</alias>\n                            <entityType>{}</entityType>\n                            <entityTypeName>{}</entityTypeName>\n                            <identityProperties>\n                                <infoTable>\n                                    <DataShape>\n                                        <FieldDefinitions>\n                                            <FieldDefinition\n                                             baseType=\"STRING\"\n                                             description=\"Match mode applied to this property's value\"\n                                             name=\"match\"\n                                             ordinal=\"2\"></FieldDefinition>\n                                            <FieldDefinition\n                                             aspect.isPrimaryKey=\"true\"\n                                             baseType=\"STRING\"\n                                             description=\"Property name on the Thing (use &quot;name&quot; for the entity name)\"\n                                             name=\"name\"\n                                             ordinal=\"1\"></FieldDefinition>\n                                        </FieldDefinitions>\n                                    </DataShape>\n                                    <Rows>\n                                        <Row>\n                                            <match><![CDATA[normalizedContains]]></match>\n                                            <name><![CDATA[name]]></name>\n                                        </Row>\n                                    </Rows>\n                                </infoTable>\n                            </identityProperties>\n                        </Row>",
        cdata(aliases),
        cdata(kind),
        cdata(name)
    )
}

/// The agent file `new ai-agent` writes.
pub(super) fn agent_xml(
    solution: &Solution,
    request: &NewRequest,
    project: &str,
) -> Result<String, NewError> {
    let known = Known::read(solution);
    if request.namespaces.is_empty() {
        return Err(NewError::Invalid(
            "ai-agent needs at least one --namespace; AgentUtilities gives the platform's tools"
                .into(),
        ));
    }
    let namespaces = request
        .namespaces
        .iter()
        .map(|spec| allowed(solution, spec))
        .collect::<Result<Vec<_>, _>>()?;
    unique(
        "namespace",
        namespaces.iter().map(|allowed| allowed.namespace.as_str()),
    )?;
    let skills = request
        .skills
        .iter()
        .map(|spec| skill_spec(spec))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(root) = solution.repositories_root() {
        for (repository, path) in &skills {
            let folder = root.join(repository);
            if folder.is_dir()
                && !folder
                    .join(path.trim_start_matches('/'))
                    .join("SKILL.md")
                    .is_file()
            {
                return Err(NewError::Unknown(format!(
                    "{repository} has no {path}/SKILL.md in its [repositories] mirror"
                )));
            }
        }
    }
    let taxonomy = request
        .taxonomy
        .iter()
        .map(|spec| taxonomy_spec(&known, spec))
        .collect::<Result<Vec<_>, _>>()?;
    unique(
        "taxonomy type",
        taxonomy.iter().map(|(_, name, _)| name.as_str()),
    )?;
    let resolver = request.resolver.as_deref().filter(|name| !name.is_empty());
    if let Some(resolver) = resolver {
        let entities = entity_map(&known.model);
        let thing = entities
            .get(&("Things", resolver))
            .copied()
            .ok_or_else(|| NewError::Unknown(format!("no Thing {resolver} in the solution")))?;
        if !implements_shape(thing, "HierarchyResolver", &entities, &mut BTreeSet::new()) {
            return Err(NewError::Invalid(format!(
                "Thing {resolver} does not implement the HierarchyResolver ThingShape"
            )));
        }
    }
    let instructions = match (&request.instructions, &request.instructions_file) {
        (Some(_), Some(_)) => {
            return Err(NewError::Invalid(
                "give --instructions or --instructions-file, not both".into(),
            ))
        }
        (Some(text), None) => text.clone(),
        (None, Some(path)) => std::fs::read_to_string(path)
            .map_err(|error| NewError::Invalid(format!("{}: {error}", path.display())))?,
        (None, None) => String::new(),
    };
    let agent_description = request.agent_description.clone().unwrap_or_default();
    for value in [
        instructions.as_str(),
        agent_description.as_str(),
        &request.name,
    ]
    .into_iter()
    .chain(
        taxonomy
            .iter()
            .flat_map(|(_, name, aliases)| [name.as_str(), aliases.as_str()]),
    )
    .chain(resolver)
    {
        if value.contains("]]>") {
            return Err(NewError::Invalid(
                "CDATA-bound values cannot contain `]]>`".into(),
            ));
        }
    }
    let display_name = request.display_name.as_deref().unwrap_or(&request.name);
    let rows: String = taxonomy
        .iter()
        .map(|(kind, name, aliases)| taxonomy_row(kind, name, aliases))
        .collect();
    let taxonomy_rows = if rows.is_empty() {
        "                    <Rows></Rows>".to_string()
    } else {
        format!("                    <Rows>{rows}\n                    </Rows>")
    };
    let resolver_value = resolver.map(cdata).unwrap_or_default();
    let mut references = String::new();
    for namespace in &namespaces {
        references.push_str(&format!(
            "\n                <MCPNamespaceReference\n                 name=\"{}\">",
            escape(&namespace.namespace)?
        ));
        for (group, item, names) in [
            ("AllowedTools", "Tool", &namespace.tools),
            ("AllowedResources", "Resource", &namespace.resources),
            ("AllowedPrompts", "Prompt", &namespace.prompts),
        ] {
            if names.is_empty() {
                continue;
            }
            references.push_str(&format!("\n                    <{group}>"));
            for name in names {
                references.push_str(&format!(
                    "\n                        <{item}\n                         name=\"{}\"></{item}>",
                    escape(name)?
                ));
            }
            references.push_str(&format!("\n                    </{group}>"));
        }
        references.push_str("\n                </MCPNamespaceReference>");
    }
    let skill_references: String = skills
        .iter()
        .map(|(repository, path)| {
            Ok(format!(
                "\n                <SkillReference\n                 repositoryName=\"{}\"\n                 skillPath=\"{}\"></SkillReference>",
                escape(repository)?,
                escape(path)?
            ))
        })
        .collect::<Result<_, NewError>>()?;
    // The server leaves the element out when there are none.
    let skill_block = if skill_references.is_empty() {
        String::new()
    } else {
        format!("\n            <SkillReferences>{skill_references}\n            </SkillReferences>")
    };
    Ok(format!(
        "{}        <AIAgent\n         agentType=\"ReAct\"\n         description=\"{}\"\n         displayName=\"{}\"\n         documentationContent=\"\"\n         enabled=\"true\"\n         homeMashup=\"\"\n         name=\"{}\"\n         projectName=\"{}\"\n         tags=\"\">\n            <avatar></avatar>\n            <DesignTimePermissions>\n                <Create></Create>\n                <Read></Read>\n                <Update></Update>\n                <Delete></Delete>\n                <Metadata></Metadata>\n            </DesignTimePermissions>\n            <RunTimePermissions>\n                <Permissions\n                 resourceName=\"GetAIAgentDefinitionWithSkills\">\n                    <PropertyRead></PropertyRead>\n                    <EventInvoke></EventInvoke>\n                    <PropertyWrite></PropertyWrite>\n                    <EventSubscribe></EventSubscribe>\n                    <ServiceInvoke>\n                        <Principal\n                         isPermitted=\"true\"\n                         name=\"AIBootstrapUsers\"\n                         type=\"Group\"></Principal>\n                    </ServiceInvoke>\n                </Permissions>\n            </RunTimePermissions>\n            <VisibilityPermissions>\n                <Visibility>\n                    <Principal\n                     isPermitted=\"true\"\n                     name=\"AIBootstrapOrg\"\n                     type=\"Organization\"></Principal>\n                </Visibility>\n            </VisibilityPermissions>\n            <ConfigurationTableDefinitions></ConfigurationTableDefinitions>\n            <ConfigurationTables>\n{TAXONOMY_TABLE}{taxonomy_rows}\n                </ConfigurationTable>\n{HIERARCHY_TABLE}                    <Rows>\n                        <Row>\n                            <resolverThingName>{resolver_value}</resolverThingName>\n                        </Row>\n                    </Rows>\n                </ConfigurationTable>\n            </ConfigurationTables>\n            <SystemPrompt>\n                <AgentDescription>{}</AgentDescription>\n                <AgentInstructions>{}</AgentInstructions>\n            </SystemPrompt>{skill_block}\n            <MCPNamespaceReferences>{references}\n            </MCPNamespaceReferences>\n            <ExternalMCPServerConfigurations></ExternalMCPServerConfigurations>\n        </AIAgent>\n{}",
        header("AIAgents"),
        escape(&request.description)?,
        escape(display_name)?,
        escape(&request.name)?,
        escape(project)?,
        cdata(&agent_description),
        cdata(&instructions),
        footer("AIAgents")
    ))
}
