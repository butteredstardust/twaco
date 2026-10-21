//! Offline public contracts, compared by entity identity and declared members only.

use super::config::Solution;
use super::datashape::Aspect;
use super::entity::key::EntityKey;
use super::index::{Confidence, EdgeKind, Index};
use super::ledger::{self, Kind, Ledger};
use super::{mashup, types, workspace};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

const LIMITS: &str = "Callers outside this repository are invisible.";

#[derive(Debug, Serialize, JsonSchema)]
pub struct Change {
    pub kind: String,
    pub entity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    pub change: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Value>,
    pub callers: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Report {
    pub summary: String,
    pub breaking: Vec<Change>,
    pub compatible: Vec<Change>,
    pub skipped: Vec<String>,
    pub limits: String,
}

#[derive(Default)]
struct Contract {
    entities: BTreeMap<String, Entity>,
    skipped: Vec<String>,
}

#[derive(Default)]
struct Entity {
    project: String,
    members: BTreeMap<(String, String), Value>,
}

impl Entity {
    fn add(&mut self, kind: &str, name: &str, value: Value) {
        self.members.insert((kind.into(), name.into()), value);
    }
}

fn typed(value: &types::TypedValue) -> Value {
    json!({"baseType": value.base_type, "dataShape": value.data_shape})
}

fn load(solution: &Solution, project: Option<&str>) -> Contract {
    let (model, skipped) = types::load_model(solution);
    let mut out = Contract {
        skipped,
        ..Contract::default()
    };
    let discovered = workspace::discover(solution);
    let projects: BTreeMap<_, _> = discovered
        .entities
        .iter()
        .map(|file| {
            (
                format!("{}/{}", file.info.collection, file.info.name),
                file.found_under.clone(),
            )
        })
        .collect();
    for entity in model.entities {
        if !matches!(
            entity.collection.as_str(),
            "Things" | "ThingTemplates" | "ThingShapes"
        ) {
            continue;
        }
        let mut contract = Entity {
            project: entity.project,
            ..Entity::default()
        };
        for member in entity.members {
            match member {
                types::Member::Service(service) => {
                    contract.add("service", &service.name, json!({}));
                    contract.add("result", &service.name, typed(&service.result));
                    for param in service.parameters {
                        let mut value = typed(&param.value);
                        value["required"] = json!(param.required);
                        value["default"] = json!(param.default);
                        contract.add("param", &format!("{}.{}", service.name, param.name), value);
                    }
                }
                types::Member::Property(property) => {
                    contract.add("property", &property.name, typed(&property.value))
                }
            }
        }
        for (name, shape) in entity.events {
            contract.add("event", &name, json!({"dataShape": shape}));
        }
        out.entities
            .insert(format!("{}/{}", entity.collection, entity.name), contract);
    }
    for shape in model.data_shapes {
        let key = format!("DataShapes/{}", shape.name);
        let mut contract = Entity {
            project: projects.get(&key).cloned().unwrap_or_default(),
            ..Entity::default()
        };
        contract.add("base_data_shape", "", json!(shape.base_data_shape));
        for field in shape.fields {
            let data_shape = match field.aspects.get("dataShape") {
                Some(Aspect::Text(text)) if !text.is_empty() => Some(text),
                _ => None,
            };
            let primary = field.aspects.get("isPrimaryKey") == Some(&Aspect::Bool(true));
            contract.add("field", &field.name, json!({"baseType": field.base_type, "dataShape": data_shape, "isPrimaryKey": primary}));
        }
        out.entities.insert(key, contract);
    }
    for file in discovered
        .entities
        .iter()
        .filter(|file| file.info.collection == "Mashups")
    {
        let params = std::fs::read(&file.path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| mashup::describe::mashup_parameters(&bytes));
        match params {
            Ok(params) => {
                let mut entity = Entity {
                    project: file.found_under.clone(),
                    ..Entity::default()
                };
                for param in params {
                    entity.add(
                        "mashup_parameter",
                        &param.name,
                        json!({"baseType": param.base_type}),
                    );
                }
                out.entities
                    .insert(format!("Mashups/{}", file.info.name), entity);
            }
            Err(error) => out
                .skipped
                .push(format!("{}: {error}", file.path.display())),
        }
    }
    out.entities
        .retain(|_, entity| project.is_none_or(|name| name == entity.project));
    out
}

fn git(root: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run git: {error}; install git and put it on PATH"))
}

/// Compare with the revision's own solution configuration. Never reads a profile or writes the workspace.
pub fn diff(solution: &Solution, against: &str, project: Option<&str>) -> Result<Report, String> {
    if against.trim().is_empty() {
        return Err("--against needs a git revision; try origin/main".into());
    }
    let top = git(&solution.root, &["rev-parse", "--show-toplevel"])?;
    if !top.status.success() {
        return Err(format!(
            "{} is not a git repository: {}",
            solution.root.display(),
            String::from_utf8_lossy(&top.stderr).trim()
        ));
    }
    let top = std::fs::canonicalize(String::from_utf8_lossy(&top.stdout).trim())
        .map_err(|error| error.to_string())?;
    let root = std::fs::canonicalize(&solution.root).map_err(|error| error.to_string())?;
    let relative = root
        .strip_prefix(&top)
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let rev = git(
        &top,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{against}^{{tree}}"),
        ],
    )?;
    if !rev.status.success() {
        return Err(format!(
            "bad git revision {against}: {}; try --against origin/main",
            String::from_utf8_lossy(&rev.stderr).trim()
        ));
    }
    let tree = String::from_utf8_lossy(&rev.stdout);
    let args = if relative.is_empty() {
        vec!["archive", "--format=tar", tree.trim()]
    } else {
        vec!["archive", "--format=tar", tree.trim(), "--", &relative]
    };
    let archive = git(&top, &args)?;
    let missing = || {
        format!(
            "{against} has no twaco.toml at {}; contract diff compares two twaco solutions",
            if relative.is_empty() { "." } else { &relative }
        )
    };
    if !archive.status.success() {
        return Err(missing());
    }
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    tar::Archive::new(archive.stdout.as_slice())
        .unpack(temp.path())
        .map_err(|error| format!("cannot unpack {against}: {error}"))?;
    let config = temp.path().join(&relative).join("twaco.toml");
    if !config.is_file() {
        return Err(missing());
    }
    let old = Solution::load(&config).map_err(|error| format!("{against}: {error}"))?;
    if let Some(project) = project {
        if solution.project(project).is_none() && old.project(project).is_none() {
            return Err(format!(
                "neither solution has a project named {project}; check --project"
            ));
        }
    }
    let ledger = Ledger::read(&solution.root.join(ledger::RELATIVE_PATH))
        .map_err(|error| error.to_string())?;
    let mut report = compare(load(&old, project), load(solution, project), &ledger);
    let targets: Vec<_> = report
        .breaking
        .iter()
        .filter_map(|change| {
            let (collection, name) = change.entity.split_once('/')?;
            EntityKey::new(collection, name).ok()
        })
        .collect();
    attach_callers(
        &mut report,
        &Index::with_targets(solution, &targets),
        solution,
    );
    report.summary = summary(&report, against);
    Ok(report)
}

fn change(
    kind: &str,
    entity: &str,
    member: &str,
    action: &str,
    from: Option<Value>,
    to: Option<Value>,
) -> Change {
    Change {
        kind: kind.into(),
        entity: entity.into(),
        member: (!member.is_empty()).then(|| member.into()),
        change: action.into(),
        from,
        to,
        callers: Vec::new(),
    }
}

fn compare(mut old: Contract, new: Contract, ledger: &Ledger) -> Report {
    let mut report = Report {
        summary: String::new(),
        breaking: Vec::new(),
        compatible: Vec::new(),
        skipped: Vec::new(),
        limits: LIMITS.into(),
    };
    report
        .skipped
        .extend(old.skipped.into_iter().map(|s| format!("against: {s}")));
    report.skipped.extend(
        new.skipped
            .into_iter()
            .map(|s| format!("working tree: {s}")),
    );
    for record in &ledger.0 {
        if record.kind.replaces_entities() {
            for pair in &record.entities {
                let before = format!("{}/{}", pair.collection, pair.old);
                let after = format!("{}/{}", pair.collection, pair.new);
                if old.entities.contains_key(&before)
                    && !new.entities.contains_key(&before)
                    && new.entities.contains_key(&after)
                    && !old.entities.contains_key(&after)
                {
                    let entity = old.entities.remove(&before).unwrap();
                    old.entities.insert(after.clone(), entity);
                    report.breaking.push(change(
                        "entity",
                        &before,
                        "",
                        &format!("renamed {} -> {}", pair.old, pair.new),
                        Some(json!(before)),
                        Some(json!(after)),
                    ));
                }
            }
        }
    }
    for (key, before) in &mut old.entities {
        let Some(after) = new.entities.get(key) else {
            report
                .breaking
                .push(change("entity", key, "", "removed", Some(json!(key)), None));
            continue;
        };
        apply_member_renames(key, before, after, ledger, &mut report);
        if before.project != after.project {
            report.compatible.push(change(
                "project",
                key,
                "",
                &format!("moved to project {}", after.project),
                Some(json!(before.project)),
                Some(json!(after.project)),
            ));
        }
        for ((kind, member), value) in &before.members {
            // A removed service is one contract change, including its signature.
            if matches!(kind.as_str(), "param" | "result") {
                let service = if kind == "param" {
                    member.rsplit_once('.').map(|p| p.0).unwrap_or(member)
                } else {
                    member
                };
                if !after
                    .members
                    .contains_key(&("service".into(), service.into()))
                {
                    continue;
                }
            }
            match after.members.get(&(kind.clone(), member.clone())) {
                None => report.breaking.push(change(
                    kind,
                    key,
                    member,
                    "removed",
                    Some(value.clone()),
                    None,
                )),
                Some(next) if next != value => {
                    let breaking = if kind == "param" {
                        value["baseType"] != next["baseType"]
                            || value["dataShape"] != next["dataShape"]
                            || (value["required"] != next["required"] && next["required"] == true)
                    } else {
                        true
                    };
                    let item = change(
                        kind,
                        key,
                        member,
                        "changed",
                        Some(value.clone()),
                        Some(next.clone()),
                    );
                    if breaking {
                        report.breaking.push(item);
                    } else {
                        report.compatible.push(item);
                    }
                }
                _ => {}
            }
        }
        for ((kind, member), value) in &after.members {
            if before.members.contains_key(&(kind.clone(), member.clone())) {
                continue;
            }
            if matches!(kind.as_str(), "param" | "result") {
                let service = if kind == "param" {
                    member.rsplit_once('.').map(|p| p.0).unwrap_or(member)
                } else {
                    member
                };
                if !before
                    .members
                    .contains_key(&("service".into(), service.into()))
                {
                    continue;
                }
            }
            let item = change(kind, key, member, "added", None, Some(value.clone()));
            if kind == "param" && value["required"] == true && value["default"].is_null() {
                report.breaking.push(item);
            } else {
                report.compatible.push(item);
            }
        }
    }
    for key in new
        .entities
        .keys()
        .filter(|key| !old.entities.contains_key(*key))
    {
        report
            .compatible
            .push(change("entity", key, "", "added", None, Some(json!(key))));
    }
    report.skipped.sort();
    report.skipped.dedup();
    report
}

fn apply_member_renames(
    key: &str,
    before: &mut Entity,
    after: &Entity,
    ledger: &Ledger,
    report: &mut Report,
) {
    for record in &ledger.0 {
        let kind = match record.kind {
            Kind::Service => "service",
            Kind::Param => "param",
            Kind::Field => "field",
            Kind::Property => "property",
            _ => continue,
        };
        let name = key.split_once('/').unwrap().1;
        if !record.entities.iter().any(|e| {
            e.collection == key.split_once('/').unwrap().0 && (e.old == name || e.new == name)
        }) && record.scope.as_deref() != Some(name)
            && record.scope.as_deref() != Some(key)
        {
            continue;
        }
        let (old, new) = if kind == "param" {
            let Some(service) = &record.service else {
                continue;
            };
            (
                format!("{service}.{}", record.old),
                format!("{service}.{}", record.new),
            )
        } else {
            (record.old.clone(), record.new.clone())
        };
        let from = (kind.into(), old.clone());
        let to = (kind.into(), new.clone());
        if before.members.contains_key(&from)
            && !after.members.contains_key(&from)
            && after.members.contains_key(&to)
            && !before.members.contains_key(&to)
        {
            let value = before.members.remove(&from).unwrap();
            before.members.insert(to, value);
            if kind == "service" {
                let children: Vec<_> = before
                    .members
                    .keys()
                    .filter(|(k, m)| {
                        (k == "result" && m == &old)
                            || (k == "param" && m.starts_with(&format!("{old}.")))
                    })
                    .cloned()
                    .collect();
                for (k, m) in children {
                    let value = before.members.remove(&(k.clone(), m.clone())).unwrap();
                    before
                        .members
                        .insert((k, format!("{new}{}", &m[old.len()..])), value);
                }
            }
            report.breaking.push(change(
                kind,
                key,
                &old,
                &format!("renamed {old} -> {new}"),
                Some(json!(old)),
                Some(json!(new)),
            ));
        }
    }
}

fn attach_callers(report: &mut Report, index: &Index, solution: &Solution) {
    let edges = index.edges();
    let (model, _) = types::load_model(solution);
    for change in &mut report.breaking {
        let member = change.member.as_deref().map(|m| {
            if change.kind == "param" {
                m.rsplit_once('.').map(|p| p.0).unwrap_or(m)
            } else {
                m
            }
        });
        let mut targets = BTreeSet::from([change.entity.clone()]);
        if change.member.is_some()
            && !change.entity.starts_with("DataShapes/")
            && !change.entity.starts_with("Mashups/")
        {
            for (key, _) in index.entities() {
                if inherits_member(index, key, change, member.unwrap(), &model) {
                    targets.insert(format!("{}/{}", key.collection(), key.name()));
                }
            }
        }
        let mut callers = BTreeSet::new();
        for (from, to, edge) in &edges {
            if edge.confidence() < Confidence::Resolved || !targets.contains(to) {
                continue;
            }
            let matches = if change.entity.starts_with("DataShapes/") {
                edge.kind == EdgeKind::DataShape || member.is_none()
            } else {
                member.is_none() || edge.to_member.as_deref() == member
            };
            if matches {
                callers.insert(match &edge.from_member {
                    Some(m) => format!("{from}.{m}"),
                    None => from.clone(),
                });
            }
        }
        if change.kind == "mashup_parameter" {
            for file in workspace::discover(solution)
                .entities
                .iter()
                .filter(|f| f.info.collection == "Mashups")
            {
                if let Ok(bytes) = std::fs::read(&file.path) {
                    if let Some(content) = mashup::extract(&bytes)
                        .ok()
                        .and_then(|assets| serde_json::from_str::<Value>(&assets.content).ok())
                    {
                        shown_callers(
                            &content["UI"],
                            change.entity.trim_start_matches("Mashups/"),
                            &format!("Mashups/{}", file.info.name),
                            &mut callers,
                        );
                    }
                }
            }
        }
        change.callers = callers.into_iter().collect();
    }
}

fn inherits_member(
    index: &Index,
    key: &EntityKey,
    change: &Change,
    member: &str,
    model: &types::Model,
) -> bool {
    let declares = |entity: &types::Entity| match change.kind.as_str() {
        "event" => entity.events.iter().any(|(name, _)| name == member),
        "property" => entity
            .members
            .iter()
            .any(|m| matches!(m, types::Member::Property(p) if p.name == member)),
        _ => entity
            .members
            .iter()
            .any(|m| matches!(m, types::Member::Service(s) if s.name == member)),
    };
    let own = model
        .entities
        .iter()
        .find(|e| e.collection == key.collection() && e.name == key.name());
    if own.is_some_and(declares) {
        return false;
    }
    for name in index.inheritance_names(key) {
        if change
            .entity
            .split_once('/')
            .is_some_and(|(_, declared)| name == declared)
        {
            return true;
        }
        if model.entities.iter().any(|e| e.name == name && declares(e)) {
            return false;
        }
    }
    false
}

fn shown_callers(widget: &Value, target: &str, entity: &str, callers: &mut BTreeSet<String>) {
    let p = &widget["Properties"];
    let shown = match p["Type"].as_str() {
        Some("mashupcontainer") => p["Name"].as_str(),
        Some("navigationfunction") => p["TargetMashup"].as_str(),
        _ => None,
    };
    if shown == Some(target) {
        callers.insert(format!("{entity}.{}", p["Id"].as_str().unwrap_or("?")));
    }
    for child in widget["Widgets"].as_array().into_iter().flatten() {
        shown_callers(child, target, entity, callers);
    }
}

fn summary(report: &Report, against: &str) -> String {
    let callers: BTreeSet<_> = report.breaking.iter().flat_map(|c| &c.callers).collect();
    let mut text = if report.breaking.is_empty() && report.compatible.is_empty() {
        format!("contract against {against}: no change")
    } else {
        format!(
            "contract against {against}: {} breaking, {} compatible; {} callers in this repository",
            report.breaking.len(),
            report.compatible.len(),
            callers.len()
        )
    };
    if !report.skipped.is_empty() {
        text.push_str(&format!("; partial ({} skipped)", report.skipped.len()));
    }
    text
}

pub fn render(report: &Report, detail: bool) -> String {
    let mut out = format!("{}\n{}\n", report.summary, report.limits);
    for item in &report.breaking {
        render_change(&mut out, "breaking", item, detail);
    }
    if detail {
        for item in &report.compatible {
            render_change(&mut out, "compatible", item, true);
        }
        for item in &report.skipped {
            out.push_str(&format!("  skipped: {item}\n"));
        }
    } else {
        let mut counts = BTreeMap::new();
        for item in &report.compatible {
            *counts.entry(&item.kind).or_insert(0usize) += 1;
        }
        for (kind, count) in counts {
            out.push_str(&format!("  compatible: {count} {kind}\n"));
        }
    }
    out
}

fn render_change(out: &mut String, class: &str, item: &Change, detail: bool) {
    let member = item
        .member
        .as_ref()
        .map(|m| format!(".{m}"))
        .unwrap_or_default();
    out.push_str(&format!(
        "  {class}: {}{member} ({}): {}\n",
        item.entity, item.kind, item.change
    ));
    if detail && item.change == "changed" {
        out.push_str(&format!(
            "    {} -> {}\n",
            item.from.as_ref().unwrap(),
            item.to.as_ref().unwrap()
        ));
    }
    for caller in item
        .callers
        .iter()
        .take(if detail { usize::MAX } else { 3 })
    {
        out.push_str(&format!("    {caller}\n"));
    }
    if !detail && item.callers.len() > 3 {
        out.push_str(&format!("    +{} more\n", item.callers.len() - 3));
    }
}

#[cfg(test)]
mod tests;
