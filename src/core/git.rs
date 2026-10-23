//! Small read-only Git queries shared by repository features.

use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn root(path: &Path) -> Option<PathBuf> {
    output(path, ["rev-parse", "--show-toplevel"])
        .ok()
        .map(|value| PathBuf::from(value.trim()))
}

pub(crate) fn output<const N: usize>(
    directory: &Path,
    arguments: [&str; N],
) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

pub(crate) fn bytes<const N: usize>(
    directory: &Path,
    arguments: [&str; N],
) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}
