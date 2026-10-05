//! Solution documentation, generated from the repository.
//!
//! The same facts the other tools use, written down once: the projects and the order they deploy
//! in, how Things, templates and shapes inherit, the services each one can be called with, the
//! DataShapes and where they are used, and the references the index could only guess at. It is a
//! snapshot of the repository's XML, with no dates and no randomness, so regenerating it changes
//! the text only when the solution changed and a diff of two runs is a diff of the solution.
//!
//! It states what it does not cover: permissions are not read yet, and references built at run
//! time are invisible.

use super::catalog::{self, CatalogService};
use super::config::Solution;
use super::entity_key::EntityKey;
use super::index::{Confidence, EdgeKind, Index, Unreadable};
use super::types;
use serde::Serialize;
use std::collections::BTreeMap;

/// What the document does not claim, said in it.
pub const LIMITS: &str = "references built at run time, and anything outside this repository, are not seen; permissions are not read yet";

/// The most entities the summary lists in one section; detail lists all.
const SUMMARY_ENTITIES: usize = 200;
/// The most review-level references the summary lists.
const SUMMARY_REVIEW: usize = 25;
/// A diagram with more edges than this is left out: nobody can read it.
const DIAGRAM_EDGES: usize = 80;

#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct Counts {
    pub structural: usize,
    pub resolved: usize,
    pub review: usize,
}

impl Counts {
    fn add(&mut self, confidence: Confidence) {
        match confidence {
            Confidence::Structural => self.structural += 1,
            Confidence::Resolved => self.resolved += 1,
            Confidence::Review => self.review += 1,
        }
    }

    fn total(&self) -> usize {
        self.structural + self.resolved + self.review
    }
}

#[derive(Debug, Serialize)]
pub struct ProjectDoc {
    pub name: String,
    /// Its place in the deploy order, from 1.
    pub deploy_order: Option<usize>,
    pub depends_on: Vec<String>,
    /// How many entity documents it holds, by collection.
    pub entities: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub struct EntityDoc {
    /// `Collection/Name`.
    pub entity: String,
    pub collection: String,
    pub name: String,
    pub project: String,
    pub file: Option<String>,
    /// A Thing's template or a template's base, as written.
    pub template: Option<String>,
    /// The ThingShapes it implements itself, as written.
    pub shapes: Vec<String>,
    /// Everything it inherits, nearest first.
    pub inherits: Vec<String>,
    /// For a ThingShape: the Things and templates that implement it.
    pub implemented_by: Vec<String>,
    pub services: Vec<CatalogService>,
    /// What it refers to, and what refers to it, by how sure each reference is.
    pub refers_to: Counts,
    pub referred_to_by: Counts,
}

#[derive(Debug, Serialize)]
pub struct FieldDoc {
    pub name: String,
    pub base_type: String,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct DataShapeDoc {
    pub name: String,
    pub project: String,
    pub base: Option<String>,
    pub fields: Vec<FieldDoc>,
    /// The entities that type a service, property or field with it, directly.
    pub used_by: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ReviewReference {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub member: Option<String>,
    pub at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Document {
    pub solution: String,
    pub projects: Vec<ProjectDoc>,
    pub entities: Vec<EntityDoc>,
    pub data_shapes: Vec<DataShapeDoc>,
    /// References that only look like a name, for a person to judge.
    pub review_references: Vec<ReviewReference>,
    pub unparsed_scripts: Vec<String>,
    pub unreadable: Vec<Unreadable>,
    pub complete: bool,
    pub limits: &'static str,
}

/// Read the solution and write it down.
pub fn build(solution: &Solution) -> Document {
    let index = Index::build(solution);
    let services: BTreeMap<(String, String), Vec<CatalogService>> =
        catalog::build(solution, catalog::Query::default())
            .map(|catalog| {
                catalog
                    .entities
                    .into_iter()
                    .map(|entity| ((entity.collection, entity.name), entity.services))
                    .collect()
            })
            .unwrap_or_default();
    let (model, _) = types::load_model(solution);

    let order = index.deploy_order();
    let projects = solution
        .projects
        .iter()
        .map(|project| {
            let mut entities: BTreeMap<String, usize> = BTreeMap::new();
            for (key, node) in index.entities() {
                if node.project == project.name {
                    *entities.entry(key.collection().to_string()).or_default() += 1;
                }
            }
            ProjectDoc {
                name: project.name.clone(),
                deploy_order: order
                    .iter()
                    .position(|name| name == &project.name)
                    .map(|at| at + 1),
                depends_on: project.depends_on.clone(),
                entities,
            }
        })
        .collect();

    let mut entities = Vec::new();
    for (key, node) in index.entities() {
        if !matches!(
            key.collection(),
            "Things" | "ThingTemplates" | "ThingShapes"
        ) {
            continue;
        }
        let mut refers_to = Counts::default();
        for step in index.references_from(key) {
            refers_to.add(step.kind.confidence());
        }
        let mut referred_to_by = Counts::default();
        for step in index.references_to(key) {
            referred_to_by.add(step.kind.confidence());
        }
        entities.push(EntityDoc {
            entity: key.to_string(),
            collection: key.collection().to_string(),
            name: key.name().to_string(),
            project: node.project.clone(),
            file: node
                .file
                .as_ref()
                .map(|path| path.to_string_lossy().replace('\\', "/")),
            template: node.template.clone(),
            shapes: node.shapes.clone(),
            inherits: index.inheritance_names(key),
            implemented_by: if key.collection() == "ThingShapes" {
                index.implementers(key, None)
            } else {
                Vec::new()
            },
            services: services
                .get(&(key.collection().to_string(), key.name().to_string()))
                .cloned()
                .unwrap_or_default(),
            refers_to,
            referred_to_by,
        });
    }

    let mut data_shapes: Vec<DataShapeDoc> = model
        .data_shapes
        .iter()
        .filter_map(|shape| {
            let key = EntityKey::new("DataShapes", &shape.name).ok()?;
            let node = index.node(&key)?;
            let base = index
                .references_from(&key)
                .into_iter()
                .find(|step| step.kind == EdgeKind::BaseDataShape)
                .map(|step| step.to);
            let mut used_by: Vec<String> = index
                .references_to(&key)
                .into_iter()
                .filter(|step| step.kind == EdgeKind::DataShape)
                .map(|step| step.from)
                .collect();
            used_by.sort();
            used_by.dedup();
            Some(DataShapeDoc {
                name: shape.name.clone(),
                project: node.project.clone(),
                base,
                fields: shape
                    .fields
                    .iter()
                    .map(|field| FieldDoc {
                        name: field.name.clone(),
                        base_type: field.base_type.clone(),
                        description: field.description.clone(),
                    })
                    .collect(),
                used_by,
            })
        })
        .collect();
    data_shapes.sort_by(|a, b| a.name.cmp(&b.name));

    let review_references = index
        .edges()
        .into_iter()
        .filter(|(_, _, edge)| edge.confidence() == Confidence::Review)
        .map(|(from, to, edge)| ReviewReference {
            from,
            to,
            kind: edge.kind.word().to_string(),
            member: edge.from_member.clone(),
            at: edge.at.clone(),
        })
        .collect();

    Document {
        solution: solution.solution.name.clone(),
        projects,
        entities,
        data_shapes,
        review_references,
        unparsed_scripts: index.unparsed_scripts().to_vec(),
        unreadable: index.unreadable().to_vec(),
        complete: index.is_complete(),
        limits: LIMITS,
    }
}

impl Document {
    /// The document as JSON. A summary drops the services' signatures and the DataShapes' fields.
    pub fn to_json(&self, detail: bool) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("a document serialises");
        if !detail {
            if let Some(list) = value["entities"].as_array_mut() {
                for entity in list {
                    if let Some(object) = entity.as_object_mut() {
                        let count = object["services"].as_array().map_or(0, Vec::len);
                        object.remove("services");
                        object.insert("service_count".to_string(), count.into());
                    }
                }
            }
            if let Some(list) = value["data_shapes"].as_array_mut() {
                for shape in list {
                    if let Some(object) = shape.as_object_mut() {
                        let count = object["fields"].as_array().map_or(0, Vec::len);
                        object.remove("fields");
                        object.insert("field_count".to_string(), count.into());
                    }
                }
            }
            if let Some(list) = value["review_references"].as_array_mut() {
                list.truncate(SUMMARY_REVIEW);
            }
        }
        value
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn strength(counts: &Counts) -> String {
    format!(
        "{} ({} structural, {} resolved, {} review)",
        counts.total(),
        counts.structural,
        counts.resolved,
        counts.review
    )
}

fn type_of(base: &str, shape: &Option<String>) -> String {
    match shape {
        Some(shape) => format!("{base}<{shape}>"),
        None => base.to_string(),
    }
}

/// A Mermaid flowchart of `edges` (from, to), or nothing when there are none or too many to read.
fn diagram(title: &str, edges: &[(String, String)]) -> String {
    if edges.is_empty() {
        return String::new();
    }
    if edges.len() > DIAGRAM_EDGES {
        return format!(
            "{title}: {} references, too many to draw; use `twaco impact <entity> --dot` for one entity.\n\n",
            edges.len()
        );
    }
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
    for (from, to) in edges {
        let next = ids.len();
        ids.entry(from.as_str()).or_insert(next);
        let next = ids.len();
        ids.entry(to.as_str()).or_insert(next);
    }
    let mut out = format!("{title}:\n\n```mermaid\nflowchart LR\n");
    let mut named: Vec<(&&str, &usize)> = ids.iter().collect();
    named.sort_by_key(|(_, id)| **id);
    for (label, id) in named {
        out.push_str(&format!("  n{id}[\"{}\"]\n", label.replace('"', "'")));
    }
    for (from, to) in edges {
        out.push_str(&format!(
            "  n{} --> n{}\n",
            ids[from.as_str()],
            ids[to.as_str()]
        ));
    }
    out.push_str("```\n\n");
    out
}

/// The document as Markdown. A summary lists at most 200 entities and 25 review references and
/// gives each entity's services as a count; `detail` lists everything, with every signature and
/// field.
pub fn render_markdown(document: &Document, detail: bool) -> String {
    let mut out = String::new();
    let title = if document.solution.is_empty() {
        "Solution"
    } else {
        document.solution.as_str()
    };
    out.push_str(&format!("# {title}: solution documentation\n\n"));
    out.push_str("Generated by `twaco docs` from the repository's entity XML. It has no dates: regenerate it and diff to see what changed.\n\n");

    out.push_str("## Projects\n\n| Deploy order | Project | Depends on | Entities |\n| --- | --- | --- | --- |\n");
    let mut projects: Vec<&ProjectDoc> = document.projects.iter().collect();
    projects.sort_by_key(|project| {
        (
            project.deploy_order.unwrap_or(usize::MAX),
            project.name.clone(),
        )
    });
    for project in projects {
        let held: Vec<String> = project
            .entities
            .iter()
            .map(|(collection, count)| format!("{count} {collection}"))
            .collect();
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            project
                .deploy_order
                .map_or("-".to_string(), |at| at.to_string()),
            project.name,
            if project.depends_on.is_empty() {
                "-".to_string()
            } else {
                project.depends_on.join(", ")
            },
            if held.is_empty() {
                "-".to_string()
            } else {
                held.join(", ")
            }
        ));
    }
    out.push('\n');
    let project_edges: Vec<(String, String)> = document
        .projects
        .iter()
        .flat_map(|project| {
            project
                .depends_on
                .iter()
                .map(|dependency| (project.name.clone(), dependency.clone()))
        })
        .collect();
    out.push_str(&diagram(
        "Project dependencies (an arrow points at what must be imported first)",
        &project_edges,
    ));

    out.push_str("## Things, templates and shapes\n\n");
    if document.entities.is_empty() {
        out.push_str("None.\n\n");
    }
    let shown = if detail {
        document.entities.len()
    } else {
        SUMMARY_ENTITIES.min(document.entities.len())
    };
    for entity in &document.entities[..shown] {
        out.push_str(&format!("### {}\n\n", entity.entity));
        out.push_str(&format!("- project: {}\n", entity.project));
        if let Some(file) = &entity.file {
            out.push_str(&format!("- file: `{file}`\n"));
        }
        if !entity.inherits.is_empty() {
            out.push_str(&format!("- inherits: {}\n", entity.inherits.join(", ")));
        }
        if !entity.implemented_by.is_empty() {
            out.push_str(&format!(
                "- implemented by: {}\n",
                entity.implemented_by.join(", ")
            ));
        }
        out.push_str(&format!("- services: {}\n", entity.services.len()));
        out.push_str(&format!("- refers to: {}\n", strength(&entity.refers_to)));
        out.push_str(&format!(
            "- referred to by: {}\n",
            strength(&entity.referred_to_by)
        ));
        if detail && !entity.services.is_empty() {
            out.push_str("\n| Service | Parameters | Returns | From | Script |\n| --- | --- | --- | --- | --- |\n");
            for service in &entity.services {
                let parameters: Vec<String> = service
                    .parameters
                    .iter()
                    .map(|parameter| {
                        format!(
                            "{}{}: {}",
                            parameter.name,
                            if parameter.required { "" } else { "?" },
                            type_of(&parameter.base_type, &parameter.data_shape)
                        )
                    })
                    .collect();
                out.push_str(&format!(
                    "| `{}` | {} | {} | {} | {} |\n",
                    service.name,
                    if parameters.is_empty() {
                        "-".to_string()
                    } else {
                        parameters.join(", ")
                    },
                    type_of(&service.result.base_type, &service.result.data_shape),
                    service.from,
                    if service.has_script { "yes" } else { "no" }
                ));
            }
        }
        out.push('\n');
    }
    if shown < document.entities.len() {
        out.push_str(&format!(
            "... and {} more (--detail lists all).\n\n",
            document.entities.len() - shown
        ));
    }
    let inheritance: Vec<(String, String)> = document
        .entities
        .iter()
        .flat_map(|entity| {
            let template = entity
                .template
                .iter()
                .map(|name| format!("ThingTemplates/{name}"));
            let shapes = entity
                .shapes
                .iter()
                .map(|name| format!("ThingShapes/{name}"));
            template
                .chain(shapes)
                .map(|target| (entity.entity.clone(), target))
                .collect::<Vec<_>>()
        })
        .collect();
    out.push_str(&diagram(
        "Inheritance (an arrow points at the template or shape it names)",
        &inheritance,
    ));

    out.push_str("## DataShapes\n\n");
    if document.data_shapes.is_empty() {
        out.push_str("None.\n\n");
    }
    for shape in &document.data_shapes {
        out.push_str(&format!(
            "### {}\n\n- project: {}\n- fields: {}\n",
            shape.name,
            shape.project,
            shape.fields.len()
        ));
        if let Some(base) = &shape.base {
            out.push_str(&format!("- base: {base}\n"));
        }
        if !shape.used_by.is_empty() {
            out.push_str(&format!("- used by: {}\n", shape.used_by.join(", ")));
        }
        if detail && !shape.fields.is_empty() {
            out.push_str("\n| Field | Type | Description |\n| --- | --- | --- |\n");
            for field in &shape.fields {
                out.push_str(&format!(
                    "| `{}` | {} | {} |\n",
                    field.name,
                    field.base_type,
                    if field.description.is_empty() {
                        "-"
                    } else {
                        field.description.as_str()
                    }
                ));
            }
        }
        out.push('\n');
    }

    out.push_str("## Unresolved and review-only references\n\n");
    if document.review_references.is_empty() {
        out.push_str("None: every reference twaco found is declared or statically resolved.\n\n");
    } else {
        out.push_str(&format!(
            "{}: each is a string that looks like an entity's name but that no binding or `Things[...]` access confirms. A person decides whether it is a real reference.\n\n",
            plural(document.review_references.len(), "reference", "references")
        ));
        let shown = if detail {
            document.review_references.len()
        } else {
            SUMMARY_REVIEW.min(document.review_references.len())
        };
        for reference in &document.review_references[..shown] {
            let member = reference
                .member
                .as_deref()
                .map(|m| format!(".{m}"))
                .unwrap_or_default();
            let at = reference
                .at
                .as_deref()
                .map(|at| format!(" (`{at}`)"))
                .unwrap_or_default();
            out.push_str(&format!(
                "- {}{member} names {} ({}){at}\n",
                reference.from, reference.to, reference.kind
            ));
        }
        if shown < document.review_references.len() {
            out.push_str(&format!(
                "- ... and {} more (--detail lists all)\n",
                document.review_references.len() - shown
            ));
        }
        out.push('\n');
    }
    if !document.unparsed_scripts.is_empty() {
        out.push_str(&format!(
            "{} script(s) could not be parsed, so what they refer to is known only as a review-level mention: {}.\n\n",
            document.unparsed_scripts.len(),
            document.unparsed_scripts.join(", ")
        ));
    }

    out.push_str("## What this does not cover\n\n");
    out.push_str(&format!("- {}\n", document.limits));
    if document.complete {
        out.push_str("- every input was read\n");
    } else {
        out.push_str(&format!(
            "- **partial**: {} input(s) could not be read, so what depends on them is missing:\n",
            document.unreadable.len()
        ));
        for entry in &document.unreadable {
            if entry.why.is_empty() {
                out.push_str(&format!("  - {}\n", entry.what));
            } else {
                out.push_str(&format!("  - {}: {}\n", entry.what, entry.why));
            }
        }
    }
    out
}

/// Write `text` to `path` in one atomic replace. An existing file is refused unless `force`, so a
/// document someone edited by hand is not replaced by a regeneration without being asked.
pub fn write(path: &std::path::Path, text: &str, force: bool) -> Result<(), String> {
    if path.is_dir() {
        return Err(format!("{} is a directory", path.display()));
    }
    if path.exists() && !force {
        return Err(format!(
            "{} exists; pass --force to replace it",
            path.display()
        ));
    }
    if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    super::workspace::atomic_replace(path, text.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    fn bundled() -> Solution {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/acme-orders");
        Solution::load(&root.join("twaco.toml")).unwrap()
    }

    fn entity<'a>(document: &'a Document, name: &str) -> &'a EntityDoc {
        document
            .entities
            .iter()
            .find(|entity| entity.entity == name)
            .unwrap_or_else(|| panic!("{name} is not documented"))
    }

    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    #[test]
    fn the_document_holds_projects_inheritance_services_and_shapes() {
        let document = build(&bundled());
        assert!(document.complete);
        assert_eq!(document.solution, "Acme.Orders");
        let project = &document.projects[0];
        assert_eq!(project.deploy_order, Some(1));
        assert_eq!(project.entities["Things"], 5);

        let manager = entity(&document, "Things/Acme.Orders.Manager");
        assert_eq!(manager.inherits, ["Acme.Orders.Base_TT"]);
        assert_eq!(manager.services.len(), 5);
        assert!(manager
            .services
            .iter()
            .any(|service| service.name == "GetOrder"));
        assert_eq!(manager.refers_to.total(), 5);

        let audit_shape = entity(&document, "ThingShapes/Acme.Orders.Audit_TS");
        assert_eq!(audit_shape.implemented_by, ["Acme.Orders.Audit"]);
        let audit = entity(&document, "Things/Acme.Orders.Audit");
        assert_eq!(audit.shapes, ["Acme.Orders.Audit_TS"]);

        let line = document
            .data_shapes
            .iter()
            .find(|shape| shape.name == "Acme.Orders.OrderLine_DS")
            .unwrap();
        assert_eq!(line.fields.len(), 4);
        assert_eq!(line.used_by, ["Things/Acme.Orders.Manager"]);
    }

    #[test]
    fn what_is_only_a_guess_is_listed_for_a_person_to_judge() {
        let document = build(&bundled());
        assert_eq!(document.review_references.len(), 1);
        let guess = &document.review_references[0];
        assert_eq!(guess.from, "Things/Acme.Orders.Database");
        assert_eq!(guess.to, "DataShapes/Acme.Orders.OrderLine_DS");
        let text = render_markdown(&document, false);
        assert!(text.contains("## Unresolved and review-only references"));
        assert!(text.contains("Things/Acme.Orders.Database.GetDBInfo names"));
    }

    #[test]
    fn it_says_what_it_does_not_cover() {
        let text = render_markdown(&build(&bundled()), false);
        assert!(text.contains("permissions are not read yet"), "{text}");
        assert!(text.contains("every input was read"));
    }

    #[test]
    fn the_same_repository_gives_the_same_text() {
        let solution = bundled();
        assert_eq!(
            render_markdown(&build(&solution), true),
            render_markdown(&build(&solution), true)
        );
        assert_eq!(
            build(&solution).to_json(true),
            build(&solution).to_json(true)
        );
    }

    #[test]
    fn a_summary_counts_and_detail_lists() {
        let document = build(&bundled());
        let summary = render_markdown(&document, false);
        let detail = render_markdown(&document, true);
        assert!(!summary.contains("| Service |"));
        assert!(detail.contains("| `GetOrder` |"), "{detail}");
        assert!(detail.contains("| Field | Type | Description |"));
        assert!(!summary.contains("| Field | Type | Description |"));
        let json = document.to_json(false);
        let manager = json["entities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entity| entity["entity"] == "Things/Acme.Orders.Manager")
            .unwrap();
        assert_eq!(manager["service_count"], 5);
        assert!(manager.get("services").is_none());
        let json = document.to_json(true);
        assert!(json["entities"][0].get("services").is_some());
    }

    #[test]
    fn what_could_not_be_read_makes_the_document_say_it_is_partial() {
        let root = std::env::temp_dir().join(format!("twaco-docs-partial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/acme-orders");
        copy(&source, &root);
        std::fs::write(root.join("Things/Acme.Orders.Broken.xml"), "<not xml").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let document = build(&solution);
        let text = render_markdown(&document, false);
        let _ = std::fs::remove_dir_all(&root);
        assert!(!document.complete);
        assert!(text.contains("**partial**"), "{text}");
        assert!(text.contains("Acme.Orders.Broken"), "{text}");
    }

    #[test]
    fn writing_refuses_an_existing_file_unless_forced() {
        let dir = std::env::temp_dir().join(format!("twaco-docs-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let target = dir.join("nested/SOLUTION.md");
        write(&target, "one", false).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "one");
        let error = write(&target, "two", false).unwrap_err();
        assert!(error.contains("--force"), "{error}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "one");
        write(&target, "two", true).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "two");
        assert!(write(&dir, "x", true).unwrap_err().contains("directory"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
