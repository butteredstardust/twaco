//! Imports, as Composer's Import/Export dialog makes them.
//!
//! A file goes through the `Importer`, which has no validate mode, so twaco's plan is its own:
//! which entities the file holds, and which the server already has, that the import would
//! replace. A source-control tree in a file repository goes through
//! `SourceControlFunctions.ImportSourceControlledEntities`, and its plan is the server's own
//! `DiffSourceControlledEntities`, which names every entity of the tree whose server copy
//! differs. Both are plans unless applied.

use super::entity_key::ServiceTarget;
use super::server::{Client, ServerError};
use serde_json::{json, Value};
use std::fmt;
use std::io::Read;
use std::time::Duration;

/// What this module asks of a server, as a trait so it is tested offline.
pub trait Remote: Sync {
    /// Whether the server has an entity.
    fn exists(&self, collection: &str, name: &str) -> Result<bool, ServerError>;
    fn import_file(
        &self,
        file_name: &str,
        bytes: &[u8],
        overwrite_properties: bool,
        overwrite_tables: bool,
    ) -> Result<(), ServerError>;
    fn source_control(&self, service: &str, body: &Value) -> Result<Option<Value>, ServerError>;
}

impl Remote for Client {
    fn exists(&self, collection: &str, name: &str) -> Result<bool, ServerError> {
        // The entity's own REST address, which answers 404 for a missing one, for every
        // collection; the Exporter answers 200 with an empty export instead.
        self.entity_exists(collection, name)
    }

    fn import_file(
        &self,
        file_name: &str,
        bytes: &[u8],
        overwrite_properties: bool,
        overwrite_tables: bool,
    ) -> Result<(), ServerError> {
        Client::import_file(
            self,
            file_name,
            bytes,
            overwrite_properties,
            overwrite_tables,
        )
    }

    fn source_control(&self, service: &str, body: &Value) -> Result<Option<Value>, ServerError> {
        self.call_service(
            &ServiceTarget::platform("Resources", "SourceControlFunctions"),
            service,
            body,
            Duration::from_secs(900),
        )
    }
}

#[derive(Debug)]
pub enum ImportError {
    Remote(ServerError),
    Invalid(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::Remote(error) => write!(f, "{error}"),
            ImportError::Invalid(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// `(collection, name)` of every entity an export document holds: the named children of each
/// collection element under `<Entities>`.
pub fn entities_in_xml(xml: &[u8]) -> Result<Vec<(String, String)>, ImportError> {
    use super::scan::Kind;
    let tokens =
        super::scan::tokenize(xml).map_err(|e| ImportError::Invalid(format!("not XML: {e}")))?;
    let mut depth = 0usize;
    let mut collection = String::new();
    let mut found = Vec::new();
    for token in &tokens {
        match token.kind {
            Kind::Start | Kind::Empty => {
                if depth == 1 {
                    collection = String::from_utf8_lossy(token.name.of(xml)).into_owned();
                } else if depth == 2 {
                    if let Ok(Some(name)) = super::scan::attribute(xml, token, "name") {
                        found.push((
                            collection.clone(),
                            super::scan::decode_entities(&String::from_utf8_lossy(name.of(xml))),
                        ));
                    }
                }
                if token.kind == Kind::Start {
                    depth += 1;
                }
            }
            Kind::End => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    if found.is_empty() {
        return Err(ImportError::Invalid(
            "it holds no entities: an export is <Entities><Things><Thing name=...>".to_string(),
        ));
    }
    Ok(found)
}

/// The entities of an import file: one export XML, or a zip of them (the `.xml` entries).
pub fn entities_in_file(
    file_name: &str,
    bytes: &[u8],
) -> Result<Vec<(String, String)>, ImportError> {
    if !bytes.starts_with(b"PK") {
        return entities_in_xml(bytes)
            .map_err(|e| ImportError::Invalid(format!("{file_name}: {e}")));
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| ImportError::Invalid(format!("{file_name}: not a readable zip ({e})")))?;
    let mut found = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| ImportError::Invalid(format!("{file_name}: {e}")))?;
        if !entry.name().to_ascii_lowercase().ends_with(".xml") {
            continue;
        }
        let name = entry.name().to_string();
        let mut xml = Vec::new();
        entry
            .read_to_end(&mut xml)
            .map_err(|e| ImportError::Invalid(format!("{file_name}/{name}: {e}")))?;
        found.extend(
            entities_in_xml(&xml)
                .map_err(|e| ImportError::Invalid(format!("{file_name}/{name}: {e}")))?,
        );
    }
    if found.is_empty() {
        return Err(ImportError::Invalid(format!(
            "{file_name}: a zip with no entity XML in it"
        )));
    }
    Ok(found)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePlan {
    pub new: Vec<(String, String)>,
    /// Entities the server has, which the import replaces.
    pub replaced: Vec<(String, String)>,
    pub applied: bool,
}

/// Import a file: what it holds and what it would replace, and unless `apply` stop there.
/// Applied, every entity it holds must then exist on the server.
pub fn import_file(
    remote: &dyn Remote,
    file_name: &str,
    bytes: &[u8],
    overwrite_properties: bool,
    overwrite_tables: bool,
    apply: bool,
) -> Result<FilePlan, ImportError> {
    let entities = entities_in_file(file_name, bytes)?;
    let present = super::parallel::map(&entities, |(collection, name)| {
        remote.exists(collection, name)
    });
    let mut plan = FilePlan {
        new: Vec::new(),
        replaced: Vec::new(),
        applied: false,
    };
    for (entity, present) in entities.iter().zip(present) {
        if present.map_err(ImportError::Remote)? {
            plan.replaced.push(entity.clone());
        } else {
            plan.new.push(entity.clone());
        }
    }
    if !apply {
        return Ok(plan);
    }
    remote
        .import_file(file_name, bytes, overwrite_properties, overwrite_tables)
        .map_err(ImportError::Remote)?;
    let after = super::parallel::map(&entities, |(collection, name)| {
        remote.exists(collection, name)
    });
    let missing: Vec<String> = entities
        .iter()
        .zip(after)
        .filter(|(_, present)| !matches!(present, Ok(true)))
        .map(|((collection, name), _)| format!("{collection}/{name}"))
        .collect();
    if !missing.is_empty() {
        return Err(ImportError::Invalid(format!(
            "the import was sent and the server said success, but {} of its entities are not there: {}",
            missing.len(),
            missing.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
        )));
    }
    plan.applied = true;
    Ok(plan)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Differs {
    pub entity_type: String,
    pub name: String,
    /// What differs, in the server's words: `diffType name` pairs.
    pub what: Vec<String>,
}

/// A source-control tree in a repository against the server: every entity whose server copy
/// differs, with what differs. Read-only. `entities` is how many the tree holds.
pub fn diff(
    remote: &dyn Remote,
    repository: &str,
    path: &str,
) -> Result<(usize, Vec<Differs>), ImportError> {
    let reply = remote
        .source_control(
            "DiffSourceControlledEntities",
            &json!({ "repositoryName": repository, "path": path, "withSubsystems": false }),
        )
        .map_err(ImportError::Remote)?;
    let rows = reply
        .as_ref()
        .and_then(|value| value.get("rows"))
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| {
            ImportError::Invalid(
                "the server's diff has no rows; it cannot say what differs".to_string(),
            )
        })?;
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut differing = Vec::new();
    for row in &rows {
        let inner = row
            .get("difference")
            .and_then(|d| d.get("rows"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if inner.is_empty() {
            continue;
        }
        let what = inner
            .iter()
            .map(|d| {
                format!("{} {}", text(d, "diffType"), text(d, "name"))
                    .trim()
                    .to_string()
            })
            .collect();
        differing.push(Differs {
            entity_type: text(row, "entityType"),
            name: text(row, "name"),
            what,
        });
    }
    Ok((rows.len(), differing))
}

/// What a source-control import found, and after applying, what still differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeImport {
    /// Entities the tree holds.
    pub total: usize,
    pub differ: Vec<Differs>,
    /// After an applied import; `None` for a plan.
    pub still_differ: Option<Vec<Differs>>,
}

/// Import a source-control tree from a repository. The plan is the server's diff; applied,
/// the import is sent and the diff taken again, and what still differs is returned (a Database
/// Thing's encrypted password, for one, always will).
pub fn import_source_control(
    remote: &dyn Remote,
    repository: &str,
    path: &str,
    overwrite_properties: bool,
    overwrite_tables: bool,
    apply: bool,
) -> Result<TreeImport, ImportError> {
    let (total, before) = diff(remote, repository, path)?;
    if total == 0 {
        return Err(ImportError::Invalid(format!(
            "{repository}:{path} holds no source-controlled entities"
        )));
    }
    if !apply {
        return Ok(TreeImport {
            total,
            differ: before,
            still_differ: None,
        });
    }
    remote
        .source_control(
            "ImportSourceControlledEntities",
            &json!({
                "repositoryName": repository,
                "path": path,
                "useDefaultDataProvider": false,
                "withSubsystems": false,
                "overwritePropertyValues": overwrite_properties,
                "overwriteConfigurationTableValues": overwrite_tables,
            }),
        )
        .map_err(ImportError::Remote)?;
    let (_, after) = diff(remote, repository, path)?;
    Ok(TreeImport {
        total,
        differ: before,
        still_differ: Some(after),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake {
        present: Mutex<Vec<String>>,
        imported: Mutex<usize>,
        diffs: Mutex<Vec<Value>>,
        swallow: bool,
    }

    impl Fake {
        fn with(present: &[&str]) -> Self {
            Fake {
                present: Mutex::new(present.iter().map(|s| s.to_string()).collect()),
                imported: Mutex::new(0),
                diffs: Mutex::new(Vec::new()),
                swallow: false,
            }
        }
    }

    impl Remote for Fake {
        fn exists(&self, collection: &str, name: &str) -> Result<bool, ServerError> {
            Ok(self
                .present
                .lock()
                .unwrap()
                .contains(&format!("{collection}/{name}")))
        }

        fn import_file(
            &self,
            file_name: &str,
            bytes: &[u8],
            _: bool,
            _: bool,
        ) -> Result<(), ServerError> {
            *self.imported.lock().unwrap() += 1;
            if !self.swallow {
                for (c, n) in entities_in_file(file_name, bytes).unwrap() {
                    self.present.lock().unwrap().push(format!("{c}/{n}"));
                }
            }
            Ok(())
        }

        fn source_control(&self, service: &str, _: &Value) -> Result<Option<Value>, ServerError> {
            match service {
                "DiffSourceControlledEntities" => {
                    Ok(Some(json!({ "rows": self.diffs.lock().unwrap().clone() })))
                }
                "ImportSourceControlledEntities" => {
                    // The import settles everything but the Database's password.
                    self.diffs.lock().unwrap().retain(|row| row["name"] == "Db");
                    Ok(None)
                }
                other => panic!("unexpected {other}"),
            }
        }
    }

    const XML: &[u8] = br#"<Entities><Things><Thing name="T1"><x/></Thing><Thing name="T2"/></Things><DataShapes><DataShape name="D"/></DataShapes></Entities>"#;

    #[test]
    fn a_file_lists_its_entities_xml_or_zip() {
        assert_eq!(
            entities_in_xml(XML).unwrap(),
            [
                ("Things".into(), "T1".into()),
                ("Things".into(), "T2".into()),
                ("DataShapes".into(), "D".into())
            ]
        );
        assert!(entities_in_xml(b"<Entities/>").is_err());
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            use std::io::Write;
            let mut zip = zip::ZipWriter::new(&mut buffer);
            zip.start_file("a/one.xml", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(XML).unwrap();
            zip.start_file("readme.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }
        assert_eq!(
            entities_in_file("p.zip", &buffer.into_inner())
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn an_import_says_what_it_adds_and_replaces_and_sends_nothing_unless_applied() {
        let fake = Fake::with(&["Things/T1"]);
        let plan = import_file(&fake, "e.xml", XML, false, false, false).unwrap();
        assert_eq!(plan.replaced, [("Things".to_string(), "T1".to_string())]);
        assert_eq!(plan.new.len(), 2);
        assert_eq!(*fake.imported.lock().unwrap(), 0);
        assert!(
            import_file(&fake, "e.xml", XML, false, false, true)
                .unwrap()
                .applied
        );
        assert_eq!(*fake.imported.lock().unwrap(), 1);
    }

    #[test]
    fn an_import_the_server_did_not_keep_is_an_error() {
        let mut fake = Fake::with(&[]);
        fake.swallow = true;
        let error = import_file(&fake, "e.xml", XML, false, false, true).unwrap_err();
        assert!(error.to_string().contains("are not there"), "{error}");
    }

    #[test]
    fn a_source_control_import_plans_with_the_servers_diff_and_reports_what_still_differs() {
        let fake = Fake::with(&[]);
        let same = json!({ "rows": [] });
        let changed = json!({ "rows": [{ "diffType": "Modified", "name": "description" }] });
        *fake.diffs.lock().unwrap() = vec![
            json!({ "entityType": "Thing", "name": "Same", "difference": same }),
            json!({ "entityType": "DataShape", "name": "D", "difference": changed }),
            json!({ "entityType": "Thing", "name": "Db", "difference": changed }),
        ];
        let plan = import_source_control(&fake, "R", "/sc", false, false, false).unwrap();
        assert_eq!(plan.total, 3);
        assert_eq!(
            plan.differ
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>(),
            ["D", "Db"]
        );
        assert_eq!(plan.differ[0].what, ["Modified description"]);
        assert!(plan.still_differ.is_none());
        let applied = import_source_control(&fake, "R", "/sc", false, false, true).unwrap();
        assert_eq!(
            applied
                .still_differ
                .unwrap()
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>(),
            ["Db"]
        );
    }
}
