//! `ext deploy`: build one of the solution's Java extensions, then have the server validate the
//! package it makes, or install it and list what its metadata.xml declares.
//!
//! The server skips a package version it has, so a change goes up under a higher version:
//! `--bump` raises it in metadata.xml (and `extension.properties`, when that names one) before
//! the build. An upgrade of a package with Java code is queued by the server for its next
//! restart, and is reported so.

use super::config::Solution;
use super::ext_check;
use super::extensions::{self, Remote};
use super::profile;
use super::progress::{self, Progress};
use super::scan::{self, Kind};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Which part of `x.y.z` `--bump` raises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

impl Bump {
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "patch" => Self::Patch,
            "minor" => Self::Minor,
            "major" => Self::Major,
            _ => return None,
        })
    }

    fn raise(self, [major, minor, patch]: [u64; 3]) -> Option<[u64; 3]> {
        Some(match self {
            Self::Patch => [major, minor, patch.checked_add(1)?],
            Self::Minor => [major, minor.checked_add(1)?, 0],
            Self::Major => [major.checked_add(1)?, 0, 0],
        })
    }
}

pub struct DeployRequest {
    /// An `[[extension]]` entry's name or root; none: the only entry.
    pub target: Option<String>,
    pub profile: String,
    /// Run the project's build first; otherwise the newest package zip it has is sent.
    pub build: bool,
    pub bump: Option<Bump>,
    pub apply: bool,
}

#[derive(Debug, Serialize)]
pub struct Deployed {
    /// The `[[extension]]` entry, and its folder relative to the solution.
    pub extension: String,
    pub root: String,
    pub package: String,
    /// What is sent: the bumped version once applied.
    pub version: String,
    /// What the server has, when it has the package.
    pub installed: Option<String>,
    /// `x.y.z -> x.y.z'`, done when applied, else what --apply would do.
    pub bump: Option<String>,
    /// The build command run, when one was.
    pub build: Option<String>,
    /// The package zip, relative to the solution.
    pub zip: String,
    /// install X, or upgrade X from A to B.
    pub plan: String,
    /// Whether the server validated the zip; not when it carries the installed version, which
    /// only the bump on --apply changes.
    pub validated: bool,
    pub applied: bool,
    /// Installed at the server's next restart.
    pub queued: bool,
    /// Entities metadata.xml declares that the server's package does not list.
    pub missing: Vec<String>,
}

/// Build, validate and, applied, install one extension. `open` makes the server client from the
/// profile, so a test can stand in a fake.
pub fn deploy<R, F>(
    solution: &Solution,
    request: &DeployRequest,
    open: F,
    progress: &dyn Progress,
) -> Result<Deployed, String>
where
    R: Remote,
    F: FnOnce(profile::Profile) -> R,
{
    let (extension, root) = target(solution, request.target.as_deref())?;
    let shown_root = solution.relative(&root);
    let check = ext_check::check(&root);
    if !check.findings.is_empty() {
        return Err(format!(
            "ext check finds {} problem(s) in {shown_root}; fix them first:\n{}",
            check.findings.len(),
            ext_check::render(&check).trim_end()
        ));
    }
    let (metadata_path, package, local) = package_version(&root)?;
    let profile = profile::load(&solution.root, &request.profile).map_err(|e| e.to_string())?;
    let remote = open(profile);
    let installed = {
        let _phase = progress::phase(progress, "reading the package list", Some(1));
        let packages = extensions::list(&remote).map_err(|e| e.to_string())?;
        progress.advance(1);
        packages
    }
    .into_iter()
    .find(|p| p.name == package)
    .map(|p| p.version);
    let local_parts = parse_version(&local)
        .ok_or_else(|| format!("{metadata_path}: packageVersion {local:?} is not x.y.z"))?;
    let installed_parts = installed.as_deref().and_then(parse_version);
    let too_large = || format!("packageVersion {local} cannot be raised further");
    let wanted = match (request.bump, installed_parts) {
        (Some(bump), Some(server)) => {
            Some(bump.raise(local_parts.max(server)).ok_or_else(too_large)?)
        }
        (Some(bump), None) => Some(bump.raise(local_parts).ok_or_else(too_large)?),
        (None, Some(server)) if server == local_parts => {
            return Err(format!(
                "{package} {local} is installed already, and the server skips a version it has: \
                 pass --bump patch, or raise packageVersion"
            ))
        }
        (None, Some(server)) if server > local_parts => {
            return Err(format!(
            "the server has {package} {}, newer than {local}: raise packageVersion or pass --bump",
            installed.as_deref().unwrap_or_default()
        ))
        }
        (None, _) => None,
    };
    let bump = wanted.map(|parts| format!("{local} -> {}", show_version(parts)));
    let sending = match wanted {
        Some(parts) if request.apply => {
            let version = show_version(parts);
            set_version(&root, &metadata_path, &version)?;
            version
        }
        _ => local.clone(),
    };
    // Until the server has the package, a failure puts the version back.
    let bumped = sending != local;
    let undo = |error: String| -> String {
        if !bumped {
            return error;
        }
        match set_version(&root, &metadata_path, &local) {
            Ok(()) => format!("{error}\n(packageVersion is {local} again)"),
            Err(why) => {
                format!("{error}\n(and putting packageVersion back to {local} failed: {why})")
            }
        }
    };
    let build = if request.build {
        let _phase = progress::phase(progress, "building the extension", Some(1));
        let command = run_build(&root).map_err(&undo)?;
        progress.advance(1);
        Some(command)
    } else {
        None
    };
    let zip_path = package_zip(&root, &shown_root, &package, &sending).map_err(&undo)?;
    let zip = std::fs::read(&zip_path)
        .map_err(|error| undo(format!("cannot read {}: {error}", zip_path.display())))?;
    let made = extensions::inspect(&zip).map_err(|e| undo(e.to_string()))?;
    if made.name != package || made.version != sending {
        return Err(undo(format!(
            "{} holds {} {}, not {package} {sending}",
            solution.relative(&zip_path),
            made.name,
            made.version
        )));
    }
    let file_name = zip_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "package.zip".to_string());
    let mut deployed = Deployed {
        extension,
        root: shown_root,
        package: package.clone(),
        version: sending.clone(),
        installed: installed.clone(),
        bump,
        build,
        zip: solution.relative(&zip_path),
        plan: match &installed {
            None => format!(
                "install {package} {}",
                wanted.map_or(local.clone(), show_version)
            ),
            Some(old) => format!(
                "upgrade {package} from {old} to {}",
                wanted.map_or(local.clone(), show_version)
            ),
        },
        validated: false,
        applied: false,
        queued: false,
        missing: Vec::new(),
    };
    // Before --apply bumps it, a zip at the installed version is one the server would skip.
    if installed.as_deref() == Some(sending.as_str()) {
        return Ok(deployed);
    }
    let imported =
        extensions::import_with_progress(&remote, &file_name, &zip, request.apply, progress)
            .map_err(|error| {
                let error = error.to_string();
                if !request.apply {
                    return error;
                }
                // The install may have gone through before the failure: only a package list
                // without the new version proves the bump can go back.
                match extensions::list(&remote) {
                    Ok(packages)
                        if !packages
                            .iter()
                            .any(|p| p.name == package && p.version == sending) =>
                    {
                        undo(error)
                    }
                    Ok(_) => format!(
                        "{error}\n(the server has {package} {sending}; packageVersion stays)"
                    ),
                    Err(_) => format!(
                "{error}\n(the server could not be asked whether it has {package} {sending}; \
                         packageVersion stays {sending})"
            ),
                }
            })?;
    deployed.validated = true;
    deployed.applied = imported.applied;
    deployed.queued = imported.queued;
    if imported.applied && !imported.queued {
        let shown = extensions::show(&remote, &package).map_err(|e| e.to_string())?;
        let listed: Vec<&str> = shown
            .extensions
            .iter()
            .filter_map(|row| row.get("name").and_then(serde_json::Value::as_str))
            .collect();
        deployed.missing = declared_names(&root)?
            .into_iter()
            .filter(|name| !listed.contains(&name.as_str()))
            .collect();
    }
    Ok(deployed)
}

/// The `[[extension]]` entry named by its name or root, or the only one.
fn target(solution: &Solution, wanted: Option<&str>) -> Result<(String, PathBuf), String> {
    let entries = &solution.extensions;
    let name_of =
        |entry: &super::config::Extension| entry.name.clone().unwrap_or_else(|| entry.root.clone());
    let chosen = match wanted {
        Some(wanted) => entries
            .iter()
            .find(|entry| entry.name.as_deref() == Some(wanted) || entry.root == wanted),
        None if entries.len() == 1 => entries.first(),
        None => None,
    };
    match chosen {
        Some(entry) => Ok((name_of(entry), solution.root.join(&entry.root))),
        None if entries.is_empty() => Err(
            "twaco.toml has no [[extension]]: add one naming the extension project's root"
                .to_string(),
        ),
        None => Err(format!(
            "{}: name one of {}",
            wanted.map_or("more than one [[extension]]".to_string(), |wanted| format!(
                "no [[extension]] is named {wanted}"
            )),
            entries.iter().map(name_of).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// metadata.xml's path, the ExtensionPackage's name and its packageVersion.
fn package_version(root: &Path) -> Result<(String, String, String), String> {
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
    Ok((path, package.name.clone(), version))
}

fn metadata(root: &Path) -> Result<(String, Vec<u8>), String> {
    ["configfiles/metadata.xml", "metadata.xml"]
        .into_iter()
        .find_map(|path| {
            std::fs::read(root.join(path))
                .ok()
                .map(|bytes| (path.to_string(), bytes))
        })
        .ok_or_else(|| format!("{} has no metadata.xml", root.display()))
}

/// The entity names metadata.xml declares, which the server's package lists once installed.
fn declared_names(root: &Path) -> Result<Vec<String>, String> {
    let (path, bytes) = metadata(root)?;
    Ok(ext_check::declarations(&bytes)
        .map_err(|why| format!("{path}: {why}"))?
        .into_iter()
        .filter(|entry| !matches!(entry.tag.as_str(), "ExtensionPackage" | "JarResources"))
        .map(|entry| entry.name)
        .filter(|name| !name.is_empty())
        .collect())
}

fn parse_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.split('.').map(|part| part.parse::<u64>().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

fn show_version([major, minor, patch]: [u64; 3]) -> String {
    format!("{major}.{minor}.{patch}")
}

/// Write `version` as metadata.xml's packageVersion, and as `extension.properties`'s
/// `package_version` when it names one, each file replaced whole.
fn set_version(root: &Path, metadata_path: &str, version: &str) -> Result<(), String> {
    let path = root.join(metadata_path);
    let bytes = std::fs::read(&path).map_err(|e| format!("{metadata_path}: {e}"))?;
    let tokens = scan::tokenize(&bytes).map_err(|e| format!("{metadata_path}: {e}"))?;
    let span = tokens
        .iter()
        .filter(|token| matches!(token.kind, Kind::Start | Kind::Empty))
        .filter(|token| token.name.of(&bytes) == b"ExtensionPackage")
        .find_map(|token| {
            scan::attribute(&bytes, token, "packageVersion")
                .ok()
                .flatten()
        })
        .ok_or_else(|| format!("{metadata_path}: the ExtensionPackage has no packageVersion"))?;
    let mut edited = bytes[..span.start].to_vec();
    edited.extend_from_slice(version.as_bytes());
    edited.extend_from_slice(&bytes[span.end..]);
    replace(&path, &edited)?;
    let properties = root.join("extension.properties");
    let is_version = |line: &str| {
        line.split_once(['=', ':'])
            .is_some_and(|(key, _)| key.trim() == "package_version")
    };
    if let Ok(text) = std::fs::read_to_string(&properties) {
        if text.lines().any(is_version) {
            let edited: String = text
                .split_inclusive('\n')
                .map(|line| {
                    if is_version(line) {
                        let ending = &line[line.trim_end_matches(['\r', '\n']).len()..];
                        format!("package_version={version}{ending}")
                    } else {
                        line.to_string()
                    }
                })
                .collect();
            // Both files or neither.
            if let Err(error) = replace(&properties, edited.as_bytes()) {
                return Err(match replace(&path, &bytes) {
                    Ok(()) => error,
                    Err(why) => format!("{error}; and putting {metadata_path} back failed: {why}"),
                });
            }
        }
    }
    Ok(())
}

fn replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    super::ext_new::replace_whole(path, bytes)
}

/// The project's build: its Gradle wrapper, else Gradle, else Ant. The command, as run.
fn run_build(root: &Path) -> Result<String, String> {
    let wrapper = if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    };
    let (tool, args): (String, &[&str]) = if root.join(wrapper).is_file() {
        (
            root.join(wrapper).to_string_lossy().into_owned(),
            &["build"],
        )
    } else if root.join("build.gradle").is_file() || root.join("build.gradle.kts").is_file() {
        ("gradle".to_string(), &["build"])
    } else if root.join("build-extension.xml").is_file() {
        ("ant".to_string(), &["-f", "build-extension.xml"])
    } else if root.join("build.xml").is_file() {
        ("ant".to_string(), &[])
    } else {
        return Err(format!(
            "{} has no Gradle or Ant build: build the zip yourself and pass --no-build",
            root.display()
        ));
    };
    // On Windows gradle and ant are batch files, which only cmd runs; a checked-out gradlew
    // may have lost its execute bit, which sh does not need.
    let mut command = if cfg!(windows) {
        let mut command = std::process::Command::new("cmd");
        command.arg("/C").arg(&tool);
        command
    } else if tool.ends_with(wrapper) {
        let mut command = std::process::Command::new("sh");
        command.arg(&tool);
        command
    } else {
        std::process::Command::new(&tool)
    };
    let shown = format!("{tool} {}", args.join(" ")).trim_end().to_string();
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
    let text: String = [stdout, stderr]
        .into_iter()
        .flatten()
        .map(|reader| reader.join().unwrap_or_default())
        .collect();
    if !status.success() {
        let lines: Vec<&str> = text.lines().collect();
        let tail = lines[lines.len().saturating_sub(30)..].join("\n");
        return Err(format!("{shown} failed:\n{tail}"));
    }
    Ok(shown)
}

/// The last 64 KiB a build writes to one stream, read as it runs so a long build neither
/// blocks on a full pipe nor fills memory.
fn tail_of(mut stream: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        const KEEP: usize = 64 * 1024;
        let mut kept = std::collections::VecDeque::with_capacity(KEEP);
        let mut buffer = [0u8; 8192];
        while let Ok(read) = stream.read(&mut buffer) {
            if read == 0 {
                break;
            }
            kept.extend(&buffer[..read]);
            let excess = kept.len().saturating_sub(KEEP);
            kept.drain(..excess);
        }
        String::from_utf8_lossy(kept.make_contiguous()).into_owned()
    })
}

/// The package zip that holds `package` at `version`: under `build/distributions/` first, where
/// the Gradle and Ant templates put it, then elsewhere under `build/`, then under `zip/`; the
/// newest in the first place that has one. A build that had nothing to do leaves the zip it made
/// before, so its age proves nothing.
fn package_zip(
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
    let place = |path: &Path| {
        if path.starts_with(&distributions) {
            0
        } else if path.starts_with(root.join("build")) {
            1
        } else {
            2
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
mod tests {
    use super::*;
    use crate::core::server::ServerError;
    use serde_json::{json, Value};
    use std::cell::RefCell;
    use std::io::Write as _;

    /// A server with packages: an install puts the zip's version in, or queues it. With `fail`,
    /// the install answers an error, after taking the package when `fail` is `Some(true)`.
    struct Fake {
        installed: RefCell<Vec<(String, String)>>,
        listed: Vec<&'static str>,
        queue: bool,
        fail: Option<bool>,
        uploads: RefCell<Vec<bool>>,
    }

    impl Remote for &Fake {
        fn service(&self, service: &str, _: &Value) -> Result<Option<Value>, ServerError> {
            let rows: Vec<Value> = match service {
                "GetExtensionPackageList" => self
                    .installed
                    .borrow()
                    .iter()
                    .map(|(n, v)| json!({ "name": n, "packageVersion": v }))
                    .collect(),
                "GetExtensionPackageDetails" => {
                    self.listed.iter().map(|n| json!({ "name": n })).collect()
                }
                "GetExtensionsInUse" => Vec::new(),
                other => panic!("unexpected {other}"),
            };
            Ok(Some(json!({ "rows": rows })))
        }

        fn upload(&self, _: &str, zip: &[u8], validate: bool) -> Result<Value, ServerError> {
            self.uploads.borrow_mut().push(validate);
            let mut rows =
                vec![json!({ "validate": { "rows": [{ "extensionReportStatus": 0 }] } })];
            if !validate {
                if !self.queue && self.fail != Some(false) {
                    let made = extensions::inspect(zip).unwrap();
                    let mut installed = self.installed.borrow_mut();
                    installed.retain(|(n, _)| *n != made.name);
                    installed.push((made.name, made.version));
                }
                if self.fail.is_some() {
                    return Err(ServerError::Transport {
                        method: crate::core::server::Method::Post,
                        url: "http://example.invalid/".to_string(),
                        why: "connection reset".to_string(),
                    });
                }
                let status = if self.queue { 3 } else { 0 };
                rows.push(json!({ "install": { "rows": [{ "extensionReportStatus": status }] } }));
            }
            Ok(json!({ "rows": rows }))
        }
    }

    fn fake(installed: &[(&str, &str)], listed: &[&'static str]) -> Fake {
        Fake {
            installed: RefCell::new(
                installed
                    .iter()
                    .map(|(n, v)| (n.to_string(), v.to_string()))
                    .collect(),
            ),
            listed: listed.to_vec(),
            queue: false,
            fail: None,
            uploads: RefCell::new(Vec::new()),
        }
    }

    const METADATA: &str = r#"<Entities>
    <ExtensionPackages>
        <ExtensionPackage name="Acme" packageVersion="VERSION" vendor="v"/>
    </ExtensionPackages>
    <Widgets>
        <Widget name="gauge">
            <UIResources>
                <FileResource type="JS" file="gauge.ide.js"/>
                <FileResource type="JS" file="gauge.runtime.js"/>
            </UIResources>
        </Widget>
    </Widgets>
</Entities>
"#;

    /// A solution whose one `[[extension]]` is a widget project beside it, with a package zip
    /// already built at `built`.
    fn setup(version: &str, built: &str) -> (tempfile::TempDir, Solution, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let solution_root = temporary.path().join("solution");
        let extension = temporary.path().join("Acme");
        std::fs::create_dir_all(solution_root.join(".twaco/profiles")).unwrap();
        std::fs::write(
            solution_root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\n\n[[extension]]\nroot = \"../Acme\"\nname = \"Acme\"\n",
        )
        .unwrap();
        std::fs::write(
            solution_root.join(".twaco/profiles/default.toml"),
            "url = \"http://example.invalid/Thingworx/\"\nusername = \"u\"\npassword = \"p\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(extension.join("configfiles")).unwrap();
        std::fs::create_dir_all(extension.join("ui/gauge")).unwrap();
        for file in ["gauge.ide.js", "gauge.runtime.js"] {
            std::fs::write(extension.join("ui/gauge").join(file), "").unwrap();
        }
        std::fs::write(
            extension.join("configfiles/metadata.xml"),
            METADATA.replace("VERSION", version),
        )
        .unwrap();
        std::fs::write(
            extension.join("extension.properties"),
            format!("package_version={version}\r\nproject_vendor=v\r\n"),
        )
        .unwrap();
        write_zip(&extension, built);
        let solution = Solution::load(&solution_root.join("twaco.toml")).unwrap();
        (temporary, solution, extension)
    }

    fn write_zip(extension: &Path, version: &str) {
        std::fs::create_dir_all(extension.join("build/distributions")).unwrap();
        let file = std::fs::File::create(extension.join("build/distributions/Acme.zip")).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("metadata.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(METADATA.replace("VERSION", version).as_bytes())
            .unwrap();
        zip.finish().unwrap();
    }

    fn request(bump: Option<Bump>, apply: bool) -> DeployRequest {
        DeployRequest {
            target: None,
            profile: "default".to_string(),
            build: false,
            bump,
            apply,
        }
    }

    fn run(
        solution: &Solution,
        server: &Fake,
        request: &DeployRequest,
    ) -> Result<Deployed, String> {
        deploy(solution, request, |_| server, &progress::NONE)
    }

    #[test]
    fn a_new_package_is_validated_then_installed_and_its_entities_confirmed() {
        let (_guard, solution, _) = setup("1.0.0", "1.0.0");
        let server = fake(&[], &["gauge"]);
        let planned = run(&solution, &server, &request(None, false)).unwrap();
        assert_eq!(planned.plan, "install Acme 1.0.0");
        assert!(planned.validated && !planned.applied);
        assert_eq!(planned.zip, "../Acme/build/distributions/Acme.zip");
        let done = run(&solution, &server, &request(None, true)).unwrap();
        assert!(done.applied && !done.queued);
        assert!(done.missing.is_empty(), "{:?}", done.missing);
        assert_eq!(*server.uploads.borrow(), [true, true, false]);
    }

    #[test]
    fn an_installed_version_needs_a_bump_which_apply_writes() {
        let (_guard, solution, extension) = setup("1.0.0", "1.0.0");
        let server = fake(&[("Acme", "1.0.0")], &[]);
        let error = run(&solution, &server, &request(None, false)).unwrap_err();
        assert!(error.contains("--bump patch"), "{error}");

        let planned = run(&solution, &server, &request(Some(Bump::Patch), false)).unwrap();
        assert_eq!(planned.bump.as_deref(), Some("1.0.0 -> 1.0.1"));
        assert_eq!(planned.plan, "upgrade Acme from 1.0.0 to 1.0.1");
        assert!(!planned.validated, "the zip still carries 1.0.0");
        assert!(server.uploads.borrow().is_empty());
        let metadata = extension.join("configfiles/metadata.xml");
        assert!(std::fs::read_to_string(&metadata)
            .unwrap()
            .contains("\"1.0.0\""));

        // Applied, the version is written; a zip still at 1.0.0 is refused, and it is put back.
        let error = run(&solution, &server, &request(Some(Bump::Patch), true)).unwrap_err();
        assert!(error.contains("no zip of Acme 1.0.1"), "{error}");
        assert!(error.contains("packageVersion is 1.0.0 again"), "{error}");
        assert!(std::fs::read_to_string(&metadata)
            .unwrap()
            .contains("\"1.0.0\""));

        write_zip(&extension, "1.0.1");
        let mut server = fake(&[("Acme", "1.0.0")], &[]);
        server.queue = true;
        let done = run(&solution, &server, &request(Some(Bump::Patch), true)).unwrap();
        assert!(done.applied && done.queued);
        assert!(std::fs::read_to_string(&metadata)
            .unwrap()
            .contains("packageVersion=\"1.0.1\""));
        assert_eq!(
            std::fs::read_to_string(extension.join("extension.properties")).unwrap(),
            "package_version=1.0.1\r\nproject_vendor=v\r\n"
        );
    }

    #[test]
    fn a_newer_server_version_and_a_missing_entity_are_reported() {
        let (_guard, solution, _) = setup("1.0.0", "1.0.0");
        let server = fake(&[("Acme", "1.2.0")], &[]);
        let error = run(&solution, &server, &request(None, false)).unwrap_err();
        assert!(error.contains("newer than 1.0.0"), "{error}");

        let (_guard, solution, extension) = setup("1.0.0", "1.2.1");
        let server = fake(&[("Acme", "1.2.0")], &[]);
        let done = run(&solution, &server, &request(Some(Bump::Patch), true)).unwrap();
        assert_eq!(
            done.version, "1.2.1",
            "the bump starts from the server's version"
        );
        assert_eq!(done.missing, ["gauge"]);
        assert!(
            std::fs::read_to_string(extension.join("configfiles/metadata.xml"))
                .unwrap()
                .contains("\"1.2.1\"")
        );
    }

    #[test]
    fn a_failed_install_puts_the_bump_back_only_when_the_server_lacks_it() {
        let (_guard, solution, extension) = setup("1.0.0", "1.0.1");
        let metadata = extension.join("configfiles/metadata.xml");
        let mut server = fake(&[("Acme", "1.0.0")], &[]);
        server.fail = Some(false);
        let error = run(&solution, &server, &request(Some(Bump::Patch), true)).unwrap_err();
        assert!(error.contains("connection reset"), "{error}");
        assert!(error.contains("packageVersion is 1.0.0 again"), "{error}");
        assert!(std::fs::read_to_string(&metadata)
            .unwrap()
            .contains("\"1.0.0\""));

        let mut server = fake(&[("Acme", "1.0.0")], &[]);
        server.fail = Some(true);
        let error = run(&solution, &server, &request(Some(Bump::Patch), true)).unwrap_err();
        assert!(
            error.contains("the server has Acme 1.0.1; packageVersion stays"),
            "{error}"
        );
        assert!(std::fs::read_to_string(&metadata)
            .unwrap()
            .contains("\"1.0.1\""));
    }

    #[test]
    fn a_bump_rewrites_only_the_package_version_key() {
        let (_guard, _solution, extension) = setup("1.0.0", "1.0.0");
        let properties = extension.join("extension.properties");
        std::fs::write(
            &properties,
            "package_version_note=keep
  package_version : 1.0.0
project_vendor=v
",
        )
        .unwrap();
        set_version(&extension, "configfiles/metadata.xml", "1.0.1").unwrap();
        assert_eq!(
            std::fs::read_to_string(&properties).unwrap(),
            "package_version_note=keep
package_version=1.0.1
project_vendor=v
"
        );
    }

    #[test]
    fn the_zip_is_at_the_version_sent_and_build_distributions_comes_first() {
        let (_guard, _solution, extension) = setup("1.0.0", "1.0.0");
        let elsewhere = extension.join("zip");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::copy(
            extension.join("build/distributions/Acme.zip"),
            elsewhere.join("Acme.zip"),
        )
        .unwrap();
        let found = package_zip(&extension, "Acme", "Acme", "1.0.0").unwrap();
        assert!(
            found.starts_with(extension.join("build/distributions")),
            "{found:?}"
        );

        let error = package_zip(&extension, "Acme", "Acme", "1.0.1").unwrap_err();
        assert!(
            error.contains("no zip of Acme 1.0.1 under Acme/build or Acme/zip, only 1.0.0:"),
            "{error}"
        );
        let error = package_zip(&extension, "Acme", "Other", "1.0.0").unwrap_err();
        assert_eq!(error, "no zip of Other under Acme/build or Acme/zip");
    }

    #[test]
    fn the_target_is_named_or_the_only_one() {
        let (_guard, solution, _) = setup("1.0.0", "1.0.0");
        assert_eq!(target(&solution, None).unwrap().0, "Acme");
        assert_eq!(target(&solution, Some("../Acme")).unwrap().0, "Acme");
        let error = target(&solution, Some("Other")).unwrap_err();
        assert!(
            error.contains("no [[extension]] is named Other: name one of Acme"),
            "{error}"
        );
        assert_eq!(parse_version("1.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(Bump::Minor.raise([1, 2, 3]), Some([1, 3, 0]));
        assert_eq!(Bump::Major.raise([u64::MAX, 0, 0]), None);
    }
}
