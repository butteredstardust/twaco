//! Edits to MCP namespaces and AI agents: their tools, resources and prompts, what an agent may
//! use, its taxonomy rows, skills, resolver and system prompt.
//!
//! An item, a taxonomy row or a skill is written as `new mcp-namespace` and `new ai-agent` write
//! it. A namespace reference or the skill list holds nothing but names, so it is written again
//! whole; everything else in the file stays byte for byte.

use super::{invalid, reindent_block, remove_child, splice_all, Doc, EditError, Plan};
use crate::core::agents::child_text;
use crate::core::config::Solution;
use crate::core::entity::new::ai::{self, Allowed, Item, Known};
use crate::core::entity::new::cdata;
use crate::core::scan::{self, escape_attribute as escape, Span};
use crate::core::splice::Edit;
use crate::core::workspace::EntityFile;
use std::path::PathBuf;

/// The sections of a namespace, in the order it writes them, and the tag each one holds.
const SECTIONS: [(&str, &str); 3] = [
    ("MCPTools", "MCPTool"),
    ("MCPResources", "MCPResource"),
    ("MCPPrompts", "MCPPrompt"),
];

/// What `agent set` changes; `None` leaves a value as it is, an empty resolver clears it.
#[derive(Clone, Debug, Default)]
pub struct AgentChange {
    pub resolver: Option<String>,
    pub instructions: Option<String>,
    pub instructions_file: Option<PathBuf>,
    pub agent_description: Option<String>,
    pub display_name: Option<String>,
}

/// Every element directly inside `parent`.
fn children(doc: &Doc, parent: usize) -> Vec<usize> {
    let Some(end) = scan::element_end(&doc.tokens, parent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut index = parent + 1;
    while index < end {
        match doc.tokens[index].kind {
            scan::Kind::Start => {
                out.push(index);
                index = scan::element_end(&doc.tokens, index).map_or(end, |at| at + 1);
            }
            scan::Kind::Empty => {
                out.push(index);
                index += 1;
            }
            _ => index += 1,
        }
    }
    out
}

fn child(doc: &Doc, parent: usize, name: &str) -> Option<usize> {
    scan::child_tags(&doc.tokens, &doc.src, name, parent)
        .into_iter()
        .next()
}

/// The whitespace an element's line opens with; none when other markup precedes it on its line,
/// as in a compact one-line export.
fn indent(doc: &Doc, element: usize) -> String {
    let before = doc.indent_of(element);
    if before.chars().all(|c| c == ' ' || c == '\t') {
        before
    } else {
        String::new()
    }
}

/// Insert `xml` (lines at `from` spaces) as the last child of `parent`, one level deeper.
fn append(doc: &Doc, parent: usize, xml: &str, from: usize) -> Result<Edit, EditError> {
    let outer = indent(doc, parent);
    let block = reindent_block(xml, from, &format!("{outer}    "));
    if let Some(&last) = children(doc, parent).last() {
        let end = doc.span(last)?.end;
        return Ok(Edit::new(Span::new(end, end), block.into_bytes()));
    }
    let inner = doc.inner(parent)?;
    if doc.src[inner.start..inner.end]
        .iter()
        .all(u8::is_ascii_whitespace)
    {
        return doc.inner_edit(parent, format!("{block}\n{outer}"));
    }
    // A comment or text with no element: keep it, and add after it.
    Ok(Edit::new(
        Span::new(inner.end, inner.end),
        format!("{block}\n{outer}").into_bytes(),
    ))
}

/// Insert a new child element of the entity after the last of `after` that it has, or as its
/// last child when it has none of them.
fn insert_after(doc: &Doc, after: &[&str], xml: &str, from: usize) -> Result<Edit, EditError> {
    let anchor = after
        .iter()
        .filter_map(|name| child(doc, doc.entity, name))
        .max();
    let Some(anchor) = anchor else {
        return append(doc, doc.entity, xml, from);
    };
    let block = reindent_block(xml, from, &indent(doc, anchor));
    let end = doc.span(anchor)?.end;
    Ok(Edit::new(Span::new(end, end), block.into_bytes()))
}

/// The element of `doc` replaced by `xml` (lines at `from` spaces) at its own depth.
fn replace(doc: &Doc, element: usize, xml: &str, from: usize) -> Result<Edit, EditError> {
    let block = reindent_block(xml, from, &indent(doc, element));
    Ok(Edit::new(
        doc.span(element)?,
        block
            .trim_start_matches(['\r', '\n'])
            .trim_start()
            .as_bytes()
            .to_vec(),
    ))
}

pub(super) fn mcp_add(
    solution: &Solution,
    doc: &Doc,
    kind: &str,
    spec: &str,
    describe: Option<&str>,
    plan: &mut Plan,
) -> Result<(), EditError> {
    doc.require(&["MCPNamespaces"], "mcp add changes an MCPNamespace")?;
    let known = Known::read(solution);
    let item = Item::parse(&known, kind, spec)?;
    let section = item.section();
    // One name per item across kinds, so an agent's allowed list says which it means.
    if SECTIONS.iter().any(|(section, tag)| {
        doc.names_in(doc.entity, section, tag)
            .iter()
            .any(|name| name == item.name())
    }) {
        return Err(EditError::Exists(format!(
            "{} already has a tool, resource or prompt named {}",
            doc.label(),
            item.name()
        )));
    }
    item.check_mirror(solution)?;
    let description = item.description(describe.map(str::trim).filter(|text| !text.is_empty()))?;
    let xml = item.xml(doc.name(), &description)?;
    let edit = match child(doc, doc.entity, section) {
        Some(list) => append(doc, list, &xml, 16)?,
        None => {
            let earlier: Vec<&str> = std::iter::once("ConfigurationTables")
                .chain(
                    SECTIONS
                        .iter()
                        .map(|(name, _)| *name)
                        .take_while(|name| *name != section),
                )
                .collect();
            let block = format!("\n            <{section}>{xml}\n            </{section}>");
            let later = SECTIONS
                .iter()
                .map(|(name, _)| *name)
                .skip_while(|name| *name != section)
                .skip(1)
                .find_map(|name| child(doc, doc.entity, name));
            match later {
                // Before the first later section, so the order is the one new writes.
                Some(next)
                    if !earlier
                        .iter()
                        .any(|name| child(doc, doc.entity, name).is_some()) =>
                {
                    // Written where the next section starts, which already has its indent, so
                    // the block drops its own and gives the next section the indent back.
                    let lead = indent(doc, next);
                    let block = reindent_block(&block, 12, &lead);
                    let at = doc.tokens[next].span.start;
                    let text = format!("{}\n{lead}", block.trim_start());
                    Edit::new(Span::new(at, at), text.into_bytes())
                }
                _ => insert_after(doc, &earlier, &block, 12)?,
            }
        }
    };
    plan.summary = format!("add {kind} {}", item.name());
    plan.changes
        .push(doc.change(splice_all(&doc.src, vec![edit])?));
    Ok(())
}

/// Remove a tool, resource or prompt, and the permission to use it from every agent in the
/// solution: the server refuses to drop an item an agent allows.
pub(super) fn mcp_remove(
    found: &[EntityFile],
    doc: &Doc,
    name: &str,
    plan: &mut Plan,
) -> Result<(), EditError> {
    doc.require(&["MCPNamespaces"], "mcp remove changes an MCPNamespace")?;
    let (list, element) = SECTIONS
        .iter()
        .find_map(|(section, tag)| {
            let list = child(doc, doc.entity, section)?;
            Some((list, doc.named(list, tag, name)?))
        })
        .ok_or_else(|| {
            EditError::Unknown(format!(
                "{} has no tool, resource or prompt {name}",
                doc.label()
            ))
        })?;
    // The server writes no empty section, so the last item takes its section with it.
    let after = if children(doc, list).len() == 1 {
        remove_child(doc, doc.entity, list)?
    } else {
        remove_child(doc, list, element)?
    };
    plan.summary = format!("remove {name}");
    plan.changes.push(doc.change(after));
    for file in found
        .iter()
        .filter(|file| file.info.collection == "AIAgents")
    {
        let agent = Doc::read(file)?;
        let Some(mut allowed) = allowance(&agent, doc.name())? else {
            continue;
        };
        let before = allowed.tools.len() + allowed.resources.len() + allowed.prompts.len();
        for names in [
            &mut allowed.tools,
            &mut allowed.resources,
            &mut allowed.prompts,
        ] {
            names.retain(|allowed| allowed != name);
        }
        if allowed.tools.len() + allowed.resources.len() + allowed.prompts.len() == before {
            continue;
        }
        let changed = write_allowance(&agent, &allowed)?;
        plan.changes.push(agent.change(changed));
        plan.warnings.push(format!(
            "{} no longer allows {name}; the server refuses a namespace that drops an item an agent on it allows: `twaco deploy` imports the agent without it first, and with `entity push`, push the agent before the namespace",
            agent.label()
        ));
    }
    Ok(())
}

/// `doc`, an agent, with none of `dropped`'s items allowed from `namespace`; `None` when it
/// allows none of them.
pub(super) fn without_allowances(
    doc: &Doc,
    namespace: &str,
    dropped: &crate::core::agents::Items,
) -> Result<Option<Vec<u8>>, EditError> {
    let Some(mut allowed) = allowance(doc, namespace)? else {
        return Ok(None);
    };
    let before = allowed.tools.len() + allowed.resources.len() + allowed.prompts.len();
    allowed.tools.retain(|name| !dropped.tools.contains(name));
    allowed
        .resources
        .retain(|name| !dropped.resources.contains(name));
    allowed
        .prompts
        .retain(|name| !dropped.prompts.contains(name));
    if allowed.tools.len() + allowed.resources.len() + allowed.prompts.len() == before {
        return Ok(None);
    }
    write_allowance(doc, &allowed).map(Some)
}

/// The agent's `MCPNamespaceReference` to `namespace`, as names.
fn allowance(doc: &Doc, namespace: &str) -> Result<Option<Allowed>, EditError> {
    let Some(references) = child(doc, doc.entity, "MCPNamespaceReferences") else {
        return Ok(None);
    };
    let Some(reference) = doc.named(references, "MCPNamespaceReference", namespace) else {
        return Ok(None);
    };
    let names = |group: &str, tag: &str| doc.names_in(reference, group, tag);
    Ok(Some(Allowed {
        namespace: namespace.to_string(),
        tools: names("AllowedTools", "Tool"),
        resources: names("AllowedResources", "Resource"),
        prompts: names("AllowedPrompts", "Prompt"),
    }))
}

/// The reference element for `allowed`, its lines at 16 spaces.
fn reference_xml(allowed: &Allowed) -> Result<String, EditError> {
    let mut xml = format!(
        "\n                <MCPNamespaceReference\n                 name=\"{}\">",
        escape(&allowed.namespace)?
    );
    for (group, tag, names) in [
        ("AllowedTools", "Tool", &allowed.tools),
        ("AllowedResources", "Resource", &allowed.resources),
        ("AllowedPrompts", "Prompt", &allowed.prompts),
    ] {
        if names.is_empty() {
            continue;
        }
        xml.push_str(&format!("\n                    <{group}>"));
        for name in names {
            xml.push_str(&format!(
                "\n                        <{tag}\n                         name=\"{}\"></{tag}>",
                escape(name)?
            ));
        }
        xml.push_str(&format!("\n                    </{group}>"));
    }
    xml.push_str("\n                </MCPNamespaceReference>");
    Ok(xml)
}

/// `doc` with its reference to `allowed.namespace` written as `allowed`, added when absent.
fn write_allowance(doc: &Doc, allowed: &Allowed) -> Result<Vec<u8>, EditError> {
    let xml = reference_xml(allowed)?;
    let Some(references) = child(doc, doc.entity, "MCPNamespaceReferences") else {
        let block = format!(
            "\n            <MCPNamespaceReferences>{}\n            </MCPNamespaceReferences>",
            reindent_block(&xml, 16, "                ")
        );
        let after = ["ConfigurationTables", "SystemPrompt", "SkillReferences"];
        return splice_all(&doc.src, vec![insert_after(doc, &after, &block, 12)?]);
    };
    let edit = match doc.named(references, "MCPNamespaceReference", &allowed.namespace) {
        Some(reference) => replace(doc, reference, &xml, 16)?,
        None => append(doc, references, &xml, 16)?,
    };
    splice_all(&doc.src, vec![edit])
}

pub(super) fn agent_allow(
    solution: &Solution,
    doc: &Doc,
    spec: &str,
    plan: &mut Plan,
) -> Result<(), EditError> {
    doc.require(&["AIAgents"], "agent allow changes an AIAgent")?;
    let wanted = ai::allowed(solution, spec)?;
    let mut allowed = allowance(doc, &wanted.namespace)?.unwrap_or(Allowed {
        namespace: wanted.namespace.clone(),
        tools: Vec::new(),
        resources: Vec::new(),
        prompts: Vec::new(),
    });
    let mut added = Vec::new();
    for (have, want) in [
        (&mut allowed.tools, &wanted.tools),
        (&mut allowed.resources, &wanted.resources),
        (&mut allowed.prompts, &wanted.prompts),
    ] {
        for name in want {
            if !have.contains(name) {
                have.push(name.clone());
                added.push(name.clone());
            }
        }
    }
    if added.is_empty() && allowance(doc, &wanted.namespace)?.is_some() {
        return Err(EditError::Exists(format!(
            "{} already allows all of that from {}",
            doc.label(),
            wanted.namespace
        )));
    }
    plan.summary = format!("allow {} from {}", added.join(", "), wanted.namespace);
    plan.changes
        .push(doc.change(write_allowance(doc, &allowed)?));
    Ok(())
}

pub(super) fn agent_disallow(doc: &Doc, spec: &str, plan: &mut Plan) -> Result<(), EditError> {
    doc.require(&["AIAgents"], "agent disallow changes an AIAgent")?;
    let (namespace, items) = match spec.split_once('=') {
        Some((namespace, items)) => (namespace, Some(items)),
        None => (spec, None),
    };
    let mut allowed = allowance(doc, namespace)?.ok_or_else(|| {
        EditError::Unknown(format!("{} does not reference {namespace}", doc.label()))
    })?;
    let references = doc.only_child(doc.entity, "MCPNamespaceReferences")?;
    let reference = doc
        .named(references, "MCPNamespaceReference", namespace)
        .expect("allowance found it");
    let Some(items) = items else {
        if scan::child_tags(&doc.tokens, &doc.src, "MCPNamespaceReference", references).len() == 1 {
            return Err(invalid(format!(
                "{namespace} is the only namespace {} uses; an agent needs one, so allow another first",
                doc.label()
            )));
        }
        plan.summary = format!("stop using {namespace}");
        plan.changes
            .push(doc.change(remove_child(doc, references, reference)?));
        return Ok(());
    };
    let names: Vec<&str> = items.split(',').map(str::trim).collect();
    if names.iter().any(|name| name.is_empty()) {
        return Err(invalid(format!(
            "agent disallow {spec:?} has an empty item; write Namespace=item,item, or Namespace alone"
        )));
    }
    ai::unique("item", names.iter().copied())?;
    for name in names {
        let before = allowed.tools.len() + allowed.resources.len() + allowed.prompts.len();
        for names in [
            &mut allowed.tools,
            &mut allowed.resources,
            &mut allowed.prompts,
        ] {
            names.retain(|allowed| allowed != name);
        }
        if allowed.tools.len() + allowed.resources.len() + allowed.prompts.len() == before {
            return Err(EditError::Unknown(format!(
                "{} does not allow {name} from {namespace}",
                doc.label()
            )));
        }
    }
    plan.summary = format!("disallow {items} from {namespace}");
    plan.changes
        .push(doc.change(write_allowance(doc, &allowed)?));
    Ok(())
}

/// The `Rows` of the agent's configuration table `name`.
fn table_rows(doc: &Doc, name: &str) -> Result<usize, EditError> {
    let tables = doc.only_child(doc.entity, "ConfigurationTables")?;
    let table = doc
        .named(tables, "ConfigurationTable", name)
        .ok_or_else(|| invalid(format!("{} has no {name} table", doc.label())))?;
    doc.only_child(table, "Rows")
}

pub(super) fn taxonomy_add(
    solution: &Solution,
    doc: &Doc,
    spec: &str,
    plan: &mut Plan,
) -> Result<(), EditError> {
    doc.require(&["AIAgents"], "agent taxonomy add changes an AIAgent")?;
    let known = Known::read(solution);
    let (kind, name, aliases) = ai::taxonomy_spec(&known, spec)?;
    for value in [name.as_str(), aliases.as_str()] {
        ai::cdata_value(value)?;
    }
    let rows = table_rows(doc, "TaxonomyConfiguration")?;
    for row in scan::child_tags(&doc.tokens, &doc.src, "Row", rows) {
        if child_text(&doc.tokens, &doc.src, "entityTypeName", row).as_deref() == Some(&name) {
            return Err(EditError::Exists(format!(
                "{} already has a taxonomy row for {name}",
                doc.label()
            )));
        }
    }
    let edit = append(doc, rows, &ai::taxonomy_row(kind, &name, &aliases), 24)?;
    plan.summary = format!("add taxonomy {kind} {name}");
    plan.changes
        .push(doc.change(splice_all(&doc.src, vec![edit])?));
    Ok(())
}

pub(super) fn taxonomy_remove(doc: &Doc, name: &str, plan: &mut Plan) -> Result<(), EditError> {
    doc.require(&["AIAgents"], "agent taxonomy remove changes an AIAgent")?;
    let rows = table_rows(doc, "TaxonomyConfiguration")?;
    let row = scan::child_tags(&doc.tokens, &doc.src, "Row", rows)
        .into_iter()
        .find(|&row| {
            child_text(&doc.tokens, &doc.src, "entityTypeName", row).as_deref() == Some(name)
        })
        .ok_or_else(|| {
            EditError::Unknown(format!("{} has no taxonomy row for {name}", doc.label()))
        })?;
    plan.summary = format!("remove taxonomy {name}");
    plan.changes.push(doc.change(remove_child(doc, rows, row)?));
    Ok(())
}

/// The agent's skills, as `(repository, path)`.
fn skills(doc: &Doc) -> Vec<(String, String)> {
    child(doc, doc.entity, "SkillReferences")
        .map(|list| {
            scan::child_tags(&doc.tokens, &doc.src, "SkillReference", list)
                .into_iter()
                .filter_map(|skill| {
                    Some((
                        doc.attr(skill, "repositoryName")?,
                        doc.attr(skill, "skillPath")?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `doc` with its skills written as `list`; none at all leaves the element out, as the server does.
fn write_skills(doc: &Doc, list: &[(String, String)]) -> Result<Vec<u8>, EditError> {
    let mut xml = String::from("\n            <SkillReferences>");
    for (repository, path) in list {
        xml.push_str(&format!(
            "\n                <SkillReference\n                 repositoryName=\"{}\"\n                 skillPath=\"{}\"></SkillReference>",
            escape(repository)?,
            escape(path)?
        ));
    }
    xml.push_str("\n            </SkillReferences>");
    match (child(doc, doc.entity, "SkillReferences"), list.is_empty()) {
        (Some(element), true) => remove_child(doc, doc.entity, element),
        (Some(element), false) => splice_all(&doc.src, vec![replace(doc, element, &xml, 12)?]),
        (None, true) => Ok(doc.src.clone()),
        (None, false) => splice_all(
            &doc.src,
            vec![insert_after(
                doc,
                &["ConfigurationTables", "SystemPrompt"],
                &xml,
                12,
            )?],
        ),
    }
}

pub(super) fn skill_add(
    solution: &Solution,
    doc: &Doc,
    spec: &str,
    plan: &mut Plan,
) -> Result<(), EditError> {
    doc.require(&["AIAgents"], "agent skill add changes an AIAgent")?;
    let (repository, path) = ai::skill_spec(spec)?;
    if let Some(root) = solution.repositories_root() {
        let folder = root.join(&repository);
        if folder.is_dir()
            && !folder
                .join(path.trim_start_matches('/'))
                .join("SKILL.md")
                .is_file()
        {
            return Err(EditError::Unknown(format!(
                "{repository} has no {path}/SKILL.md in its [repositories] mirror"
            )));
        }
    }
    let mut list = skills(doc);
    let same = |(r, p): &(String, String)| {
        *r == repository && p.trim_matches('/') == path.trim_matches('/')
    };
    if list.iter().any(same) {
        return Err(EditError::Exists(format!(
            "{} already has skill {repository}:{path}",
            doc.label()
        )));
    }
    list.push((repository.clone(), path.clone()));
    plan.summary = format!("add skill {repository}:{path}");
    plan.changes.push(doc.change(write_skills(doc, &list)?));
    Ok(())
}

pub(super) fn skill_remove(doc: &Doc, spec: &str, plan: &mut Plan) -> Result<(), EditError> {
    doc.require(&["AIAgents"], "agent skill remove changes an AIAgent")?;
    let (repository, path) = ai::skill_spec(spec)?;
    let mut list = skills(doc);
    let before = list.len();
    list.retain(|(r, p)| !(*r == repository && p.trim_matches('/') == path.trim_matches('/')));
    if list.len() == before {
        return Err(EditError::Unknown(format!(
            "{} has no skill {repository}:{path}",
            doc.label()
        )));
    }
    plan.summary = format!("remove skill {repository}:{path}");
    plan.changes.push(doc.change(write_skills(doc, &list)?));
    Ok(())
}

pub(super) fn agent_set(
    solution: &Solution,
    doc: &Doc,
    change: &AgentChange,
    plan: &mut Plan,
) -> Result<(), EditError> {
    doc.require(&["AIAgents"], "agent set changes an AIAgent")?;
    let instructions = match (&change.instructions, &change.instructions_file) {
        (Some(_), Some(_)) => {
            return Err(invalid(
                "give --instructions or --instructions-file, not both",
            ))
        }
        (Some(text), None) => Some(text.clone()),
        (None, Some(path)) => Some(
            std::fs::read_to_string(path)
                .map_err(|error| invalid(format!("{}: {error}", path.display())))?,
        ),
        (None, None) => None,
    };
    let mut edits = Vec::new();
    let mut said = Vec::new();
    if let Some(resolver) = &change.resolver {
        if !resolver.is_empty() {
            ai::check_resolver(&Known::read(solution), resolver)?;
        }
        let rows = table_rows(doc, "HierarchyConfiguration")?;
        let row = doc.only_child(rows, "Row")?;
        let element = doc.only_child(row, "resolverThingName")?;
        let value = if resolver.is_empty() {
            String::new()
        } else {
            cdata(resolver)
        };
        edits.push(doc.inner_edit(element, value)?);
        said.push(if resolver.is_empty() {
            "clear the resolver".to_string()
        } else {
            format!("resolver {resolver}")
        });
    }
    let prompt = child(doc, doc.entity, "SystemPrompt");
    for (tag, value, word) in [
        ("AgentInstructions", instructions.as_deref(), "instructions"),
        (
            "AgentDescription",
            change.agent_description.as_deref(),
            "agent description",
        ),
    ] {
        let Some(value) = value else {
            continue;
        };
        ai::cdata_value(value).map_err(|error| invalid(format!("the {word}: {error}")))?;
        let prompt =
            prompt.ok_or_else(|| invalid(format!("{} has no SystemPrompt", doc.label())))?;
        let element = doc.only_child(prompt, tag)?;
        edits.push(doc.inner_edit(element, cdata(value))?);
        said.push(word.to_string());
    }
    if let Some(name) = &change.display_name {
        let value = scan::attribute(&doc.src, &doc.tokens[doc.entity], "displayName")
            .map_err(invalid)?
            .ok_or_else(|| invalid(format!("{} has no displayName", doc.label())))?;
        edits.push(Edit::new(value, escape(name)?.into_bytes()));
        said.push(format!("display name {name}"));
    }
    if edits.is_empty() {
        return Err(invalid(
            "agent set needs --resolver, --instructions, --instructions-file, --agent-description or --display-name",
        ));
    }
    plan.summary = format!("set {}", said.join(", "));
    plan.changes.push(doc.change(splice_all(&doc.src, edits)?));
    Ok(())
}
