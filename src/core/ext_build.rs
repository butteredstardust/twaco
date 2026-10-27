//! `ext build`: make an extension project's package zip and say what it holds.
//!
//! Three kinds of project build: a package.json widget project runs its `build` script (after
//! installing its dependencies the first time), a Java project its Gradle wrapper, Gradle or Ant,
//! and a project with neither a build nor Java sources is packed by twaco itself, as the
//! extension import expects it: metadata.xml at the root, `ui/`, `Entities/`, `Localization/`,
//! and `lib/*.jar` under `lib/common/`. The zip is then read back: every file metadata.xml names
//! must be in it.

use super::ext_check;
use super::ext_widget;
use super::extensions;
use serde::Serialize;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Where a project keeps its package name and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// metadata.xml, at this path under the root.
    Metadata(String),
    /// package.json, whose build generates metadata.xml.
    PackageJson,
}

/// An extension project's package and version, and where they are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) source: Source,
    pub(crate) package: String,
    pub(crate) version: String,
}

impl Identity {
    /// The file that holds the version, for messages.
    pub(crate) fn file(&self) -> &str {
        match &self.source {
            Source::Metadata(path) => path,
            Source::PackageJson => "package.json",
        }
    }
}

pub(crate) fn identity(root: &Path) -> Result<Identity, String> {
    if let Some(project) = ext_widget::read(root) {
        let project = project?;
        return Ok(Identity {
            source: Source::PackageJson,
            package: project.package,
            version: project.version,
        });
    }
    let (path, bytes) = metadata(root)?;
    let declared = ext_check::declarations(&bytes).map_err(|why| format!("{path}: {why}"))?;
    let package = declared
        .iter()
        .find(|entry| entry.tag == "ExtensionPackage")
        .ok_or_else(|| format!("{path} has no ExtensionPackage"))?;
    let version = package
        .attributes
        .get("packageVersion")
        .cloned()
        .ok_or_else(|| format!("{path}: the ExtensionPackage has no packageVersion"))?;
    Ok(Identity {
        source: Source::Metadata(path),
        package: package.name.clone(),
        version,
    })
}

pub(crate) fn metadata(root: &Path) -> Result<(String, Vec<u8>), String> {
    ["configfiles/metadata.xml", "metadata.xml"]
        .into_iter()
        .find_map(|path| {
            std::fs::read(root.join(path))
                .ok()
                .map(|bytes| (path.to_string(), bytes))
        })
        .ok_or_else(|| format!("{} has no metadata.xml", root.display()))
}

/// What `ext build` made.
#[derive(Debug, Serialize)]
pub struct Built {
    pub package: String,
    pub version: String,
    /// The commands run, or `packed by twaco`.
    pub build: Vec<String>,
    /// The package zip, relative to the solution or the folder given.
    pub zip: String,
    /// What the zip's metadata.xml declares, by kind.
    pub widgets: Vec<String>,
    pub entities: Vec<String>,
    /// Files metadata.xml names that the zip does not hold.
    pub problems: Vec<String>,
}

/// Build the project at `root` and read back its zip. `shown_root` is how messages name it.
pub fn build(root: &Path, shown_root: &str) -> Result<Built, String> {
    let identity = identity(root)?;
    let build = make(root, &identity)?;
    let zip_path = package_zip(root, shown_root, &identity.package, &identity.version)?;
    let zip = std::fs::read(&zip_path)
        .map_err(|error| format!("cannot read {}: {error}", zip_path.display()))?;
    let contents = contents(&zip)?;
    let shown_zip = zip_path
        .strip_prefix(root)
        .map(|relative| format!("{shown_root}/{}", relative.display()))
        .unwrap_or_else(|_| zip_path.display().to_string())
        .replace('\\', "/");
    Ok(Built {
        package: identity.package,
        version: identity.version,
        build,
        zip: shown_zip,
        widgets: contents.widgets,
        entities: contents.entities,
        problems: contents.problems,
    })
}

/// Run the project's build, or pack a project that has none: the commands as run.
pub(crate) fn make(root: &Path, identity: &Identity) -> Result<Vec<String>, String> {
    match run_build(root)? {
        Some(commands) => Ok(commands),
        None => {
            pack(root, identity)?;
            Ok(vec!["packed by twaco".to_string()])
        }
    }
}

/// What a package zip's metadata.xml declares, and the files it names that the zip lacks.
pub(crate) struct Contents {
    pub(crate) widgets: Vec<String>,
    pub(crate) entities: Vec<String>,
    pub(crate) problems: Vec<String>,
    /// Every name metadata.xml declares, which an installed package lists.
    pub(crate) declared: Vec<String>,
}

pub(crate) fn contents(zip: &[u8]) -> Result<Contents, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip))
        .map_err(|error| format!("not a zip: {error}"))?;
    let names: Vec<String> = archive
        .file_names()
        .filter_map(|name| name.ok().map(std::borrow::Cow::into_owned))
        .collect();
    let mut metadata = Vec::new();
    archive
        .by_name("metadata.xml")
        .map_err(|_| "the zip has no metadata.xml at its root".to_string())?
        .take(1 << 20)
        .read_to_end(&mut metadata)
        .map_err(|error| format!("metadata.xml: {error}"))?;
    let declared =
        ext_check::declarations(&metadata).map_err(|why| format!("metadata.xml: {why}"))?;
    let has = |path: &str| names.iter().any(|name| name == path);
    let mut out = Contents {
        widgets: Vec::new(),
        entities: Vec::new(),
        problems: Vec::new(),
        declared: Vec::new(),
    };
    for item in &declared {
        match item.tag.as_str() {
            "ExtensionPackage" => {}
            "Widget" => {
                out.widgets.push(item.name.clone());
                for file in &item.files {
                    let path = format!("ui/{}/{file}", item.name);
                    if !has(&path) {
                        out.problems.push(format!(
                            "widget {} names {file}, which is not at {path}",
                            item.name
                        ));
                    }
                }
            }
            "JarResources" => {
                for file in &item.files {
                    let path = format!("lib/common/{file}");
                    if !has(&path) {
                        out.problems
                            .push(format!("JarResources names {file}, which is not at {path}"));
                    }
                }
            }
            _ => out.entities.push(format!("{} {}", item.tag, item.name)),
        }
        if !matches!(item.tag.as_str(), "ExtensionPackage" | "JarResources")
            && !item.name.is_empty()
        {
            out.declared.push(item.name.clone());
        }
    }
    Ok(out)
}

/// Run the project's build: its package.json `build` script, Gradle wrapper, Gradle or Ant. The
/// commands as run; `None` for a project with no build and no Java sources, which twaco packs.
pub(crate) fn run_build(root: &Path) -> Result<Option<Vec<String>>, String> {
    if let Some(project) = ext_widget::read(root) {
        let runner = project?.runner;
        let mut done = Vec::new();
        if !root.join("node_modules").is_dir() {
            done.push(run(root, runner, &["install"])?);
        }
        done.push(match run(root, runner, &["run", "build"]) {
            // webpack 4 hashes with MD4, which Node 17 and later refuse unless OpenSSL's legacy
            // provider is on: the template's older projects build again with it.
            Err(error) if error.contains(OPENSSL_REFUSED) => {
                let options = match std::env::var("NODE_OPTIONS") {
                    Ok(given) if !given.trim().is_empty() => {
                        format!("{given} {LEGACY_OPENSSL}")
                    }
                    _ => LEGACY_OPENSSL.to_string(),
                };
                let again = run_with(
                    root,
                    runner,
                    &["run", "build"],
                    Some(("NODE_OPTIONS", &options)),
                )
                .map_err(|again| {
                    format!(
                        "{again}
(retried with NODE_OPTIONS={LEGACY_OPENSSL}, since the first run stopped on ERR_OSSL_EVP_UNSUPPORTED)"
                    )
                })?;
                format!("{again}, again with NODE_OPTIONS={LEGACY_OPENSSL} (an old webpack)")
            }
            other => other?,
        });
        return Ok(Some(done));
    }
    let wrapper = if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    };
    let (tool, args): (String, &[&str]) = if root.join(wrapper).is_file() {
        // Named from the project folder, where the build runs, not from where twaco was started.
        (
            Path::new(".").join(wrapper).to_string_lossy().into_owned(),
            &["build", "--no-daemon"],
        )
    } else if root.join("build.gradle").is_file() || root.join("build.gradle.kts").is_file() {
        ("gradle".to_string(), &["build", "--no-daemon"])
    } else if root.join("build-extension.xml").is_file() {
        ("ant".to_string(), &["-f", "build-extension.xml"])
    } else if root.join("build.xml").is_file() {
        ("ant".to_string(), &[])
    } else if has_java(&root.join("src")) {
        return Err(format!(
            "{} has Java sources but no Gradle or Ant build: build the zip yourself and pass --no-build",
            root.display()
        ));
    } else {
        return Ok(None);
    };
    Ok(Some(vec![run(root, &tool, args)?]))
}

pub(crate) fn has_java(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            has_java(&path)
        } else {
            path.extension()
                .is_some_and(|extension| extension == "java")
        }
    })
}

/// Add the Gradle wrapper to a new project with the Gradle installed here: the command run.
pub fn gradle_wrapper(root: &Path) -> Result<String, String> {
    run(root, "gradle", &["wrapper", "--no-daemon"])
}

/// Run `tool args` in `root`; the command as shown, or its output's end when it fails.
/// The Node option that lets webpack 4's MD4 hashing run on Node 17 and later.
const LEGACY_OPENSSL: &str = "--openssl-legacy-provider";
/// What Node says when it refuses MD4.
const OPENSSL_REFUSED: &str = "ERR_OSSL_EVP_UNSUPPORTED";

fn run(root: &Path, tool: &str, args: &[&str]) -> Result<String, String> {
    run_with(root, tool, args, None)
}

/// [`run`], with one environment variable set for the child.
fn run_with(
    root: &Path,
    tool: &str,
    args: &[&str],
    env: Option<(&str, &str)>,
) -> Result<String, String> {
    let wrapper = if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    };
    // On Windows gradle, ant and the package managers are batch files, which only cmd runs; a
    // checked-out gradlew may have lost its execute bit, which sh does not need.
    let mut command = if cfg!(windows) {
        let mut command = std::process::Command::new("cmd");
        command.arg("/C").arg(tool);
        command
    } else if tool.ends_with(wrapper) {
        let mut command = std::process::Command::new("sh");
        command.arg(tool);
        command
    } else {
        std::process::Command::new(tool)
    };
    let shown = format!("{tool} {}", args.join(" ")).trim_end().to_string();
    if let Some((key, value)) = env {
        command.env(key, value);
    }
    let mut child = command
        .args(args)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run {shown}: {error}"))?;
    let stdout = child.stdout.take().map(tail_of);
    let stderr = child.stderr.take().map(tail_of);
    let status = child
        .wait()
        .map_err(|error| format!("{shown} did not finish: {error}"))?;
    // A process the build left running (a daemon) may hold the pipes open for good, so the
    // output is waited for only a little after the build itself ends.
    let text: String = [stdout, stderr]
        .into_iter()
        .flatten()
        .map(|tail| tail.text(std::time::Duration::from_secs(5)))
        .collect();
    if !status.success() {
        let lines: Vec<String> = text.lines().map(masked).collect();
        let mut tail = lines[lines.len().saturating_sub(30)..].join("\n");
        // The retry for an old webpack looks for this, which long diagnostics can push out.
        if text.contains(OPENSSL_REFUSED) && !tail.contains(OPENSSL_REFUSED) {
            tail.push_str(&format!("\n(earlier in the output: {OPENSSL_REFUSED})"));
        }
        return Err(format!("{shown} failed:\n{tail}"));
    }
    Ok(shown)
}

/// A build's output line with what looks like a credential after `=` or `:` hidden: a registry
/// token in an npm error, a password a script echoes.
fn masked(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let marker = ["_authtoken", "_auth", "token", "password", "secret"]
        .iter()
        .find_map(|word| lower.find(word).map(|at| at + word.len()));
    let Some(at) = marker else {
        return line.to_string();
    };
    match line[at..].find(['=', ':']) {
        Some(offset) => format!("{}<hidden>", &line[..at + offset + 1]),
        None => line.to_string(),
    }
}

/// The last 64 KiB a build writes to one stream, read as it runs so a long build neither
/// blocks on a full pipe nor fills memory.
struct Tail {
    kept: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<u8>>>,
    ended: std::sync::mpsc::Receiver<()>,
}

impl Tail {
    /// What was read, once the stream ends or `grace` passes.
    fn text(self, grace: std::time::Duration) -> String {
        let _ = self.ended.recv_timeout(grace);
        let mut kept = self
            .kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(kept.make_contiguous()).into_owned()
    }
}

fn tail_of(mut stream: impl std::io::Read + Send + 'static) -> Tail {
    const KEEP: usize = 64 * 1024;
    let kept = std::sync::Arc::new(std::sync::Mutex::new(
        std::collections::VecDeque::with_capacity(KEEP),
    ));
    let (done, ended) = std::sync::mpsc::channel();
    let writer = std::sync::Arc::clone(&kept);
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        while let Ok(read) = stream.read(&mut buffer) {
            if read == 0 {
                break;
            }
            let mut kept = writer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            kept.extend(&buffer[..read]);
            let excess = kept.len().saturating_sub(KEEP);
            kept.drain(..excess);
        }
        let _ = done.send(());
    });
    Tail { kept, ended }
}

/// Pack a project with no build into `build/distributions/<package>.zip`.
fn pack(root: &Path, identity: &Identity) -> Result<PathBuf, String> {
    use std::io::Write as _;
    let name = &identity.package;
    if name.is_empty()
        || name.starts_with('.')
        || name
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
    {
        return Err(format!(
            "the ExtensionPackage name {name:?} cannot name a zip file"
        ));
    }
    let (_, metadata) = metadata(root)?;
    let mut files: Vec<(String, Vec<u8>)> = vec![("metadata.xml".to_string(), metadata)];
    for folder in ["ui", "Entities", "Localization"] {
        collect_files(root, &root.join(folder), &mut files)?;
    }
    if let Ok(entries) = std::fs::read_dir(root.join("lib")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                let bytes = std::fs::read(&path)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
                files.push((format!("lib/common/{name}"), bytes));
            }
        }
    }
    let out = root.join("build/distributions");
    std::fs::create_dir_all(&out)
        .map_err(|error| format!("cannot create {}: {error}", out.display()))?;
    let path = out.join(format!("{}.zip", identity.package));
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in &files {
            zip.start_file(name.as_str(), options)
                .and_then(|()| zip.write_all(bytes).map_err(Into::into))
                .map_err(|error| format!("cannot pack {name}: {error}"))?;
        }
        zip.finish()
            .map_err(|error| format!("cannot finish the zip: {error}"))?;
    }
    super::ext_new::replace_whole(&path, buffer.get_ref())?;
    Ok(path)
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if entry.file_name() != ".gitkeep" {
            let relative = path
                .strip_prefix(root)
                .expect("collected under the root")
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = std::fs::read(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            out.push((relative, bytes));
        }
    }
    Ok(())
}

/// The package zip that holds `package` at `version`: under `build/distributions/` first, where
/// the Gradle and Ant templates put it, then elsewhere under `build/`, then under `zip/`, where
/// a package.json build puts it; the newest in the first place that has one. A build that had
/// nothing to do leaves the zip it made before, so its age proves nothing.
pub(crate) fn package_zip(
    root: &Path,
    shown_root: &str,
    package: &str,
    version: &str,
) -> Result<PathBuf, String> {
    let distributions = root.join("build/distributions");
    let mut found = Vec::new();
    for dir in ["build", "zip"] {
        collect_zips(&root.join(dir), 3, &mut found);
    }
    // A package.json build writes its zip under zip/; a Java build under build/distributions.
    let widget = ext_widget::read(root).is_some();
    let place = |path: &Path| {
        if path.starts_with(&distributions) {
            if widget {
                1
            } else {
                0
            }
        } else if path.starts_with(root.join("build")) {
            2
        } else if widget {
            0
        } else {
            3
        }
    };
    found.sort_by_key(|(path, modified)| (place(path), std::cmp::Reverse(*modified)));
    let mut other_versions = Vec::new();
    for (path, _) in found {
        let Some(made) = std::fs::read(&path)
            .ok()
            .and_then(|zip| extensions::inspect(&zip).ok())
        else {
            continue;
        };
        if made.name != package {
            continue;
        }
        if made.version == version {
            return Ok(path);
        }
        if !other_versions.contains(&made.version) {
            other_versions.push(made.version);
        }
    }
    Err(if other_versions.is_empty() {
        format!("no zip of {package} under {shown_root}/build or {shown_root}/zip")
    } else {
        format!(
            "no zip of {package} {version} under {shown_root}/build or {shown_root}/zip, only {}: \
             build it again, or leave out --no-build",
            other_versions.join(", ")
        )
    })
}

fn collect_zips(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, SystemTime)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_zips(&path, depth - 1, out);
            }
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
        {
            if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                out.push((path, modified));
            }
        }
    }
}

#[cfg(test)]
mod tests;
