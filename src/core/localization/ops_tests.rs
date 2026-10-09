use super::*;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::entity_key::EntityKey;
use crate::core::server::ServerError;
use crate::core::{imports, progress};
use serde_json::Value;
use std::sync::Mutex;

const FLAT_DEFAULT: &[u8] = include_bytes!(
    "../../../tests/fixtures/localization/flat/Acme.App_LocalizationTable_Default.xml"
);
const FLAT_DE: &[u8] =
    include_bytes!("../../../tests/fixtures/localization/flat/Acme.App_LocalizationTable_DE.xml");

/// A server that merges imported tables the way ThingWorx does: a missing table is created with
/// the file's header, an existing one takes the file's tokens and answers `partial-success`, and
/// no token is ever removed by an import.
#[derive(Default)]
struct Server {
    tables: Mutex<BTreeMap<String, (Header, Vec<Token>)>>,
    log: Mutex<Vec<String>>,
    /// A token an import silently does not take, to prove the read-back.
    drops: Option<String>,
}

impl Server {
    fn with(tables: &[(&str, Vec<Token>)]) -> Self {
        let server = Server::default();
        for (name, tokens) in tables {
            server
                .tables
                .lock()
                .unwrap()
                .insert(name.to_string(), (Header::default(), tokens.clone()));
        }
        server
    }

    fn tokens_of(&self, table: &str) -> Vec<Token> {
        let mut tokens = self.tables.lock().unwrap()[table].1.clone();
        tokens.sort();
        tokens
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl imports::Remote for Server {
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
        Ok(key.collection() == "LocalizationTables"
            && self.tables.lock().unwrap().contains_key(key.name()))
    }

    fn import_file(&self, _: &str, bytes: &[u8], _: bool, _: bool) -> Result<(), ServerError> {
        let file = read(Path::new("sent.xml"), bytes).unwrap().unwrap();
        self.log
            .lock()
            .unwrap()
            .push(format!("import {}", file.table));
        let mut tables = self.tables.lock().unwrap();
        let existed = tables.contains_key(&file.table);
        let entry = tables
            .entry(file.table.clone())
            .or_insert_with(|| (file.header.clone(), Vec::new()));
        for token in file.tokens {
            if self.drops.as_deref() == Some(token.name.as_str()) {
                continue;
            }
            entry.1.retain(|old| old.name != token.name);
            entry.1.push(token);
        }
        if existed {
            Err(ServerError::Rejected {
                url: "http://example.invalid/Thingworx/Importer".to_string(),
                body: "partial-success".to_string(),
            })
        } else {
            Ok(())
        }
    }

    fn source_control(&self, _: &str, _: &Value) -> Result<Option<Value>, ServerError> {
        unreachable!()
    }
}

impl Remote for Server {
    fn tables(&self) -> Result<Vec<String>, ServerError> {
        Ok(self.tables.lock().unwrap().keys().cloned().collect())
    }

    fn tokens(&self, table: &str) -> Result<Vec<Token>, ServerError> {
        Ok(self.tables.lock().unwrap()[table].1.clone())
    }

    fn header(&self, table: &str) -> Result<Header, ServerError> {
        Ok(self.tables.lock().unwrap()[table].0.clone())
    }

    fn delete_token(&self, table: &str, name: &str) -> Result<(), ServerError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("delete {table}/{name}"));
        self.tables
            .lock()
            .unwrap()
            .get_mut(table)
            .unwrap()
            .1
            .retain(|token| token.name != name);
        Ok(())
    }
}

struct Workspace {
    _temp: tempfile::TempDir,
    solution: Solution,
}

impl Workspace {
    /// A solution with the given projects and the given files under `localization/`.
    fn new(projects: &[&str], files: &[(&str, &[u8])]) -> Self {
        let temp = tempfile::Builder::new()
            .prefix("twaco-localization-")
            .tempdir()
            .unwrap();
        let toml: String = projects
            .iter()
            .map(|name| format!("[[project]]\nname = \"{name}\"\n"))
            .collect();
        std::fs::write(temp.path().join("twaco.toml"), toml).unwrap();
        for (path, bytes) in files {
            let path = temp.path().join("localization").join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let solution = Solution::load(&temp.path().join("twaco.toml")).unwrap();
        Workspace {
            _temp: temp,
            solution,
        }
    }

    /// The curated Default and de files of project Acme.App.
    fn curated() -> Self {
        Self::new(
            &["Acme.App"],
            &[
                ("Acme.App_LocalizationTable_Default.xml", FLAT_DEFAULT),
                ("Acme.App_LocalizationTable_DE.xml", FLAT_DE),
            ],
        )
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.solution.root.join("localization").join(relative)
    }

    fn text(&self, relative: &str) -> String {
        String::from_utf8(std::fs::read(self.path(relative)).unwrap()).unwrap()
    }

    /// Every file under the root with its bytes, to prove a plan wrote nothing.
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut stack = vec![self.solution.root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.insert(path.clone(), std::fs::read(path).unwrap());
                }
            }
        }
        out
    }
}

fn token(name: &str, value: &str) -> Token {
    Token {
        name: name.to_string(),
        value: value.to_string(),
        usage: "label".to_string(),
        context: "Acme label.".to_string(),
    }
}

fn plain(name: &str, value: &str) -> Token {
    Token {
        context: String::new(),
        ..token(name, value)
    }
}

const EN: [(&str, &str); 6] = [
    ("Save", "Save"),
    ("Open", "Open"),
    ("Delete", "Delete"),
    ("Close", "Close"),
    ("Cancel", "Cancel"),
    ("Help", "Help"),
];
const DE: [(&str, &str); 6] = [
    ("Save", "Speichern"),
    ("Open", "Öffnen"),
    ("Delete", "Löschen"),
    ("Close", "Schließen"),
    ("Cancel", "Abbrechen"),
    ("Help", "Hilfe"),
];

fn tokens(values: &[(&str, &str)]) -> Vec<Token> {
    values
        .iter()
        .map(|(name, value)| token(&format!("Acme.App.{name}"), value))
        .collect()
}

/// The server as the curated files say, plus tokens of another solution.
fn in_step() -> Server {
    let mut default = tokens(&EN);
    default.push(plain("Someone.Else", "theirs"));
    Server::with(&[(DEFAULT_TABLE, default), ("de", tokens(&DE))])
}

fn states(status: &Status) -> Vec<(String, String, State)> {
    status
        .compared
        .iter()
        .filter(|c| c.state != State::Same)
        .map(|c| (c.table.clone(), c.name.clone(), c.state))
        .collect()
}

fn names(change: &FileChange) -> (Vec<&str>, Vec<&str>) {
    (
        change.set.iter().map(String::as_str).collect(),
        change.removed.iter().map(String::as_str).collect(),
    )
}

// ---------- status ----------

#[test]
fn status_compares_the_owned_tokens_and_names_tables_the_server_lacks() {
    let workspace = Workspace::curated();
    let server = in_step();
    let found = super::status(&workspace.solution, &server, None).unwrap();
    assert!(states(&found).is_empty(), "{:?}", states(&found));
    assert_eq!(found.compared.len(), 12);
    assert!(found.problems.is_empty() && found.missing_tables.is_empty());

    let server = in_step();
    {
        let mut tables = server.tables.lock().unwrap();
        let default = &mut tables.get_mut(DEFAULT_TABLE).unwrap().1;
        default.retain(|t| t.name != "Acme.App.Help");
        default.push(token("Acme.App.Server", "s"));
        default
            .iter_mut()
            .find(|t| t.name == "Acme.App.Save")
            .unwrap()
            .value = "Store".to_string();
        tables.remove("de");
    }
    let found = super::status(&workspace.solution, &server, None).unwrap();
    assert_eq!(
        states(&found),
        [
            ("Default".into(), "Acme.App.Help".into(), State::LocalOnly),
            ("Default".into(), "Acme.App.Save".into(), State::Differs),
            (
                "Default".into(),
                "Acme.App.Server".into(),
                State::ServerOnly
            ),
            ("de".into(), "Acme.App.Cancel".into(), State::LocalOnly),
            ("de".into(), "Acme.App.Close".into(), State::LocalOnly),
            ("de".into(), "Acme.App.Delete".into(), State::LocalOnly),
            ("de".into(), "Acme.App.Help".into(), State::LocalOnly),
            ("de".into(), "Acme.App.Open".into(), State::LocalOnly),
            ("de".into(), "Acme.App.Save".into(), State::LocalOnly),
        ]
    );
    assert_eq!(found.missing_tables, ["de"]);

    let only_de = status_for(&workspace, &in_step(), "de");
    assert!(only_de.compared.iter().all(|c| c.table == "de"));
    let error = super::status(&workspace.solution, &in_step(), Some("xx")).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
}

fn status_for(workspace: &Workspace, server: &Server, table: &str) -> Status {
    super::status(&workspace.solution, server, Some(table)).unwrap()
}

#[test]
fn status_reports_problems_and_unreadable_files() {
    let de_extra = String::from_utf8(FLAT_DE.to_vec())
        .unwrap()
        .replace("Acme.App.Help", "Acme.App.Extra");
    let workspace = Workspace::new(
        &["Acme.App"],
        &[
            ("Acme.App_LocalizationTable_Default.xml", FLAT_DEFAULT),
            ("Acme.App_LocalizationTable_DE.xml", de_extra.as_bytes()),
            ("broken.xml", b"<Entities><LocalizationTables><LocalizationTable/></LocalizationTables></Entities>"),
        ],
    );
    let found = super::status(&workspace.solution, &in_step(), None).unwrap();
    assert_eq!(
        found.problems,
        [
            Problem::NotInDefault {
                table: "de".to_string(),
                name: "Acme.App.Extra".to_string(),
                file: workspace.path("Acme.App_LocalizationTable_DE.xml"),
            },
            Problem::Untranslated {
                table: "de".to_string(),
                name: "Acme.App.Help".to_string()
            },
        ]
    );
    assert_eq!(found.unreadable.len(), 1);
}

// ---------- pull ----------

#[test]
fn a_pull_plan_writes_nothing() {
    let workspace = Workspace::curated();
    let server = in_step();
    server.tables.lock().unwrap().get_mut("de").unwrap().1[0].value = "Sichern".to_string();
    let before = workspace.snapshot();
    let pulled = pull(&workspace.solution, &server, None, true, false).unwrap();
    assert!(!pulled.applied);
    assert_eq!(pulled.files.len(), 1);
    assert_eq!(workspace.snapshot(), before);
}

#[test]
fn a_pull_updates_a_curated_file_in_place_and_adds_what_the_server_has() {
    let workspace = Workspace::curated();
    let server = in_step();
    {
        let mut tables = server.tables.lock().unwrap();
        let de = &mut tables.get_mut("de").unwrap().1;
        de[0].value = "  Sichern \n".to_string();
        de.push(token("Acme.App.Print", "Drucken"));
    }
    let pulled = pull(&workspace.solution, &server, None, false, true).unwrap();
    assert!(pulled.applied);
    assert_eq!(pulled.files.len(), 1);
    assert_eq!(
        names(&pulled.files[0]),
        (vec!["Acme.App.Print", "Acme.App.Save"], vec![])
    );
    let expected = String::from_utf8(
        edit(
            Path::new("de.xml"),
            FLAT_DE,
            &[
                Edit::Set(token("Acme.App.Save", "Sichern")),
                Edit::Set(token("Acme.App.Print", "Drucken")),
            ],
        )
        .unwrap(),
    )
    .unwrap();
    let written = workspace.text("Acme.App_LocalizationTable_DE.xml");
    assert_eq!(written, expected);
    assert!(written.contains("<!-- Messages -->") && written.contains("<![CDATA[Sichern]]>"));
    // The Default file did not change by a byte.
    assert_eq!(
        workspace
            .text("Acme.App_LocalizationTable_Default.xml")
            .as_bytes(),
        FLAT_DEFAULT
    );
}

#[test]
fn a_pull_creates_a_file_for_a_server_table_the_solution_has_none_of() {
    let workspace = Workspace::curated();
    let server = in_step();
    let header = Header {
        description: Some("fr localization table".to_string()),
        language_common: Some("French".to_string()),
        language_native: Some("Français".to_string()),
    };
    server.tables.lock().unwrap().insert(
        "fr".to_string(),
        (
            header.clone(),
            vec![
                token("Acme.App.Save", "Enregistrer"),
                plain("Someone.Else", "x"),
            ],
        ),
    );
    server.tables.lock().unwrap().insert(
        "it".to_string(),
        (Header::default(), vec![plain("Someone.Else", "y")]),
    );
    let pulled = pull(&workspace.solution, &server, None, false, true).unwrap();
    assert_eq!(pulled.files.len(), 1, "it holds no token of this solution");
    let created = &pulled.files[0];
    assert!(created.created);
    assert_eq!(
        created.path,
        workspace.path("Acme.App/LocalizationTable_fr.xml")
    );
    assert_eq!(
        workspace
            .text("Acme.App/LocalizationTable_fr.xml")
            .as_bytes(),
        render("fr", &header, &[token("Acme.App.Save", "Enregistrer")])
    );
}

#[test]
fn a_pull_with_prune_removes_what_the_server_lacks() {
    let workspace = Workspace::curated();
    let server = in_step();
    server
        .tables
        .lock()
        .unwrap()
        .get_mut(DEFAULT_TABLE)
        .unwrap()
        .1
        .retain(|t| t.name != "Acme.App.Open");
    let kept = pull(&workspace.solution, &server, None, false, false).unwrap();
    assert!(kept.files.is_empty(), "without --prune a local token stays");
    let pulled = pull(&workspace.solution, &server, None, true, true).unwrap();
    assert_eq!(names(&pulled.files[0]), (vec![], vec!["Acme.App.Open"]));
    assert_eq!(
        workspace
            .text("Acme.App_LocalizationTable_Default.xml")
            .as_bytes(),
        edit(
            Path::new("d.xml"),
            FLAT_DEFAULT,
            &[Edit::Remove("Acme.App.Open".to_string())]
        )
        .unwrap()
    );
}

#[test]
fn a_pull_refuses_duplicates_before_writing_anything() {
    let workspace = Workspace::new(
        &["Acme.App"],
        &[
            ("Acme.App_LocalizationTable_DE.xml", FLAT_DE),
            ("copy/de.xml", FLAT_DE),
        ],
    );
    let server = in_step();
    server.tables.lock().unwrap().get_mut("de").unwrap().1[0].value = "Sichern".to_string();
    let before = workspace.snapshot();
    let error = pull(&workspace.solution, &server, None, false, true).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidData);
    assert!(
        error
            .to_string()
            .contains("6 problem(s) to resolve before pulling"),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("de/Acme.App.Cancel is in 2 rows"),
        "{error}"
    );
    assert_eq!(workspace.snapshot(), before);
}

// ---------- push ----------

#[test]
fn a_push_plan_sends_nothing() {
    let workspace = Workspace::curated();
    let server = Server::with(&[(DEFAULT_TABLE, tokens(&EN[..3]))]);
    let pushed = push(
        &workspace.solution,
        &server,
        None,
        true,
        false,
        &progress::NONE,
    )
    .unwrap();
    assert!(!pushed.applied);
    assert_eq!(
        pushed
            .tables
            .iter()
            .map(|t| (t.table.as_str(), t.create, t.set.len()))
            .collect::<Vec<_>>(),
        [("Default", false, 3), ("de", true, 6)]
    );
    assert!(server.log().is_empty());
    assert_eq!(server.tokens_of(DEFAULT_TABLE).len(), 3);
}

#[test]
fn a_push_imports_default_first_creates_a_missing_table_and_reads_back() {
    let workspace = Workspace::curated();
    let server = Server::with(&[(DEFAULT_TABLE, vec![token("Acme.App.Save", "Old")])]);
    let pushed = push(
        &workspace.solution,
        &server,
        None,
        false,
        true,
        &progress::NONE,
    )
    .unwrap();
    assert!(pushed.applied);
    assert_eq!(server.log(), ["import Default", "import de"]);
    let mut want = tokens(&EN);
    want.sort();
    assert_eq!(server.tokens_of(DEFAULT_TABLE), want);
    assert_eq!(server.tokens_of("de").len(), 6);
    // Nothing left to do afterwards.
    let again = push(
        &workspace.solution,
        &server,
        None,
        false,
        false,
        &progress::NONE,
    )
    .unwrap();
    assert!(again.tables.is_empty());
}

#[test]
fn a_token_the_server_did_not_keep_is_not_verified() {
    let workspace = Workspace::curated();
    let server = Server {
        drops: Some("Acme.App.Help".to_string()),
        ..Server::with(&[(DEFAULT_TABLE, vec![])])
    };
    let error = push(
        &workspace.solution,
        &server,
        Some(DEFAULT_TABLE),
        false,
        true,
        &progress::NONE,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::NotVerified);
    assert!(
        error.to_string().contains("Default/Acme.App.Help"),
        "{error}"
    );
}

#[test]
fn a_prune_deletes_language_tokens_before_default_and_imports_nothing_unchanged() {
    let workspace = Workspace::curated();
    let server = in_step();
    {
        let mut tables = server.tables.lock().unwrap();
        tables
            .get_mut(DEFAULT_TABLE)
            .unwrap()
            .1
            .push(token("Acme.App.Retired", "r"));
        tables
            .get_mut("de")
            .unwrap()
            .1
            .push(token("Acme.App.Retired", "r"));
    }
    let kept = push(
        &workspace.solution,
        &server,
        None,
        false,
        true,
        &progress::NONE,
    )
    .unwrap();
    assert!(kept.tables.is_empty(), "without --prune nothing is deleted");
    let pushed = push(
        &workspace.solution,
        &server,
        None,
        true,
        true,
        &progress::NONE,
    )
    .unwrap();
    assert_eq!(pushed.tables.len(), 2);
    assert_eq!(
        server.log(),
        [
            "delete de/Acme.App.Retired",
            "delete Default/Acme.App.Retired"
        ]
    );
    assert!(server
        .tokens_of(DEFAULT_TABLE)
        .iter()
        .any(|t| t.name == "Someone.Else"));
}

#[test]
fn a_push_refuses_a_language_token_default_lacks() {
    let de_extra = String::from_utf8(FLAT_DE.to_vec())
        .unwrap()
        .replace("Acme.App.Help", "Acme.App.Extra");
    let workspace = Workspace::new(
        &["Acme.App"],
        &[
            ("Acme.App_LocalizationTable_Default.xml", FLAT_DEFAULT),
            ("Acme.App_LocalizationTable_DE.xml", de_extra.as_bytes()),
        ],
    );
    let server = in_step();
    let error = push(
        &workspace.solution,
        &server,
        None,
        false,
        true,
        &progress::NONE,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidData);
    assert!(error.to_string().contains("de/Acme.App.Extra"), "{error}");
    assert!(server.log().is_empty());
}

// ---------- new ----------

#[test]
fn new_seeds_from_default_and_refuses_a_table_the_project_has() {
    let workspace = Workspace::curated();
    let header = Header {
        description: None,
        language_common: Some("French".to_string()),
        language_native: Some("Français".to_string()),
    };
    let before = workspace.snapshot();
    let planned = new(&workspace.solution, "fr", None, header.clone(), false).unwrap();
    assert_eq!(workspace.snapshot(), before);
    let path = workspace.path("Acme.App/LocalizationTable_fr.xml");
    assert_eq!(planned.files[0].path, path);
    new(&workspace.solution, "fr", None, header.clone(), true).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        render("fr", &header, &tokens(&EN))
    );

    for table in ["fr", "de", DEFAULT_TABLE] {
        let error = new(&workspace.solution, table, None, Header::default(), true).unwrap_err();
        assert_eq!(error.code(), ErrorCode::AlreadyExists, "{table}");
    }
}

#[test]
fn new_needs_a_project_when_there_are_several() {
    let workspace = Workspace::new(&["Acme.App", "Acme.Other"], &[]);
    let error = new(&workspace.solution, "fr", None, Header::default(), false).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    assert!(error.to_string().contains("Acme.Other"), "{error}");
    let planned = new(
        &workspace.solution,
        "fr",
        Some("Acme.Other"),
        Header::default(),
        false,
    )
    .unwrap();
    assert_eq!(
        planned.files[0].path,
        workspace.path("Acme.Other/LocalizationTable_fr.xml")
    );
}

// ---------- set ----------

#[test]
fn set_updates_in_place_keeping_usage_and_context_unless_given() {
    let workspace = Workspace::curated();
    let before = workspace.snapshot();
    set(
        &workspace.solution,
        "Acme.App.Save",
        "Sichern",
        Some("de"),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    assert_eq!(workspace.snapshot(), before);
    let edited = set(
        &workspace.solution,
        "Acme.App.Save",
        "Sichern",
        Some("de"),
        None,
        None,
        None,
        true,
    )
    .unwrap();
    assert!(!edited.files[0].created);
    assert_eq!(
        workspace.text("Acme.App_LocalizationTable_DE.xml"),
        String::from_utf8(FLAT_DE.to_vec())
            .unwrap()
            .replace("<![CDATA[Speichern]]>", "<![CDATA[Sichern]]>")
    );
    set(
        &workspace.solution,
        "Acme.App.Save",
        "Sichern",
        Some("de"),
        Some("tooltip"),
        Some("Button"),
        None,
        true,
    )
    .unwrap();
    let de = read(
        Path::new("de.xml"),
        workspace
            .text("Acme.App_LocalizationTable_DE.xml")
            .as_bytes(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(de.tokens[0].usage, "tooltip");
    assert_eq!(de.tokens[0].context, "Button");
}

#[test]
fn set_adds_a_new_token_with_label_usage() {
    let workspace = Workspace::curated();
    set(
        &workspace.solution,
        "Acme.App.Print",
        "Print",
        None,
        None,
        None,
        None,
        true,
    )
    .unwrap();
    let default = read(
        Path::new("d.xml"),
        workspace
            .text("Acme.App_LocalizationTable_Default.xml")
            .as_bytes(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        default.tokens.last().unwrap(),
        &plain("Acme.App.Print", "Print")
    );
}

#[test]
fn set_refuses_what_the_server_would_refuse_or_trim() {
    let workspace = Workspace::curated();
    let error = set(
        &workspace.solution,
        "Acme.App.New",
        "Neu",
        Some("de"),
        None,
        None,
        None,
        true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("not in Default"), "{error}");
    let error = set(
        &workspace.solution,
        "Acme.App.Save",
        " x",
        None,
        None,
        None,
        None,
        true,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
    let error = set(
        &workspace.solution,
        "Other.Thing",
        "x",
        None,
        None,
        None,
        None,
        true,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("matches no project prefix"),
        "{error}"
    );
    let error = set(
        &workspace.solution,
        "Acme.App.Save",
        "x",
        Some("fr"),
        None,
        None,
        None,
        true,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("twaco localization new fr"),
        "{error}"
    );
}

#[test]
fn set_with_a_project_places_a_foreign_name_and_keeps_an_existing_one_where_it_is() {
    let workspace = Workspace::curated();
    set(
        &workspace.solution,
        "Legacy.Title",
        "Title",
        None,
        None,
        None,
        Some("Acme.App"),
        true,
    )
    .unwrap();
    let default = workspace.text("Acme.App_LocalizationTable_Default.xml");
    assert!(default.contains("Legacy.Title"));
    // Already in a file: no prefix or project is needed to change it.
    set(
        &workspace.solution,
        "Legacy.Title",
        "Heading",
        None,
        None,
        None,
        None,
        true,
    )
    .unwrap();
    assert!(workspace
        .text("Acme.App_LocalizationTable_Default.xml")
        .contains("<![CDATA[Heading]]>"));
}

#[test]
fn set_creates_the_default_file_when_the_project_has_none() {
    let workspace = Workspace::new(&["Acme.App"], &[]);
    let edited = set(
        &workspace.solution,
        "Acme.App.Title",
        "Title",
        None,
        None,
        None,
        None,
        true,
    )
    .unwrap();
    assert!(edited.files[0].created);
    assert_eq!(
        std::fs::read(workspace.path("Acme.App/LocalizationTable.xml")).unwrap(),
        render(
            DEFAULT_TABLE,
            &Header::default(),
            &[plain("Acme.App.Title", "Title")]
        )
    );
}

// ---------- remove ----------

#[test]
fn remove_takes_a_token_from_every_table_or_from_one() {
    let workspace = Workspace::curated();
    let before = workspace.snapshot();
    let planned = remove(&workspace.solution, "Acme.App.Open", None, false).unwrap();
    assert_eq!(planned.files.len(), 2);
    assert_eq!(workspace.snapshot(), before);

    let only = remove(&workspace.solution, "Acme.App.Open", Some("de"), true).unwrap();
    assert_eq!(only.files.len(), 1);
    assert_eq!(
        workspace
            .text("Acme.App_LocalizationTable_Default.xml")
            .as_bytes(),
        FLAT_DEFAULT
    );
    assert_eq!(
        workspace
            .text("Acme.App_LocalizationTable_DE.xml")
            .as_bytes(),
        edit(
            Path::new("de.xml"),
            FLAT_DE,
            &[Edit::Remove("Acme.App.Open".to_string())]
        )
        .unwrap()
    );

    let every = remove(
        &workspace.solution,
        "Acme.App.Save",
        Some(DEFAULT_TABLE),
        true,
    )
    .unwrap();
    assert_eq!(every.files.len(), 2, "Default takes the languages with it");

    let error = remove(&workspace.solution, "Acme.App.Nothing", None, true).unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnknownEntity);
}
