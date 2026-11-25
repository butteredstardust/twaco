//! The server's extension packages: listed, described, imported and removed, as Composer's
//! Manage > Extensions page does.
//!
//! PlatformSubsystem lists and removes packages. An import is not a service but the
//! `ExtensionPackageUploader` servlet, which with `validate=true` checks a package and installs
//! nothing, which makes it the plan, and with `validate=false` installs it. A package the server
//! cannot read is a bare 406, so twaco opens the zip itself first, to say what is wrong.

use super::codes::{Coded, ErrorCode};
use super::entity::key::ServiceTarget;
use super::progress::{self, Progress};
use super::server::{Client, ServerError};
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(120);

/// What this module asks of a server, as a trait so it is tested offline.
pub trait Remote {
    fn service(&self, service: &str, body: &Value) -> Result<Option<Value>, ServerError>;
    fn upload(&self, file_name: &str, zip: &[u8], validate: bool) -> Result<Value, ServerError>;
}

impl Remote for Client {
    fn service(&self, service: &str, body: &Value) -> Result<Option<Value>, ServerError> {
        self.call_service(
            &ServiceTarget::platform("Subsystems", "PlatformSubsystem"),
            service,
            body,
            TIMEOUT,
        )
    }

    fn upload(&self, file_name: &str, zip: &[u8], validate: bool) -> Result<Value, ServerError> {
        self.upload_extension(file_name, zip, validate)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    #[error("{0}")]
    Remote(ServerError),
    #[error("unexpected extension response: {0}")]
    Shape(String),
    #[error("{0}")]
    Invalid(String),
    /// The change was sent, but the read that confirms it failed: it may have happened.
    #[error("{sent}, but reading the package list to confirm it failed: {source}; run `twaco ext list` before trying again")]
    Unconfirmed {
        sent: String,
        source: Box<ExtensionError>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub vendor: String,
    pub description: String,
    pub minimum_thingworx: String,
    pub group: String,
    pub artifact: String,
    pub(crate) build: String,
    /// Names of the packages this one needs, from metadata.xml's `dependsOn`.
    pub(crate) depends_on: Vec<String>,
    required_versions: std::collections::BTreeMap<String, String>,
}

fn rows(reply: Option<Value>, what: &str) -> Result<Vec<Value>, ExtensionError> {
    reply
        .and_then(|value| value.get("rows").and_then(Value::as_array).cloned())
        .ok_or_else(|| ExtensionError::Shape(format!("{what} returned no rows")))
}

fn text(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// Every installed package, sorted by name.
pub(crate) fn list(remote: &dyn Remote) -> Result<Vec<Package>, ExtensionError> {
    let reply = remote
        .service("GetExtensionPackageList", &json!({}))
        .map_err(ExtensionError::Remote)?;
    let mut packages: Vec<Package> = rows(reply, "GetExtensionPackageList")?
        .iter()
        .map(|row| Package {
            name: text(row, "name"),
            version: text(row, "packageVersion"),
            vendor: text(row, "vendor"),
            description: text(row, "description"),
            minimum_thingworx: text(row, "minimumThingWorxVersion"),
            group: text(row, "groupId"),
            artifact: text(row, "artifactId"),
            build: text(row, "buildNumber"),
            depends_on: Vec::new(),
            required_versions: Default::default(),
        })
        .collect();
    packages.sort_by_key(|p| p.name.to_lowercase());
    Ok(packages)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub package: Package,
    /// The package's extensions, as the server reports them.
    pub extensions: Vec<Value>,
    /// Those in use, which block removal.
    pub in_use: Vec<Value>,
}

pub(crate) fn show(remote: &dyn Remote, name: &str) -> Result<Shown, ExtensionError> {
    let package = list(remote)?
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| {
            ExtensionError::Invalid(format!("no extension package {name:?} is installed"))
        })?;
    let body = json!({ "packageName": name });
    let extensions = rows(
        remote
            .service("GetExtensionPackageDetails", &body)
            .map_err(ExtensionError::Remote)?,
        "GetExtensionPackageDetails",
    )?;
    let in_use = rows(
        remote
            .service("GetExtensionsInUse", &body)
            .map_err(ExtensionError::Remote)?,
        "GetExtensionsInUse",
    )?;
    Ok(Shown {
        package,
        extensions,
        in_use,
    })
}

/// What a package zip says it is, read from its `metadata.xml`, so a package the server would
/// answer with a bare 406 is refused with a reason.
pub(crate) fn inspect(zip: &[u8]) -> Result<Package, ExtensionError> {
    let bad = |why: String| ExtensionError::Invalid(format!("not an extension package: {why}"));
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip))
        .map_err(|e| bad(format!("not a zip ({e})")))?;
    // A crafted zip can expand without end; a real metadata.xml is a few kilobytes.
    const MAX_METADATA_BYTES: u64 = 1 << 20;
    let mut metadata = String::new();
    archive
        .by_name("metadata.xml")
        .map_err(|_| bad("it has no metadata.xml at its root".to_string()))?
        .take(MAX_METADATA_BYTES + 1)
        .read_to_string(&mut metadata)
        .map_err(|e| bad(format!("metadata.xml cannot be read ({e})")))?;
    if metadata.len() as u64 > MAX_METADATA_BYTES {
        return Err(bad("metadata.xml is larger than 1 MiB".to_string()));
    }
    let src = metadata.as_bytes();
    let tokens =
        super::scan::tokenize(src).map_err(|e| bad(format!("metadata.xml is not XML ({e})")))?;
    let element = tokens
        .iter()
        .find(|t| {
            matches!(t.kind, super::scan::Kind::Start | super::scan::Kind::Empty)
                && t.name.of(src) == b"ExtensionPackage"
        })
        .ok_or_else(|| bad("metadata.xml declares no ExtensionPackage".to_string()))?;
    let attribute = |name: &str| -> String {
        super::scan::attribute(src, element, name)
            .ok()
            .flatten()
            .map(|span| super::scan::decode_entities(&String::from_utf8_lossy(span.of(src))))
            .unwrap_or_default()
    };
    let package = Package {
        name: attribute("name"),
        version: attribute("packageVersion"),
        vendor: attribute("vendor"),
        description: attribute("description"),
        minimum_thingworx: attribute("minimumThingWorxVersion"),
        group: attribute("groupId"),
        artifact: attribute("artifactId"),
        build: attribute("buildNumber"),
        depends_on: dependencies(&attribute("dependsOn")),
        required_versions: dependency_versions(&attribute("dependsOn")),
    };
    if package.name.is_empty() || package.version.is_empty() {
        return Err(bad(
            "its ExtensionPackage has no name or no packageVersion".to_string()
        ));
    }
    version_segments(&package.name, &package.version)?;
    Ok(package)
}

/// The package names in a `dependsOn` attribute: JSON such as `{"Acme.Base":{"version":"1.0"}}`
/// or `{"extensions":"Acme.Base:1.0,Acme.Db:2.0"}`, or a plain `Acme.Base:1.0,Acme.Db` list.
/// Required versions are read separately for bundle validation planning.
fn dependencies(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    let names = |list: &str| -> Vec<String> {
        list.split(',')
            .filter_map(|item| {
                let name = item.split(':').next().unwrap_or("").trim();
                (!name.is_empty()).then(|| name.to_string())
            })
            .collect()
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
        return names(raw);
    };
    let mut found = Vec::new();
    for (key, value) in &map {
        match (key.as_str(), value) {
            ("projects", _) => {}
            ("extensions", Value::String(list)) => found.extend(names(list)),
            _ => found.push(key.clone()),
        }
    }
    found
}

fn dependency_versions(raw: &str) -> std::collections::BTreeMap<String, String> {
    let mut versions = std::collections::BTreeMap::new();
    let mut list = |raw: &str| {
        for item in raw.split(',') {
            if let Some((name, version)) = item.trim().split_once(':') {
                versions.insert(name.trim().to_string(), version.trim().to_string());
            }
        }
    };
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) {
        if let Some(Value::String(raw)) = map.get("extensions") {
            list(raw);
        }
        for (name, value) in map {
            if name == "extensions" || name == "projects" {
                continue;
            }
            if let Some(version) = value
                .as_str()
                .or_else(|| value.get("version").and_then(Value::as_str))
            {
                versions.insert(name, version.to_string());
            }
        }
    } else {
        list(raw);
    }
    versions
}

/// Only dotted numeric versions can be compared safely; trailing zero segments are equivalent.
fn version_segments(name: &str, version: &str) -> Result<Vec<u64>, ExtensionError> {
    version
        .split('.')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ExtensionError::Invalid(format!(
                "package {name} has unsupported version {version:?}; use dotted numeric segments"
            )));
            }
            part.parse().map_err(|_| {
                ExtensionError::Invalid(format!(
            "package {name} has overflowing version {version:?}; use numeric segments within u64"
        ))
            })
        })
        .collect()
}

pub(crate) fn version_order(
    name: &str,
    a: &str,
    b: &str,
) -> Result<std::cmp::Ordering, ExtensionError> {
    let (mut a, mut b) = (version_segments(name, a)?, version_segments(name, b)?);
    let longest = a.len().max(b.len());
    a.resize(longest, 0);
    b.resize(longest, 0);
    Ok(a.cmp(&b))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub(crate) package: Package,
    /// new, or upgrade from X.
    pub plan: String,
    pub applied: bool,
    /// The server took it but installs it at its next restart: an upgrade of a package with
    /// Java code.
    pub queued: bool,
    /// The install failed with a transient Windows error and succeeded when sent once more.
    pub retried: bool,
}

/// One part (`validate` or `install`) of the uploader's report.
fn report_row<'a>(reply: &'a Value, part: &str) -> Option<&'a Value> {
    reply
        .get("rows")?
        .as_array()?
        .iter()
        .find_map(|row| row.get(part)?.pointer("/rows/0"))
}

/// The server's report from the uploader: status 0 is fine, and 3 (queued for the next
/// restart); anything else carries a message. Measured on 10.2: 1 failed, 2 skipped (that
/// version is installed), 3 queued.
fn report(reply: &Value, part: &str) -> Result<i64, String> {
    let row = report_row(reply, part)
        .ok_or_else(|| format!("the uploader's answer has no {part} report"))?;
    let status = row
        .get("extensionReportStatus")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    if status == 0 || status == 3 {
        return Ok(status);
    }
    let mut parts = vec![text(row, "reportMessage"), text(row, "extensionException")];
    parts.retain(|part| !part.is_empty());
    parts.dedup();
    let message = parts.join("; ");
    Err(if message.is_empty() {
        format!("status {status}")
    } else {
        message
    })
}

/// [`import_with_progress`] without a progress reporter.
#[cfg(test)]
pub(crate) fn import(
    remote: &dyn Remote,
    file_name: &str,
    zip: &[u8],
    apply: bool,
) -> Result<Imported, ExtensionError> {
    import_with_progress(remote, file_name, zip, apply, &progress::NONE)
}

/// The server's message when Runtime/index.html is held by an antivirus or indexer while an
/// install registers its CSS; sent again a few seconds later it succeeds.
const TRANSIENT_INSTALL: &str = "An error occurred while registering CSS resources";

fn retry_delay() -> Duration {
    if cfg!(test) {
        Duration::ZERO
    } else {
        Duration::from_secs(3)
    }
}

/// Import a package: twaco's own check, then the server's validation, and unless `apply` stop
/// there. Applied, the package list must then show the package at its version.
///
/// It reports progress: one step per upload and per package-list read.
pub(crate) fn import_with_progress(
    remote: &dyn Remote,
    file_name: &str,
    zip: &[u8],
    apply: bool,
    progress: &dyn Progress,
) -> Result<Imported, ExtensionError> {
    let package = inspect(zip)?;
    let installed = {
        let _phase = progress::phase(progress, "reading the package list", Some(1));
        let packages = list(remote)?;
        progress.advance(1);
        packages
    }
    .into_iter()
    .find(|p| p.name == package.name);
    if let Some(old) = &installed {
        version_segments(&old.name, &old.version)?;
    }
    if let Some(old) = installed.as_ref().filter(|o| o.version == package.version) {
        // The uploader validates it, then skips the install.
        return Err(ExtensionError::Invalid(format!(
            "{} {} is installed already, and the server skips a version it has: raise \
             packageVersion to replace it",
            package.name, old.version
        )));
    }
    import_package(
        remote,
        file_name,
        zip,
        package,
        installed.as_ref(),
        apply,
        progress,
    )
}

/// One package against what the server holds: the plan, the validation, and with `apply` the
/// install, sent once more after a transient Windows error, then the confirmation.
fn import_package(
    remote: &dyn Remote,
    file_name: &str,
    zip: &[u8],
    package: Package,
    installed: Option<&Package>,
    apply: bool,
    progress: &dyn Progress,
) -> Result<Imported, ExtensionError> {
    let plan = match installed {
        None => format!("install {} {}", package.name, package.version),
        Some(old) => format!(
            "upgrade {} from {} to {}",
            package.name, old.version, package.version
        ),
    };
    let rejected = |why: String, stage: &str| {
        ExtensionError::Invalid(format!(
            "the server's {stage} refused {}: {why}",
            package.name
        ))
    };
    let reply = {
        let _phase = progress::phase(progress, "validating the package", Some(1));
        let reply = remote.upload(file_name, zip, true).map_err(|e| match e {
            ServerError::Http { status: 406, .. } => {
                rejected("406 Not Acceptable".to_string(), "validation")
            }
            other => ExtensionError::Remote(other),
        })?;
        progress.advance(1);
        reply
    };
    report(&reply, "validate").map_err(|why| rejected(why, "validation"))?;
    if !apply {
        return Ok(Imported {
            package,
            plan,
            applied: false,
            queued: false,
            retried: false,
        });
    }
    let install = || -> Result<i64, ExtensionError> {
        let reply = remote
            .upload(file_name, zip, false)
            .map_err(ExtensionError::Remote)?;
        report(&reply, "validate").map_err(|why| rejected(why, "import"))?;
        // Without an install report, the package list below is the evidence.
        match report_row(&reply, "install") {
            Some(_) => report(&reply, "install").map_err(|why| rejected(why, "install")),
            None => Ok(0),
        }
    };
    let mut retried = false;
    let install_status = {
        let _phase = progress::phase(progress, "importing the package", Some(1));
        let mut result = install();
        if matches!(&result, Err(e) if e.to_string().contains(TRANSIENT_INSTALL)) {
            retried = true;
            progress.warning(
                "extension install hit a transient Windows CSS error; retrying once in 3 seconds",
            );
            std::thread::sleep(retry_delay());
            result = install();
        }
        progress.advance(1);
        result?
    };
    if install_status == 3 {
        return Ok(Imported {
            package,
            plan,
            applied: true,
            queued: true,
            retried,
        });
    }
    let now = {
        let _phase = progress::phase(progress, "confirming the package", Some(1));
        let packages = list(remote).map_err(|error| ExtensionError::Unconfirmed {
            sent: format!(
                "the import was sent and may have installed {} {}",
                package.name, package.version
            ),
            source: Box::new(error),
        })?;
        progress.advance(1);
        packages
    }
    .into_iter()
    .find(|p| p.name == package.name);
    // The server may normalise the version (1.1 to 1.1.0), so compare the numbers.
    let confirmed = now.as_ref().is_some_and(|p| {
        p.version == package.version
            || version_order(&package.name, &p.version, &package.version)
                .is_ok_and(std::cmp::Ordering::is_eq)
    });
    if !confirmed {
        return Err(ExtensionError::Shape(format!(
            "the import was sent, but the package list shows {} at {}",
            package.name,
            now.map_or("nothing".to_string(), |p| p.version)
        )));
    }
    Ok(Imported {
        package,
        plan,
        applied: true,
        queued: false,
        retried,
    })
}

/// One inner package of a bundle zip.
#[derive(Debug, Clone)]
pub(crate) struct Member {
    pub(crate) file: String,
    pub(crate) zip: Vec<u8>,
    pub(crate) package: Package,
}

/// What became of one package of a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleStatus {
    /// Installed (or, in a plan, would be); the plan says new or upgrade.
    Imported(Box<Imported>),
    /// The server holds this version or a newer one.
    Skipped {
        installed: String,
    },
    /// Planned only: it needs a package of the bundle that a plan does not install, so the
    /// server cannot validate it yet.
    Pending {
        after: Vec<String>,
    },
    WaitsForRestart {
        after: Vec<String>,
    },
    Failed(String),
    /// An earlier package failed, so this one was not sent.
    NotAttempted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleEntry {
    pub name: String,
    pub version: String,
    pub file: String,
    pub status: BundleStatus,
}

impl BundleEntry {
    /// One line: `Name version: what happened`.
    pub fn line(&self) -> String {
        let what = match &self.status {
            BundleStatus::Imported(done) if done.queued => {
                format!("queued ({}); installs at the next restart", done.plan)
            }
            BundleStatus::Imported(done) if done.applied && done.retried => {
                format!(
                    "imported ({}), after one retry on a transient Windows error",
                    done.plan
                )
            }
            BundleStatus::Imported(done) if done.applied => format!("imported ({})", done.plan),
            BundleStatus::Imported(done) => format!("would import ({}), validated", done.plan),
            BundleStatus::Skipped { installed } => format!("skipped (installed {installed})"),
            BundleStatus::Pending { after } => format!(
                "would import after {} (server validation waits for it)",
                after.join(", ")
            ),
            BundleStatus::WaitsForRestart { after } => {
                format!("waits for restart (needs {})", after.join(", "))
            }
            BundleStatus::Failed(why) => format!("failed: {why}"),
            BundleStatus::NotAttempted => "not attempted: an earlier package failed".to_string(),
        };
        format!("{} {}: {what}", self.name, self.version)
    }

    pub fn failed(&self) -> bool {
        matches!(self.status, BundleStatus::Failed(_))
    }

    /// Queued for the next restart, or waiting on a package that is.
    pub fn waits_for_restart(&self) -> bool {
        match &self.status {
            BundleStatus::WaitsForRestart { .. } => true,
            BundleStatus::Imported(done) => done.queued,
            _ => false,
        }
    }
}

/// A zip of extension packages: no metadata.xml at its root, and nothing but `*.zip` files, each
/// an extension package. `None` when the zip is anything else, which the caller reads as one
/// package.
pub(crate) fn bundle_members(zip: &[u8]) -> Result<Option<Vec<Member>>, ExtensionError> {
    // A crafted inner zip can expand without end.
    const MAX_MEMBER_BYTES: u64 = 1 << 30;
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(zip)) else {
        return Ok(None);
    };
    if archive.by_name("metadata.xml").is_ok() {
        return Ok(None);
    }
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            return Ok(None);
        };
        if entry.is_dir() {
            continue;
        }
        let Ok(name) = entry.name().map(|n| n.to_string()) else {
            return Ok(None);
        };
        if name.contains(['\\', ':']) {
            return Err(ExtensionError::Invalid(format!("{name:?} in the bundle is not a root zip filename; remove separators and drive syntax")));
        }
        if name.contains('/') || !name.to_lowercase().ends_with(".zip") {
            return Ok(None);
        }
        files.push((index, name));
    }
    if files.is_empty() {
        return Ok(None);
    }
    let mut members = Vec::new();
    for (index, file) in files {
        let unread = |e: &dyn std::fmt::Display| {
            ExtensionError::Invalid(format!("{file} in the bundle cannot be read ({e})"))
        };
        let mut bytes = Vec::new();
        archive
            .by_index(index)
            .map_err(|e| unread(&e))?
            .take(MAX_MEMBER_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| unread(&e))?;
        if bytes.len() as u64 > MAX_MEMBER_BYTES {
            return Err(ExtensionError::Invalid(format!(
                "{file} in the bundle is larger than 1 GiB"
            )));
        }
        let package = inspect(&bytes)
            .map_err(|e| ExtensionError::Invalid(format!("{file} in the bundle: {e}")))?;
        members.push(Member {
            file,
            zip: bytes,
            package,
        });
    }
    Ok(Some(members))
}

/// The members in an order that installs each after what it needs. A dependency outside the
/// bundle must be installed on the server; a cycle is refused. Both name the packages.
fn install_order(
    members: Vec<Member>,
    installed: &[Package],
) -> Result<Vec<Member>, ExtensionError> {
    let bundled: Vec<String> = members.iter().map(|m| m.package.name.clone()).collect();
    let mut missing: Vec<String> = Vec::new();
    for member in &members {
        for need in &member.package.depends_on {
            if !bundled.contains(need)
                && !installed.iter().any(|p| &p.name == need)
                && !missing.contains(need)
            {
                missing.push(need.clone());
            }
        }
    }
    if !missing.is_empty() {
        return Err(ExtensionError::Invalid(format!(
            "the bundle needs {}, which is neither in it nor installed on the server; import \
             that first; nothing was sent",
            missing.join(", ")
        )));
    }
    let mut done: Vec<String> = Vec::new();
    let mut waiting = members;
    let mut ordered = Vec::new();
    while !waiting.is_empty() {
        let ready = waiting.iter().position(|m| {
            m.package
                .depends_on
                .iter()
                .all(|need| !bundled.contains(need) || done.contains(need))
        });
        let Some(at) = ready else {
            let names: Vec<&str> = waiting.iter().map(|m| m.package.name.as_str()).collect();
            return Err(ExtensionError::Invalid(format!(
                "the packages {} depend on each other in a cycle; nothing was sent",
                names.join(", ")
            )));
        };
        let member = waiting.remove(at);
        done.push(member.package.name.clone());
        ordered.push(member);
    }
    Ok(ordered)
}

// Per-package phases must not replace the bundle's progress bar.
struct BundleWarnings<'a>(&'a dyn Progress);

impl Progress for BundleWarnings<'_> {
    fn start(&self, _: &str, _: Option<u64>) {}
    fn advance(&self, _: u64) {}
    fn message(&self, _: &str) {}
    fn finish(&self) {}
    fn warning(&self, text: &str) {
        self.0.warning(text);
    }
}

/// Import a bundle's packages one by one in dependency order, skipping those the server holds
/// at the same or a newer version. A plan validates each package whose needs are installed
/// already; the rest are `Pending`. The first failure stops the run, and says what was left.
pub(crate) fn import_bundle_with_progress(
    remote: &dyn Remote,
    members: Vec<Member>,
    apply: bool,
    progress: &dyn Progress,
) -> Result<Vec<BundleEntry>, ExtensionError> {
    let mut installed = {
        let _phase = progress::phase(progress, "reading the package list", Some(1));
        let packages = list(remote)?;
        progress.advance(1);
        packages
    };
    let ordered = install_order(members, &installed)?;
    for member in &ordered {
        if let Some(held) = installed.iter().find(|p| p.name == member.package.name) {
            version_segments(&held.name, &held.version)?;
        }
    }
    let bundled_versions: std::collections::BTreeMap<_, _> = ordered
        .iter()
        .map(|member| (member.package.name.clone(), member.package.version.clone()))
        .collect();
    let mut waiting_for_restart = std::collections::BTreeSet::new();
    let mut entries = Vec::new();
    let mut failed = false;
    let _phase = progress::phase(progress, "importing the bundle", Some(ordered.len() as u64));
    for member in ordered {
        progress.message(&member.package.name);
        let package = member.package.clone();
        let entry = |status| BundleEntry {
            name: package.name.clone(),
            version: package.version.clone(),
            file: member.file.clone(),
            status,
        };
        if failed {
            entries.push(entry(BundleStatus::NotAttempted));
            progress.advance(1);
            continue;
        }
        let held = installed.iter().find(|p| p.name == package.name).cloned();
        let skip = held
            .as_ref()
            .map(|h| version_order(&package.name, &h.version, &package.version))
            .transpose()?
            .is_some_and(|order| order.is_ge());
        if let Some(held) = held.as_ref().filter(|_| skip) {
            entries.push(entry(BundleStatus::Skipped {
                installed: held.version.clone(),
            }));
            progress.advance(1);
            continue;
        }
        let restart: Vec<String> = package
            .depends_on
            .iter()
            .filter(|need| waiting_for_restart.contains(*need))
            .cloned()
            .collect();
        if !restart.is_empty() {
            waiting_for_restart.insert(package.name.clone());
            entries.push(entry(BundleStatus::WaitsForRestart { after: restart }));
            progress.advance(1);
            continue;
        }
        let mut after = Vec::new();
        if !apply {
            for need in &package.depends_on {
                let held = installed.iter().find(|p| &p.name == need);
                let satisfied = match (held, package.required_versions.get(need)) {
                    (Some(held), Some(required)) => {
                        version_order(need, &held.version, required)?.is_ge()
                    }
                    (Some(_), None) => true,
                    (None, _) => false,
                };
                if !satisfied {
                    if let Some(version) = bundled_versions.get(need) {
                        after.push(format!("{need} {version}"));
                    }
                }
            }
        }
        if !apply && !after.is_empty() {
            entries.push(entry(BundleStatus::Pending { after }));
            progress.advance(1);
            continue;
        }
        match import_package(
            remote,
            &member.file,
            &member.zip,
            package.clone(),
            held.as_ref(),
            apply,
            &BundleWarnings(progress),
        ) {
            Ok(done) => {
                if done.queued {
                    waiting_for_restart.insert(package.name.clone());
                } else if apply {
                    installed.retain(|p| p.name != package.name);
                    installed.push(package.clone());
                }
                entries.push(entry(BundleStatus::Imported(Box::new(done))));
            }
            Err(error) => {
                failed = true;
                entries.push(entry(BundleStatus::Failed(error.to_string())));
            }
        }
        progress.advance(1);
    }
    Ok(entries)
}

/// Remove a package, refused while any of its extensions is in use. A plan unless `apply`;
/// applied, the package must be gone from the list.
pub(crate) fn remove(
    remote: &dyn Remote,
    name: &str,
    apply: bool,
) -> Result<String, ExtensionError> {
    let shown = show(remote, name)?;
    if !shown.in_use.is_empty() {
        let names: Vec<String> = shown
            .in_use
            .iter()
            .map(|row| text(row, "name"))
            .filter(|n| !n.is_empty())
            .collect();
        return Err(ExtensionError::Invalid(format!(
            "{name} is in use ({}); remove what uses it first; nothing was sent",
            if names.is_empty() {
                format!("{} extension(s)", shown.in_use.len())
            } else {
                names.join(", ")
            }
        )));
    }
    let plan = format!(
        "remove {name} {} with {} extension(s)",
        shown.package.version,
        shown.extensions.len()
    );
    if !apply {
        return Ok(plan);
    }
    remote
        .service("DeleteExtensionPackage", &json!({ "packageName": name }))
        .map_err(ExtensionError::Remote)?;
    let after = list(remote).map_err(|error| ExtensionError::Unconfirmed {
        sent: format!("{plan} was sent and may have removed {name}"),
        source: Box::new(error),
    })?;
    if after.iter().any(|p| p.name == name) {
        return Err(ExtensionError::Shape(format!(
            "{plan} was sent, but {name} is still installed"
        )));
    }
    Ok(plan)
}

impl Coded for ExtensionError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Remote(error) => error.code(),
            Self::Shape(_) => ErrorCode::InvalidData,
            Self::Unconfirmed { source, .. } => source.code(),
            Self::Invalid(_) => ErrorCode::InvalidArguments,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;

    fn package_zip(metadata: Option<&str>) -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            match metadata {
                Some(text) => {
                    zip.start_file("metadata.xml", options).unwrap();
                    zip.write_all(text.as_bytes()).unwrap();
                }
                None => {
                    zip.start_file("readme.txt", options).unwrap();
                    zip.write_all(b"x").unwrap();
                }
            }
            zip.finish().unwrap();
        }
        buffer.into_inner()
    }

    fn metadata(name: &str, version: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><Entities><ExtensionPackages><ExtensionPackage name="{name}" packageVersion="{version}" vendor="v" minimumThingWorxVersion="9.0.0"/></ExtensionPackages></Entities>"#
        )
    }

    /// A server as measured: validate checks only, an import installs, a delete removes.
    struct Fake {
        installed: RefCell<Vec<(String, String)>>,
        in_use: Vec<&'static str>,
        calls: RefCell<Vec<String>>,
        reject: bool,
        /// The install row's status: 0 installs, 3 queues for the next restart.
        install_status: i64,
        /// The package list cannot be read once a delete or an install was sent.
        list_fails_after_delete: bool,
        /// How many installs fail with the transient Windows error before one succeeds.
        transient: RefCell<u32>,
        /// A package whose install the server refuses.
        refuse: Option<&'static str>,
        /// The version the list shows for an installed package, as a server that normalises it.
        shows: Option<&'static str>,
    }

    impl Fake {
        fn with(installed: &[(&str, &str)]) -> Self {
            Fake {
                installed: RefCell::new(
                    installed
                        .iter()
                        .map(|(n, v)| (n.to_string(), v.to_string()))
                        .collect(),
                ),
                in_use: Vec::new(),
                calls: RefCell::new(Vec::new()),
                reject: false,
                install_status: 0,
                list_fails_after_delete: false,
                transient: RefCell::new(0),
                refuse: None,
                shows: None,
            }
        }
    }

    impl Remote for Fake {
        fn service(&self, service: &str, body: &Value) -> Result<Option<Value>, ServerError> {
            self.calls.borrow_mut().push(service.to_string());
            let deleted =
                self.calls.borrow().iter().any(|call| {
                    call == "DeleteExtensionPackage" || call == "upload validate=false"
                });
            if service == "GetExtensionPackageList" && self.list_fails_after_delete && deleted {
                return Err(ServerError::InvalidUrl("connection reset".to_string()));
            }
            let rows: Vec<Value> = match service {
                "GetExtensionPackageList" => self
                    .installed
                    .borrow()
                    .iter()
                    .map(|(n, v)| json!({ "name": n, "packageVersion": v }))
                    .collect(),
                "GetExtensionPackageDetails" => vec![json!({ "name": "OneExtension" })],
                "GetExtensionsInUse" => self.in_use.iter().map(|n| json!({ "name": n })).collect(),
                "DeleteExtensionPackage" => {
                    let name = body["packageName"].as_str().unwrap().to_string();
                    self.installed.borrow_mut().retain(|(n, _)| *n != name);
                    return Ok(None);
                }
                other => panic!("unexpected {other}"),
            };
            Ok(Some(json!({ "rows": rows })))
        }

        fn upload(&self, _: &str, zip: &[u8], validate: bool) -> Result<Value, ServerError> {
            self.calls
                .borrow_mut()
                .push(format!("upload validate={validate}"));
            let status = if self.reject { 1 } else { 0 };
            let package = inspect(zip).unwrap();
            for need in &package.depends_on {
                let installed = self.installed.borrow();
                let satisfied = installed.iter().any(|(name, version)| {
                    name == need
                        && package.required_versions.get(need).is_none_or(|required| {
                            version_order(need, version, required).unwrap().is_ge()
                        })
                });
                if !satisfied {
                    return Ok(json!({ "rows": [
                        { "validate": { "rows": [{ "extensionReportStatus": 1,
                            "reportMessage": format!("Could not find extension {need} depended upon by {}", package.name) }] } }
                    ] }));
                }
            }
            if !validate && *self.transient.borrow() > 0 {
                *self.transient.borrow_mut() -= 1;
                return Ok(json!({ "rows": [
                    { "validate": { "rows": [{ "extensionReportStatus": 0 }] } },
                    { "install": { "rows": [{ "extensionReportStatus": 1, "reportMessage": "An error occurred while registering CSS resources" }] } }
                ] }));
            }
            if !validate && self.refuse == Some(package.name.as_str()) {
                return Ok(json!({ "rows": [
                    { "validate": { "rows": [{ "extensionReportStatus": 0 }] } },
                    { "install": { "rows": [{ "extensionReportStatus": 1, "reportMessage": "ThingPackage className is invalid" }] } }
                ] }));
            }
            if !validate && !self.reject && self.install_status == 0 {
                let mut installed = self.installed.borrow_mut();
                installed.retain(|(n, _)| *n != package.name);
                let version = self.shows.map_or(package.version, str::to_string);
                installed.push((package.name, version));
            }
            let mut rows = vec![
                json!({ "validate": { "rows": [{ "extensionReportStatus": status, "reportMessage": if self.reject { "bad package" } else { "" } }] } }),
            ];
            if !validate {
                let message = match self.install_status {
                    3 => "Extension P is queued for installation on the next server restart",
                    1 => "ThingPackage className is invalid",
                    _ => "",
                };
                rows.push(json!({ "install": { "rows": [{ "extensionReportStatus": self.install_status, "reportMessage": message }] } }));
            }
            Ok(json!({ "rows": rows }))
        }
    }

    #[test]
    fn a_zip_is_read_for_what_it_declares_and_a_bad_one_is_named() {
        let package = inspect(&package_zip(Some(&metadata("P", "1.2.0")))).unwrap();
        assert_eq!(
            (
                package.name.as_str(),
                package.version.as_str(),
                package.vendor.as_str()
            ),
            ("P", "1.2.0", "v")
        );
        assert!(inspect(b"not a zip")
            .unwrap_err()
            .to_string()
            .contains("not a zip"));
        assert!(inspect(&package_zip(None))
            .unwrap_err()
            .to_string()
            .contains("no metadata.xml"));
        assert!(inspect(&package_zip(Some("<Entities/>")))
            .unwrap_err()
            .to_string()
            .contains("no ExtensionPackage"));
    }

    #[test]
    fn an_applied_import_reports_its_uploads_as_phases() {
        use crate::core::progress::Recorder;
        let fake = Fake::with(&[("P", "1.0.0")]);
        let zip = package_zip(Some(&metadata("P", "1.2.0")));
        let recorder = Recorder::default();
        import_with_progress(&fake, "p.zip", &zip, true, &recorder).unwrap();
        let names: Vec<String> = recorder.phases().into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            [
                "reading the package list",
                "validating the package",
                "importing the package",
                "confirming the package"
            ]
        );
        assert_eq!(recorder.advanced(), 4);
    }

    #[test]
    fn an_import_is_validated_only_unless_applied_and_says_what_it_does() {
        let fake = Fake::with(&[("P", "1.0.0")]);
        let zip = package_zip(Some(&metadata("P", "1.2.0")));
        let planned = import(&fake, "p.zip", &zip, false).unwrap();
        assert_eq!(planned.plan, "upgrade P from 1.0.0 to 1.2.0");
        assert!(
            !fake
                .calls
                .borrow()
                .contains(&"upload validate=false".to_string()),
            "a plan installs nothing"
        );
        assert!(import(&fake, "p.zip", &zip, true).unwrap().applied);
        assert_eq!(
            fake.installed.borrow()[0],
            ("P".to_string(), "1.2.0".to_string())
        );
        let again = import(&fake, "p.zip", &zip, false).unwrap_err().to_string();
        assert!(again.contains("raise packageVersion"), "{again}");
        let fresh = package_zip(Some(&metadata("Q", "1.0.0")));
        assert_eq!(
            import(&fake, "q.zip", &fresh, false).unwrap().plan,
            "install Q 1.0.0"
        );
    }

    #[test]
    fn an_upgrade_the_server_queues_for_its_restart_is_reported_so() {
        let mut fake = Fake::with(&[("P", "1.0.0")]);
        fake.install_status = 3;
        let zip = package_zip(Some(&metadata("P", "1.0.1")));
        let imported = import(&fake, "p.zip", &zip, true).unwrap();
        assert!(imported.applied && imported.queued);
        assert_eq!(
            fake.installed.borrow()[0],
            ("P".to_string(), "1.0.0".to_string())
        );
        fake.install_status = 1;
        let error = import(&fake, "p.zip", &zip, true).unwrap_err().to_string();
        assert!(error.contains("install refused P"), "{error}");
        assert!(error.contains("className is invalid"), "{error}");
    }

    #[test]
    fn a_metadata_xml_past_a_megabyte_is_refused() {
        let huge = format!("<Entities>{}</Entities>", " ".repeat((1 << 20) + 1));
        let error = inspect(&package_zip(Some(&huge))).unwrap_err().to_string();
        assert!(error.contains("larger than 1 MiB"), "{error}");
    }

    #[test]
    fn a_package_the_server_rejects_is_never_installed() {
        let mut fake = Fake::with(&[]);
        fake.reject = true;
        let error = import(
            &fake,
            "p.zip",
            &package_zip(Some(&metadata("P", "1.0.0"))),
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("bad package"), "{error}");
        assert!(!fake
            .calls
            .borrow()
            .contains(&"upload validate=false".to_string()));
    }

    #[test]
    fn a_package_in_use_is_not_removed() {
        let mut fake = Fake::with(&[("P", "1.0.0")]);
        fake.in_use = vec!["Widget"];
        let error = remove(&fake, "P", true).unwrap_err();
        assert!(error.to_string().contains("in use (Widget)"), "{error}");
        assert!(!fake
            .calls
            .borrow()
            .contains(&"DeleteExtensionPackage".to_string()));

        let fake = Fake::with(&[("P", "1.0.0")]);
        assert_eq!(
            remove(&fake, "P", false).unwrap(),
            "remove P 1.0.0 with 1 extension(s)"
        );
        assert!(!fake
            .calls
            .borrow()
            .contains(&"DeleteExtensionPackage".to_string()));
        remove(&fake, "P", true).unwrap();
        assert!(fake.installed.borrow().is_empty());
        assert!(remove(&fake, "P", false).is_err(), "no longer installed");
    }

    #[test]
    fn an_import_whose_confirmation_cannot_be_read_says_it_was_sent() {
        let mut fake = Fake::with(&[("P", "1.0.0")]);
        fake.list_fails_after_delete = true;
        let zip = package_zip(Some(&metadata("P", "1.2.0")));
        let error = import(&fake, "p.zip", &zip, true).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("the import was sent and may have installed P 1.2.0"),
            "{message}"
        );
        assert!(message.contains("twaco ext list"), "{message}");
    }

    #[test]
    fn a_server_that_normalises_the_version_still_confirms_the_import() {
        let mut fake = Fake::with(&[]);
        fake.shows = Some("1.1.0");
        let zip = package_zip(Some(&metadata("P", "1.1")));
        let done = import(&fake, "p.zip", &zip, true).unwrap();
        assert!(done.applied);

        let mut fake = Fake::with(&[]);
        fake.shows = Some("1.2.0");
        let error = import(&fake, "p.zip", &zip, true).unwrap_err();
        assert!(error.to_string().contains("shows P at 1.2.0"), "{error}");
    }

    #[test]
    fn a_remove_whose_confirmation_cannot_be_read_says_it_was_sent() {
        let mut fake = Fake::with(&[("P", "1.0.0")]);
        fake.list_fails_after_delete = true;
        let error = remove(&fake, "P", true).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("was sent and may have removed P"),
            "{message}"
        );
        assert!(message.contains("twaco ext list"), "{message}");
        // The code is the read's own, not a malformed-reply one.
        assert_eq!(error.code(), ServerError::InvalidUrl(String::new()).code());
    }

    fn metadata_needing(name: &str, version: &str, depends_on: &str) -> String {
        format!(
            r#"<Entities><ExtensionPackages><ExtensionPackage name="{name}" packageVersion="{version}" dependsOn="{depends_on}"/></ExtensionPackages></Entities>"#
        )
    }

    fn bundle_zip(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            for (name, bytes) in members {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        buffer.into_inner()
    }

    /// Acme.Impl needs Acme.Base and Acme.Db; Acme.Db needs Acme.Base. Listed in the wrong order.
    fn acme_bundle() -> Vec<u8> {
        let inner =
            |name: &str, needs: &str| package_zip(Some(&metadata_needing(name, "2.0.0", needs)));
        bundle_zip(&[
            (
                "Acme.Impl.zip",
                inner(
                    "Acme.Impl",
                    "{&quot;extensions&quot;:&quot;Acme.Base:2.0.0,Acme.Db:2.0.0&quot;}",
                ),
            ),
            (
                "Acme.Db.zip",
                inner(
                    "Acme.Db",
                    "{&quot;Acme.Base&quot;:{&quot;version&quot;:&quot;2.0.0&quot;}}",
                ),
            ),
            ("Acme.Base.zip", inner("Acme.Base", "")),
        ])
    }

    fn run_bundle(
        fake: &Fake,
        zip: &[u8],
        apply: bool,
    ) -> Result<Vec<BundleEntry>, ExtensionError> {
        let members = bundle_members(zip)?.expect("a bundle");
        import_bundle_with_progress(fake, members, apply, &progress::NONE)
    }

    #[test]
    fn an_install_hitting_the_transient_css_error_is_sent_once_more_and_says_so() {
        let fake = Fake::with(&[]);
        *fake.transient.borrow_mut() = 1;
        let zip = package_zip(Some(&metadata("P", "1.0.0")));
        let imported = import(&fake, "p.zip", &zip, true).unwrap();
        assert!(imported.applied && imported.retried);

        let fake = Fake::with(&[]);
        *fake.transient.borrow_mut() = 2;
        let recorder = progress::Recorder::default();
        let error = import_with_progress(&fake, "p.zip", &zip, true, &recorder)
            .unwrap_err()
            .to_string();
        assert!(recorder.events().iter().any(|event| matches!(event, progress::Event::Warning(text) if text.contains("retrying once in 3 seconds"))));
        assert!(error.contains("registering CSS resources"), "{error}");
        let uploads = fake
            .calls
            .borrow()
            .iter()
            .filter(|c| *c == "upload validate=false")
            .count();
        assert_eq!(uploads, 2, "exactly one retry");

        let fake = Fake::with(&[]);
        assert!(!import(&fake, "p.zip", &zip, true).unwrap().retried);
    }

    #[test]
    fn a_zip_of_extension_zips_is_a_bundle_and_anything_else_is_not() {
        assert_eq!(bundle_members(&acme_bundle()).unwrap().unwrap().len(), 3);
        assert!(bundle_members(&package_zip(Some(&metadata("P", "1.0.0"))))
            .unwrap()
            .is_none());
        let mixed = bundle_zip(&[
            ("a.zip", package_zip(Some(&metadata("A", "1.0.0")))),
            ("notes.txt", b"x".to_vec()),
        ]);
        assert!(bundle_members(&mixed).unwrap().is_none());
        assert!(bundle_members(b"not a zip").unwrap().is_none());
        let bad = bundle_zip(&[("a.zip", package_zip(None))]);
        let error = bundle_members(&bad).unwrap_err().to_string();
        assert!(error.contains("a.zip in the bundle"), "{error}");
    }

    #[test]
    fn a_bundle_installs_in_dependency_order_and_skips_what_is_installed() {
        let fake = Fake::with(&[("Acme.Db", "2.1.0")]);
        let entries = run_bundle(&fake, &acme_bundle(), true).unwrap();
        let order: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(order, ["Acme.Base", "Acme.Db", "Acme.Impl"]);
        assert!(matches!(entries[0].status, BundleStatus::Imported(_)));
        assert_eq!(
            entries[1].status,
            BundleStatus::Skipped {
                installed: "2.1.0".into()
            }
        );
        assert!(entries[1].line().contains("skipped (installed 2.1.0)"));
        assert!(matches!(entries[2].status, BundleStatus::Imported(_)));
        let installed: Vec<String> = fake
            .installed
            .borrow()
            .iter()
            .map(|p| p.0.clone())
            .collect();
        assert!(installed.contains(&"Acme.Base".to_string()));
        assert!(installed.contains(&"Acme.Impl".to_string()));
    }

    #[test]
    fn queued_upgrades_defer_direct_and_transitive_dependents_until_restart() {
        let mut fake = Fake::with(&[("Acme.Base", "1.0.0")]);
        fake.install_status = 3;
        let entries = run_bundle(&fake, &acme_bundle(), true).unwrap();
        assert!(matches!(entries[0].status, BundleStatus::Imported(ref done) if done.queued));
        assert_eq!(
            entries[1].status,
            BundleStatus::WaitsForRestart {
                after: vec!["Acme.Base".into()]
            }
        );
        assert_eq!(
            entries[2].status,
            BundleStatus::WaitsForRestart {
                after: vec!["Acme.Base".into(), "Acme.Db".into()]
            }
        );
        assert!(entries[1]
            .line()
            .contains("waits for restart (needs Acme.Base)"));
        assert_eq!(
            fake.calls
                .borrow()
                .iter()
                .filter(|call| *call == "upload validate=false")
                .count(),
            1
        );
        assert_eq!(fake.installed.borrow()[0].1, "1.0.0");
    }

    #[test]
    fn unsupported_and_overflowing_versions_name_the_package_before_uploading() {
        for version in [
            "2.invalid",
            "2.0-rc1",
            "2.18446744073709551616",
            "2..0",
            "+2.0",
        ] {
            let fake = Fake::with(&[]);
            let zip = package_zip(Some(&metadata("Acme.Base", version)));
            let error = import(&fake, "Acme.Base.zip", &zip, true)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("Acme.Base") && error.contains(version),
                "{error}"
            );
            assert!(fake.calls.borrow().is_empty());
            let fake = Fake::with(&[("Acme.Base", version)]);
            let error = run_bundle(&fake, &acme_bundle(), true)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("Acme.Base") && error.contains(version),
                "{error}"
            );
            assert!(fake
                .calls
                .borrow()
                .iter()
                .all(|call| call == "GetExtensionPackageList"));
        }
    }

    #[test]
    fn inner_zip_filenames_refuse_windows_separators_and_drive_syntax() {
        for name in [
            r"folder\Acme.Base.zip",
            "C:Acme.Base.zip",
            r"C:\Acme.Base.zip",
            "Acme:Base.zip",
        ] {
            let zip = bundle_zip(&[(name, package_zip(Some(&metadata("Acme.Base", "1.0.0"))))]);
            let error = bundle_members(&zip).unwrap_err().to_string();
            assert!(error.contains("root zip filename"), "{error}");
        }
    }

    #[test]
    fn a_bundle_plan_installs_nothing_and_marks_what_waits_for_the_bundle() {
        let fake = Fake::with(&[]);
        let entries = run_bundle(&fake, &acme_bundle(), false).unwrap();
        assert!(matches!(entries[0].status, BundleStatus::Imported(ref i) if !i.applied));
        assert!(matches!(entries[1].status, BundleStatus::Pending { .. }));
        assert!(matches!(entries[2].status, BundleStatus::Pending { .. }));
        assert!(fake.installed.borrow().is_empty());
        assert!(!fake
            .calls
            .borrow()
            .contains(&"upload validate=false".to_string()));
    }

    #[test]
    fn a_bundle_plan_defers_dependencies_on_upgrades_without_stopping_the_rest() {
        for installed in [vec![], vec![("Acme.Base", "1.0.0"), ("Acme.Db", "1.0.0")]] {
            let fake = Fake::with(&installed);
            let zip = acme_bundle();
            let mut members = bundle_members(&zip).unwrap().unwrap();
            let refused = import_with_progress(
                &fake,
                &members[1].file,
                &members[1].zip,
                false,
                &progress::NONE,
            )
            .unwrap_err()
            .to_string();
            assert!(
                refused.contains("Could not find extension Acme.Base"),
                "{refused}"
            );
            fake.calls.borrow_mut().clear();

            let other = package_zip(Some(&metadata("Acme.Other", "2.0.0")));
            members.push(Member {
                file: "Acme.Other.zip".into(),
                package: inspect(&other).unwrap(),
                zip: other,
            });
            let entries =
                import_bundle_with_progress(&fake, members, false, &progress::NONE).unwrap();
            assert!(matches!(entries[0].status, BundleStatus::Imported(ref done) if !done.applied));
            assert_eq!(entries[1].line(), "Acme.Db 2.0.0: would import after Acme.Base 2.0.0 (server validation waits for it)");
            assert_eq!(
                entries[2].status,
                BundleStatus::Pending {
                    after: vec!["Acme.Base 2.0.0".into(), "Acme.Db 2.0.0".into()],
                }
            );
            assert!(matches!(entries[3].status, BundleStatus::Imported(ref done) if !done.applied));
            assert!(entries.iter().all(|entry| !entry.failed()));
            assert_eq!(
                fake.calls
                    .borrow()
                    .iter()
                    .filter(|call| call.starts_with("upload"))
                    .collect::<Vec<_>>(),
                ["upload validate=true", "upload validate=true"]
            );
            assert_eq!(
                *fake.installed.borrow(),
                installed
                    .iter()
                    .map(|(n, v)| (n.to_string(), v.to_string()))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn a_bundle_plan_validates_dependencies_already_satisfied_on_the_server() {
        for version in ["2.0.0", "2.1.0"] {
            let fake = Fake::with(&[("Acme.Base", version), ("Acme.Db", version)]);
            let entries = run_bundle(&fake, &acme_bundle(), false).unwrap();
            assert!(matches!(entries[0].status, BundleStatus::Skipped { .. }));
            assert!(matches!(entries[1].status, BundleStatus::Skipped { .. }));
            assert!(matches!(entries[2].status, BundleStatus::Imported(ref done) if !done.applied));
            assert!(entries[2].line().ends_with(", validated"));
            assert_eq!(
                fake.calls
                    .borrow()
                    .iter()
                    .filter(|call| call.starts_with("upload"))
                    .collect::<Vec<_>>(),
                ["upload validate=true"]
            );
        }

        let fake = Fake::with(&[("Acme.Base", "1.0.0")]);
        let zip = bundle_zip(&[
            (
                "Acme.Base.zip",
                package_zip(Some(&metadata("Acme.Base", "2.0.0"))),
            ),
            (
                "Acme.Other.zip",
                package_zip(Some(&metadata_needing(
                    "Acme.Other",
                    "2.0.0",
                    "Acme.Base:1.0.0",
                ))),
            ),
        ]);
        let entries = run_bundle(&fake, &zip, false).unwrap();
        assert!(entries.iter().all(
            |entry| matches!(entry.status, BundleStatus::Imported(ref done) if !done.applied)
        ));
        assert_eq!(
            fake.calls
                .borrow()
                .iter()
                .filter(|call| call.starts_with("upload"))
                .count(),
            2
        );
    }

    #[test]
    fn a_bundle_refuses_a_missing_dependency_and_a_cycle_before_sending_anything() {
        let one = |name: &str, needs: &str| {
            (
                format!("{name}.zip"),
                package_zip(Some(&metadata_needing(name, "1.0.0", needs))),
            )
        };
        let zip_of = |items: Vec<(String, Vec<u8>)>| {
            let borrowed: Vec<(&str, Vec<u8>)> =
                items.iter().map(|(n, b)| (n.as_str(), b.clone())).collect();
            bundle_zip(&borrowed)
        };
        let fake = Fake::with(&[]);
        let missing = zip_of(vec![one("Acme.A", "Acme.Ghost")]);
        let error = run_bundle(&fake, &missing, true).unwrap_err().to_string();
        assert!(error.contains("Acme.Ghost"), "{error}");
        let cycle = zip_of(vec![one("Acme.A", "Acme.B"), one("Acme.B", "Acme.A")]);
        let error = run_bundle(&fake, &cycle, true).unwrap_err().to_string();
        assert!(
            error.contains("cycle") && error.contains("Acme.A"),
            "{error}"
        );
        assert!(fake
            .calls
            .borrow()
            .iter()
            .all(|call| call == "GetExtensionPackageList"));
    }

    #[test]
    fn a_failed_package_stops_a_bundle_and_names_what_was_not_sent() {
        let mut fake = Fake::with(&[]);
        fake.refuse = Some("Acme.Db");
        let entries = run_bundle(&fake, &acme_bundle(), true).unwrap();
        assert!(matches!(entries[0].status, BundleStatus::Imported(_)));
        assert!(entries[1].failed());
        assert!(entries[1].line().contains("className is invalid"));
        assert_eq!(entries[2].status, BundleStatus::NotAttempted);
    }

    #[test]
    fn dependencies_are_read_from_each_dependson_shape() {
        assert_eq!(dependencies(""), Vec::<String>::new());
        assert_eq!(dependencies("A:1.0, B"), ["A", "B"]);
        assert_eq!(dependencies(r#"{"A":{"version":"1"},"B":"2"}"#), ["A", "B"]);
        assert_eq!(
            dependencies(r#"{"extensions":"A:1,B:2","projects":"P:0"}"#),
            ["A", "B"]
        );
        assert!(version_order("Acme.Base", "10.2.0", "10.10")
            .unwrap()
            .is_lt());
        assert!(version_order("Acme.Base", "2.0", "2.0.0").unwrap().is_eq());
    }
}
