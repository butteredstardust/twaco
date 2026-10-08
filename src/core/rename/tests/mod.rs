use super::*;
use std::collections::BTreeMap;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    solution: Solution,
}

fn fixture() -> Fixture {
    let root_guard = tempfile::Builder::new()
        .prefix("twaco-rename-plan-")
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    write(&root, ".gitignore", "ignored.txt\n");
    write(
            &root,
            "twaco.toml",
            "[[project]]\nname = \"Acme.App\"\n[project.deploy]\nentry_point_thing = \"Acme.App.Manager\"\n",
        );
    entity(&root, "Things", "Acme.App.Manager", "Acme.App");
    entity(&root, "Things", "Acme.App.Manager.Child", "Acme.App");
    entity(&root, "DataShapes", "Acme.App.Model_DS", "Acme.App");
    entity(&root, "Mashups", "Acme.App.Dashboard", "Acme.App");
    entity(&root, "Projects", "Acme.App", "Acme.App");
    entity(&root, "Things", "T", "Acme.App");
    write(
        &root,
        "src/Acme.App.Manager/services/Run/script.js",
        "const manager = Things[\"Acme.App.Manager\"];\n",
    );
    write(
        &root,
        "src/Acme.App.Manager/services/Run/definition.xml",
        "<ServiceDefinition description=\"Acme.App.Manager\"/>\n",
    );
    write(
        &root,
        "src/Acme.App.Dashboard/mashup/content.json",
        "{\"id\":\"DynamicThingShapes_Acme.App.Manager\"}\n",
    );
    write(
        &root,
        "src/T/services/Run/script.js",
        "const exact = Things[\"T\"];\nconst topic = \"T/T1\";\n",
    );
    write(
        &root,
        "docs/GUIDE.md",
        "Use Acme.App.Manager and the Acme.App building block.\n",
    );
    write(
        &root,
        "sql/m.sql",
        "select 'Acme.App.Manager', 'Acme.App.Model_DS';\n",
    );
    write(&root, "ignored.txt", "Acme.App.Manager Acme.App\n");
    let solution = Solution::load(&root.join(CONFIG_FILE)).unwrap();
    Fixture {
        _dir: root_guard,
        root,
        solution,
    }
}

fn run_fixture() -> Fixture {
    let root_guard = tempfile::Builder::new()
        .prefix("twaco-rename-run-")
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    write(
        &root,
        "twaco.toml",
        "[[project]]\nname = \"P\"\ncollections = [\"Things\"]\n",
    );
    write(
            &root,
            "Things/P.Manager.xml",
            "<Entities><Things><Thing name=\"P.Manager\" projectName=\"P\"></Thing></Things></Entities>\n",
        );
    let solution = Solution::load(&root.join(CONFIG_FILE)).unwrap();
    Fixture {
        _dir: root_guard,
        root,
        solution,
    }
}

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn entity(root: &Path, collection: &str, name: &str, project: &str) {
    write(
            root,
            &format!("{collection}/{name}.xml"),
            &format!(
                "<Entities><{collection}><Entity name=\"{name}\" projectName=\"{project}\"><Description>uses {name}</Description></Entity></{collection}></Entities>"
            ),
        );
}

fn spec(kind: Kind, old: &str, new: &str) -> Spec {
    Spec {
        kind,
        old: old.to_string(),
        new: new.to_string(),
        scope: None,
        service: None,
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .display()
        .to_string()
        .replace('\\', "/")
}

/// The workspace lock, held for the call it is passed to.
fn locked(fixture: &Fixture) -> crate::core::lock::WorkspaceLock {
    locked_root(&fixture.root)
}

fn locked_root(root: &Path) -> crate::core::lock::WorkspaceLock {
    crate::core::lock::acquire(root, "test", &[]).unwrap()
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(dir: &Path, root: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                visit(&path, root, files);
            } else if name == "lock" || name == "lock.holder" {
                // The workspace lock a test takes to apply a plan is not part of the workspace.
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn snapshot_without_twaco(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    snapshot(root)
        .into_iter()
        .filter(|(path, _)| !path.starts_with(".twaco"))
        .collect()
}

fn options(include_outside: bool) -> ApplyOptions {
    ApplyOptions {
        include_outside,
        date: "2026-10-02".to_string(),
        extra_files: Vec::new(),
    }
}

fn seed_baseline(fixture: &Fixture, planned: &Plan) {
    let mut baseline = Baseline::default();
    for (collection, name) in &planned.baseline_keys {
        baseline.set(collection, name, "v5:local".into(), "v5:server".into());
    }
    baseline.set("Things", "Unchanged", "v5:keep".into(), "v5:keep".into());
    baseline.write(&fixture.root).unwrap();
}

fn assert_no_rename_temporaries(root: &Path) {
    for path in snapshot(root).keys() {
        assert!(
            !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("twaco-tmp"),
            "temporary left at {}",
            path.display()
        );
    }
}

fn run_options(apply: bool, skip_checks: bool) -> RunOptions {
    RunOptions {
        apply,
        include_outside: false,
        skip_checks,
        date: "2026-10-02".to_string(),
        sql: SqlChoice::Unset,
        expect_digest: None,
    }
}

mod apply;
#[cfg(feature = "test-failpoints")]
mod crash;
mod database;
mod field;
mod identity;
mod member;
mod property;
mod run;
mod table;
