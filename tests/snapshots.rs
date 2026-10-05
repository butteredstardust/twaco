//! What the command line says, held to files.
//!
//! Each scenario runs the built binary in a fresh copy of the bundled `Acme.Orders` repository and
//! compares its exit code, standard output and standard error with a golden file in
//! `tests/fixtures/snapshots/`. The text is the interface people and scripts read: a usage line, a
//! refusal, the shape of a plan, a JSON line. A change to it should be a decision, so it fails here
//! until the golden file is re-blessed and the diff reviewed:
//!
//! ```sh
//! TWACO_BLESS=1 cargo test --test snapshots
//! git diff tests/fixtures/snapshots
//! ```
//!
//! The temporary directory is written `<ROOT>`, and path separators are made `/`, so one golden
//! file serves Windows, Linux and macOS. Nothing here contacts a server: every scenario is a plan,
//! a refusal, or an offline command.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One scenario: its file name, and the arguments after `twaco`.
const SCENARIOS: &[(&str, &[&str])] = &[
    ("usage", &[]),
    ("unknown_command", &["bogus"]),
    ("projects", &["projects"]),
    ("check", &["check"]),
    ("sync_all_check", &["sync", "--all", "--check"]),
    (
        "catalog_json",
        &["catalog", "Acme.Orders.Manager", "--json"],
    ),
    ("guide_topics", &["guide"]),
    (
        "impact_service_plan",
        &["impact", "Audit", "--member", "Record"],
    ),
    (
        "impact_service_detail",
        &["impact", "Audit", "--member", "Record", "--detail"],
    ),
    ("impact_template_json", &["impact", "Base_TT", "--json"]),
    (
        "impact_service_dot",
        &["impact", "Audit", "--member", "Record", "--dot"],
    ),
    ("impact_unknown_entity", &["impact", "Acme.Orders.Nope"]),
    (
        "impact_bad_confidence",
        &["impact", "Audit", "--min-confidence", "sure"],
    ),
    (
        "impact_json_and_dot",
        &["impact", "Audit", "--json", "--dot"],
    ),
    ("unused_plan", &["unused"]),
    ("unused_detail", &["unused", "--detail"]),
    ("unused_json", &["unused", "--json"]),
    (
        "unused_structural_only",
        &[
            "unused",
            "--min-confidence",
            "structural",
            "--collection",
            "Things",
        ],
    ),
    (
        "unused_bad_collection",
        &["unused", "--collection", "Mashups"],
    ),
    (
        "rename_service_plan",
        &[
            "rename",
            "service",
            "Acme.Orders.Manager",
            "Normalise",
            "Tidy",
        ],
    ),
    (
        "rename_service_plan_detail",
        &[
            "rename",
            "service",
            "Acme.Orders.Manager",
            "Normalise",
            "Tidy",
            "--detail",
        ],
    ),
    (
        "rename_service_plan_json",
        &[
            "rename",
            "service",
            "Acme.Orders.Manager",
            "Normalise",
            "Tidy",
            "--json",
        ],
    ),
    (
        "rename_param_plan",
        &[
            "rename",
            "param",
            "Acme.Orders.Manager",
            "GetOrder",
            "orderId",
            "order",
        ],
    ),
    (
        "rename_entity_plan_detail",
        &[
            "rename",
            "entity",
            "Acme.Orders.Audit",
            "Acme.Orders.Ledger",
            "--detail",
            "--no-sql",
        ],
    ),
    (
        "rename_entity_needs_a_database_decision",
        &[
            "rename",
            "entity",
            "Acme.Orders.Audit",
            "Acme.Orders.Ledger",
        ],
    ),
    (
        "rename_field_plan",
        &[
            "rename",
            "field",
            "Acme.Orders.OrderLine_DS",
            "quantity",
            "count",
            "--sql",
        ],
    ),
    (
        "rename_field_needs_a_database_decision",
        &[
            "rename",
            "field",
            "Acme.Orders.OrderLine_DS",
            "quantity",
            "count",
        ],
    ),
    (
        "rename_same_name_refused",
        &["rename", "entity", "Acme.Orders.Audit", "Acme.Orders.Audit"],
    ),
    (
        "rename_unknown_entity_refused",
        &["rename", "entity", "Acme.Orders.Nope", "Acme.Orders.Other"],
    ),
    ("rename_missing_arguments", &["rename", "service"]),
    (
        "rename_field_text_refused",
        &[
            "rename",
            "field",
            "Acme.Orders.OrderLine_DS",
            "quantity",
            "count",
            "--text",
        ],
    ),
    ("extract_unknown_entity", &["extract", "Nope.Entity"]),
    ("entity_delete_needs_an_entity", &["entity", "delete"]),
    (
        "entity_push_needs_a_profile",
        &["entity", "push", "Acme.Orders.Manager"],
    ),
    (
        "new_building_block_plan",
        &[
            "new",
            "building-block",
            "Acme.Payments",
            "--base-extension",
            "PTC.Base:10.1.0",
        ],
    ),
    (
        "move_service_plan",
        &[
            "move",
            "service",
            "Acme.Orders.Manager",
            "Acme.Orders.Audit",
            "Normalise",
        ],
    ),
];

fn bundled_repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/acme-orders")
}

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/snapshots")
        .join(format!("{name}.txt"))
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn fresh_copy(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "twaco-snapshot-{name}-{}-{nonce}",
        std::process::id()
    ));
    copy_tree(&bundled_repository(), &root);
    root
}

/// The text with the temporary directory written `<ROOT>` and path separators made `/`, in the
/// forms a platform may print it (as created, canonical, with either separator).
fn normalise(text: &str, root: &Path) -> String {
    let mut forms: Vec<String> = Vec::new();
    for base in [Some(root.to_path_buf()), root.canonicalize().ok()]
        .into_iter()
        .flatten()
    {
        let shown = base.display().to_string();
        let shown = shown.strip_prefix(r"\\?\").unwrap_or(&shown).to_string();
        forms.push(shown.replace('\\', "/"));
        forms.push(shown);
    }
    forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
    forms.dedup();
    let mut out = text.replace("\r\n", "\n");
    for form in &forms {
        out = out.replace(form.as_str(), "<ROOT>");
    }
    let out = mask_dates(&mask_digests(&out));
    out.lines()
        .map(|line| {
            let pathish = line.contains("<ROOT>")
                || line.contains(".xml")
                || line.contains(".js")
                || line.contains(".json")
                || line.contains(".toml")
                || line.contains(".sql");
            if pathish {
                line.replace('\\', "/")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `YYYY-MM-DD` written `<DATE>`: a migration script is named for the day it was planned.
fn mask_dates(text: &str) -> String {
    let is_date = |candidate: &str| {
        let bytes = candidate.as_bytes();
        bytes.len() == 10
            && bytes.iter().enumerate().all(|(at, byte)| match at {
                4 | 7 => *byte == b'-',
                _ => byte.is_ascii_digit(),
            })
    };
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        let previous_is_digit = out.chars().next_back().is_some_and(|c| c.is_ascii_digit());
        let next_is_digit = text
            .get(at + 10..)
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c.is_ascii_digit());
        if !previous_is_digit && !next_is_digit && text.get(at..at + 10).is_some_and(is_date) {
            out.push_str("<DATE>");
            at += 10;
        } else {
            let c = text[at..].chars().next().expect("at is on a boundary");
            out.push(c);
            at += c.len_utf8();
        }
    }
    out
}

/// Every run of 64 or more hex digits written `<DIGEST>`: a plan digest hashes the absolute paths
/// it reads, which differ per machine, so its value says nothing a snapshot could hold.
fn mask_digests(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= 64 {
            out.push_str("<DIGEST>");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if c.is_ascii_hexdigit() && !c.is_ascii_uppercase() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn run(name: &str, args: &[&str]) -> String {
    let root = fresh_copy(name);
    let output = Command::new(env!("CARGO_BIN_EXE_twaco"))
        .args(args)
        .current_dir(&root)
        .env_clear()
        .output()
        .unwrap();
    let text = format!(
        "$ twaco {}\nexit: {}\n\n--- stdout\n{}\n--- stderr\n{}\n",
        args.join(" "),
        output
            .status
            .code()
            .map_or("signal".into(), |c| c.to_string()),
        normalise(&String::from_utf8_lossy(&output.stdout), &root),
        normalise(&String::from_utf8_lossy(&output.stderr), &root),
    );
    let _ = std::fs::remove_dir_all(root);
    text
}

#[test]
fn command_line_output_matches_the_snapshots() {
    let bless = std::env::var("TWACO_BLESS").as_deref() == Ok("1");
    let mut differing = Vec::new();
    for (name, args) in SCENARIOS {
        let actual = run(name, args);
        let path = snapshot_path(name);
        if bless {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &actual).unwrap();
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(expected) if expected.replace("\r\n", "\n") == actual => {}
            Ok(expected) => differing.push(format!(
                "{name}: differs from {}\n--- expected\n{expected}\n--- actual\n{actual}",
                path.display()
            )),
            Err(_) => differing.push(format!("{name}: no snapshot at {}", path.display())),
        }
    }
    assert!(
        differing.is_empty(),
        "{} snapshot(s) differ; if the change is meant, run `TWACO_BLESS=1 cargo test --test \
         snapshots` and review the diff:\n\n{}",
        differing.len(),
        differing.join("\n\n")
    );
}

/// A golden file for a scenario that no longer exists is a stale promise nobody checks.
#[test]
fn every_snapshot_file_belongs_to_a_scenario() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/snapshots");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let known: Vec<String> = SCENARIOS
        .iter()
        .map(|(name, _)| format!("{name}.txt"))
        .collect();
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        assert!(known.contains(&file), "{file} belongs to no scenario");
    }
}
