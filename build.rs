//! Stamp the build with the commit it came from, so `twaco --version` tells a stale binary from a
//! current one. A build outside a git checkout (a published crate, a source archive) has no stamp
//! and says only the version.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn main() {
    // A commit or a checkout moves HEAD or the reflog; either makes the stamp stale.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/logs/HEAD");
    println!("cargo:rerun-if-changed=build.rs");
    // So that `+dirty` follows the working tree, not only the last commit: whatever is tracked.
    for tracked in [
        "src",
        "tests",
        "knowledge",
        "documentation",
        "scripts",
        ".github",
        "Cargo.toml",
        "Cargo.lock",
        "deny.toml",
        "README.md",
        "CHANGELOG.md",
    ] {
        println!("cargo:rerun-if-changed={tracked}");
    }
    let Some(commit) = git(&["rev-parse", "--short=9", "HEAD"]) else {
        return;
    };
    // The commit's own date, not the build's: the same source then gives the same binary.
    let date = git(&["log", "-1", "--format=%cs"]).unwrap_or_default();
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some();
    let mut stamp = commit;
    if dirty {
        stamp.push_str("+dirty");
    }
    if !date.is_empty() {
        stamp.push(' ');
        stamp.push_str(&date);
    }
    println!("cargo:rustc-env=TWACO_BUILD={stamp}");
}
