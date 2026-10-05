//! Exports, as Composer's Import/Export dialog makes them.
//!
//! The `Exporter` writes one entity, a collection, or everything, each narrowable to a project,
//! as one XML document. `SourceControlFunctions` writes the source-control layout into a file
//! repository folder, or a zip there. Exports read the server; only the source-control export
//! writes to it, so only it is a plan unless applied.

use super::entity_key::ServiceTarget;
use super::server::{Client, ServerError};
use serde_json::{json, Value};
use std::fmt;
use std::time::Duration;

/// What this module asks of a server, as a trait so it is tested offline.
pub trait Remote {
    fn export_xml(&self, collection: Option<&str>, name: Option<&str>, project: Option<&str>) -> Result<Vec<u8>, ServerError>;
    fn source_control(&self, service: &str, body: &Value) -> Result<Option<Value>, ServerError>;
}

impl Remote for Client {
    fn export_xml(&self, collection: Option<&str>, name: Option<&str>, project: Option<&str>) -> Result<Vec<u8>, ServerError> {
        Client::export_xml(self, collection, name, project)
    }

    fn source_control(&self, service: &str, body: &Value) -> Result<Option<Value>, ServerError> {
        self.call_service(&ServiceTarget::platform("Resources", "SourceControlFunctions"), service, body, Duration::from_secs(900))
    }
}

#[derive(Debug)]
pub enum ExportError {
    Remote(ServerError),
    Invalid(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportError::Remote(error) => write!(f, "{error}"),
            ExportError::Invalid(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for ExportError {}

/// What to export through the Exporter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    /// `Collection/Name`.
    Entity { collection: String, name: String },
    Collection { collection: String, project: Option<String> },
    Project { project: String },
}

impl What {
    /// `Things/X` as an entity.
    pub fn entity(text: &str) -> Result<What, ExportError> {
        match text.split_once('/') {
            Some((collection, name)) if !collection.is_empty() && !name.is_empty() && !name.contains('/') => {
                Ok(What::Entity { collection: collection.to_string(), name: name.to_string() })
            }
            _ => Err(ExportError::Invalid(format!("an entity is Collection/Name, such as Things/My.Thing, not {text:?}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exported {
    pub xml: Vec<u8>,
    /// Entities in the document, by collection.
    pub counts: Vec<(String, usize)>,
}

/// The export's XML, with how many entities of each collection it holds.
pub fn export(remote: &dyn Remote, what: &What) -> Result<Exported, ExportError> {
    let xml = match what {
        What::Entity { collection, name } => remote.export_xml(Some(collection), Some(name), None),
        What::Collection { collection, project } => remote.export_xml(Some(collection), None, project.as_deref()),
        What::Project { project } => remote.export_xml(None, None, Some(project)),
    }
    .map_err(ExportError::Remote)?;
    // A login page or a proxy's answer can come back with a 200; it is not an export.
    let root = super::scan::tokenize(&xml).ok().and_then(|tokens| {
        tokens
            .iter()
            .find(|t| matches!(t.kind, super::scan::Kind::Start | super::scan::Kind::Empty))
            .map(|t| t.name.of(&xml).to_vec())
    });
    if root.as_deref() != Some(b"Entities".as_slice()) {
        return Err(ExportError::Invalid(format!(
            "the server's answer is not an <Entities> export: {}",
            String::from_utf8_lossy(&xml[..xml.len().min(120)])
        )));
    }
    let counts = count_entities(&xml);
    Ok(Exported { xml, counts })
}

/// Entities by collection: the children of each collection element under `<Entities>`.
pub fn count_entities(xml: &[u8]) -> Vec<(String, usize)> {
    use super::scan::Kind;
    let Ok(tokens) = super::scan::tokenize(xml) else { return Vec::new() };
    let mut depth = 0usize;
    let mut collection = String::new();
    let mut counts: Vec<(String, usize)> = Vec::new();
    for token in &tokens {
        match token.kind {
            Kind::Start | Kind::Empty => {
                let name = String::from_utf8_lossy(token.name.of(xml)).into_owned();
                match depth {
                    1 => collection = name,
                    2 => match counts.iter_mut().find(|(c, _)| *c == collection) {
                        Some((_, n)) => *n += 1,
                        None => counts.push((collection.clone(), 1)),
                    },
                    _ => {}
                }
                if token.kind == Kind::Start {
                    depth += 1;
                }
            }
            Kind::End => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    counts
}

/// Filters for a source-control export, as the service takes them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub project: Option<String>,
    pub collection: Option<String>,
    pub tags: Option<String>,
    pub include_dependents: bool,
}

/// Write the source-control layout of what `filters` select into a repository folder, or into
/// a zip there when `zip` names one. A plan unless `apply`. Returns what was done, and for a
/// zip, the download link the server gives.
pub fn source_control(
    remote: &dyn Remote,
    repository: &str,
    path: &str,
    filters: &Filters,
    zip: Option<&str>,
    apply: bool,
) -> Result<(String, Option<String>), ExportError> {
    // An empty value is no filter: sent, the server would take it as none and export everything.
    let set = |value: &Option<String>| value.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    let filters = &Filters {
        project: set(&filters.project),
        collection: set(&filters.collection),
        tags: set(&filters.tags),
        include_dependents: filters.include_dependents,
    };
    if filters.project.is_none() && filters.collection.is_none() && filters.tags.is_none() {
        return Err(ExportError::Invalid(
            "a source-control export needs a project, a collection or tags; everything at once is not a development export".to_string(),
        ));
    }
    let mut chosen = Vec::new();
    if let Some(project) = &filters.project {
        chosen.push(format!("project {project}"));
    }
    if let Some(collection) = &filters.collection {
        chosen.push(format!("collection {collection}"));
    }
    if let Some(tags) = &filters.tags {
        chosen.push(format!("tags {tags}"));
    }
    let target = match zip {
        Some(name) => format!("a zip {name} in {repository}:{path}"),
        None => format!("{repository}:{path}"),
    };
    let plan = format!("export {} to {target}{}", chosen.join(", "), if filters.include_dependents { ", with dependents" } else { "" });
    if !apply {
        return Ok((plan, None));
    }
    let mut body = json!({
        "repositoryName": repository,
        "path": path,
        "includeDependents": filters.include_dependents,
    });
    for (key, value) in [("projectName", &filters.project), ("collection", &filters.collection), ("tags", &filters.tags)] {
        if let Some(value) = value {
            body[key] = json!(value);
        }
    }
    let link = match zip {
        Some(name) => {
            // The server appends ".zip" itself: p.zip would become p.zip.zip.
            body["name"] = json!(name.strip_suffix(".zip").unwrap_or(name));
            let reply = remote.source_control("ExportSourceControlledEntitiesToZipFile", &body).map_err(ExportError::Remote)?;
            let link = reply
                .as_ref()
                .and_then(|value| value.pointer("/rows/0/result"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| ExportError::Invalid("the server made the zip export but returned no download link".to_string()))?;
            Some(link)
        }
        None => {
            remote.source_control("ExportSourceControlledEntities", &body).map_err(ExportError::Remote)?;
            None
        }
    };
    Ok((plan, link))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        calls: RefCell<Vec<(String, Value)>>,
    }

    impl Remote for Fake {
        fn export_xml(&self, collection: Option<&str>, name: Option<&str>, project: Option<&str>) -> Result<Vec<u8>, ServerError> {
            self.calls.borrow_mut().push((format!("{collection:?} {name:?} {project:?}"), Value::Null));
            if project == Some("login") {
                return Ok(b"<html><body>Please log in</body></html>".to_vec());
            }
            Ok(b"<Entities><Things><Thing name=\"A\"><x/></Thing><Thing name=\"B\"/></Things><DataShapes><DataShape name=\"D\"/></DataShapes></Entities>".to_vec())
        }

        fn source_control(&self, service: &str, body: &Value) -> Result<Option<Value>, ServerError> {
            self.calls.borrow_mut().push((service.to_string(), body.clone()));
            Ok(Some(json!({ "rows": [{ "result": "/Thingworx/FileRepositories/R/out/x.zip" }] })))
        }
    }

    #[test]
    fn an_entity_is_collection_slash_name() {
        assert_eq!(What::entity("Things/My.Thing").unwrap(), What::Entity { collection: "Things".into(), name: "My.Thing".into() });
        for bad in ["Things", "Things/", "/X", "A/B/C"] {
            assert!(What::entity(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_export_counts_its_entities_by_collection() {
        let fake = Fake { calls: RefCell::new(Vec::new()) };
        let exported = export(&fake, &What::Project { project: "P".into() }).unwrap();
        assert_eq!(exported.counts, [("Things".to_string(), 2), ("DataShapes".to_string(), 1)]);
        assert_eq!(fake.calls.borrow()[0].0, "None None Some(\"P\")");
        export(&fake, &What::Collection { collection: "Things".into(), project: Some("P".into()) }).unwrap();
        assert_eq!(fake.calls.borrow()[1].0, "Some(\"Things\") None Some(\"P\")");
        let error = export(&fake, &What::Project { project: "login".into() }).unwrap_err();
        assert!(error.to_string().contains("not an <Entities> export"), "{error}");
    }

    #[test]
    fn a_source_control_export_is_a_plan_unless_applied_and_needs_a_filter() {
        let fake = Fake { calls: RefCell::new(Vec::new()) };
        let filters = Filters { project: Some("P".into()), ..Filters::default() };
        let (plan, link) = source_control(&fake, "R", "/out", &filters, Some("x.zip"), false).unwrap();
        assert_eq!(plan, "export project P to a zip x.zip in R:/out");
        assert!(link.is_none() && fake.calls.borrow().is_empty());
        let (_, link) = source_control(&fake, "R", "/out", &filters, Some("x.zip"), true).unwrap();
        assert_eq!(link.as_deref(), Some("/Thingworx/FileRepositories/R/out/x.zip"));
        let (service, body) = fake.calls.borrow()[0].clone();
        assert_eq!(service, "ExportSourceControlledEntitiesToZipFile");
        assert_eq!(body["projectName"], "P");
        assert_eq!(body["name"], "x", "the server adds .zip itself");
        assert!(body.get("tags").is_none(), "only what is set is sent");
        assert!(source_control(&fake, "R", "/out", &Filters::default(), None, true).is_err());
        let empty = Filters { project: Some("  ".into()), ..Filters::default() };
        assert!(source_control(&fake, "R", "/out", &empty, None, true).is_err(), "an empty filter is no filter");
    }
}
