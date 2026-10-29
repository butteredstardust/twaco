//! The reviewed contract for dependencies a solution does not ship. Reads XML spans and script
//! facts only; entity files are never changed. Unreadable inputs refuse an incomplete draft.

use super::codes::{Coded, ErrorCode};
use super::config::Solution;
use super::progress::{self, Progress};
use super::{index, mashup, parallel, scan, script, sidecar, widgets, workspace};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const FILE: &str = "requirements.toml";
type References = BTreeMap<String, BTreeMap<String, BTreeSet<String>>>;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub(crate) String);

impl Coded for Error {
    fn code(&self) -> ErrorCode {
        ErrorCode::InvalidData
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Contract {
    pub(crate) platform: Option<Platform>,
    #[serde(default)]
    pub(crate) extension: Vec<Extension>,
    #[serde(default)]
    pub(crate) project: Vec<Project>,
    #[serde(default)]
    pub(crate) entities: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub(crate) optional: Optional,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Optional {
    #[serde(default)]
    pub(crate) entities: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Platform {
    pub(crate) minimum: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Extension {
    pub(crate) name: String,
    pub(crate) minimum: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Project {
    pub(crate) name: String,
}

/// Numeric dotted parts, padded with zeroes; build suffixes do not affect ordering.
fn version(text: &str) -> Result<Vec<u64>, String> {
    text.split('-')
        .next()
        .unwrap_or("")
        .split('.')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(format!(
                    "version {text:?} has a non-numeric part {part:?}; use dotted numeric versions"
                ));
            }
            part.parse().map_err(|_| {
                format!(
                    "version {text:?} has an oversized numeric part; use dotted numeric versions"
                )
            })
        })
        .collect()
}

fn compare(left: &str, right: &str) -> Result<Ordering, String> {
    let left = version(left)?;
    let right = version(right)?;
    Ok((0..left.len().max(right.len()))
        .map(|i| {
            left.get(i)
                .copied()
                .unwrap_or(0)
                .cmp(&right.get(i).copied().unwrap_or(0))
        })
        .find(|order| *order != Ordering::Equal)
        .unwrap_or(Ordering::Equal))
}

fn invalid(key: &str, why: impl std::fmt::Display) -> Error {
    Error(format!(
        "{FILE}: {key}: {why}; edit {FILE} and run requirements check"
    ))
}

pub(crate) fn parse(text: &str) -> Result<Contract, Error> {
    let contract: Contract = toml::from_str(text).map_err(|error| invalid("format", error))?;
    if let Some(platform) = &contract.platform {
        version(&platform.minimum).map_err(|error| invalid("platform.minimum", error))?;
    }
    let mut extensions = BTreeSet::new();
    for extension in &contract.extension {
        valid_name("extension.name", &extension.name)?;
        if !extensions.insert(&extension.name) {
            return Err(invalid(
                "extension.name",
                format!("duplicate {:?}; list it once", extension.name),
            ));
        }
        version(&extension.minimum)
            .map_err(|error| invalid(&format!("extension.{}.minimum", extension.name), error))?;
    }
    let mut projects = BTreeSet::new();
    for project in &contract.project {
        valid_name("project.name", &project.name)?;
        if !projects.insert(&project.name) {
            return Err(invalid(
                "project.name",
                format!("duplicate {:?}; list it once", project.name),
            ));
        }
    }
    for (collection, names) in &contract.entities {
        validate_entities("entities", collection, names)?;
    }
    for (collection, names) in &contract.optional.entities {
        validate_entities("optional.entities", collection, names)?;
        if let Some(required) = contract.entities.get(collection) {
            if let Some(name) = names.iter().find(|name| required.contains(name)) {
                return Err(invalid(
                    &format!("optional.entities.{collection}"),
                    format!("{name:?} is also in [entities]; list it in one table"),
                ));
            }
        }
    }
    Ok(contract)
}

fn validate_entities(key: &str, collection: &str, names: &[String]) -> Result<(), Error> {
    if !workspace::KNOWN_COLLECTIONS.contains(&collection) {
        return Err(invalid(
            &format!("{key}.{collection}"),
            "unknown collection; use a ThingWorx collection name",
        ));
    }
    let mut seen = BTreeSet::new();
    for name in names {
        valid_name(&format!("{key}.{collection}"), name)?;
        if !seen.insert(name) {
            return Err(invalid(
                &format!("{key}.{collection}"),
                format!("duplicate {name:?}; list it once"),
            ));
        }
    }
    Ok(())
}

fn valid_name(key: &str, name: &str) -> Result<(), Error> {
    super::entity::key::EntityKey::new("Things", name)
        .map(|_| ())
        .map_err(|error| invalid(key, error))
}

pub(crate) fn load(solution: &Solution) -> Result<Contract, Error> {
    let path = solution.root.join(FILE);
    let text = std::fs::read_to_string(&path).map_err(|error| {
        Error(format!(
            "{}: {error}; run twaco requirements init to draft the contract",
            path.display()
        ))
    })?;
    parse(&text)
}

#[derive(Debug, Default)]
pub(crate) struct Collected {
    pub(crate) references: References,
    extensions: BTreeMap<String, (String, BTreeSet<String>)>,
    projects: BTreeMap<String, BTreeSet<String>>,
    platform: Option<(String, String)>,
}

impl Collected {
    fn add(&mut self, collection: &str, name: &str, from: &str) {
        if !name.trim().is_empty() && !matches!(collection, "Resources" | "Subsystems" | "Users") {
            self.references
                .entry(collection.into())
                .or_default()
                .entry(name.into())
                .or_default()
                .insert(from.into());
        }
    }

    fn platform(&mut self, value: &str, source: &str) -> Result<(), Error> {
        if value.trim().is_empty() {
            return Ok(());
        }
        version(value)
            .map_err(|error| Error(format!("{source}: {error}; fix the exported version")))?;
        if self
            .platform
            .as_ref()
            .is_none_or(|(old, _)| compare(value, old) == Ok(Ordering::Greater))
        {
            self.platform = Some((value.into(), source.into()));
        }
        Ok(())
    }

    fn declared(
        &mut self,
        text: &str,
        from: &str,
        defined: &BTreeSet<(String, String)>,
    ) -> Result<(), Error> {
        #[derive(Deserialize, Default)]
        struct Depends {
            #[serde(default)]
            extensions: String,
            #[serde(default)]
            projects: String,
        }
        if text.trim().is_empty() {
            return Ok(());
        }
        let depends: Depends = serde_json::from_str(text)
            .map_err(|error| Error(format!("{from}: dependsOn: {error}; fix the exported JSON")))?;
        for item in depends
            .extensions
            .split(',')
            .filter(|item| !item.trim().is_empty())
        {
            let (name, minimum) = item.trim().split_once(':').ok_or_else(|| {
                Error(format!(
                    "{from}: dependsOn.extensions {item:?}; use name:version"
                ))
            })?;
            valid_name("dependsOn.extensions", name)?;
            version(minimum)
                .map_err(|error| Error(format!("{from}: dependsOn.extensions: {error}")))?;
            let entry = self
                .extensions
                .entry(name.into())
                .or_insert_with(|| (minimum.into(), BTreeSet::new()));
            if compare(minimum, &entry.0) == Ok(Ordering::Greater) {
                entry.0 = minimum.into();
            }
            entry.1.insert(from.into());
        }
        for item in depends
            .projects
            .split(',')
            .filter(|item| !item.trim().is_empty())
        {
            let name = item.trim().split(':').next().unwrap_or("");
            valid_name("dependsOn.projects", name)?;
            if !defined.contains(&("Projects".into(), name.into())) {
                self.projects
                    .entry(name.into())
                    .or_default()
                    .insert(from.into());
            }
        }
        Ok(())
    }
}

/// Collect only the configured source trees. A repository-defined name is never external.
pub(crate) fn collect(solution: &Solution) -> Result<Collected, Error> {
    let found = workspace::discover(solution);
    collect_in(solution, &found)
}

/// [`collect`] over a scan already made.
pub(crate) fn collect_in(
    solution: &Solution,
    found: &workspace::Discovery,
) -> Result<Collected, Error> {
    if !found.unreadable.is_empty() {
        return Err(Error(format!(
            "requirements collection: {}; fix unreadable entity files and retry",
            found.unreadable.join("\n")
        )));
    }
    let defined: BTreeSet<_> = found
        .entities
        .iter()
        .map(|file| (file.info.collection.clone(), file.info.name.clone()))
        .collect();
    let mut out = Collected::default();
    let mut exported = Collected::default();
    for file in &found.entities {
        let from = format!("{}/{}", file.info.collection, file.info.name);
        let bytes = std::fs::read(&file.path).map_err(|error| {
            Error(format!(
                "{}: {error}; restore the entity file",
                file.path.display()
            ))
        })?;
        let tokens = scan::tokenize(&bytes).map_err(|error| {
            Error(format!(
                "{}: {error}; fix the entity XML",
                file.path.display()
            ))
        })?;
        for token in tokens
            .iter()
            .filter(|token| matches!(token.kind, scan::Kind::Start | scan::Kind::Empty))
        {
            let tag = token.name.of(&bytes);
            let attr = |name: &str| -> Result<String, Error> {
                scan::attribute(&bytes, token, name)
                    .map(|span| {
                        span.map(|span| {
                            scan::decode_entities(&String::from_utf8_lossy(span.of(&bytes)))
                        })
                        .unwrap_or_default()
                    })
                    .map_err(|error| Error(format!("{}: {name}: {error}", file.path.display())))
            };
            if tag == b"Entities" {
                let major = attr("majorVersion")?;
                let minor = attr("minorVersion")?;
                if !major.is_empty() && !minor.is_empty() {
                    exported.platform(
                        &format!("{major}.{minor}"),
                        "entity exports (Entities majorVersion.minorVersion)",
                    )?;
                }
            }
            if tag == b"Project" && file.info.collection == "Projects" {
                out.declared(&attr("dependsOn")?, &from, &defined)?;
                out.platform(
                    &attr("minPlatformVersion")?,
                    &format!("{from} minPlatformVersion"),
                )?;
            }
            if tag == b"Owner" {
                continue;
            }
            for (name, collection) in [
                ("thingTemplate", "ThingTemplates"),
                ("baseThingTemplate", "ThingTemplates"),
                ("aspect.thingTemplate", "ThingTemplates"),
                ("aspect.thingShape", "ThingShapes"),
                ("baseDataShape", "DataShapes"),
                ("aspect.dataShape", "DataShapes"),
                ("dataShape", "DataShapes"),
            ] {
                out.add(collection, &attr(name)?, &from);
            }
            if tag == b"ConfigurationTable" {
                out.add("DataShapes", &attr("dataShapeName")?, &from);
            }
            if tag == b"ImplementedShape" {
                out.add("ThingShapes", &attr("name")?, &from);
            }
            if tag == b"Thing" {
                out.add("Things", &attr("valueStream")?, &from);
            }
            if tag == b"Principal" {
                let name = attr("name")?;
                match attr("type")?.as_str() {
                    "Group" => out.add("Groups", &name, &from),
                    "Organization" => out.add("Organizations", &name, &from),
                    "OrganizationalUnit" => {
                        out.add("Organizations", name.split(':').next().unwrap_or(""), &from)
                    }
                    _ => {}
                }
            }
        }
        if matches!(
            file.info.collection.as_str(),
            "Things" | "ThingTemplates" | "ThingShapes"
        ) {
            let services = sidecar::script_services(&bytes).map_err(|error| {
                Error(format!(
                    "{}: {error}; fix service scripts",
                    file.path.display()
                ))
            })?;
            for service in services {
                // A parser refusal yields no resolved facts, just as in the index. Mentions
                // cannot prove a dependency; the script traps gate reports unsupported syntax.
                if let Ok(facts) = script::parse(service.script.as_bytes()) {
                    for member in &facts.members {
                        if let script::Receiver::Variable(collection) = &member.receiver {
                            if matches!(
                                collection.as_str(),
                                "Things" | "ThingTemplates" | "ThingShapes" | "DataShapes"
                            ) {
                                out.add(
                                    collection,
                                    &String::from_utf8_lossy(
                                        member.property.of(service.script.as_bytes()),
                                    ),
                                    &from,
                                );
                            }
                        }
                    }
                }
            }
        }
        if matches!(file.info.collection.as_str(), "Mashups" | "MashupGadgets") {
            let assets = mashup::extract(&bytes).map_err(|error| {
                Error(format!(
                    "{}: {error}; fix mashup content",
                    file.path.display()
                ))
            })?;
            let value: Value = serde_json::from_str(&assets.content).map_err(|error| {
                Error(format!(
                    "{}: mashup content: {error}; fix the JSON",
                    file.path.display()
                ))
            })?;
            let mut bound = Vec::new();
            index::mashups::bindings(&value, &mut bound);
            for (kind, name, _) in bound {
                let collection = if kind.is_empty() {
                    "Things".into()
                } else {
                    index::mashups::collection_of(&kind)
                };
                if workspace::KNOWN_COLLECTIONS.contains(&collection.as_str()) {
                    out.add(&collection, &name, &from);
                }
            }
            // UI itself is the mashup root, not an extension widget. Only its widget tree
            // carries widget Types; other JSON objects can also have a field called Type.
            for widget in value
                .pointer("/UI/Widgets")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                collect_widgets(widget, &from, &mut out);
            }
        }
    }
    if out.platform.is_none() {
        if let Some(minimum) = solution
            .package
            .minimum_thingworx
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            out.platform(minimum, "twaco.toml [package] minimum_thingworx")?;
        } else {
            out.platform = exported.platform;
        }
    }
    for (collection, names) in &mut out.references {
        names.retain(|name, _| !defined.contains(&(collection.clone(), name.clone())));
    }
    out.references.retain(|_, names| !names.is_empty());
    Ok(out)
}

fn collect_widgets(value: &Value, from: &str, out: &mut Collected) {
    if let Some(kind) = value.pointer("/Properties/Type").and_then(Value::as_str) {
        if !widgets::catalog().platform_types.contains(kind) {
            out.add("Widgets", kind, from);
        }
    }
    for child in value
        .get("Widgets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        collect_widgets(child, from, out);
    }
}

fn quoted(text: &str) -> String {
    toml::Value::String(text.into()).to_string()
}

pub(crate) fn draft(collected: &Collected) -> String {
    let mut text = "# What this solution needs from a server and does not ship itself.\n# Drafted by `twaco requirements init`; `twaco requirements check [--server]` holds the repository\n# and a server to it. Edit freely.\n".to_string();
    if let Some((minimum, source)) = &collected.platform {
        text.push_str(&format!(
            "\n[platform]\nminimum = {} # from {source}\n",
            quoted(minimum)
        ));
    }
    for (name, (minimum, _)) in &collected.extensions {
        text.push_str(&format!(
            "\n[[extension]]\nname = {}\nminimum = {}\n",
            quoted(name),
            quoted(minimum)
        ));
    }
    for name in collected.projects.keys() {
        text.push_str(&format!("\n[[project]]\nname = {}\n", quoted(name)));
    }
    text.push_str("\n[entities]\n");
    let order = [
        "ThingTemplates",
        "ThingShapes",
        "DataShapes",
        "Things",
        "Organizations",
        "Groups",
        "Widgets",
    ];
    let mut collections: Vec<_> = collected.references.keys().collect();
    collections.sort_by_key(|name| {
        (
            order
                .iter()
                .position(|item| item == name)
                .unwrap_or(order.len()),
            *name,
        )
    });
    for collection in collections {
        text.push_str(&format!("{collection} = [\n"));
        for (name, refs) in &collected.references[collection] {
            let first = refs.first().expect("a collected name has a referrer");
            text.push_str(&format!(
                "    {}, # {} referrer{}: {first}\n",
                quoted(name),
                refs.len(),
                if refs.len() == 1 { "" } else { "s" }
            ));
        }
        text.push_str("]\n");
    }
    text
}

#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub rule: String,
    pub requirement: String,
    pub blocking: bool,
    pub message: String,
    pub installed: Option<String>,
    pub referrers: BTreeSet<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub entities: usize,
    pub collections: BTreeMap<String, usize>,
    pub extensions: usize,
    pub projects: usize,
    pub platform: Option<String>,
    pub server_checked: bool,
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn blocks(&self) -> bool {
        self.findings.iter().any(|item| item.blocking)
    }
    pub fn summary(&self) -> String {
        let count = |rule| {
            self.findings
                .iter()
                .filter(|item| item.rule == rule)
                .count()
        };
        let noun = |count: usize, one: &str, many: &str| {
            format!("{count} {}", if count == 1 { one } else { many })
        };
        // Without --server nothing was compared with a server, so "0 unmet" would claim too much.
        let server = if self.server_checked {
            format!("{} unmet on the server, ", count("unmet"))
        } else {
            String::new()
        };
        format!(
            "requirements: {}, {}, {}, platform {}; {server}{} undeclared, {} declared lower, {} stale",
            noun(self.entities, "entity", "entities"),
            noun(self.extensions, "extension", "extensions"),
            noun(self.projects, "project", "projects"),
            self.platform.as_deref().unwrap_or("unspecified"),
            count("undeclared"),
            count("declared_lower"),
            count("stale")
        )
    }
    pub fn json(&self, detail: bool) -> Value {
        let mut value = serde_json::to_value(self).expect("report is serializable");
        value["summary"] = json!(self.summary());
        value["ok"] = json!(!self.blocks());
        value["total_findings"] = json!(self.findings.len());
        if !detail {
            let findings = value["findings"].as_array_mut().expect("findings array");
            findings.truncate(8);
            for finding in findings {
                finding
                    .as_object_mut()
                    .expect("finding object")
                    .remove("referrers");
            }
        }
        value
    }
    fn finding(
        &mut self,
        rule: &str,
        requirement: String,
        message: String,
        referrers: BTreeSet<String>,
    ) {
        self.findings.push(Finding {
            rule: rule.into(),
            requirement,
            blocking: rule != "stale",
            message,
            installed: None,
            referrers,
        });
    }

    fn note(
        &mut self,
        rule: &str,
        requirement: String,
        message: String,
        referrers: BTreeSet<String>,
    ) {
        self.findings.push(Finding {
            rule: rule.into(),
            requirement,
            blocking: false,
            message,
            installed: None,
            referrers,
        });
    }
}

pub(crate) fn offline(contract: &Contract, collected: &Collected) -> Report {
    let mut report = Report {
        entities: contract.entities.values().map(Vec::len).sum(),
        collections: contract
            .entities
            .iter()
            .map(|(k, v)| (k.clone(), v.len()))
            .collect(),
        extensions: contract.extension.len(),
        projects: contract.project.len(),
        platform: contract.platform.as_ref().map(|p| p.minimum.clone()),
        server_checked: false,
        findings: Vec::new(),
    };
    for (collection, names) in &collected.references {
        for (name, refs) in names {
            if !contract
                .entities
                .get(collection)
                .is_some_and(|names| names.contains(name))
                && !contract
                    .optional
                    .entities
                    .get(collection)
                    .is_some_and(|names| names.contains(name))
            {
                report.finding(
                    "undeclared",
                    format!("{collection}/{name}"),
                    format!("add {name:?} to [entities].{collection} in {FILE}"),
                    refs.clone(),
                );
            }
        }
    }
    for (collection, names) in &contract.entities {
        for name in names {
            if !collected
                .references
                .get(collection)
                .is_some_and(|names| names.contains_key(name))
            {
                report.finding(
                    "stale",
                    format!("{collection}/{name}"),
                    "nothing in the repository references it; review whether to keep it".into(),
                    BTreeSet::new(),
                );
            }
        }
    }
    for (collection, names) in &contract.optional.entities {
        for name in names {
            if !collected
                .references
                .get(collection)
                .is_some_and(|names| names.contains_key(name))
            {
                report.note(
                    "stale",
                    format!("{collection}/{name}"),
                    "nothing in the repository references it; review whether to keep it".into(),
                    BTreeSet::new(),
                );
            }
        }
    }
    for (name, (minimum, refs)) in &collected.extensions {
        if !contract.extension.iter().any(|e| {
            &e.name == name && compare(&e.minimum, minimum).is_ok_and(|o| o != Ordering::Less)
        }) {
            report.finding("declared_lower", format!("extension/{name}"), format!("dependsOn needs {minimum}; set [[extension]] {name:?} minimum to at least {minimum} in {FILE}"), refs.clone());
        }
    }
    for extension in &contract.extension {
        if !collected.extensions.contains_key(&extension.name) {
            report.finding(
                "stale",
                format!("extension/{}", extension.name),
                "no Project dependsOn references it; review whether to keep it".into(),
                BTreeSet::new(),
            );
        }
    }
    for (name, refs) in &collected.projects {
        if !contract.project.iter().any(|p| &p.name == name) {
            report.finding(
                "undeclared",
                format!("Projects/{name}"),
                format!("add [[project]] name = {} to {FILE}", quoted(name)),
                refs.clone(),
            );
        }
    }
    for project in &contract.project {
        if !collected.projects.contains_key(&project.name) {
            report.finding(
                "stale",
                format!("Projects/{}", project.name),
                "no Project dependsOn references it; review whether to keep it".into(),
                BTreeSet::new(),
            );
        }
    }
    if let Some((minimum, source)) = &collected.platform {
        if !contract
            .platform
            .as_ref()
            .is_some_and(|p| compare(&p.minimum, minimum).is_ok_and(|o| o != Ordering::Less))
        {
            report.finding(if contract.platform.is_some() { "declared_lower" } else { "undeclared" }, "platform".into(), format!("{source} needs {minimum}; set [platform] minimum to at least {minimum} in {FILE}"), BTreeSet::from([source.clone()]));
        }
    }
    report
}

/// Read-only server access. Errors are findings for the named requirement, never passes.
pub trait Remote: Sync {
    fn platform_version(&self) -> Result<String, String>;
    fn extensions(&self) -> Result<BTreeMap<String, String>, String>;
    fn entity_exists(&self, collection: &str, name: &str) -> Result<bool, String>;
}

impl Remote for super::server::Client {
    fn platform_version(&self) -> Result<String, String> {
        super::help::server_version(self)
    }
    fn extensions(&self) -> Result<BTreeMap<String, String>, String> {
        super::extensions::list(self)
            .map(|packages| packages.into_iter().map(|p| (p.name, p.version)).collect())
            .map_err(|error| error.to_string())
    }
    fn entity_exists(&self, collection: &str, name: &str) -> Result<bool, String> {
        let key = super::entity::key::EntityKey::address(collection, name)
            .map_err(|error| error.to_string())?;
        super::server::Client::entity_exists(self, &key).map_err(|error| error.to_string())
    }
}

fn check_version(
    report: &mut Report,
    requirement: String,
    minimum: &str,
    installed: Result<String, String>,
    refs: BTreeSet<String>,
) {
    let (message, installed) = match installed {
        Ok(installed) => match compare(&installed, minimum) {
            Ok(order) if order != Ordering::Less => return,
            Ok(_) => (
                format!("needs >= {minimum}; installed {installed}; upgrade the server dependency"),
                Some(installed),
            ),
            Err(error) => (
                format!("installed {installed}: {error}; verify the server dependency"),
                Some(installed),
            ),
        },
        Err(error) => (
            format!("needs >= {minimum}: {error}; verify the server dependency"),
            None,
        ),
    };
    report.finding("unmet", requirement, message, refs);
    report.findings.last_mut().expect("just added").installed = installed;
}

pub(crate) fn server_check(
    contract: &Contract,
    collected: &Collected,
    remote: &dyn Remote,
    report: &mut Report,
    progress: &dyn Progress,
) {
    report.server_checked = true;
    let mut entities: Vec<_> = contract
        .entities
        .iter()
        .flat_map(|(collection, names)| {
            names
                .iter()
                .map(move |name| (collection.as_str(), name.as_str(), false))
        })
        .collect();
    entities.extend(
        contract
            .optional
            .entities
            .iter()
            .flat_map(|(collection, names)| {
                names
                    .iter()
                    .map(move |name| (collection.as_str(), name.as_str(), true))
            }),
    );
    entities.extend(
        contract
            .project
            .iter()
            .map(|p| ("Projects", p.name.as_str(), false)),
    );
    // Opened before the platform and extension reads, which wait on the server too.
    let _phase = progress::phase(
        progress,
        "checking requirements",
        Some(entities.len() as u64),
    );
    if let Some(platform) = &contract.platform {
        check_version(
            report,
            "platform".into(),
            &platform.minimum,
            remote.platform_version(),
            BTreeSet::new(),
        );
    }
    if !contract.extension.is_empty() {
        let installed = remote.extensions();
        for extension in &contract.extension {
            let version = installed.as_ref().map_err(Clone::clone).and_then(|all| {
                all.get(&extension.name)
                    .cloned()
                    .ok_or_else(|| "extension is not installed".into())
            });
            let refs = collected
                .extensions
                .get(&extension.name)
                .map(|(_, refs)| refs.clone())
                .unwrap_or_default();
            check_version(
                report,
                format!("extension/{}", extension.name),
                &extension.minimum,
                version,
                refs,
            );
        }
    }
    let answers = parallel::map_progress(&entities, progress, |(collection, name, _)| {
        progress.message(&format!("{collection}/{name}"));
        remote.entity_exists(collection, name)
    });
    for ((collection, name, optional), answer) in entities.into_iter().zip(answers) {
        let missing = matches!(&answer, Ok(false));
        let message = match answer {
            Ok(true) => continue,
            Ok(false) => "entity is missing; install the dependency before deploying".into(),
            Err(error) => format!(
                "could not verify existence: {error}; resolve the server read failure and retry"
            ),
        };
        let refs = if collection == "Projects" {
            collected.projects.get(name)
        } else {
            collected
                .references
                .get(collection)
                .and_then(|names| names.get(name))
        }
        .cloned()
        .unwrap_or_default();
        if optional && missing {
            report.note(
                "optional_missing",
                format!("{collection}/{name}"),
                "optional entity is missing; install it when this server needs that feature".into(),
                refs,
            );
        } else {
            report.finding("unmet", format!("{collection}/{name}"), message, refs);
        }
    }
}

#[cfg(test)]
mod tests;
