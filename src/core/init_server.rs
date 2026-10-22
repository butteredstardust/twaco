//! Start an empty source repository from Exporter project documents.

use super::{
    adopt, commands, config, entity, export, gitignore, init, layout, localization, lock,
    normalise, progress, push, status, transaction, workflow, workspace,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const REFUSAL: &str = "this folder has entity files but no twaco.toml; run `twaco init --write` first, or start in an empty folder";

#[derive(Debug)]
pub struct Request {
    pub root: PathBuf,
    pub projects: Vec<String>,
    pub apply: bool,
    /// The profile the baseline is recorded against: the server the exports came from.
    pub profile: String,
}

#[derive(Debug)]
pub struct Outcome {
    pub projects: Vec<String>,
    pub files: usize,
    pub entities: BTreeMap<String, usize>,
    pub toml: String,
    pub notes: Vec<String>,
    pub applied: bool,
    pub added: bool,
    pub recorded: usize,
    pub not_in_sync: Vec<String>,
    pub localization_files: usize,
    pub localization_tokens: usize,
    pub localization_failure: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Refused(&'static str),
    #[error("twaco.toml does not load: {0}")]
    Config(#[from] config::ConfigError),
    #[error("project {0} is already in this solution")]
    AlreadyPresent(String),
    #[error("planned files already exist: {0}")]
    ExistingFiles(String),
    #[error("project {0} is named twice")]
    Duplicate(String),
    #[error("{0}; fix it before adding projects, whose sidecars and baseline would stop on it")]
    Unreadable(String),
    #[error("init --from-server needs at least one project name")]
    Projects,
    #[error("project {project}: {why}")]
    Export { project: String, why: String },
    #[error("{entity}: the name {name:?} cannot be a file name; nothing was written")]
    UnsafeName { entity: String, name: String },
    #[error("{0}")]
    Write(#[from] transaction::TransactionError),
    #[error("{0}")]
    Lock(#[from] lock::LockError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Everything after the repository's files were written: they stay, and a rerun refuses.
    #[error("the repository was written, but {why}; finish it with `twaco extract --all` and `twaco entity status --all --record`")]
    Unfinished { why: String },
}

impl super::codes::Coded for Error {
    fn code(&self) -> super::codes::ErrorCode {
        match self {
            Self::Refused(_) | Self::Projects | Self::Duplicate(_) => {
                super::codes::ErrorCode::InvalidArguments
            }
            Self::Unreadable(_) => super::codes::ErrorCode::InvalidData,
            Self::Config(error) => error.code(),
            Self::AlreadyPresent(_) | Self::ExistingFiles(_) => {
                super::codes::ErrorCode::AlreadyExists
            }
            Self::Export { .. } | Self::UnsafeName { .. } => super::codes::ErrorCode::InvalidData,
            Self::Write(error) => error.code(),
            Self::Lock(error) => error.code(),
            Self::Io(_) => super::codes::ErrorCode::IoError,
            Self::Unfinished { .. } => super::codes::ErrorCode::Unclassified,
        }
    }
}

struct File {
    path: PathBuf,
    bytes: Vec<u8>,
}

/// Everything that can be refused without the server, so a front end can say it before it needs a
/// profile: the existing solution when there is one.
pub fn preflight(root: &Path, projects: &[String]) -> Result<Option<config::Solution>, Error> {
    if projects.is_empty() {
        return Err(Error::Projects);
    }
    let mut seen = BTreeSet::new();
    if let Some(twice) = projects.iter().find(|project| !seen.insert(*project)) {
        return Err(Error::Duplicate(twice.clone()));
    }
    let config_path = root.join(super::config::CONFIG_FILE);
    if !config_path.exists() {
        if init::has_entity_files(root) {
            return Err(Error::Refused(REFUSAL));
        }
        return Ok(None);
    }
    let solution = config::Solution::load(&config_path)?;
    if let Some(project) = projects.iter().find(|p| solution.project(p).is_some()) {
        return Err(Error::AlreadyPresent(project.clone()));
    }
    // Sidecars and the baseline of the added projects are made after the files are written; an
    // unreadable entity file would stop them then.
    let found = workspace::discover(&solution);
    if let Some(unreadable) = found.unreadable.first() {
        return Err(Error::Unreadable(unreadable.clone()));
    }
    Ok(Some(solution))
}

/// Read each project through the same Exporter endpoint as `export project`, plan every local
/// file first, and only then write all of them as one transaction.
pub fn execute<R>(remote: &R, request: Request) -> Result<Outcome, Error>
where
    R: export::Remote + push::Remote + localization::Remote + Sync,
{
    let existing = preflight(&request.root, &request.projects)?;
    let config_path = request.root.join(super::config::CONFIG_FILE);
    let config_before = match &existing {
        Some(_) => Some(std::fs::read(&config_path)?),
        None => None,
    };
    let added = existing.is_some();
    let named: BTreeSet<String> = request.projects.iter().cloned().collect();
    let multiple = named.len() > 1;
    let mut files = Vec::new();
    let mut notes = Vec::new();
    let mut counts = BTreeMap::new();
    let mut per_project = BTreeMap::<String, usize>::new();
    let mut dependencies = BTreeMap::<String, Vec<String>>::new();
    let mut members_owned = Vec::new();
    let mut names = Vec::new();
    for project in &request.projects {
        let exported = export::export(
            remote,
            &export::What::Project {
                project: project.clone(),
            },
        )
        .map_err(|e| Error::Export {
            project: project.clone(),
            why: e.to_string(),
        })?;
        let entities =
            adopt::export_entities_bytes(&exported.xml, Path::new(project)).map_err(|e| {
                Error::Export {
                    project: project.clone(),
                    why: e.to_string(),
                }
            })?;
        for (key, element) in entities {
            let raw = adopt::raw_entity_bytes(&exported.xml, &key).map_err(|e| Error::Export {
                project: project.clone(),
                why: format!("{e:?}"),
            })?;
            let info = entity::parse(
                &[
                    b"<Entities><".as_slice(),
                    key.collection.as_bytes(),
                    b">",
                    &raw,
                    b"</",
                    key.collection.as_bytes(),
                    b"></Entities>",
                ]
                .concat(),
            )
            .map_err(|e| Error::Export {
                project: project.clone(),
                why: format!("{e:?}"),
            })?;
            // A project's export carries entities of others it needs; each named project's own
            // export brings its own, so an entity is written once, from its project's export.
            if info.project != *project {
                if !named.contains(&info.project) {
                    notes.push(format!(
                        "skipped {}: it declares projectName {:?}",
                        key.path(),
                        info.project
                    ));
                }
                continue;
            }
            if !adopt::is_file_name(&info.name) {
                return Err(Error::UnsafeName {
                    entity: key.path(),
                    name: info.name,
                });
            }
            if key.collection == "Projects" {
                let needs: Vec<String> = depends_on_projects(&element)
                    .into_iter()
                    .filter(|name| {
                        *name != info.project
                            && (named.contains(name)
                                || existing.as_ref().is_some_and(|s| s.project(name).is_some()))
                    })
                    .collect();
                dependencies.insert(info.project.clone(), needs);
            }
            let laid_out = layout::entity(&raw, None, false).map_err(|e| Error::Export {
                project: project.clone(),
                why: e.to_string(),
            })?;
            // As adopt writes a new entity that has no peer file to copy.
            let document = entity::new::document(&key.collection, &laid_out);
            if matches!(key.collection.as_str(), "Groups" | "Organizations")
                && differs_only_in_members(remote, &key.collection, &info.name, &document).map_err(
                    |why| Error::Export {
                        project: project.clone(),
                        why,
                    },
                )?
            {
                members_owned.push(key.path());
            }
            let base = if added || multiple {
                request.root.join(&info.project)
            } else {
                request.root.clone()
            };
            let path = base
                .join(&key.collection)
                .join(format!("{}.xml", info.name));
            *counts.entry(key.collection.clone()).or_insert(0usize) += 1;
            *per_project.entry(info.project).or_insert(0usize) += 1;
            names.push(info.name.clone());
            files.push(File {
                path,
                bytes: document,
            });
        }
    }
    if files.is_empty() {
        notes.push("the server exports contained no entities for the named projects".to_string());
    }
    let mut toml = if existing.is_some() {
        let before = String::from_utf8_lossy(config_before.as_deref().unwrap_or_default());
        append_projects(&before, &request.projects, &dependencies)
    } else {
        let proposal_files = files
            .iter()
            .map(|file| (file.path.clone(), file.bytes.clone()))
            .collect();
        let mut proposal = init::propose_files(&request.root, proposal_files);
        add_project_dependencies(&mut proposal.toml, &dependencies);
        proposal.notes.extend(notes);
        notes = proposal.notes;
        proposal.toml
    };
    if !members_owned.is_empty() {
        add_members_owned(&mut toml, &members_owned);
        notes.push(format!(
            "{} keep their members on the server: the Exporter leaves out user members, so membership is [[server_owned]]",
            members_owned.join(", ")
        ));
    }
    for (project, count) in per_project {
        notes.push(format!("{project}: {count} entities"));
    }
    let entity_files = files.len();
    let mut all = files;
    if !added {
        let project_names: Vec<String> = request.projects.clone();
        for (name, text) in init::agent_files(&request.projects[0], &project_names) {
            let path = request.root.join(name);
            if !path.exists() {
                all.push(File {
                    path,
                    bytes: text.into_bytes(),
                });
            }
        }
        if gitignore::in_git_work_tree(&request.root) {
            if let Some(bytes) = gitignore::planned_append(&request.root)? {
                all.push(File {
                    path: request.root.join(".gitignore"),
                    bytes,
                });
            }
        }
    }
    all.push(File {
        path: request.root.join(super::config::CONFIG_FILE),
        bytes: toml.clone().into_bytes(),
    });
    if let Some(solution) = &existing {
        // Extraction writes each added entity's sidecar folder after the transaction.
        let src = solution.src_root();
        let existing: Vec<_> = all
            .iter()
            .map(|file| file.path.clone())
            .filter(|path| *path != config_path)
            .chain(names.iter().map(|name| src.join(name)))
            .filter(|path| path.exists())
            .take(5)
            .map(|path| path.display().to_string())
            .collect();
        if !existing.is_empty() {
            return Err(Error::ExistingFiles(existing.join(", ")));
        }
    }
    let files_count = all.len();
    let mut recorded = 0;
    let mut not_in_sync = Vec::new();
    let mut localization_files = 0;
    let mut localization_tokens = 0;
    let mut localization_failure = None;
    if request.apply {
        let held = lock::acquire(&request.root, "init --from-server", &[])?;
        let mut write = transaction::Transaction::new(&request.root, "init --from-server");
        for file in all {
            // The configuration replaces exactly what was planned on, so an edit made meanwhile is
            // refused rather than lost.
            match &config_before {
                Some(before) if file.path == config_path => {
                    write.replace_file(&file.path, before, file.bytes)?
                }
                _ => write.write_file(&file.path, file.bytes)?,
            }
        }
        write.apply(&held)?;
        // The sidecars are what `twaco extract --all` makes of the files just written, made by
        // the same code; each entity's are one transaction of their own. Then the baseline,
        // under the same lock, so it describes exactly these files.
        let unfinished = |why: String| Error::Unfinished { why };
        let solution = config::Solution::load(&request.root.join(config::CONFIG_FILE))
            .map_err(|error| unfinished(format!("its twaco.toml does not load: {error}")))?;
        let found = workspace::discover(&solution);
        let chosen: Vec<_> = if added {
            found
                .entities
                .iter()
                .filter(|entity| named.contains(&entity.found_under))
                .cloned()
                .collect()
        } else {
            found.entities.clone()
        };
        let extracted = workflow::extract(&solution, &chosen, &found.unreadable, false, &held);
        if extracted.failed > 0 {
            return Err(unfinished(format!(
                "{} entities' sidecars could not be extracted",
                extracted.failed
            )));
        }
        let status_projects: Vec<Option<String>> = if added {
            request.projects.iter().cloned().map(Some).collect()
        } else {
            vec![None]
        };
        for project in status_projects {
            let status = commands::status::execute_holding(
                &solution,
                &commands::status::StatusRequest {
                    target: commands::status::StatusTarget::All,
                    project,
                    record: true,
                    profile: request.profile.clone(),
                    lock_label: "init --from-server",
                    refuse_unreadable: true,
                    refuse_record_failures: false,
                },
                |_| Borrowed(remote),
                &held,
                &progress::NONE,
            )
            .map_err(|error| unfinished(format!("the baseline was not recorded: {error}")))?;
            recorded += status.recorded.unwrap_or(0);
            not_in_sync.extend(
                status
                    .statuses
                    .iter()
                    .filter(|entity| entity.verdict != status::Verdict::InSync)
                    .map(|entity| format!("{}/{}", entity.collection, entity.name))
                    .chain(status.failures),
            );
        }
        match localization::ops::pull_projects(
            &solution,
            remote,
            None,
            false,
            Some(&held),
            Some(&request.projects),
        ) {
            Ok(pulled) => {
                localization_files = pulled.files.len();
                localization_tokens = pulled.files.iter().map(|file| file.set.len()).sum();
            }
            Err(error) => {
                let note =
                    format!("localization was not pulled: {error}; run `twaco localization pull`");
                notes.push(note.clone());
                localization_failure = Some(note);
            }
        }
    } else {
        notes.push(format!(
            "{entity_files} entity files; their sidecars are extracted once they are written"
        ));
    }
    Ok(Outcome {
        projects: request.projects,
        files: files_count,
        entities: counts,
        toml,
        notes,
        applied: request.apply,
        added,
        recorded,
        not_in_sync,
        localization_files,
        localization_tokens,
        localization_failure,
    })
}

/// Exporter stores project dependencies in a Project's `dependsOn` JSON attribute.  Its exact
/// extension-version syntax is server-owned, but project names are stable; only names that were
/// requested become local deploy dependencies.
fn add_project_dependencies(toml: &mut String, dependencies: &BTreeMap<String, Vec<String>>) {
    for (project, needed) in dependencies {
        if needed.is_empty() {
            continue;
        }
        let quoted = format!("\"{}\"", project.replace('"', "\\\""));
        let marker = format!("[[project]]\nname = {quoted}\n");
        let Some(start) = toml.find(&marker) else {
            continue;
        };
        let root_end = toml[start + marker.len()..]
            .find('\n')
            .map(|at| start + marker.len() + at + 1)
            .unwrap_or(toml.len());
        let names = needed
            .iter()
            .map(|name| format!("\"{}\"", name.replace('"', "\\\"")))
            .collect::<Vec<_>>()
            .join(", ");
        toml.insert_str(root_end, &format!("depends_on = [{names}]\n"));
    }
    // The server said the deploy order, so the files' hint that they cannot is not needed.
    if dependencies.values().any(|needed| !needed.is_empty()) {
        *toml = toml.replace("\n# Deploy order is not something the files can say. If a project imports after another,\n# give it depends_on = [\"<that project>\"].\n", "");
    }
}

/// Keep a configured solution's text intact and put each newly discovered project after it.
fn append_projects(
    before: &str,
    projects: &[String],
    dependencies: &BTreeMap<String, Vec<String>>,
) -> String {
    let mut toml = before.to_string();
    if !toml.ends_with('\n') {
        toml.push('\n');
    }
    for project in projects {
        toml.push_str("\n[[project]]\n");
        toml.push_str(&format!("name = \"{}\"\n", project.replace('"', "\\\"")));
        toml.push_str(&format!("root = \"{}\"\n", project.replace('"', "\\\"")));
        if let Some(needs) = dependencies.get(project).filter(|needs| !needs.is_empty()) {
            let names = needs
                .iter()
                .map(|name| format!("\"{}\"", name.replace('"', "\\\"")))
                .collect::<Vec<_>>()
                .join(", ");
            toml.push_str(&format!("depends_on = [{names}]\n"));
        }
    }
    toml
}

/// Whether the server's own copy of a Group or Organization differs from its exported file only
/// in members: the Exporter leaves out user members (observed 2026-10-22), which the read
/// `entity status` compares holds, so such an entity could never be in sync.
fn differs_only_in_members<R: push::Remote>(
    remote: &R,
    collection: &str,
    name: &str,
    document: &[u8],
) -> Result<bool, String> {
    let key =
        super::entity::key::EntityKey::address(collection, name).map_err(|e| e.to_string())?;
    let Some(server) = remote.fetch(&key).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    let hash = |bytes: &[u8], parts: &[super::server_owned::Part]| {
        normalise::hash_with_parts(bytes, parts).map_err(|e| e.to_string())
    };
    let members = [super::server_owned::Part::Members];
    Ok(hash(document, &[])? != hash(&server, &[])?
        && hash(document, &members)? == hash(&server, &members)?)
}

/// Membership is assigned per server; the rule keeps it out of comparison and as the server has it.
fn add_members_owned(toml: &mut String, entities: &[String]) {
    let names = entities
        .iter()
        .map(|name| format!("\"{}\"", name.replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ");
    toml.push_str(&format!(
        "\n# Membership is assigned per server; the export leaves out user members. All members are\n# then the server's, group members in these files included: a change to them is not deployed.\n[[server_owned]]\nentities = [{names}]\nparts = [\"members\"]\n"
    ));
}

/// The remote the exports came from, lent to the baseline step.
struct Borrowed<'a, R>(&'a R);

impl<R: push::Remote> push::Remote for Borrowed<'_, R> {
    fn fetch(
        &self,
        key: &super::entity::key::EntityKey,
    ) -> Result<Option<Vec<u8>>, super::server::ServerError> {
        self.0.fetch(key)
    }
    fn export_owned(
        &self,
        key: &super::entity::key::EntityKey,
    ) -> Result<Option<Vec<u8>>, super::server::ServerError> {
        self.0.export_owned(key)
    }
    fn import(&self, file_name: &str, xml: &[u8]) -> Result<(), super::server::ServerError> {
        self.0.import(file_name, xml)
    }
}

/// The projects a Project entity's `dependsOn` names: `{"projects": "A:1.0.0,B:0.0.0"}`.
fn depends_on_projects(project: &normalise::Element) -> Vec<String> {
    #[derive(serde::Deserialize, Default)]
    struct Depends {
        #[serde(default)]
        projects: String,
    }
    let Some(text) = project
        .attributes
        .iter()
        .find(|(name, _)| name == b"dependsOn")
        .map(|(_, value)| String::from_utf8_lossy(value).into_owned())
    else {
        return Vec::new();
    };
    let depends: Depends = serde_json::from_str(&text).unwrap_or_default();
    depends
        .projects
        .split(',')
        .filter_map(|item| item.trim().split(':').next())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    struct Fake(BTreeMap<String, Vec<u8>>);
    impl export::Remote for Fake {
        fn export_xml(
            &self,
            _: Option<&str>,
            _: Option<&str>,
            project: Option<&str>,
        ) -> Result<Vec<u8>, super::super::server::ServerError> {
            Ok(self.0.get(project.unwrap()).cloned().unwrap())
        }
        fn source_control(
            &self,
            _: &str,
            _: &Value,
        ) -> Result<Option<Value>, super::super::server::ServerError> {
            unreachable!("init --from-server only reads Exporter")
        }
    }

    impl push::Remote for Fake {
        fn fetch(
            &self,
            _: &super::super::entity::key::EntityKey,
        ) -> Result<Option<Vec<u8>>, super::super::server::ServerError> {
            Ok(None)
        }
        fn import(&self, _: &str, _: &[u8]) -> Result<(), super::super::server::ServerError> {
            unreachable!("init --from-server never imports")
        }
    }
    impl super::super::imports::Remote for Fake {
        fn exists(
            &self,
            _: &super::super::entity::key::EntityKey,
        ) -> Result<bool, super::super::server::ServerError> {
            Ok(false)
        }
        fn import_file(
            &self,
            _: &str,
            _: &[u8],
            _: bool,
            _: bool,
        ) -> Result<(), super::super::server::ServerError> {
            unreachable!("init --from-server never imports")
        }
        fn source_control(
            &self,
            _: &str,
            _: &Value,
        ) -> Result<Option<Value>, super::super::server::ServerError> {
            unreachable!("init --from-server does not use source control")
        }
    }
    impl localization::Remote for Fake {
        fn tables(&self) -> Result<Vec<String>, super::super::server::ServerError> {
            Ok(Vec::new())
        }
        fn tokens(
            &self,
            _: &str,
        ) -> Result<Vec<localization::Token>, super::super::server::ServerError> {
            Ok(Vec::new())
        }
        fn header(
            &self,
            _: &str,
        ) -> Result<localization::Header, super::super::server::ServerError> {
            Ok(localization::Header::default())
        }
        fn delete_token(&self, _: &str, _: &str) -> Result<(), super::super::server::ServerError> {
            unreachable!("init --from-server never deletes tokens")
        }
    }

    /// A profile in `root`, as one in the user's home would serve, for the baseline step.
    pub(super) fn profile(root: &Path) {
        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "url = \"http://127.0.0.1:1\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
    }

    fn export(project: &str, name: &str) -> Vec<u8> {
        format!("<Entities><Things><Thing name=\"{name}\" projectName=\"{project}\"><ThingShape><ServiceDefinitions><ServiceDefinition name=\"S\"/></ServiceDefinitions><ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[me();]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>").into_bytes()
    }

    #[test]
    fn plans_then_writes_the_adopt_layout_and_sidecars() {
        let temp = tempfile::tempdir().unwrap();
        let remote = Fake(BTreeMap::from([("P".to_string(), export("P", "P.T"))]));
        let request = || Request {
            root: temp.path().to_path_buf(),
            projects: vec!["P".into()],
            apply: false,
            profile: "default".into(),
        };
        profile(temp.path());
        let plan = execute(&remote, request()).unwrap();
        assert!(!plan.applied);
        assert!(!temp.path().join("twaco.toml").exists());
        let mut apply = request();
        apply.apply = true;
        execute(&remote, apply).unwrap();
        assert!(temp.path().join("Things/P.T.xml").exists());
        assert_eq!(
            std::fs::read_to_string(temp.path().join("src/P.T/services/S/script.js")).unwrap(),
            "me();"
        );
        assert!(
            std::fs::read_to_string(temp.path().join("src/P.T/services/S/definition.xml"))
                .unwrap()
                .contains("name=\"S\"")
        );
    }

    #[test]
    fn refuses_an_invalid_config_before_asking_the_remote() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("twaco.toml"), "").unwrap();
        let remote = Fake(BTreeMap::new());
        let error = execute(
            &remote,
            Request {
                root: temp.path().to_path_buf(),
                projects: vec!["P".into()],
                apply: false,
                profile: "default".into(),
            },
        )
        .unwrap_err();
        assert!(matches!(error, Error::Config(_)));
    }

    #[test]
    fn adds_to_a_solution_without_rewriting_its_configuration_or_existing_project() {
        let temp = tempfile::tempdir().unwrap();
        profile(temp.path());
        let before = "# keep this comment\n[[project]]\nname = \"A\"\nroot = \".\"\n";
        std::fs::write(temp.path().join("twaco.toml"), before).unwrap();
        std::fs::create_dir_all(temp.path().join("Things")).unwrap();
        let existing = temp.path().join("Things/A.T.xml");
        std::fs::write(
            &existing,
            "<Entities><Things><Thing name=\"A.T\" projectName=\"A\"/></Things></Entities>",
        )
        .unwrap();
        std::fs::write(
            temp.path().join(".twaco/baseline.json"),
            r#"{"entities":{"Things":{"A.T":{"local":"v7:old","server":"v7:old"}}}}"#,
        )
        .unwrap();
        let remote = Fake(BTreeMap::from([("B".to_string(), export("B", "B.T"))]));
        let outcome = execute(
            &remote,
            Request {
                root: temp.path().to_path_buf(),
                projects: vec!["B".into()],
                apply: true,
                profile: "default".into(),
            },
        )
        .unwrap();
        assert!(outcome.added);
        assert_eq!(
            std::fs::read_to_string(&existing).unwrap(),
            "<Entities><Things><Thing name=\"A.T\" projectName=\"A\"/></Things></Entities>"
        );
        assert!(temp.path().join("B/Things/B.T.xml").is_file());
        let toml = std::fs::read_to_string(temp.path().join("twaco.toml")).unwrap();
        assert!(toml.starts_with(before));
        assert!(toml.ends_with("[[project]]\nname = \"B\"\nroot = \"B\"\n"));
        assert!(
            std::fs::read_to_string(temp.path().join(".twaco/baseline.json"))
                .unwrap()
                .contains("\"A.T\"")
        );
        let found =
            workspace::discover(&config::Solution::load(&temp.path().join("twaco.toml")).unwrap());
        assert_eq!(found.entities.len(), 2);
        assert_eq!(
            found
                .entities
                .iter()
                .filter(|entity| entity.found_under == "B")
                .count(),
            1
        );
    }

    #[test]
    fn refuses_before_writing_what_would_stop_it_later() {
        let solution = || {
            let temp = tempfile::tempdir().unwrap();
            profile(temp.path());
            std::fs::write(
                temp.path().join("twaco.toml"),
                "[[project]]\nname = \"A\"\nroot = \".\"\n",
            )
            .unwrap();
            temp
        };
        let remote = Fake(BTreeMap::from([("B".to_string(), export("B", "B.T"))]));
        let run = |root: &Path, projects: &[&str]| {
            execute(
                &remote,
                Request {
                    root: root.to_path_buf(),
                    projects: projects.iter().map(|p| p.to_string()).collect(),
                    apply: true,
                    profile: "default".into(),
                },
            )
            .unwrap_err()
        };
        let named_twice = solution();
        assert!(matches!(
            run(named_twice.path(), &["B", "B"]),
            Error::Duplicate(_)
        ));
        // An orphaned sidecar folder of an entity to be added.
        let orphan = solution();
        std::fs::create_dir_all(orphan.path().join("src/B.T/services/S")).unwrap();
        assert!(matches!(
            run(orphan.path(), &["B"]),
            Error::ExistingFiles(files) if files.contains("B.T")
        ));
        // An entity file of the existing project that discovery cannot read as one entity.
        let unreadable = solution();
        std::fs::create_dir_all(unreadable.path().join("Things")).unwrap();
        std::fs::write(
            unreadable.path().join("Things/A.T.xml"),
            "<Entities><Groups></Groups><Things><Thing name=\"A.T\" projectName=\"A\"/></Things></Entities>",
        )
        .unwrap();
        assert!(matches!(
            run(unreadable.path(), &["B"]),
            Error::Unreadable(_)
        ));
        for temp in [&named_twice, &orphan, &unreadable] {
            assert!(!temp.path().join("B").exists());
            assert_eq!(
                std::fs::read_to_string(temp.path().join("twaco.toml")).unwrap(),
                "[[project]]\nname = \"A\"\nroot = \".\"\n"
            );
        }
    }

    #[test]
    fn refuses_an_existing_project_and_an_entity_folder_without_config() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("twaco.toml"),
            "[[project]]\nname = \"P\"\n",
        )
        .unwrap();
        let remote = Fake(BTreeMap::new());
        let error = execute(
            &remote,
            Request {
                root: temp.path().to_path_buf(),
                projects: vec!["P".into()],
                apply: false,
                profile: "default".into(),
            },
        )
        .unwrap_err();
        assert!(matches!(error, Error::AlreadyPresent(_)));

        let empty = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(empty.path().join("Things")).unwrap();
        std::fs::write(empty.path().join("Things/P.T.xml"), export("P", "P.T")).unwrap();
        let error = execute(
            &remote,
            Request {
                root: empty.path().to_path_buf(),
                projects: vec!["P".into()],
                apply: false,
                profile: "default".into(),
            },
        )
        .unwrap_err();
        assert!(matches!(error, Error::Refused(_)));
        assert!(error.to_string().contains("twaco init --write"));
    }

    #[test]
    fn an_entity_another_named_project_owns_comes_from_its_own_export_once() {
        let temp = tempfile::tempdir().unwrap();
        profile(temp.path());
        // A's export carries B.T, which A needs; B's export carries it too.
        let mut a = String::from_utf8(export("A", "A.T")).unwrap();
        let b_thing = String::from_utf8(export("B", "B.T")).unwrap();
        let inner =
            &b_thing["<Entities><Things>".len()..b_thing.len() - "</Things></Entities>".len()];
        a = a.replace(
            "</Things></Entities>",
            &format!("{inner}</Things></Entities>"),
        );
        let remote = Fake(BTreeMap::from([
            ("A".to_string(), a.into_bytes()),
            ("B".to_string(), export("B", "B.T")),
        ]));
        let outcome = execute(
            &remote,
            Request {
                root: temp.path().to_path_buf(),
                projects: vec!["A".into(), "B".into()],
                apply: true,
                profile: "default".into(),
            },
        )
        .unwrap();
        assert_eq!(outcome.entities.get("Things"), Some(&2));
        assert!(temp.path().join("B/Things/B.T.xml").exists());
        assert!(!temp.path().join("A/Things/B.T.xml").exists());
    }

    #[test]
    fn a_failure_after_the_files_are_written_says_how_to_finish() {
        let temp = tempfile::tempdir().unwrap();
        // No profile: the baseline step cannot run once the files are there.
        let remote = Fake(BTreeMap::from([("P".to_string(), export("P", "P.T"))]));
        let error = execute(
            &remote,
            Request {
                root: temp.path().to_path_buf(),
                projects: vec!["P".into()],
                apply: true,
                profile: "missing".into(),
            },
        )
        .unwrap_err();
        assert!(matches!(error, Error::Unfinished { .. }), "{error}");
        let message = error.to_string();
        assert!(
            message.contains("the repository was written")
                && message.contains("twaco extract --all")
                && message.contains("twaco entity status --all --record"),
            "{message}"
        );
        assert!(temp.path().join("Things/P.T.xml").exists());
    }

    /// The project export, and the server's own copy of each entity by `Collection/Name`.
    struct Members(Vec<u8>, BTreeMap<String, Vec<u8>>);
    impl export::Remote for Members {
        fn export_xml(
            &self,
            _: Option<&str>,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<Vec<u8>, super::super::server::ServerError> {
            Ok(self.0.clone())
        }
        fn source_control(
            &self,
            _: &str,
            _: &Value,
        ) -> Result<Option<Value>, super::super::server::ServerError> {
            unreachable!("init --from-server only reads Exporter")
        }
    }
    impl push::Remote for Members {
        fn fetch(
            &self,
            key: &super::super::entity::key::EntityKey,
        ) -> Result<Option<Vec<u8>>, super::super::server::ServerError> {
            Ok(self.1.get(&key.to_string()).cloned())
        }
        fn import(&self, _: &str, _: &[u8]) -> Result<(), super::super::server::ServerError> {
            unreachable!("init --from-server never imports")
        }
    }
    impl super::super::imports::Remote for Members {
        fn exists(
            &self,
            _: &super::super::entity::key::EntityKey,
        ) -> Result<bool, super::super::server::ServerError> {
            Ok(false)
        }
        fn import_file(
            &self,
            _: &str,
            _: &[u8],
            _: bool,
            _: bool,
        ) -> Result<(), super::super::server::ServerError> {
            unreachable!()
        }
        fn source_control(
            &self,
            _: &str,
            _: &Value,
        ) -> Result<Option<Value>, super::super::server::ServerError> {
            unreachable!()
        }
    }
    impl localization::Remote for Members {
        fn tables(&self) -> Result<Vec<String>, super::super::server::ServerError> {
            Ok(Vec::new())
        }
        fn tokens(
            &self,
            _: &str,
        ) -> Result<Vec<localization::Token>, super::super::server::ServerError> {
            Ok(Vec::new())
        }
        fn header(
            &self,
            _: &str,
        ) -> Result<localization::Header, super::super::server::ServerError> {
            Ok(localization::Header::default())
        }
        fn delete_token(&self, _: &str, _: &str) -> Result<(), super::super::server::ServerError> {
            unreachable!()
        }
    }

    #[test]
    fn a_group_whose_user_members_the_export_leaves_out_keeps_them_on_the_server() {
        let temp = tempfile::tempdir().unwrap();
        profile(temp.path());
        let group = |name: &str, members: &str| {
            format!("<Group name=\"{name}\" projectName=\"P\"><Members><Members>{members}</Members></Members></Group>")
        };
        let wrap = |inner: String| format!("<Entities><Groups>{inner}</Groups></Entities>");
        let in_group = "<Member name=\"P.Other_UG\" type=\"Group\"/>";
        let user = "<Member name=\"someone\" type=\"User\"/>";
        let remote = Members(
            wrap(group("P.Editors_UG", "") + group("P.Admin_UG", in_group).as_str()).into_bytes(),
            BTreeMap::from([
                (
                    "Groups/P.Editors_UG".to_string(),
                    wrap(group("P.Editors_UG", user)).into_bytes(),
                ),
                (
                    "Groups/P.Admin_UG".to_string(),
                    wrap(group("P.Admin_UG", in_group)).into_bytes(),
                ),
            ]),
        );
        let outcome = execute(
            &remote,
            Request {
                root: temp.path().to_path_buf(),
                projects: vec!["P".into()],
                apply: true,
                profile: "default".into(),
            },
        )
        .unwrap();
        assert!(outcome.not_in_sync.is_empty(), "{:?}", outcome.not_in_sync);
        assert_eq!(outcome.recorded, 2);
        assert!(
            outcome.toml.contains(
                "[[server_owned]]\nentities = [\"Groups/P.Editors_UG\"]\nparts = [\"members\"]\n"
            ),
            "{}",
            outcome.toml
        );
    }

    #[test]
    fn several_projects_get_separate_entity_roots() {
        let temp = tempfile::tempdir().unwrap();
        profile(temp.path());
        let remote = Fake(BTreeMap::from([
            ("A".to_string(), export("A", "A.T")),
            ("B".to_string(), export("B", "B.T")),
        ]));
        execute(
            &remote,
            Request {
                root: temp.path().to_path_buf(),
                projects: vec!["A".into(), "B".into()],
                apply: true,
                profile: "default".into(),
            },
        )
        .unwrap();
        assert!(temp.path().join("A/Things/A.T.xml").exists());
        assert!(temp.path().join("B/Things/B.T.xml").exists());
        assert!(temp.path().join("src/A.T/services/S/script.js").exists());
    }
}

/// The exit criterion: the bundled corpus, as the server's Exporter presents it, comes back as the
/// corpus itself.
#[cfg(test)]
mod corpus_tests {
    use super::*;
    use crate::core::scan::{self, Kind};
    use serde_json::Value;

    /// The project export, and each entity's own export by `Collection/Name`.
    struct Server(Vec<u8>, BTreeMap<String, Vec<u8>>);
    impl push::Remote for Server {
        fn fetch(
            &self,
            key: &super::super::entity::key::EntityKey,
        ) -> Result<Option<Vec<u8>>, super::super::server::ServerError> {
            Ok(self.1.get(&key.to_string()).cloned())
        }
        fn import(&self, _: &str, _: &[u8]) -> Result<(), super::super::server::ServerError> {
            unreachable!("init --from-server never imports")
        }
    }
    impl export::Remote for Server {
        fn export_xml(
            &self,
            _: Option<&str>,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<Vec<u8>, super::super::server::ServerError> {
            Ok(self.0.clone())
        }
        fn source_control(
            &self,
            _: &str,
            _: &Value,
        ) -> Result<Option<Value>, super::super::server::ServerError> {
            unreachable!("init --from-server only reads Exporter")
        }
    }
    impl super::super::imports::Remote for Server {
        fn exists(
            &self,
            _: &super::super::entity::key::EntityKey,
        ) -> Result<bool, super::super::server::ServerError> {
            Ok(false)
        }
        fn import_file(
            &self,
            _: &str,
            _: &[u8],
            _: bool,
            _: bool,
        ) -> Result<(), super::super::server::ServerError> {
            unreachable!()
        }
        fn source_control(
            &self,
            _: &str,
            _: &Value,
        ) -> Result<Option<Value>, super::super::server::ServerError> {
            unreachable!()
        }
    }
    impl localization::Remote for Server {
        fn tables(&self) -> Result<Vec<String>, super::super::server::ServerError> {
            Ok(Vec::new())
        }
        fn tokens(
            &self,
            _: &str,
        ) -> Result<Vec<localization::Token>, super::super::server::ServerError> {
            Ok(Vec::new())
        }
        fn header(
            &self,
            _: &str,
        ) -> Result<localization::Header, super::super::server::ServerError> {
            Ok(localization::Header::default())
        }
        fn delete_token(&self, _: &str, _: &str) -> Result<(), super::super::server::ServerError> {
            unreachable!()
        }
    }

    fn corpus() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/acme-orders")
    }

    fn files(root: &Path, skip: &[&str]) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if skip.contains(&name.as_str()) || name.starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    pending.push(path);
                } else {
                    out.push(path.strip_prefix(root).unwrap().to_path_buf());
                }
            }
        }
        out.sort();
        out
    }

    /// One entity element as the Exporter writes it: no whitespace between tags, attributes on
    /// one line, CDATA as it is, and the metadata a live export carries.
    fn exporter(entity: &[u8]) -> Vec<u8> {
        let tokens = scan::tokenize(entity).unwrap();
        let mut out = Vec::new();
        let mut at = 0;
        while at < tokens.len() {
            let t = &tokens[at];
            match t.kind {
                Kind::Text if t.span.of(entity).iter().all(u8::is_ascii_whitespace) => {}
                Kind::Start | Kind::Empty => {
                    out.push(b'<');
                    out.extend_from_slice(t.name.of(entity));
                    for a in scan::attributes(entity, t).unwrap() {
                        out.push(b' ');
                        out.extend_from_slice(&entity[a.name.start..a.value.end + 1]);
                    }
                    if at == 0 {
                        out.extend_from_slice(b" lastModifiedDate=\"0\"");
                    }
                    if t.kind == Kind::Empty {
                        out.extend_from_slice(b"/>");
                    } else {
                        out.push(b'>');
                    }
                }
                Kind::End if at == tokens.len() - 1 => {
                    out.extend_from_slice(b"<Owner>Acme.Exporter</Owner>");
                    out.extend_from_slice(t.span.of(entity));
                }
                _ => out.extend_from_slice(t.span.of(entity)),
            }
            at += 1;
        }
        out
    }

    /// The corpus's entities as one project export, grouped by collection, and each alone.
    fn project_export() -> Server {
        let mut by_collection: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut each = BTreeMap::new();
        for relative in files(&corpus(), &["src", "dist"]) {
            let bytes = std::fs::read(corpus().join(&relative)).unwrap();
            let Ok(info) = entity::parse(&bytes) else {
                continue;
            };
            let tokens = scan::tokenize(&bytes).unwrap();
            let at = tokens
                .iter()
                .position(|t| {
                    matches!(t.kind, Kind::Start | Kind::Empty)
                        && scan::attribute(&bytes, t, "name")
                            .ok()
                            .flatten()
                            .is_some_and(|v| {
                                scan::decode_entities(std::str::from_utf8(v.of(&bytes)).unwrap())
                                    == info.name
                            })
                })
                .unwrap();
            let end = scan::element_end_in(&tokens, &bytes, at).unwrap();
            let span = scan::Span::new(tokens[at].span.start, tokens[end].span.end);
            let flat = exporter(span.of(&bytes));
            each.insert(
                format!("{}/{}", info.collection, info.name),
                [
                    format!("<Entities><{}>", info.collection).into_bytes(),
                    flat.clone(),
                    format!("</{}></Entities>", info.collection).into_bytes(),
                ]
                .concat(),
            );
            by_collection
                .entry(info.collection)
                .or_default()
                .extend(flat);
        }
        let mut out =
            b"<Entities majorVersion=\"10\" minorVersion=\"1\" universal=\"password\">".to_vec();
        for (collection, entities) in by_collection {
            out.extend_from_slice(format!("<{collection}>").as_bytes());
            out.extend(entities);
            out.extend_from_slice(format!("</{collection}>").as_bytes());
        }
        out.extend_from_slice(b"</Entities>");
        Server(out, each)
    }

    /// What the server exports of the corpus comes back as the same files: every entity holds
    /// what the corpus file holds (so its baseline records), in the layout adopt gives a new
    /// entity, stable under another layout pass; every sidecar is the corpus's, byte for byte; and
    /// twaco.toml is what `twaco init` proposes for the files.
    #[test]
    fn the_corpus_exported_by_a_server_comes_back_as_the_corpus() {
        let temp = tempfile::tempdir().unwrap();
        super::tests::profile(temp.path());
        let outcome = execute(
            &project_export(),
            Request {
                root: temp.path().to_path_buf(),
                projects: vec!["Acme.Orders".into()],
                apply: true,
                profile: "default".into(),
            },
        )
        .unwrap();
        // The baseline step itself finds every entity in sync with what the server holds.
        assert!(outcome.not_in_sync.is_empty(), "{:?}", outcome.not_in_sync);
        assert_eq!(outcome.recorded, outcome.entities.values().sum::<usize>());
        let source = files(&corpus(), &["twaco.toml", "network-assets.csv"]);
        let written = files(
            temp.path(),
            &["twaco.toml", "AGENTS.md", "CLAUDE.md", ".gitignore"],
        );
        assert_eq!(written, source, "the same files");
        let mut differ = Vec::new();
        for relative in &source {
            let ours = std::fs::read(corpus().join(relative)).unwrap();
            let theirs = std::fs::read(temp.path().join(relative)).unwrap();
            if relative.starts_with("src") {
                if theirs != ours {
                    differ.push(format!("sidecar differs: {}", relative.display()));
                }
                continue;
            }
            // In sync with what the server exported, so its baseline records.
            let info = entity::parse(&ours).unwrap();
            let key = adopt::EntityRef {
                collection: info.collection.clone(),
                name: info.name.clone(),
            };
            let exported = [
                format!("<Entities><{}>", info.collection).into_bytes(),
                exporter(&adopt::raw_entity_bytes(&ours, &key).unwrap()),
                format!("</{}></Entities>", info.collection).into_bytes(),
            ]
            .concat();
            if normalise::hash(&theirs).unwrap() != normalise::hash(&exported).unwrap() {
                differ.push(format!(
                    "not in sync with the export: {}",
                    relative.display()
                ));
            }
            let info = entity::parse(&theirs).unwrap();
            let raw = adopt::raw_entity_bytes(
                &theirs,
                &adopt::EntityRef {
                    collection: info.collection.clone(),
                    name: info.name.clone(),
                },
            )
            .unwrap();
            if layout::entity(&raw, Some(&raw), false).unwrap() != raw {
                differ.push(format!("layout not stable: {}", relative.display()));
            }
        }
        assert!(differ.is_empty(), "{differ:#?}");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("twaco.toml")).unwrap(),
            init::propose(temp.path()).toml
        );
    }
}
