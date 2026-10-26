//! What changing one entity (or one service, property or field of it) would reach.
//!
//! The answer comes from the solution index: every Thing, template, shape, mashup and project that
//! refers to the entity, directly or through others, each reported at the strength of the weakest
//! reference on its way. It is read-only and offline. It is **not** a proof that nothing else is
//! affected: names built at run time and anything outside the repository are invisible, and the
//! report lists the inputs it could not read so the caller can see what it left out.
//!
//! Asked to, it also asks the server, which sees what the repository cannot: entities of other
//! projects, extensions and anything never exported. The server's graph is structural only and
//! one step deep, so it is reported beside the index's answer, never merged into it.

use super::codes::{Coded, ErrorCode};
use super::config::Solution;
use super::entity::delete;
use super::entity::key::{EntityKey, ServiceTarget};
use super::index::{Confidence, Dependent, DependentOptions, Index, Unreadable};
use super::server::{Client, ServerError};
use super::workspace::WorkspaceError;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// What the index cannot see, said wherever its answer is shown.
pub(crate) const LIMITS: &str =
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

#[derive(Debug, thiserror::Error)]
pub enum ImpactError {
    #[error("{0}")]
    Entity(WorkspaceError),
}

impl Coded for ImpactError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Entity(error) => error.code(),
        }
    }
}

/// What the server is asked, as a trait so it is tested offline.
pub trait ServerRemote {
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError>;
    /// `GetIncomingDependencies`: `Collection/Name` of each entity that refers to `key`.
    fn incoming(&self, key: &EntityKey) -> Result<Vec<(String, String)>, ServerError>;
    /// `GetImplementingThings` of a template or shape: the names of the Things that have it.
    fn implementing(&self, key: &EntityKey) -> Result<Vec<String>, ServerError>;
}

impl ServerRemote for Client {
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
        self.entity_exists(key)
    }

    fn incoming(&self, key: &EntityKey) -> Result<Vec<(String, String)>, ServerError> {
        Ok(delete::incoming_dependencies(self, key)?
            .into_iter()
            .map(|dependent| (dependent.collection, dependent.name))
            .collect())
    }

    fn implementing(&self, key: &EntityKey) -> Result<Vec<String>, ServerError> {
        let target = ServiceTarget::Entity(key.clone());
        let reply = self
            .call_service(
                &target,
                "GetImplementingThings",
                &serde_json::json!({}),
                Duration::from_secs(120),
            )?
            .unwrap_or_else(|| serde_json::json!({ "rows": [] }));
        let rows = reply.get("rows").and_then(Value::as_array).ok_or_else(|| {
            ServerError::InvalidResponse {
                url: format!("{target}/Services/GetImplementingThings"),
                why: "expected an InfoTable with rows".to_string(),
            }
        })?;
        rows.iter()
            .map(|row| {
                row.get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| ServerError::InvalidResponse {
                        url: format!("{target}/Services/GetImplementingThings"),
                        why: "a row has no string name".to_string(),
                    })
            })
            .collect()
    }
}

/// One entity the server says refers to the entity asked about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ServerDependent {
    /// `Collection/Name`.
    pub(crate) entity: String,
    /// How the server knows: `dependency` (its dependency graph), `implements` (a Thing that has
    /// the template or shape), or both.
    pub(crate) via: Vec<&'static str>,
    pub(crate) in_repository: bool,
}

/// The server's answer, beside the repository's.
#[derive(Debug, Default, Serialize)]
pub(crate) struct ServerSide {
    pub(crate) dependents: Vec<ServerDependent>,
    /// How many of them the repository does not hold: what the offline answer cannot see.
    pub(crate) outside_repository: usize,
    pub(crate) notes: Vec<String>,
    pub(crate) limits: &'static str,
}

impl From<WorkspaceError> for ImpactError {
    fn from(error: WorkspaceError) -> Self {
        ImpactError::Entity(error)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Counts {
    pub(crate) structural: usize,
    pub(crate) resolved: usize,
    pub(crate) review: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ProjectAt {
    /// Its place in the deploy order, from 1; none when the order does not name it.
    pub(crate) deploy_order: Option<usize>,
    pub(crate) name: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    /// `Collection/Name`.
    pub(crate) entity: String,
    pub(crate) project: String,
    pub(crate) member: Option<String>,
    pub(crate) min_confidence: Confidence,
    pub(crate) depth: Option<usize>,
    pub(crate) dependents: Vec<Dependent>,
    pub(crate) counts: Counts,
    /// Dependents that reach the entity only through inheritance: they have what it has.
    pub(crate) inherited_by: Vec<String>,
    /// The projects that hold the entity or any dependent, in deploy order.
    pub(crate) projects: Vec<ProjectAt>,
    /// Whether every input was read. A report from an incomplete index misses what depends on the
    /// inputs it could not read.
    pub(crate) complete: bool,
    pub(crate) unreadable: Vec<Unreadable>,
    pub(crate) unparsed_scripts: Vec<String>,
    pub(crate) notes: Vec<String>,
    pub(crate) limits: &'static str,
    /// What the server says, when it was asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) server: Option<ServerSide>,
}

/// Ask the index. Offline, read-only, and never writes.
pub fn run(solution: &Solution, request: &Request) -> Result<Report, ImpactError> {
    run_with(solution, request, None)
}

/// Ask the index and, given a server, the server too. Read-only on both.
pub fn run_with(
    solution: &Solution,
    request: &Request,
    remote: Option<&dyn ServerRemote>,
) -> Result<Report, ImpactError> {
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
        server: remote
            .map(|remote| ask_server(&index, &key, request, remote))
            .transpose()?,
        notes,
        limits: LIMITS,
    })
}

/// The server's one-step answer for `key`, each dependent marked by whether the repository has it.
fn ask_server(
    index: &Index,
    key: &EntityKey,
    request: &Request,
    remote: &dyn ServerRemote,
) -> Result<ServerSide, ImpactError> {
    let mut side = ServerSide {
        limits: delete::DEPENDENCY_LIMIT,
        ..ServerSide::default()
    };
    // The repository's answer is already made; a server that cannot answer is said beside it
    // rather than taking it away.
    let unanswered = |error: ServerError| {
        format!("the server could not be asked ({error}); what is shown is the repository's answer only")
    };
    match remote.exists(key) {
        Ok(true) => {}
        Ok(false) => {
            side.notes.push(format!(
                "the server has no {key}, so it knows of nothing that depends on it"
            ));
            return Ok(side);
        }
        Err(error) => {
            side.notes.push(unanswered(error));
            return Ok(side);
        }
    }
    let mut found: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    match remote.incoming(key) {
        Ok(dependents) => {
            for (collection, name) in dependents {
                found
                    .entry(format!("{collection}/{name}"))
                    .or_default()
                    .push("dependency");
            }
        }
        Err(error) if delete::is_disabled_thing(key, &error) => side.notes.push(format!(
            "{key} is disabled on the server, which then does not say what depends on it; enable it and ask again"
        )),
        Err(error) => {
            side.notes.push(unanswered(error));
            return Ok(side);
        }
    }
    if matches!(key.collection(), "ThingTemplates" | "ThingShapes") {
        match remote.implementing(key) {
            Ok(names) => {
                for name in names {
                    let via = found.entry(format!("Things/{name}")).or_default();
                    if !via.contains(&"implements") {
                        via.push("implements");
                    }
                }
            }
            Err(error) => side.notes.push(format!(
                "the server did not say which Things have {key} ({error}); they are missing here"
            )),
        }
    }
    side.dependents = found
        .into_iter()
        .map(|(entity, via)| {
            let in_repository = entity
                .split_once('/')
                .and_then(|(collection, name)| EntityKey::new(collection, name).ok())
                .is_some_and(|key| index.node(&key).is_some());
            ServerDependent {
                entity,
                via,
                in_repository,
            }
        })
        .collect();
    side.outside_repository = side
        .dependents
        .iter()
        .filter(|dependent| !dependent.in_repository)
        .count();
    if request.member.is_some() {
        side.notes.push(
            "the server's dependencies are whole entities; the member does not narrow them"
                .to_string(),
        );
    }
    if key.collection() == "ThingTemplates"
        && side
            .dependents
            .iter()
            .any(|dependent| dependent.entity.starts_with("ThingTemplates/"))
    {
        side.notes.push(
            "the Things of a template derived from this one may be listed under that template only; ask about each one listed here".to_string(),
        );
    }
    Ok(side)
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
    if let Some(server) = &report.server {
        out.push_str(&format!(
            "  server      {}: {} not in the repository\n",
            server.dependents.len(),
            server.outside_repository
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
    if let Some(server) = &report.server {
        if !server.dependents.is_empty() {
            out.push_str("\nserver: what the server says refers to it, one step away\n");
            let shown = if detail {
                server.dependents.len()
            } else {
                SUMMARY_LIMIT.min(server.dependents.len())
            };
            for dependent in &server.dependents[..shown] {
                let outside = if dependent.in_repository {
                    ""
                } else {
                    ", not in the repository"
                };
                out.push_str(&format!(
                    "  {}  ({}{outside})\n",
                    dependent.entity,
                    dependent.via.join(", ")
                ));
            }
            if shown < server.dependents.len() {
                out.push_str(&format!(
                    "  ... and {} more (--detail lists all)\n",
                    server.dependents.len() - shown
                ));
            }
        }
    }
    out.push('\n');
    out.push_str(&format!("limits: {}\n", report.limits));
    if let Some(server) = &report.server {
        out.push_str(&format!("server limits: {}\n", server.limits));
        for note in &server.notes {
            out.push_str(&format!("note: {note}\n"));
        }
    }
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

    struct Server {
        exists: bool,
        incoming: Result<Vec<(&'static str, &'static str)>, u16>,
        implementing: Vec<&'static str>,
        asked: std::cell::RefCell<Vec<String>>,
    }

    impl Server {
        fn new(
            incoming: Vec<(&'static str, &'static str)>,
            implementing: Vec<&'static str>,
        ) -> Self {
            Server {
                exists: true,
                incoming: Ok(incoming),
                implementing,
                asked: Default::default(),
            }
        }
    }

    impl ServerRemote for Server {
        fn exists(&self, _: &EntityKey) -> Result<bool, ServerError> {
            Ok(self.exists)
        }

        fn incoming(&self, key: &EntityKey) -> Result<Vec<(String, String)>, ServerError> {
            self.asked.borrow_mut().push(format!("incoming {key}"));
            match &self.incoming {
                Ok(rows) => Ok(rows
                    .iter()
                    .map(|(collection, name)| (collection.to_string(), name.to_string()))
                    .collect()),
                Err(status) => Err(ServerError::Http {
                    method: crate::core::server::Method::Post,
                    status: *status,
                    url: format!("{key}/Services/GetIncomingDependencies"),
                    body: format!("Thing [{}] is not enabled", key.name()),
                }),
            }
        }

        fn implementing(&self, key: &EntityKey) -> Result<Vec<String>, ServerError> {
            self.asked.borrow_mut().push(format!("implementing {key}"));
            Ok(self
                .implementing
                .iter()
                .map(|name| name.to_string())
                .collect())
        }
    }

    fn ask_with(entity: &str, member: Option<&str>, server: &Server) -> Report {
        run_with(
            &bundled(),
            &Request {
                entity: entity.to_string(),
                member: member.map(str::to_string),
                min: Confidence::Review,
                depth: None,
            },
            Some(server),
        )
        .unwrap()
    }

    #[test]
    fn the_server_adds_what_the_repository_cannot_see_beside_the_index() {
        let server = Server::new(
            vec![
                ("Things", "Acme.Orders.Manager"),
                ("Mashups", "Other.Team.Board"),
            ],
            vec!["Acme.Orders.Manager", "Other.Team.Pump"],
        );
        let report = ask_with("Base_TT", None, &server);
        // The offline answer is unchanged.
        assert_eq!(report.inherited_by, ["Things/Acme.Orders.Manager"]);
        let side = report.server.as_ref().unwrap();
        assert_eq!(
            side.dependents,
            [
                ServerDependent {
                    entity: "Mashups/Other.Team.Board".into(),
                    via: vec!["dependency"],
                    in_repository: false,
                },
                ServerDependent {
                    entity: "Things/Acme.Orders.Manager".into(),
                    via: vec!["dependency", "implements"],
                    in_repository: true,
                },
                ServerDependent {
                    entity: "Things/Other.Team.Pump".into(),
                    via: vec!["implements"],
                    in_repository: false,
                },
            ]
        );
        assert_eq!(side.outside_repository, 2);
        assert_eq!(
            *server.asked.borrow(),
            [
                "incoming ThingTemplates/Acme.Orders.Base_TT",
                "implementing ThingTemplates/Acme.Orders.Base_TT"
            ]
        );
        let text = render_text(&report, false);
        assert!(
            text.contains("  server      3: 2 not in the repository\n"),
            "{text}"
        );
        assert!(
            text.contains("  Things/Other.Team.Pump  (implements, not in the repository)\n"),
            "{text}"
        );
        assert!(
            text.contains("server limits: GetIncomingDependencies"),
            "{text}"
        );
        let json = report.to_json(false);
        assert_eq!(json["server"]["outside_repository"], 2);
        assert_eq!(json["server"]["dependents"][1]["via"][1], "implements");
        // Without the server, the report says nothing about it.
        assert!(ask("Base_TT", None, Confidence::Review)
            .to_json(false)
            .get("server")
            .is_none());
    }

    #[test]
    fn the_server_is_asked_for_implementing_things_only_of_templates_and_shapes() {
        let server = Server::new(vec![], vec!["Ignored"]);
        let report = ask_with("Audit", Some("Record"), &server);
        assert_eq!(
            *server.asked.borrow(),
            ["incoming Things/Acme.Orders.Audit"]
        );
        let side = report.server.unwrap();
        assert!(side.dependents.is_empty());
        assert!(
            side.notes
                .iter()
                .any(|note| note.contains("member does not narrow")),
            "{:?}",
            side.notes
        );
    }

    #[test]
    fn an_entity_the_server_lacks_or_a_disabled_thing_is_said_not_shown_as_no_dependents() {
        let mut server = Server::new(vec![], vec![]);
        server.exists = false;
        let side = ask_with("Audit", None, &server).server.unwrap();
        assert!(server.asked.borrow().is_empty());
        assert_eq!(
            side.notes,
            ["the server has no Things/Acme.Orders.Audit, so it knows of nothing that depends on it"]
        );
        let mut server = Server::new(vec![], vec![]);
        server.incoming = Err(503);
        let side = ask_with("Audit", None, &server).server.unwrap();
        assert!(
            side.notes[0].contains("is disabled on the server"),
            "{:?}",
            side.notes
        );
        let mut server = Server::new(vec![], vec![]);
        server.incoming = Err(500);
        // Any other failure keeps the repository's answer and says the server's is missing.
        let report = ask_with("Audit", None, &server);
        assert!(!report.dependents.is_empty(), "the offline answer is kept");
        let side = report.server.unwrap();
        assert!(side.dependents.is_empty());
        assert!(
            side.notes[0].starts_with("the server could not be asked (")
                && side.notes[0].ends_with("what is shown is the repository's answer only"),
            "{:?}",
            side.notes
        );
    }

    #[test]
    fn a_template_is_inherited_by_what_names_it_and_a_summary_leaves_the_chains_out_of_json() {
        let report = ask("Base_TT", None, Confidence::Review);
        assert_eq!(report.inherited_by, ["Things/Acme.Orders.Manager"]);
        // The Manager, and the plant Network holding it.
        assert_eq!(report.counts.structural, 2);
        let compact = report.to_json(false);
        assert!(compact["dependents"][0].get("path").is_none(), "{compact}");
        assert_eq!(
            compact["dependents"][0]["label"],
            "Things/Acme.Orders.Manager"
        );
        assert_eq!(compact["counts"]["structural"], 2);
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
