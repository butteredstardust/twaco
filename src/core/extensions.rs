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
    };
    if package.name.is_empty() || package.version.is_empty() {
        return Err(bad(
            "its ExtensionPackage has no name or no packageVersion".to_string()
        ));
    }
    Ok(package)
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
    let plan = match &installed {
        None => format!("install {} {}", package.name, package.version),
        // The uploader validates it, then skips the install.
        Some(old) if old.version == package.version => {
            return Err(ExtensionError::Invalid(format!(
                "{} {} is installed already, and the server skips a version it has: raise \
                 packageVersion to replace it",
                package.name, package.version
            )))
        }
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
        });
    }
    let reply = {
        let _phase = progress::phase(progress, "importing the package", Some(1));
        let reply = remote
            .upload(file_name, zip, false)
            .map_err(ExtensionError::Remote)?;
        progress.advance(1);
        reply
    };
    report(&reply, "validate").map_err(|why| rejected(why, "import"))?;
    // Without an install report, the package list below is the evidence.
    let install_status = match report_row(&reply, "install") {
        Some(_) => report(&reply, "install").map_err(|why| rejected(why, "install"))?,
        None => 0,
    };
    if install_status == 3 {
        return Ok(Imported {
            package,
            plan,
            applied: true,
            queued: true,
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
    if now.as_ref().map(|p| p.version.as_str()) != Some(package.version.as_str()) {
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
    })
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
            if !validate && !self.reject && self.install_status == 0 {
                let package = inspect(zip).unwrap();
                let mut installed = self.installed.borrow_mut();
                installed.retain(|(n, _)| *n != package.name);
                installed.push((package.name, package.version));
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
}
