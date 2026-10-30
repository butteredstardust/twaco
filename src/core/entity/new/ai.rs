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
pub(crate) struct Known {
    model: Model,
    platform: Option<Platform>,
}

impl Known {
    pub(crate) fn read(solution: &Solution) -> Self {
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
pub(crate) struct ServiceSpec {
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
        if !declares_service(
            entity,
            service,
            &entities,
            known.platform.as_ref(),
            &mut BTreeSet::new(),
        ) {
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
pub(crate) struct FileSpec {
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
    if name.is_empty() || !repository_name(repository) {
        return Err(shape());
    }
    let path = repository_path(path).ok_or_else(shape)?;
    Ok(FileSpec {
        name: name.to_string(),
        repository: repository.to_string(),
        path,
    })
}

/// A FileRepository name that names one folder of the `[repositories]` mirror.
pub(crate) fn repository_name(name: &str) -> bool {
    plain_step(name) && !name.contains(['/', '\\', ':'])
}

/// One step of a path that means what it says on every platform: not empty, `.` or `..`, and
/// not ending in a dot or a space, which Windows drops (so `.. ` would be `..`).
fn plain_step(step: &str) -> bool {
    !step.is_empty() && step != "." && step != ".." && !step.ends_with(['.', ' '])
}

/// A path inside a file repository as `/a/b`, or none when it would leave it: `..`, `.`,
/// an empty step, a backslash or a drive.
pub(crate) fn repository_path(path: &str) -> Option<String> {
    let path = path.trim_start_matches('/').trim_end_matches('/');
    if path.is_empty() || path.contains(['\\', ':']) {
        return None;
    }
    let steps: Vec<&str> = path.split('/').collect();
    if !steps.iter().all(|step| plain_step(step)) {
        return None;
    }
    Some(format!("/{}", steps.join("/")))
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

pub(crate) fn unique<'s>(
    kind: &str,
    names: impl IntoIterator<Item = &'s str>,
) -> Result<(), NewError> {
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

/// One tool, resource or prompt of a namespace.
pub(crate) enum Item {
    Tool(ServiceSpec),
    ServiceResource(ServiceSpec),
    FileResource(FileSpec),
    Prompt { name: String, text: String },
}

impl Item {
    /// Read one `--tool`, `--service-resource`, `--file-resource` or `--prompt` value; `kind`
    /// is the flag's name without its dashes.
    pub(crate) fn parse(known: &Known, kind: &str, spec: &str) -> Result<Item, NewError> {
        Ok(match kind {
            "tool" => Item::Tool(service_spec(known, "--tool", spec)?),
            "service-resource" => {
                Item::ServiceResource(service_spec(known, "--service-resource", spec)?)
            }
            "file-resource" => Item::FileResource(file_spec(spec)?),
            "prompt" => {
                let (name, text) = spec
                    .split_once('=')
                    .filter(|(name, text)| !name.is_empty() && !text.is_empty())
                    .ok_or_else(|| {
                        NewError::Invalid(format!("--prompt {spec:?} must be name=text"))
                    })?;
                Item::Prompt {
                    name: name.to_string(),
                    text: text.to_string(),
                }
            }
            other => {
                return Err(NewError::Invalid(format!(
                    "{other} is not tool, service-resource, file-resource or prompt"
                )))
            }
        })
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            Item::Tool(spec) | Item::ServiceResource(spec) => &spec.name,
            Item::FileResource(file) => &file.name,
            Item::Prompt { name, .. } => name,
        }
    }

    /// The section that holds it, in the order a namespace writes them.
    pub(crate) fn section(&self) -> &'static str {
        match self {
            Item::Tool(_) => "MCPTools",
            Item::ServiceResource(_) | Item::FileResource(_) => "MCPResources",
            Item::Prompt { .. } => "MCPPrompts",
        }
    }

    fn default_description(&self) -> &str {
        match self {
            Item::Tool(spec) | Item::ServiceResource(spec) => &spec.description,
            Item::FileResource(file) => file.path.rsplit('/').next().unwrap_or_default(),
            Item::Prompt { text, .. } => text,
        }
    }

    /// The description it is written with: `given`, else its default. The server refuses an
    /// empty one.
    pub(crate) fn description(&self, given: Option<&str>) -> Result<String, NewError> {
        let text = given.unwrap_or(self.default_description()).trim();
        if text.is_empty() {
            return Err(NewError::Invalid(format!(
                "{} has no description, which ThingWorx requires; give --describe {}=text",
                self.name(),
                self.name()
            )));
        }
        Ok(text.to_string())
    }

    /// Refuse a File resource the `[repositories]` mirror says is missing: the import would.
    pub(crate) fn check_mirror(&self, solution: &Solution) -> Result<(), NewError> {
        let Item::FileResource(file) = self else {
            return Ok(());
        };
        if let Some(root) = solution.repositories_root() {
            let folder = root.join(&file.repository);
            if folder.is_dir() && !folder.join(file.path.trim_start_matches('/')).is_file() {
                return Err(NewError::Unknown(format!(
                    "{} has no {} in its [repositories] mirror; the import refuses a File resource whose file is missing",
                    file.repository, file.path
                )));
            }
        }
        Ok(())
    }

    /// Its element, as a line of its own at the 16-space depth a new namespace writes it.
    pub(crate) fn xml(&self, namespace: &str, description: &str) -> Result<String, NewError> {
        let namespace = uri_segment(namespace);
        let description = escape(description)?;
        Ok(match self {
            Item::Tool(tool) => format!(
                "\n                <MCPTool\n                 _meta=\"{}\"\n                 description=\"{description}\"\n                 name=\"{}\"\n                 serviceName=\"{}\"\n                 serviceProvider=\"{}\"\n                 serviceProviderType=\"{}\"\n                 title=\"\"></MCPTool>",
                escape(&serde_json::json!({ "serviceName": tool.service }).to_string())?,
                escape(&tool.name)?,
                escape(&tool.service)?,
                escape(&tool.provider)?,
                tool.provider_type
            ),
            Item::ServiceResource(resource) => {
                let uri = format!(
                    "twx://MCPNamespaces/{namespace}/Resources/{}/{}/Services/{}",
                    resource.provider_type,
                    uri_segment(&resource.provider),
                    uri_segment(&resource.service)
                );
                format!(
                    "\n                <MCPResource\n                 _meta=\"{{}}\"\n                 annotation.audience=\"\"\n                 description=\"{description}\"\n                 mimeType=\"\"\n                 name=\"{}\"\n                 title=\"\"\n                 type=\"Service\"\n                 uri=\"{}\">\n                    <ServiceDefinition\n                     serviceName=\"{}\"\n                     serviceProvider=\"{}\"\n                     serviceProviderType=\"{}\"></ServiceDefinition>\n                </MCPResource>",
                    escape(&resource.name)?,
                    escape(&uri)?,
                    escape(&resource.service)?,
                    escape(&resource.provider)?,
                    resource.provider_type
                )
            }
            Item::FileResource(file) => {
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
                format!(
                    "\n                <MCPResource\n                 _meta=\"{{}}\"\n                 annotation.audience=\"\"\n                 description=\"{description}\"\n                 mimeType=\"{}\"\n                 name=\"{}\"\n                 title=\"\"\n                 type=\"File\"\n                 uri=\"{}\">\n                    <FileDefinition\n                     filePath=\"{}\"\n                     fileRepository=\"{}\"></FileDefinition>\n                </MCPResource>",
                    mime_type(&file.path),
                    escape(&file.name)?,
                    escape(&uri)?,
                    escape(&file.path)?,
                    escape(&file.repository)?
                )
            }
            Item::Prompt { name, text } => {
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
                format!(
                    "\n                <MCPPrompt\n                 _meta=\"{{}}\"\n                 content=\"{}\"\n                 description=\"{description}\"\n                 name=\"{}\"\n                 role=\"user\"\n                 title=\"\">{arguments}\n                </MCPPrompt>",
                    escape(&content)?,
                    escape(name)?
                )
            }
        })
    }
}

/// `--describe name=text` values by name; each must name one of `items`.
pub(crate) fn descriptions<'s>(
    describe: &'s [String],
    items: &[Item],
) -> Result<BTreeMap<&'s str, &'s str>, NewError> {
    let mut described = BTreeMap::new();
    for spec in describe {
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
    if let Some(name) = described
        .keys()
        .find(|name| !items.iter().any(|item| item.name() == **name))
    {
        return Err(NewError::Invalid(format!(
            "--describe {name} names no tool, resource or prompt of this namespace"
        )));
    }
    Ok(described)
}

/// The namespace file `new mcp-namespace` writes.
pub(super) fn namespace_xml(
    solution: &Solution,
    request: &NewRequest,
    project: &str,
) -> Result<String, NewError> {
    let known = Known::read(solution);
    let mut items = Vec::new();
    for (kind, specs) in [
        ("tool", &request.tools),
        ("service-resource", &request.service_resources),
        ("file-resource", &request.file_resources),
        ("prompt", &request.prompts),
    ] {
        for spec in specs {
            items.push(Item::parse(&known, kind, spec)?);
        }
    }
    if items.is_empty() {
        return Err(NewError::Invalid(
            "mcp-namespace needs at least one --tool, --service-resource, --file-resource or --prompt"
                .into(),
        ));
    }
    // One name per item across kinds, so an agent's allowed list says which it means.
    unique("tool, resource or prompt", items.iter().map(Item::name))?;
    let described = descriptions(&request.describe, &items)?;
    for item in &items {
        item.check_mirror(solution)?;
    }
    let mut body = common_tail(&[])?;
    for section in ["MCPTools", "MCPResources", "MCPPrompts"] {
        let mut written = String::new();
        for item in items.iter().filter(|item| item.section() == section) {
            let description = item.description(described.get(item.name()).copied())?;
            written.push_str(&item.xml(&request.name, &description)?);
        }
        if !written.is_empty() {
            body.push_str(&format!(
                "\n            <{section}>{written}\n            </{section}>"
            ));
        }
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
pub(crate) struct Allowed {
    pub(crate) namespace: String,
    pub(crate) tools: Vec<String>,
    pub(crate) resources: Vec<String>,
    pub(crate) prompts: Vec<String>,
}

pub(crate) fn allowed(solution: &Solution, spec: &str) -> Result<Allowed, NewError> {
    let (namespace, items) = match spec.split_once('=') {
        Some((namespace, items)) => (namespace, Some(items)),
        None => (spec, None),
    };
    if namespace.is_empty() {
        return Err(NewError::Invalid(format!(
            "--namespace {spec:?} must be Namespace or Namespace=item,item"
        )));
    }
    let names: Option<Vec<&str>> = items.map(|items| items.split(',').map(str::trim).collect());
    if names
        .as_ref()
        .is_some_and(|names| names.iter().any(|name| name.is_empty()))
    {
        return Err(NewError::Invalid(format!(
            "--namespace {spec:?} has an empty item; write Namespace=item,item"
        )));
    }
    if let Some(names) = &names {
        unique("item", names.iter().copied())?;
    }
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
                let kinds = [&tools, &resources, &prompts]
                    .iter()
                    .filter(|defined| defined.contains(&name))
                    .count();
                if kinds > 1 {
                    return Err(NewError::Invalid(format!(
                        "MCPNamespace {namespace} has more than one kind of item named {name}; give each kind its own name"
                    )));
                }
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
pub(crate) fn skill_spec(spec: &str) -> Result<(String, String), NewError> {
    spec.split_once(':')
        .filter(|(repository, _)| repository_name(repository))
        .and_then(|(repository, path)| Some((repository.to_string(), repository_path(path)?)))
        .ok_or_else(|| {
            NewError::Invalid(format!(
                "--skill {spec:?} must be Repository:/path, inside the repository"
            ))
        })
}

/// `TemplateOrShape` or `TemplateOrShape=alias, alias`.
pub(crate) fn taxonomy_spec(
    known: &Known,
    spec: &str,
) -> Result<(&'static str, String, String), NewError> {
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

pub(crate) fn taxonomy_row(kind: &str, name: &str, aliases: &str) -> String {
    format!(
        "\n                        <Row>\n                            <alias>{}</alias>\n                            <entityType>{}</entityType>\n                            <entityTypeName>{}</entityTypeName>\n                            <identityProperties>\n                                <infoTable>\n                                    <DataShape>\n                                        <FieldDefinitions>\n                                            <FieldDefinition\n                                             baseType=\"STRING\"\n                                             description=\"Match mode applied to this property's value\"\n                                             name=\"match\"\n                                             ordinal=\"2\"></FieldDefinition>\n                                            <FieldDefinition\n                                             aspect.isPrimaryKey=\"true\"\n                                             baseType=\"STRING\"\n                                             description=\"Property name on the Thing (use &quot;name&quot; for the entity name)\"\n                                             name=\"name\"\n                                             ordinal=\"1\"></FieldDefinition>\n                                        </FieldDefinitions>\n                                    </DataShape>\n                                    <Rows>\n                                        <Row>\n                                            <match><![CDATA[normalizedContains]]></match>\n                                            <name><![CDATA[name]]></name>\n                                        </Row>\n                                    </Rows>\n                                </infoTable>\n                            </identityProperties>\n                        </Row>",
        cdata(aliases),
        cdata(kind),
        cdata(name)
    )
}

/// Refuse a character XML cannot hold; `]]>` is written as split CDATA.
pub(crate) fn cdata_value(value: &str) -> Result<(), NewError> {
    match crate::core::scan::illegal_xml_char(value) {
        Some(illegal) => Err(illegal.into()),
        None => Ok(()),
    }
}

/// Refuse a resolver that is not a Thing implementing the HierarchyResolver ThingShape.
pub(crate) fn check_resolver(known: &Known, resolver: &str) -> Result<(), NewError> {
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
    Ok(())
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
        check_resolver(&known, resolver)?;
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
        cdata_value(value)?;
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
