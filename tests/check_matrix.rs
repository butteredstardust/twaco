//! The ways a removed DataShape field can remain in use, exercised through the CLI.

use std::path::Path;
use std::process::{Command, Output};

const SHAPE: &str = "P.Rows_DS";
const FIELD: &str = "gone";

#[derive(Clone, Copy)]
enum Use {
    GridColumn,
    SelectedRowBinding,
    ResultRow,
    Picker,
    Chart,
    DataTable,
}

struct Row {
    name: &'static str,
    usage: Use,
    rule: Option<&'static str>,
    file: &'static str,
}

fn twaco(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_twaco"))
        .args(args)
        .current_dir(root)
        .env_clear()
        .output()
        .unwrap()
}

fn assert_success(root: &Path, args: &[&str]) {
    let output = twaco(root, args);
    assert!(
        output.status.success(),
        "twaco {args:?}\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn check(root: &Path, detail: bool) -> Output {
    if detail {
        twaco(root, &["check", "--detail"])
    } else {
        twaco(root, &["check"])
    }
}

fn create_solution(root: &Path, usage: Use) {
    // `new project` needs a solution to extend. The bootstrap declaration is deliberately empty.
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"Bootstrap\"\n",
    )
    .unwrap();
    std::fs::create_dir(root.join(".twaco")).unwrap();
    // A cached GenericThing makes `new thing --template GenericThing` genuinely offline.
    std::fs::write(
        root.join(".twaco/platform.json"),
        r#"{"templates":{"GenericThing":{"services":{}}},"shapes":{},"resources":{}}"#,
    )
    .unwrap();

    assert_success(root, &["new", "project", "P", "--apply"]);
    assert_success(
        root,
        &[
            "new",
            "data-shape",
            SHAPE,
            "--field",
            "kept:STRING:primary",
            "--field",
            "gone:STRING",
            "--project",
            "P",
            "--apply",
        ],
    );
    assert_success(
        root,
        &[
            "new",
            "thing",
            "P.Manager",
            "--template",
            "GenericThing",
            "--project",
            "P",
            "--apply",
        ],
    );

    let script = match usage {
        Use::ResultRow => concat!(
            "let rows = Resources[\"InfoTableFunctions\"].CreateInfoTableFromDataShape({ ",
            "dataShapeName: \"P.Rows_DS\" });\n",
            "rows.AddRow({ gone: \"gone\" });\n",
            "result = rows;\n",
        ),
        _ => concat!(
            "result = Resources[\"InfoTableFunctions\"].CreateInfoTableFromDataShape({ ",
            "dataShapeName: \"P.Rows_DS\" });\n",
        ),
    };
    std::fs::write(root.join("result.js"), script).unwrap();
    assert_success(
        root,
        &[
            "thing",
            "service",
            "add",
            "P.Manager",
            "GetRows",
            "--returns",
            "INFOTABLE:P.Rows_DS",
            "--script",
            "result.js",
            "--apply",
        ],
    );

    match usage {
        Use::GridColumn => {
            mashup(root, "ptcsgrid");
            assert_success(
                root,
                &[
                    "mashup",
                    "recipe",
                    "columns",
                    "P.Matrix_MU",
                    "ptcsgrid-3",
                    "--column",
                    FIELD,
                    "--data",
                    "data:P.Manager.GetRows",
                    "--apply",
                ],
            );
        }
        Use::SelectedRowBinding => {
            mashup(root, "ptcstextfield");
            assert_success(
                root,
                &[
                    "mashup",
                    "bind",
                    "P.Matrix_MU",
                    "--from",
                    "data:P.Manager.GetRows[SelectedRows].gone",
                    "--to",
                    "ui:ptcstextfield-3.Text",
                    "--apply",
                ],
            );
        }
        Use::ResultRow => {}
        Use::Picker => {
            mashup(root, "dropdown");
            assert_success(
                root,
                &[
                    "mashup",
                    "recipe",
                    "picker",
                    "P.Matrix_MU",
                    "ptcsdropdown-3",
                    "--data",
                    "data:P.Manager.GetRows",
                    "--x",
                    FIELD,
                    "--value",
                    FIELD,
                    "--apply",
                ],
            );
        }
        Use::Chart => {
            mashup(root, "bar-chart");
            assert_success(
                root,
                &[
                    "mashup",
                    "recipe",
                    "chart",
                    "P.Matrix_MU",
                    "ptcschartbar-3",
                    "--data",
                    "data:P.Manager.GetRows",
                    "--x",
                    FIELD,
                    "--apply",
                ],
            );
        }
        Use::DataTable => assert_success(
            root,
            &[
                "new",
                "data-table",
                "P.Rows_DT",
                "--data-shape",
                SHAPE,
                "--project",
                "P",
                "--apply",
            ],
        ),
    }
}

fn mashup(root: &Path, widget: &str) {
    assert_success(
        root,
        &["mashup", "new", "P.Matrix_MU", "--project", "P", "--apply"],
    );
    assert_success(root, &["mashup", "add", "P.Matrix_MU", widget, "--apply"]);
}

fn remove_field(path: &Path) {
    let mut fields: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    fields.retain(|field| field["name"] != FIELD);
    std::fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&fields).unwrap()),
    )
    .unwrap();
}

#[test]
fn check_reports_or_records_every_removed_datashape_field_use() {
    let rows = [
        Row {
            name: "grid ColumnFormat",
            usage: Use::GridColumn,
            rule: Some("mashup bindings/unknown-column"),
            file: "P/Mashups/P.Matrix_MU.xml",
        },
        Row {
            name: "SelectedRows field binding",
            usage: Use::SelectedRowBinding,
            rule: Some("mashup bindings/unknown-field"),
            file: "P/Mashups/P.Matrix_MU.xml",
        },
        Row {
            name: "result AddRow",
            usage: Use::ResultRow,
            rule: Some("result shapes/unknown-field"),
            file: "src/P.Manager/services/GetRows/script.js",
        },
        Row {
            name: "picker display and value fields",
            usage: Use::Picker,
            rule: Some("mashup bindings/unknown-field-property"),
            file: "P/Mashups/P.Matrix_MU.xml",
        },
        Row {
            name: "chart x field",
            usage: Use::Chart,
            rule: Some("mashup bindings/unknown-field-property"),
            file: "P/Mashups/P.Matrix_MU.xml",
        },
        // By design: sync carries the removal into the DataTable's accumulated shape (its stored
        // rows lose the column on the next import), so there is nothing left for check to find.
        Row {
            name: "DataTable accumulated shape",
            usage: Use::DataTable,
            rule: None,
            file: "P/Things/P.Rows_DT.xml",
        },
    ];

    for row in rows {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        create_solution(root, row.usage);

        let present = check(root, false);
        assert!(
            present.status.success(),
            "{} with {FIELD} present\nstdout={}\nstderr={}",
            row.name,
            String::from_utf8_lossy(&present.stdout),
            String::from_utf8_lossy(&present.stderr),
        );

        let fields = root.join("src").join(SHAPE).join("fields.json");
        let original = std::fs::read(&fields).unwrap();
        remove_field(&fields);
        assert_success(root, &["sync", "--all", "--allow-add-remove"]);

        let removed = check(root, true);
        let output = format!(
            "{}{}",
            String::from_utf8_lossy(&removed.stdout),
            String::from_utf8_lossy(&removed.stderr),
        );
        if let Some(rule) = row.rule {
            assert!(
                !removed.status.success(),
                "{} should be caught by {rule}\n{output}",
                row.name
            );
            assert!(output.contains(rule), "{}\n{output}", row.name);
            assert!(output.contains(row.file), "{}\n{output}", row.name);
        } else {
            assert!(
                removed.status.success(),
                "{} is a documented current gap, not a new finding\n{output}",
                row.name
            );
        }

        std::fs::write(&fields, original).unwrap();
        assert_success(root, &["sync", "--all", "--allow-add-remove"]);
        let restored = check(root, false);
        assert!(
            restored.status.success(),
            "{} after restoring {FIELD}\nstdout={}\nstderr={}",
            row.name,
            String::from_utf8_lossy(&restored.stdout),
            String::from_utf8_lossy(&restored.stderr),
        );
    }
}
