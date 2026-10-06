//! Stamp the build with the commit it came from, so `twaco --version` tells a stale binary from a
//! current one. A build outside a git checkout (a published crate, a source archive) has no stamp
//! and says only the version. A Windows build also embeds the icon in `twaco.exe`.

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
    // `twaco update` picks its platform's payload from the release manifest by this name.
    println!(
        "cargo:rustc-env=TWACO_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
    stamp_build();
    #[cfg(windows)]
    embed_icon();
    // winresource needs the Windows resource compiler, so only a Windows host embeds the icon.
    // A cross-build to Windows still works; its twaco.exe has the default icon.
    #[cfg(not(windows))]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:warning=twaco.exe has no icon: only a Windows host can embed it");
    }
}

/// Give `twaco.exe` its icon and file version in Explorer. A cross-build to another target has no
/// use for a Windows resource, so only a Windows target gets one.
#[cfg(windows)]
fn embed_icon() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/icon/twaco.ico");
    if let Err(error) = resource.compile() {
        panic!("cannot embed assets/icon/twaco.ico in twaco.exe: {error}");
    }
}

fn stamp_build() {
    // A commit or a checkout moves HEAD or the reflog; either makes the stamp stale.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/logs/HEAD");
    println!("cargo:rerun-if-changed=build.rs");
    // So that `+dirty` follows the working tree, not only the last commit: whatever is tracked.
    for tracked in [
        "src",
        "tests",
        "assets",
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
