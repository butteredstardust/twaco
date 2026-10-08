//! Colour is on only when the environment asks for it. Piped output stays plain.

use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/acme-orders")
}

/// Run twaco with a clean colour environment, then apply the given variables.
fn twaco(args: &[&str], vars: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_twaco"));
    command
        .args(args)
        .current_dir(fixture())
        .env_remove("NO_COLOR")
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("CI");
    for (key, value) in vars {
        command.env(key, value);
    }
    command.output().expect("run twaco")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("utf-8 output")
}

#[test]
fn force_color_paints_check_results() {
    let out = twaco(&["check"], &[("FORCE_COLOR", "1")]);
    assert!(text(&out.stdout).contains("\x1b["));
}

#[test]
fn no_color_leaves_check_results_plain() {
    let out = twaco(&["check"], &[("NO_COLOR", "1")]);
    assert!(!text(&out.stdout).contains('\x1b'));
}

#[test]
fn piped_output_is_plain_and_equals_the_stripped_coloured_output() {
    let plain = text(&twaco(&["check"], &[]).stdout);
    assert!(!plain.contains('\x1b'));
    assert!(plain.contains("  ok      line endings   32 examined"));
    // Strip the escapes from the coloured text. Nothing else may differ.
    let coloured = text(&twaco(&["check"], &[("FORCE_COLOR", "1")]).stdout);
    let mut stripped = String::new();
    let mut chars = coloured.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for d in chars.by_ref() {
                if d == 'm' {
                    break;
                }
            }
        } else {
            stripped.push(c);
        }
    }
    assert_eq!(stripped, plain);
}

#[test]
fn the_error_prefix_is_painted_on_stderr_only_when_asked() {
    let coloured = twaco(&["no-such-command"], &[("FORCE_COLOR", "1")]);
    assert!(text(&coloured.stderr).contains("\x1b["));
    assert!(text(&coloured.stdout).is_empty());
    let plain = twaco(&["no-such-command"], &[]);
    assert!(text(&plain.stderr).starts_with("twaco: "));
}

#[test]
fn no_color_wins_over_force_color_on_both_streams() {
    let vars = [("NO_COLOR", "1"), ("FORCE_COLOR", "1")];
    let check = twaco(&["check"], &vars);
    assert!(!text(&check.stdout).contains('\x1b'));
    let error = twaco(&["no-such-command"], &vars);
    assert!(text(&error.stderr).starts_with("twaco: "));
}
