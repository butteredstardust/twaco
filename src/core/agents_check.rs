//! The `ai references` gate.

use super::agents::{self, collection, Agent, Namespace, ServiceReference};
use super::config::Solution;
use super::types::{self, Member, Model};
use super::workspace;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

pub(crate) const GATE: &str = "ai references";

#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) examined: usize,
    pub(crate) problems: Vec<Problem>,
}

#[derive(Debug)]
pub(crate) struct Problem {
    pub(crate) file: String,
    pub(crate) rule: String,
    pub(crate) message: String,
}

pub(crate) fn run_in(
    solution: &Solution,
    discovered: &workspace::Discovery,
    model: &Model,
) -> Report {
    let entities: BTreeMap<_, _> = model
        .entities
        .iter()
        .map(|entity| ((entity.collection.as_str(), entity.name.as_str()), entity))
        .collect();
    let platform = types::cached_platform(solution);
    let mut namespaces = BTreeMap::new();
    let mut agent_documents = Vec::new();
    let mut report = Report::default();
    for unreadable in &discovered.unreadable {
        let (path, why) = unreadable
            .split_once(": ")
            .unwrap_or((unreadable, "could not read"));
        let ai = Path::new(path).components().any(|part| {
            matches!(
                part.as_os_str().to_str(),
                Some("AIAgents" | "MCPNamespaces")
            )
        });
        if ai {
            report.examined += 1;
            report.problems.push(problem(
                &relative(solution, Path::new(path)),
                "unreadable",
                why,
            ));
        }
    }
    for file in discovered
        .entities
        .iter()
        .filter(|file| matches!(file.info.collection.as_str(), "MCPNamespaces" | "AIAgents"))
    {
        report.examined += 1;
        let relative = relative(solution, &file.path);
        let parsed = std::fs::read(&file.path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                if file.info.collection == "MCPNamespaces" {
                    agents::namespace(&bytes).map(Parsed::Namespace)
                } else {
                    agents::agent(&bytes).map(Parsed::Agent)
                }
            });
        match parsed {
            Ok(Parsed::Namespace(namespace)) => {
                namespaces.insert(file.info.name.clone(), (relative, namespace));
            }
            Ok(Parsed::Agent(agent)) => agent_documents.push((relative, agent)),
            Err(error) => report
                .problems
                .push(problem(&relative, "unreadable", error)),
        }
    }
    for (file, namespace) in namespaces.values() {
        for reference in namespace.tools.iter().chain(
            namespace
                .resources
                .iter()
                .filter_map(|resource| resource.service.as_ref()),
        ) {
            check_provider(
                solution,
                file,
                reference,
                &entities,
                platform.as_ref(),
                &mut report,
            );
        }
        for resource in &namespace.resources {
            if let Some(reference) = &resource.file {
                let Some(root) = solution.repositories_root() else {
                    continue;
                };
                let folder = root.join(&reference.repository);
                if folder.is_dir() && !file_in(&folder, &reference.path) {
                    report.problems.push(problem(
                        file,
                        "missing-file",
                        format!(
                            "{} names {} in {}, but it is not present",
                            resource.name, reference.path, reference.repository
                        ),
                    ));
                }
            }
        }
    }
    for (file, agent) in agent_documents {
        check_agent(
            solution,
            file.as_str(),
            &agent,
            &namespaces,
            &entities,
            platform.as_ref(),
            &mut report,
        );
    }
    report
}

enum Parsed {
    Namespace(Namespace),
    Agent(Agent),
}

fn check_provider(
    solution: &Solution,
    file: &str,
    reference: &ServiceReference,
    entities: &BTreeMap<(&str, &str), &types::Entity>,
    platform: Option<&types::Platform>,
    report: &mut Report,
) {
    let Some(collection) = collection(&reference.provider_type) else {
        return;
    };
    let entity = entities
        .get(&(collection, reference.provider.as_str()))
        .copied();
    if entity.is_none() {
        if platform_has(platform, collection, &reference.provider) {
            return;
        }
        if own_name(solution, &reference.provider)
            || entities
                .keys()
                .any(|(_, name)| *name == reference.provider.as_str())
        {
            report.problems.push(problem(
                file,
                "missing-provider",
                format!(
                    "{} names {} as a {}, but that provider is not present",
                    reference.name, reference.provider, reference.provider_type
                ),
            ));
        }
        return;
    }
    if !declares_service(
        entity.unwrap(),
        &reference.service,
        entities,
        &mut BTreeSet::new(),
    ) {
        report.problems.push(problem(
            file,
            "missing-service",
            format!(
                "{} calls {}.{} but that service is not declared",
                reference.name, reference.provider, reference.service
            ),
        ));
    }
}

fn check_agent(
    solution: &Solution,
    file: &str,
    agent: &Agent,
    namespaces: &BTreeMap<String, (String, Namespace)>,
    entities: &BTreeMap<(&str, &str), &types::Entity>,
    platform: Option<&types::Platform>,
    report: &mut Report,
) {
    for reference in &agent.namespaces {
        let Some((_, namespace)) = namespaces.get(&reference.name) else {
            if reference.name != "AgentUtilities" {
                report.problems.push(problem(
                    file,
                    "unknown-namespace",
                    format!(
                        "{} is not an MCPNamespace in this repository",
                        reference.name
                    ),
                ));
            }
            continue;
        };
        let check = |names: &[String], known: BTreeSet<&str>, kind: &str, report: &mut Report| {
            for name in names {
                if !known.contains(name.as_str()) {
                    report.problems.push(problem(
                        file,
                        "unknown-allowed",
                        format!(
                            "{} allows {kind} {name}, which {} does not define",
                            reference.name, reference.name
                        ),
                    ));
                }
            }
        };
        check(
            &reference.tools,
            namespace
                .tools
                .iter()
                .map(|item| item.name.as_str())
                .collect(),
            "Tool",
            report,
        );
        check(
            &reference.resources,
            namespace
                .resources
                .iter()
                .map(|item| item.name.as_str())
                .collect(),
            "Resource",
            report,
        );
        check(
            &reference.prompts,
            namespace.prompts.iter().map(String::as_str).collect(),
            "Prompt",
            report,
        );
    }
    for (kind, name) in &agent.taxonomy {
        let Some(collection) = collection(kind) else {
            continue;
        };
        let other = if collection == "ThingTemplates" {
            "ThingShapes"
        } else {
            "ThingTemplates"
        };
        if entities.contains_key(&(other, name.as_str())) {
            report.problems.push(problem(
                file,
                "taxonomy-type-mismatch",
                format!("{name} is a {other}, not a {kind}"),
            ));
        } else if !entities.contains_key(&(collection, name.as_str()))
            && !platform_has(platform, collection, name)
            && own_name(solution, name)
        {
            report.problems.push(problem(
                file,
                "missing-taxonomy-type",
                format!("{kind} {name} is not in the repository or platform cache"),
            ));
        }
    }
    for resolver in &agent.resolvers {
        let Some(entity) = entities.get(&("Things", resolver.as_str())).copied() else {
            if own_name(solution, resolver) {
                report.problems.push(problem(
                    file,
                    "missing-resolver",
                    format!("Thing {resolver} is not in the repository"),
                ));
            }
            continue;
        };
        if !implements_shape(entity, "HierarchyResolver", entities, &mut BTreeSet::new()) {
            report.problems.push(problem(
                file,
                "resolver-shape",
                format!("Thing {resolver} does not implement HierarchyResolver"),
            ));
        }
    }
    if let Some(root) = solution.repositories_root() {
        for skill in &agent.skills {
            let folder = root.join(&skill.repository);
            if folder.is_dir()
                && !file_in(
                    &folder,
                    &format!(
                        "{}/SKILL.md",
                        skill.path.trim_matches('/').replace('\\', "/")
                    ),
                )
            {
                report.problems.push(problem(
                    file,
                    "missing-file",
                    format!(
                        "skill {} in {} has no SKILL.md",
                        skill.path, skill.repository
                    ),
                ));
            }
        }
    }
}

fn declares_service(
    entity: &types::Entity,
    service: &str,
    entities: &BTreeMap<(&str, &str), &types::Entity>,
    seen: &mut BTreeSet<(String, String)>,
) -> bool {
    let key = (entity.collection.clone(), entity.name.clone());
    if !seen.insert(key) {
        return false;
    }
    if entity
        .members
        .iter()
        .any(|member| matches!(member, Member::Service(value) if value.name == service))
    {
        return true;
    }
    entity
        .template
        .as_ref()
        .and_then(|name| entities.get(&("ThingTemplates", name.as_str())).copied())
        .is_some_and(|parent| declares_service(parent, service, entities, seen))
        || entity
            .shapes
            .iter()
            .filter_map(|name| entities.get(&("ThingShapes", name.as_str())).copied())
            .any(|shape| declares_service(shape, service, entities, seen))
}

fn implements_shape(
    entity: &types::Entity,
    wanted: &str,
    entities: &BTreeMap<(&str, &str), &types::Entity>,
    seen: &mut BTreeSet<(String, String)>,
) -> bool {
    let key = (entity.collection.clone(), entity.name.clone());
    if !seen.insert(key) {
        return false;
    }
    entity.shapes.iter().any(|shape| {
        shape == wanted
            || entities
                .get(&("ThingShapes", shape.as_str()))
                .copied()
                .is_some_and(|shape| implements_shape(shape, wanted, entities, seen))
    }) || entity
        .template
        .as_ref()
        .and_then(|name| entities.get(&("ThingTemplates", name.as_str())).copied())
        .is_some_and(|template| implements_shape(template, wanted, entities, seen))
}

/// A name under one of the solution's own projects belongs in the repository, so its absence is
/// a rename or a deletion; any other name may be another solution's, and is not judged.
fn own_name(solution: &Solution, name: &str) -> bool {
    solution
        .projects
        .iter()
        .any(|project| name.starts_with(&format!("{}.", project.name)))
}

fn platform_has(platform: Option<&types::Platform>, collection: &str, name: &str) -> bool {
    let Some(platform) = platform else {
        return false;
    };
    match collection {
        "ThingTemplates" => platform.templates.contains_key(name),
        "ThingShapes" => platform.shapes.contains_key(name),
        _ => false,
    }
}

fn file_in(folder: &Path, named: &str) -> bool {
    let path = Path::new(named.trim_start_matches(['/', '\\']));
    !path.components().any(|part| {
        matches!(
            part,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) && folder.join(path).is_file()
}

fn relative(solution: &Solution, path: &Path) -> String {
    path.strip_prefix(&solution.root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
fn problem(file: &str, rule: &str, message: impl Into<String>) -> Problem {
    Problem {
        file: file.to_string(),
        rule: rule.to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(files: &[(&str, &str)], repositories: bool) -> Report {
        let dir = tempfile::tempdir().unwrap();
        let config = if repositories {
            "[[project]]\nname = 'P'\n[repositories]\nroot = 'repos'\n"
        } else {
            "[[project]]\nname = 'P'\n"
        };
        std::fs::write(dir.path().join("twaco.toml"), config).unwrap();
        for (path, text) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        let discovery = workspace::discover(&solution);
        let (model, _) = types::load_model_from(&discovery);
        run_in(&solution, &discovery, &model)
    }

    fn thing(name: &str, body: &str) -> String {
        format!("<Entities><Things><Thing name=\"{name}\" projectName=\"P\"><ThingShape>{body}</ThingShape></Thing></Things></Entities>")
    }

    #[test]
    fn reports_each_ai_reference_rule() {
        let namespace = r#"<Entities><MCPNamespaces><MCPNamespace name="P.N" projectName="P"><MCPTools><MCPTool name="wrong-provider" serviceName="Nope" serviceProvider="P.Template" serviceProviderType="Thing"/><MCPTool name="missing-service" serviceName="Nope" serviceProvider="P.Provider" serviceProviderType="Thing"/></MCPTools><MCPResources><MCPResource name="file" type="File"><FileDefinition filePath="/lost.md" fileRepository="P.Files"/></MCPResource></MCPResources><MCPPrompts><MCPPrompt name="known"/></MCPPrompts></MCPNamespace></MCPNamespaces></Entities>"#;
        let agent = r#"<Entities><AIAgents><AIAgent name="P.A" projectName="P"><ConfigurationTables><ConfigurationTable><Rows><Row><entityType>ThingTemplate</entityType><entityTypeName>P.Shape</entityTypeName><resolverThingName>P.Resolver</resolverThingName></Row><Row><entityType>ThingShape</entityType><entityTypeName>P.Missing</entityTypeName><resolverThingName>P.MissingResolver</resolverThingName></Row></Rows></ConfigurationTable></ConfigurationTables><SkillReferences><SkillReference repositoryName="P.Files" skillPath="/lost-skill"/></SkillReferences><MCPNamespaceReferences><MCPNamespaceReference name="P.N"><AllowedTools><Tool name="nope"/></AllowedTools><AllowedResources><Resource name="nope"/></AllowedResources><AllowedPrompts><Prompt name="nope"/></AllowedPrompts></MCPNamespaceReference><MCPNamespaceReference name="Missing"/></MCPNamespaceReferences></AIAgent></AIAgents></Entities>"#;
        let report = run(&[
            ("MCPNamespaces/P.N.xml", namespace),
            ("AIAgents/P.A.xml", agent),
            ("Things/P.Provider.xml", &thing("P.Provider", "")),
            ("ThingTemplates/P.Template.xml", "<Entities><ThingTemplates><ThingTemplate name=\"P.Template\" projectName=\"P\"/></ThingTemplates></Entities>"),
            ("ThingShapes/P.Shape.xml", "<Entities><ThingShapes><ThingShape name=\"P.Shape\" projectName=\"P\"/></ThingShapes></Entities>"),
            ("Things/P.Resolver.xml", &thing("P.Resolver", "")),
            ("Things/P.Files.xml", &thing("P.Files", "")),
            ("repos/P.Files/.keep", ""),
        ], true);
        let rules: BTreeSet<_> = report
            .problems
            .iter()
            .map(|problem| problem.rule.as_str())
            .collect();
        for rule in [
            "missing-provider",
            "missing-service",
            "unknown-namespace",
            "unknown-allowed",
            "missing-taxonomy-type",
            "taxonomy-type-mismatch",
            "missing-resolver",
            "resolver-shape",
            "missing-file",
        ] {
            assert!(rules.contains(rule), "{rule}: {rules:?}");
        }
    }

    #[test]
    fn leaves_external_and_platform_namespace_references_unchecked_without_mirrors() {
        let namespace = r#"<Entities><MCPNamespaces><MCPNamespace name="P.N" projectName="P"><MCPTools><MCPTool name="external" serviceName="Run" serviceProvider="Elsewhere.T" serviceProviderType="Thing"/></MCPTools></MCPNamespace></MCPNamespaces></Entities>"#;
        let agent = r#"<Entities><AIAgents><AIAgent name="P.A" projectName="P"><ConfigurationTables><ConfigurationTable><Rows><Row><entityType>ThingTemplate</entityType><entityTypeName>Cell_TT</entityTypeName><resolverThingName>Elsewhere.Resolver</resolverThingName></Row></Rows></ConfigurationTable></ConfigurationTables><MCPNamespaceReferences><MCPNamespaceReference name="P.N"/><MCPNamespaceReference name="AgentUtilities"/></MCPNamespaceReferences></AIAgent></AIAgents></Entities>"#;
        let report = run(
            &[
                ("MCPNamespaces/P.N.xml", namespace),
                ("AIAgents/P.A.xml", agent),
            ],
            false,
        );
        assert_eq!(report.examined, 2);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
    }

    #[test]
    fn a_provider_named_under_the_solutions_own_project_must_be_in_the_repository() {
        let namespace = r#"<Entities><MCPNamespaces><MCPNamespace name="P.N" projectName="P"><MCPTools><MCPTool name="renamed" serviceName="Run" serviceProvider="P.Gone" serviceProviderType="Thing"/><MCPTool name="other" serviceName="Run" serviceProvider="Q.Elsewhere" serviceProviderType="Thing"/></MCPTools></MCPNamespace></MCPNamespaces></Entities>"#;
        let report = run(&[("MCPNamespaces/P.N.xml", namespace)], false);
        let found: Vec<_> = report
            .problems
            .iter()
            .map(|problem| (problem.rule.as_str(), problem.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [(
                "missing-provider",
                "renamed names P.Gone as a Thing, but that provider is not present"
            )]
        );
    }
}
