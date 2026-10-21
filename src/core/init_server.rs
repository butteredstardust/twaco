//! Start an empty source repository from Exporter project documents.

use super::{
    adopt, commands, config, entity, export, gitignore, init, layout, lock, normalise, progress,
    push, status, transaction, workflow, workspace,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const REFUSAL: &str = "init --from-server starts a new repository; to add a project to this one, `twaco export project <P> --out <file>` and `twaco adopt <file>`";

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
    pub recorded: usize,
    pub not_in_sync: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Refused(&'static str),
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

struct File {
    path: PathBuf,
    bytes: Vec<u8>,
}

/// Read each project through the same Exporter endpoint as `export project`, plan every local
/// file first, and only then write all of them as one transaction.
pub fn execute<R>(remote: &R, request: Request) -> Result<Outcome, Error>
where
    R: export::Remote + push::Remote + Sync,
{
    if request.projects.is_empty() {
        return Err(Error::Projects);
    }
    if request.root.join(super::config::CONFIG_FILE).exists()
        || init::has_entity_files(&request.root)
    {
        return Err(Error::Refused(REFUSAL));
    }
    let named: BTreeSet<String> = request.projects.iter().cloned().collect();
    let multiple = named.len() > 1;
    let mut files = Vec::new();
    let mut notes = Vec::new();
    let mut counts = BTreeMap::new();
    let mut per_project = BTreeMap::<String, usize>::new();
    let mut dependencies = BTreeMap::<String, Vec<String>>::new();
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
                    .filter(|name| *name != info.project && named.contains(name))
                    .collect();
                dependencies.insert(info.project.clone(), needs);
            }
            let laid_out = layout::entity(&raw, None, false).map_err(|e| Error::Export {
                project: project.clone(),
                why: e.to_string(),
            })?;
            // As adopt writes a new entity that has no peer file to copy.
            let document = entity::new::document(&key.collection, &laid_out);
            let base = if multiple {
                request.root.join(&info.project)
            } else {
                request.root.clone()
            };
            let path = base
                .join(&key.collection)
                .join(format!("{}.xml", info.name));
            *counts.entry(key.collection.clone()).or_insert(0usize) += 1;
            *per_project.entry(info.project).or_insert(0usize) += 1;
            files.push(File {
                path,
                bytes: document,
            });
        }
    }
    if files.is_empty() {
        notes.push("the server exports contained no entities for the named projects".to_string());
    }
    let proposal_files = files
        .iter()
        .map(|file| (file.path.clone(), file.bytes.clone()))
        .collect();
    let mut proposal = init::propose_files(&request.root, proposal_files);
    add_project_dependencies(&mut proposal.toml, &dependencies);
    notes.extend(proposal.notes);
    for (project, count) in per_project {
        notes.push(format!("{project}: {count} entities"));
    }
    let entity_files = files.len();
    let mut all = files;
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
    all.push(File {
        path: request.root.join(super::config::CONFIG_FILE),
        bytes: proposal.toml.clone().into_bytes(),
    });
    let files_count = all.len();
    let mut recorded = 0;
    let mut not_in_sync = Vec::new();
    if request.apply {
        let held = lock::acquire(&request.root, "init --from-server", &[])?;
        let mut write = transaction::Transaction::new(&request.root, "init --from-server");
        for file in all {
            write.write_file(&file.path, file.bytes)?;
        }
        write.apply(&held)?;
        // The sidecars are what `twaco extract --all` makes of the files just written, made by
        // the same code; each entity's are one transaction of their own. Then the baseline,
        // under the same lock, so it describes exactly these files.
        let unfinished = |why: String| Error::Unfinished { why };
        let solution = config::Solution::load(&request.root.join(config::CONFIG_FILE))
            .map_err(|error| unfinished(format!("its twaco.toml does not load: {error}")))?;
        let found = workspace::discover(&solution);
        let extracted =
            workflow::extract(&solution, &found.entities, &found.unreadable, false, &held);
        if extracted.failed > 0 {
            return Err(unfinished(format!(
                "{} entities' sidecars could not be extracted",
                extracted.failed
            )));
        }
        let status = commands::status::execute_holding(
            &solution,
            &commands::status::StatusRequest {
                target: commands::status::StatusTarget::All,
                project: None,
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
        recorded = status.recorded.unwrap_or(0);
        not_in_sync = status
            .statuses
            .iter()
            .filter(|entity| entity.verdict != status::Verdict::InSync)
            .map(|entity| format!("{}/{}", entity.collection, entity.name))
            .chain(status.failures)
            .collect();
    } else {
        notes.push(format!(
            "{entity_files} entity files; their sidecars are extracted once they are written"
        ));
    }
    Ok(Outcome {
        projects: request.projects,
        files: files_count,
        entities: counts,
        toml: proposal.toml,
        notes,
        applied: request.apply,
        recorded,
        not_in_sync,
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
    fn refuses_existing_config_before_asking_the_remote() {
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
        assert!(matches!(error, Error::Refused(_)));
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
