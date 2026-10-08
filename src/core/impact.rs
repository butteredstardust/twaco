//! What changing one entity (or one service, property or field of it) would reach.
//!
//! The answer comes from the solution index: every Thing, template, shape, mashup and project that
//! refers to the entity, directly or through others, each reported at the strength of the weakest
//! reference on its way. It is read-only and offline. It is **not** a proof that nothing else is
//! affected: names built at run time and anything outside the repository are invisible, and the
//! report lists the inputs it could not read so the caller can see what it left out.

use super::config::Solution;
use super::entity_key::EntityKey;
use super::index::{Confidence, Dependent, DependentOptions, Index, Unreadable};
use super::workspace::WorkspaceError;
use serde::Serialize;
use std::collections::BTreeSet;
use std::fmt;

/// What the index cannot see, said wherever its answer is shown.
pub const LIMITS: &str =
    "references built at run time, and anything outside this repository, are not seen";

/// The most dependents a summary lists per confidence group; detail lists all.
const SUMMARY_LIMIT: usize = 20;

pub struct Request {
    /// `Collection/Name`, a full name, or its last dotted segment.
    pub entity: String,
    /// A service, property or field of the entity, to ask only about what names it.
    pub member: Option<String>,
    /// The weakest reference to follow.
    pub min: Confidence,
    /// How many references away to look.
    pub depth: Option<usize>,
}

#[derive(Debug)]
pub enum ImpactError {
    Entity(WorkspaceError),
}

impl fmt::Display for ImpactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImpactError::Entity(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ImpactError {}

impl From<WorkspaceError> for ImpactError {
    fn from(error: WorkspaceError) -> Self {
        ImpactError::Entity(error)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub structural: usize,
    pub resolved: usize,
    pub review: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProjectAt {
    /// Its place in the deploy order, from 1; none when the order does not name it.
    pub deploy_order: Option<usize>,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    /// `Collection/Name`.
    pub entity: String,
    pub project: String,
    pub member: Option<String>,
    pub min_confidence: Confidence,
    pub depth: Option<usize>,
    pub dependents: Vec<Dependent>,
    pub counts: Counts,
    /// Dependents that reach the entity only through inheritance: they have what it has.
    pub inherited_by: Vec<String>,
    /// The projects that hold the entity or any dependent, in deploy order.
    pub projects: Vec<ProjectAt>,
    /// Whether every input was read. A report from an incomplete index misses what depends on the
    /// inputs it could not read.
    pub complete: bool,
    pub unreadable: Vec<Unreadable>,
    pub unparsed_scripts: Vec<String>,
    pub notes: Vec<String>,
    pub limits: &'static str,
}

/// Ask the index. Offline, read-only, and never writes.
pub fn run(solution: &Solution, request: &Request) -> Result<Report, ImpactError> {
    let index = Index::build(solution);
    let key = index.resolve(&request.entity)?;
    let options = DependentOptions {
        member: request.member.clone(),
        min: request.min,
        max_depth: request.depth,
    };
    let dependents = index.dependents(&key, &options);
    let mut counts = Counts::default();
    for dependent in &dependents {
        match dependent.confidence {
            Confidence::Structural => counts.structural += 1,
            Confidence::Resolved => counts.resolved += 1,
            Confidence::Review => counts.review += 1,
        }
    }
    let inherited_by = dependents
        .iter()
        .filter(|dependent| {
            !dependent.path.is_empty()
                && dependent.path.iter().all(|step| step.kind.is_inheritance())
        })
        .map(|dependent| dependent.label.clone())
        .collect();
    let mut labels: Vec<String> = dependents
        .iter()
        .map(|dependent| dependent.label.clone())
        .collect();
    labels.push(key.to_string());
    let projects = index
        .projects_of(&labels)
        .into_iter()
        .map(|(place, name)| ProjectAt {
            deploy_order: (place != usize::MAX).then_some(place + 1),
            name,
        })
        .collect();
    let mut notes = Vec::new();
    if let Some(member) = &request.member {
        if !declares(&index, &key, member) {
            notes.push(format!(
                "{key} declares no service named {member}; it may be a property or field, or inherited from outside the repository, so what is shown is what names it"
            ));
        }
    }
    if !index.unparsed_scripts().is_empty() {
        notes.push(format!(
            "{} script(s) could not be parsed; what they refer to is known only as a review-level mention",
            index.unparsed_scripts().len()
        ));
    }
    let project = index
        .node(&key)
        .map(|node| node.project.clone())
        .unwrap_or_default();
    Ok(Report {
        entity: key.to_string(),
        project,
        member: request.member.clone(),
        min_confidence: request.min,
        depth: request.depth,
        dependents,
        counts,
        inherited_by,
        projects,
        complete: index.is_complete(),
        unreadable: index.unreadable().to_vec(),
        unparsed_scripts: index.unparsed_scripts().to_vec(),
        notes,
        limits: LIMITS,
    })
}

/// Whether `entity`, or anything it inherits, declares a service called `member`.
fn declares(index: &Index, entity: &EntityKey, member: &str) -> bool {
    let own = index
        .node(entity)
        .is_some_and(|node| node.services.contains(member));
    own || index.inheritance_names(entity).iter().any(|name| {
        ["ThingTemplates", "ThingShapes"].iter().any(|collection| {
            EntityKey::new(*collection, name)
                .ok()
                .and_then(|key| index.node(&key))
                .is_some_and(|node| node.services.contains(member))
        })
    })
}

impl Report {
    /// The report as JSON. The chain to each dependent is included only when `detail` asks.
    pub fn to_json(&self, detail: bool) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("a report serialises");
        if !detail {
            if let Some(list) = value["dependents"].as_array_mut() {
                for dependent in list {
                    if let Some(object) = dependent.as_object_mut() {
                        object.remove("path");
                    }
                }
            }
        }
        value
    }
}

fn subject(report: &Report) -> String {
    match &report.member {
        Some(member) => format!("{}.{member}", report.entity),
        None => report.entity.clone(),
    }
}

fn line(dependent: &Dependent) -> String {
    let members = if dependent.members.is_empty() {
        String::new()
    } else {
        format!("; {}", dependent.members.join(", "))
    };
    format!(
        "  {}  (depth {}{members})",
        dependent.label, dependent.depth
    )
}

/// The report as text: a summary first, then the dependents by confidence. A summary lists at most
/// twenty per group; `detail` lists all and the chain of references to each.
pub fn render_text(report: &Report, detail: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!("impact of {}\n", subject(report)));
    out.push_str(&format!("  project     {}\n", report.project));
    if report.dependents.is_empty() {
        out.push_str(&format!(
            "  dependents  none at {} confidence or stronger\n",
            report.min_confidence.word()
        ));
    } else {
        out.push_str(&format!(
            "  dependents  {}: {} structural, {} resolved, {} review\n",
            report.dependents.len(),
            report.counts.structural,
            report.counts.resolved,
            report.counts.review
        ));
    }
    if !report.projects.is_empty() {
        let projects: Vec<String> = report
            .projects
            .iter()
            .map(|project| match project.deploy_order {
                Some(place) => format!("{} (deploy {place})", project.name),
                None => project.name.clone(),
            })
            .collect();
        out.push_str(&format!("  projects    {}\n", projects.join(", ")));
    }
    if !report.inherited_by.is_empty() {
        out.push_str(&format!(
            "  inherited by {}\n",
            report.inherited_by.join(", ")
        ));
    }
    for (confidence, heading) in [
        (
            Confidence::Structural,
            "structural: declared in the XML or twaco.toml",
        ),
        (
            Confidence::Resolved,
            "resolved: a static name in a script or a mashup binding",
        ),
        (
            Confidence::Review,
            "review: a string that looks like the name; a person decides",
        ),
    ] {
        let group: Vec<&Dependent> = report
            .dependents
            .iter()
            .filter(|dependent| dependent.confidence == confidence)
            .collect();
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{heading}\n"));
        let shown = if detail {
            group.len()
        } else {
            SUMMARY_LIMIT.min(group.len())
        };
        for dependent in &group[..shown] {
            out.push_str(&line(dependent));
            out.push('\n');
            if detail {
                for step in &dependent.path {
                    let from = step
                        .from_member
                        .as_deref()
                        .map(|m| format!(".{m}"))
                        .unwrap_or_default();
                    let to = step
                        .to_member
                        .as_deref()
                        .map(|m| format!(".{m}"))
                        .unwrap_or_default();
                    let at = step
                        .at
                        .as_deref()
                        .map(|at| format!("  [{at}]"))
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "      {}{from} -> {}{to}  ({}){at}\n",
                        step.from,
                        step.to,
                        step.kind.word()
                    ));
                }
            }
        }
        if shown < group.len() {
            out.push_str(&format!(
                "  ... and {} more (--detail lists all)\n",
                group.len() - shown
            ));
        }
    }
    out.push('\n');
    out.push_str(&format!("limits: {}\n", report.limits));
    if !report.complete {
        out.push_str(&format!(
            "partial: {} input(s) could not be read, so what depends on them is missing:\n",
            report.unreadable.len()
        ));
        for entry in &report.unreadable {
            if entry.why.is_empty() {
                out.push_str(&format!("  {}\n", entry.what));
            } else {
                out.push_str(&format!("  {}: {}\n", entry.what, entry.why));
            }
        }
    }
    for note in &report.notes {
        out.push_str(&format!("note: {note}\n"));
    }
    out
}

/// The dependents as a Graphviz graph: an arrow from each thing that refers to another to the
/// thing it refers to, solid for structural references, dashed for resolved, dotted for review.
pub fn render_dot(report: &Report) -> String {
    let quote = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
    let mut out =
        String::from("digraph impact {\n  rankdir=LR;\n  node [shape=box, fontsize=10];\n");
    // A graph cannot say "and more I could not see" by what it draws, so it says it in words.
    let mut caveats: Vec<String> = Vec::new();
    if !report.complete {
        caveats.push(format!(
            "partial: {} input(s) could not be read",
            report.unreadable.len()
        ));
    }
    if !report.unparsed_scripts.is_empty() {
        caveats.push(format!(
            "{} script(s) not parsed: their references are review-level only",
            report.unparsed_scripts.len()
        ));
    }
    if !caveats.is_empty() {
        out.push_str(&format!(
            "  labelloc=t;\n  label={};\n",
            quote(&caveats.join("\\n"))
        ));
        for entry in &report.unreadable {
            out.push_str(&format!(
                "  // unreadable: {}\n",
                entry.what.replace(['\n', '\r'], " ")
            ));
        }
        for script in &report.unparsed_scripts {
            out.push_str(&format!(
                "  // unparsed: {}\n",
                script.replace(['\n', '\r'], " ")
            ));
        }
    }
    out.push_str(&format!(
        "  {} [style=filled, fillcolor=\"#dbe6ff\"];\n",
        quote(&report.entity)
    ));
    let mut edges: BTreeSet<(String, String, String, &'static str)> = BTreeSet::new();
    for dependent in &report.dependents {
        for step in &dependent.path {
            let style = match step.kind.confidence() {
                Confidence::Structural => "solid",
                Confidence::Resolved => "dashed",
                Confidence::Review => "dotted",
            };
            let label = match &step.to_member {
                Some(member) => format!("{} {member}", step.kind.word()),
                None => step.kind.word().to_string(),
            };
            edges.insert((step.from.clone(), step.to.clone(), label, style));
        }
    }
    for (from, to, label, style) in edges {
        out.push_str(&format!(
            "  {} -> {} [label={}, style={style}];\n",
            quote(&from),
            quote(&to),
            quote(&label)
        ));
    }
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn bundled() -> Solution {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/acme-orders");
        Solution::load(&root.join("twaco.toml")).unwrap()
    }

    fn ask(entity: &str, member: Option<&str>, min: Confidence) -> Report {
        run(
            &bundled(),
            &Request {
                entity: entity.to_string(),
                member: member.map(str::to_string),
                min,
                depth: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn a_service_change_reaches_its_callers_and_the_mashup_that_calls_them() {
        let report = ask("Audit", Some("Record"), Confidence::Review);
        assert_eq!(report.entity, "Things/Acme.Orders.Audit");
        let labels: Vec<&str> = report.dependents.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Things/Acme.Orders.Manager",
                "Mashups/Acme.Orders.Dashboard"
            ]
        );
        assert_eq!(
            report.counts,
            Counts {
                structural: 0,
                resolved: 2,
                review: 0
            }
        );
        assert_eq!(
            report.projects,
            [ProjectAt {
                deploy_order: Some(1),
                name: "Acme.Orders".into()
            }]
        );
        assert!(report.complete && report.unreadable.is_empty());
        let text = render_text(&report, false);
        assert!(
            text.starts_with("impact of Things/Acme.Orders.Audit.Record\n"),
            "{text}"
        );
        assert!(
            text.contains("2 resolved") && text.contains("Acme.Orders (deploy 1)"),
            "{text}"
        );
        assert!(
            text.contains("limits: references built at run time"),
            "{text}"
        );
        assert!(!text.contains(" -> "), "no chains without detail: {text}");
        let detailed = render_text(&report, true);
        assert!(
            detailed.contains("Things/Acme.Orders.Manager.GetOrder -> Things/Acme.Orders.Audit.Record  (script_reference)"),
            "{detailed}"
        );
    }

    #[test]
    fn a_template_is_inherited_by_what_names_it_and_a_summary_leaves_the_chains_out_of_json() {
        let report = ask("Base_TT", None, Confidence::Review);
        assert_eq!(report.inherited_by, ["Things/Acme.Orders.Manager"]);
        assert_eq!(report.counts.structural, 1);
        let compact = report.to_json(false);
        assert!(compact["dependents"][0].get("path").is_none(), "{compact}");
        assert_eq!(
            compact["dependents"][0]["label"],
            "Things/Acme.Orders.Manager"
        );
        assert_eq!(compact["counts"]["structural"], 1);
        assert_eq!(compact["complete"], true);
        let full = report.to_json(true);
        assert_eq!(full["dependents"][0]["path"][0]["kind"], "template");
    }

    #[test]
    fn the_minimum_confidence_leaves_the_weaker_references_out() {
        let everything = ask("OrderLine_DS", None, Confidence::Review);
        let strong = ask("OrderLine_DS", None, Confidence::Structural);
        assert!(strong.dependents.len() < everything.dependents.len());
        assert!(strong
            .dependents
            .iter()
            .all(|d| d.confidence == Confidence::Structural));
        assert!(
            everything
                .dependents
                .iter()
                .any(|d| d.label == "Things/Acme.Orders.Database"
                    && d.confidence == Confidence::Review),
            "the GetDBInfo script names the shape as a string"
        );
        let limited = run(
            &bundled(),
            &Request {
                entity: "OrderLine_DS".into(),
                member: None,
                min: Confidence::Review,
                depth: Some(1),
            },
        )
        .unwrap();
        assert!(limited.dependents.iter().all(|d| d.depth == 1));
    }

    #[test]
    fn dot_draws_an_arrow_from_each_referrer_in_the_style_of_its_confidence() {
        let dot = render_dot(&ask("Audit", Some("Record"), Confidence::Review));
        assert!(dot.starts_with("digraph impact {"), "{dot}");
        assert!(
            dot.contains(r#""Things/Acme.Orders.Audit" [style=filled"#),
            "{dot}"
        );
        assert!(
            dot.contains(r#""Things/Acme.Orders.Manager" -> "Things/Acme.Orders.Audit" [label="script_reference Record", style=dashed];"#),
            "{dot}"
        );
        assert!(
            dot.contains(r#""Mashups/Acme.Orders.Dashboard" -> "Things/Acme.Orders.Manager" [label="mashup_binding GetOrder", style=dashed];"#),
            "{dot}"
        );
        assert!(dot.trim_end().ends_with('}'));
    }

    #[test]
    fn an_unknown_name_is_an_error_and_a_member_nobody_declares_is_a_note() {
        let missing = run(
            &bundled(),
            &Request {
                entity: "Nope".into(),
                member: None,
                min: Confidence::Review,
                depth: None,
            },
        );
        assert!(matches!(
            missing,
            Err(ImpactError::Entity(WorkspaceError::UnknownEntity { .. }))
        ));
        let note = ask("Audit", Some("Imaginary"), Confidence::Review);
        assert!(
            note.notes
                .iter()
                .any(|n| n.contains("declares no service named Imaginary")),
            "{:?}",
            note.notes
        );
        assert!(ask("Audit", Some("Record"), Confidence::Review)
            .notes
            .is_empty());
        // The Manager's own services are declared, and Describe is inherited from its template.
        assert!(ask("Manager", Some("GetOrder"), Confidence::Review)
            .notes
            .is_empty());
        assert!(ask("Manager", Some("Describe"), Confidence::Review)
            .notes
            .is_empty());
    }

    #[test]
    fn a_thing_nothing_refers_to_has_no_dependents_and_says_so() {
        let report = ask("Dashboard", None, Confidence::Review);
        assert!(report.dependents.is_empty());
        let text = render_text(&report, false);
        assert!(
            text.contains("dependents  none at review confidence or stronger"),
            "{text}"
        );
    }

    #[test]
    fn a_graph_of_an_incomplete_index_says_so() {
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
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-impact-dot-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/acme-orders");
        copy(&source, &root);
        let complete = render_dot(&ask("Audit", Some("Record"), Confidence::Review));
        assert!(!complete.contains("labelloc"), "{complete}");
        std::fs::write(root.join("Things/Acme.Orders.Broken.xml"), "<not xml").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let report = run(
            &solution,
            &Request {
                entity: "Audit".into(),
                member: Some("Record".into()),
                min: Confidence::Review,
                depth: None,
            },
        )
        .unwrap();
        let dot = render_dot(&report);
        assert!(dot.contains("label=\"partial: 1 input(s)"), "{dot}");
        assert!(dot.contains("// unreadable: "), "{dot}");
    }
}
