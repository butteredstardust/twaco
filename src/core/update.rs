//! Updates from the GitHub releases: a once-a-day notice that a newer twaco exists, and
//! `twaco update`, which replaces this binary with the newer release.
//!
//! WARNING: `install` replaces an executable, on Windows always the running one. `owner` names
//! the installs it must not touch: a file that dpkg or an AppImage owns is updated through
//! them, or the package database no longer matches the disk.
//!
//! Trust comes from minisign signatures, not from the connection. Each signature's trusted
//! comment names the file it is for:
//! - `updater.json` names an archive per platform. Its signature is `updater.json.minisig`.
//! - Each archive's signature is in the manifest. Its comment names the archive, and so its
//!   version and platform, so a signed archive of another release does not pass as this one.
//!
//! A replayed manifest of an older release still has a valid signature. The check record keeps
//! the highest version that a signed manifest showed, so an older manifest cannot hide a newer
//! release that twaco saw before: the notice still names it, and `twaco update` refuses the
//! older manifest.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::help::Fetch;

/// The manifest of the latest release. GitHub redirects it to the newest release's asset.
pub const MANIFEST_URL: &str =
    "https://github.com/butteredstardust/twaco/releases/latest/download/updater.json";
/// Where a person downloads an installer by hand.
pub const RELEASES_URL: &str = "https://github.com/butteredstardust/twaco/releases/latest";
/// The minisign key the release workflow signs with. The `TWACO_SIGNING_KEY` repository secret
/// holds its secret half.
pub const PUBLIC_KEY: &str = "RWRqPjLVLgJ9nu7C3x2lGYTU1k31WEjJpTkAiuMBL8UFlEFmM5NG+cRf";
/// The platform this binary was built for, as the manifest names it.
pub const TARGET: &str = env!("TWACO_TARGET");
/// How long one check stands before the notice asks again.
pub const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;
/// Set it to anything but `0` to turn the notice off.
pub const OPT_OUT: &str = "TWACO_NO_UPDATE_CHECK";
/// The most a release archive may weigh. A larger download is not a twaco release.
const MOST_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub platforms: BTreeMap<String, Payload>,
}

#[derive(Debug, Deserialize)]
pub struct Payload {
    pub url: String,
    /// The whole `.minisig` file: comments, signature and global signature.
    pub signature: String,
}

/// The release web, over HTTPS only. Redirects are followed: GitHub serves assets from
/// another host, and the signature, not the host, is what is trusted.
pub struct Web {
    agent: ureq::Agent,
}

impl Web {
    pub fn new(timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .https_only(true)
            .build();
        Web {
            agent: config.into(),
        }
    }
}

impl Fetch for Web {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        self.agent
            .get(url)
            .call()
            .map_err(|e| e.to_string())?
            .body_mut()
            .with_config()
            .limit(MOST_BYTES)
            .read_to_vec()
            .map_err(|e| e.to_string())
    }
}

/// `X.Y.Z`, as the release workflow writes it. Anything else is not a release version.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(version)
}

/// Whether `latest` is a newer release than `current`. An unreadable version is never newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// The signed manifest at `url`. Its signature is at `<url>.minisig`.
pub fn manifest(fetch: &dyn Fetch, url: &str, public_key: &str) -> Result<Manifest, String> {
    let bytes = fetch
        .get(url)
        .map_err(|e| format!("cannot read the release manifest {url}: {e}"))?;
    let signature = fetch
        .get(&format!("{url}.minisig"))
        .map_err(|e| format!("cannot read the signature of the release manifest {url}: {e}"))?;
    let signature = String::from_utf8(signature)
        .map_err(|_| format!("the signature of the release manifest {url} is not text"))?;
    verify(&bytes, &signature, "updater.json", public_key)?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| format!("the release manifest {url} is not valid: {e}"))?;
    if parse_version(&manifest.version).is_none() {
        return Err(format!(
            "the release manifest {url} names version {:?}, which is not X.Y.Z",
            manifest.version
        ));
    }
    Ok(manifest)
}

/// The release's archive for a platform: `twaco-<version>-<target>.zip` on Windows and
/// `.tar.gz` elsewhere.
pub fn archive_name(version: &str, target: &str) -> String {
    let extension = if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("twaco-{version}-{target}.{extension}")
}

fn binary_name(target: &str) -> &'static str {
    if target.contains("windows") {
        "twaco.exe"
    } else {
        "twaco"
    }
}

/// Download this platform's archive, verify it, and return the twaco binary inside it.
pub fn download(
    fetch: &dyn Fetch,
    manifest: &Manifest,
    target: &str,
    public_key: &str,
) -> Result<Vec<u8>, String> {
    let payload = manifest.platforms.get(target).ok_or_else(|| {
        format!(
            "release {} has no build for {target}; build from source or see {RELEASES_URL}",
            manifest.version
        )
    })?;
    let expected = archive_name(&manifest.version, target);
    if payload.url.rsplit('/').next() != Some(expected.as_str()) {
        return Err(format!(
            "the manifest offers {} for {target}, not {expected}; nothing was installed",
            payload.url
        ));
    }
    let archive = fetch
        .get(&payload.url)
        .map_err(|e| format!("cannot download {}: {e}", payload.url))?;
    verify(&archive, &payload.signature, &expected, public_key)
        .map_err(|e| format!("{e}; nothing was installed"))?;
    let stem = expected
        .strip_suffix(".zip")
        .or_else(|| expected.strip_suffix(".tar.gz"))
        .unwrap_or(&expected);
    let inner = format!("{stem}/{}", binary_name(target));
    if expected.ends_with(".zip") {
        from_zip(&archive, &inner)
    } else {
        from_tar_gz(&archive, &inner)
    }
    .map_err(|e| format!("{expected}: {e}"))
}

fn verify(archive: &[u8], signature: &str, name: &str, public_key: &str) -> Result<(), String> {
    let key = minisign_verify::PublicKey::from_base64(public_key)
        .map_err(|e| format!("the built-in public key does not read: {e}"))?;
    let signature = minisign_verify::Signature::decode(signature)
        .map_err(|e| format!("the signature of {name} does not read: {e}"))?;
    key.verify(archive, &signature, false)
        .map_err(|e| format!("{name} does not match its signature ({e})"))?;
    // Signed by the right key, but for which file: the comment is covered by the signature.
    if signature.trusted_comment() != format!("file:{name}") {
        return Err(format!(
            "the signature is for {:?}, not {name}",
            signature.trusted_comment()
        ));
    }
    Ok(())
}

fn from_zip(archive: &[u8], inner: &str) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).map_err(|e| e.to_string())?;
    let mut file = zip
        .by_name(inner)
        .map_err(|_| format!("the archive holds no {inner}"))?;
    let mut binary = Vec::new();
    file.read_to_end(&mut binary).map_err(|e| e.to_string())?;
    Ok(binary)
}

fn from_tar_gz(archive: &[u8], inner: &str) -> Result<Vec<u8>, String> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        if entry.path().map_err(|e| e.to_string())? == Path::new(inner) {
            let mut binary = Vec::new();
            entry.read_to_end(&mut binary).map_err(|e| e.to_string())?;
            return Ok(binary);
        }
    }
    Err(format!("the archive holds no {inner}"))
}

/// Why `install` must leave `exe` alone, when something else owns it.
pub fn owner(exe: &Path, appimage: Option<&str>) -> Option<String> {
    if appimage.is_some_and(|path| !path.is_empty()) {
        return Some(format!(
            "this twaco runs from an AppImage, which is replaced as a whole; download the new AppImage from {RELEASES_URL}"
        ));
    }
    if exe.starts_with("/usr/bin") {
        return Some(format!(
            "{} belongs to the .deb package; install the new .deb from {RELEASES_URL} so dpkg keeps track of it",
            exe.display()
        ));
    }
    None
}

/// Replace `exe` with `binary`. The new file is written beside it first, so a failure leaves
/// the old binary in place. On Windows `exe` must be the running executable.
///
/// Leftover files are named `.twaco-update-<pid>` and `.twaco-backup-<pid>.exe`. The Windows
/// uninstaller in packaging/windows/twaco.nsi removes these names; keep the two in step.
pub fn install(binary: &[u8], exe: &Path) -> Result<(), String> {
    let dir = exe
        .parent()
        .ok_or_else(|| format!("{} has no folder", exe.display()))?;
    let staged = dir.join(format!(".twaco-update-{}", std::process::id()));
    let denied = |error: std::io::Error| {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            format!(
                "cannot write in {}: permission denied. Run `sudo twaco update --apply`, or install the new release from {RELEASES_URL}",
                dir.display()
            )
        } else {
            format!("cannot write in {}: {error}", dir.display())
        }
    };
    // create_new refuses a file or link already at that name, so the write cannot follow a
    // link that someone else put there.
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(binary)?;
            file.sync_all()
        });
    if let Err(error) = written {
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            let _ = std::fs::remove_file(&staged);
        }
        return Err(denied(error));
    }
    let replaced = replace(&staged, exe);
    let _ = std::fs::remove_file(&staged);
    replaced.map_err(denied)
}

#[cfg(unix)]
fn replace(staged: &Path, exe: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))?;
    // A rename is atomic, and a running process keeps the file it opened.
    std::fs::rename(staged, exe)
}

/// WARNING: self_replace moves the running executable aside before it moves the new one in.
/// When a later step fails, `exe` is missing, so a copy of the old binary is kept to put back.
#[cfg(windows)]
fn replace(staged: &Path, exe: &Path) -> std::io::Result<()> {
    with_backup(exe, || self_replace::self_replace(staged))
}

/// Run `swap`, which replaces `exe`, with a copy of `exe` kept aside. When `swap` fails and
/// `exe` is missing, put the copy back. When that fails too, keep the copy and name it in the
/// error, so the person can put it back by hand.
#[cfg_attr(not(windows), allow(dead_code))]
fn with_backup(exe: &Path, swap: impl FnOnce() -> std::io::Result<()>) -> std::io::Result<()> {
    let backup = exe.with_file_name(format!(".twaco-backup-{}.exe", std::process::id()));
    std::fs::copy(exe, &backup)?;
    let swapped = swap();
    if swapped.is_err() && !exe.exists() {
        if let Err(error) = std::fs::rename(&backup, exe) {
            return Err(std::io::Error::new(
                error.kind(),
                format!(
                    "the update failed and {} is missing. The old twaco is at {}: rename it to {} ({error})",
                    exe.display(),
                    backup.display(),
                    exe.display()
                ),
            ));
        }
        return swapped;
    }
    let _ = std::fs::remove_file(&backup);
    swapped
}

/// The last check, kept in the user's cache folder so the network is asked once a day.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CheckRecord {
    /// Unix seconds.
    pub checked: i64,
    /// The latest release then, or none when the check failed.
    pub latest: Option<String>,
    /// The highest version that any signed manifest showed.
    #[serde(default)]
    pub highest: Option<String>,
}

fn read_record(cache: &Path) -> CheckRecord {
    std::fs::read(cache)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Best effort: a record that is not written only means the network is asked again.
fn write_record(cache: &Path, record: &CheckRecord) {
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(record) {
        let _ = std::fs::write(cache, json);
    }
}

/// The higher of two versions. Either can be missing.
fn higher(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if is_newer(&b, &a) { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// Accept the version of a verified manifest for `twaco update`, and record it.
///
/// Refuse it when a signed manifest showed a newer version before: then this manifest is an old
/// copy, replayed or cached on the way.
pub fn accept(cache: &Path, version: &str) -> Result<(), String> {
    let mut record = read_record(cache);
    if let Some(highest) = record.highest.as_deref() {
        if is_newer(highest, version) {
            return Err(format!(
                "the release manifest names twaco {version}, but twaco {highest} was released before it. The manifest is an old copy; try again later, or download {highest} from {RELEASES_URL}"
            ));
        }
    }
    record.highest = higher(record.highest, Some(version.to_string()));
    write_record(cache, &record);
    Ok(())
}

pub fn cache_file() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("twaco").join("update-check.json"))
}

/// Whether a command may print the notice. It never runs where its output reaches a program:
/// the MCP server's stdio, CI, or a redirected stderr.
pub fn notice_wanted(
    command: &str,
    var: &dyn Fn(&str) -> Option<String>,
    stderr_is_terminal: bool,
) -> bool {
    let set = |name: &str| var(name).is_some_and(|value| !value.is_empty() && value != "0");
    stderr_is_terminal
        && !matches!(command, "mcp" | "update" | "--version" | "-V")
        && !set(OPT_OUT)
        && !set("CI")
}

/// The notice line, when a newer release exists. Only a manifest that `public_key` signed
/// counts. Failures are silent: no network means no
/// notice, and the failed check still counts, so an offline machine waits a day to try again.
pub fn notice(
    fetch: &dyn Fetch,
    url: &str,
    public_key: &str,
    cache: &Path,
    now: i64,
    current: &str,
) -> Option<String> {
    let record = read_record(cache);
    let fresh = (0..CHECK_INTERVAL_SECS).contains(&(now - record.checked));
    let record = if fresh {
        record
    } else {
        let latest = manifest(fetch, url, public_key).ok().map(|m| m.version);
        let record = CheckRecord {
            checked: now,
            highest: higher(record.highest, latest.clone()),
            latest,
        };
        write_record(cache, &record);
        record
    };
    // An older manifest does not hide a newer release that twaco saw before.
    let latest =
        higher(record.latest, record.highest).filter(|latest| is_newer(latest, current))?;
    Some(format!(
        "twaco {latest} is available (this is {current}); `twaco update --apply` installs it. {OPT_OUT}=1 turns this notice off"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::HashMap;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/update");
    const LINUX: &str = "x86_64-unknown-linux-gnu";
    const WINDOWS: &str = "x86_64-pc-windows-msvc";
    const BASE: &str = "https://example.test/releases/download/v9.9.9";

    struct Fake {
        files: HashMap<String, Vec<u8>>,
        calls: Cell<usize>,
    }

    impl Fetch for Fake {
        fn get(&self, url: &str) -> Result<Vec<u8>, String> {
            self.calls.set(self.calls.get() + 1);
            self.files
                .get(url)
                .cloned()
                .ok_or_else(|| format!("404 {url}"))
        }
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{FIXTURES}/{name}")).unwrap()
    }

    fn test_key() -> String {
        String::from_utf8(fixture("test-key.pub"))
            .unwrap()
            .trim()
            .to_string()
    }

    fn payload(target: &str) -> Payload {
        let name = archive_name("9.9.9", target);
        Payload {
            url: format!("{BASE}/{name}"),
            signature: String::from_utf8(fixture(&format!("{name}.minisig"))).unwrap(),
        }
    }

    fn release(targets: &[&str]) -> (Manifest, Fake) {
        let mut files = HashMap::new();
        let mut platforms = BTreeMap::new();
        for target in targets {
            let name = archive_name("9.9.9", target);
            files.insert(format!("{BASE}/{name}"), fixture(&name));
            platforms.insert(target.to_string(), payload(target));
        }
        let manifest = Manifest {
            version: "9.9.9".to_string(),
            notes: String::new(),
            platforms,
        };
        let fake = Fake {
            files,
            calls: Cell::new(0),
        };
        (manifest, fake)
    }

    /// A fake server for the signed manifest of `version` (0.1.0, 0.2.0 or latest) at "m".
    fn signed_manifest(version: &str) -> Fake {
        let name = format!("updater-{version}.json");
        Fake {
            files: HashMap::from([
                ("m".to_string(), fixture(&name)),
                ("m.minisig".to_string(), fixture(&format!("{name}.minisig"))),
            ]),
            calls: Cell::new(0),
        }
    }

    #[test]
    fn versions_compare_by_number_and_refuse_anything_else() {
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.1", "0.1.1"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("0.2", "0.1.1"));
        assert!(!is_newer("0.2.0-rc1", "0.1.1"));
        assert!(!is_newer("0.2.0.1", "0.1.1"));
    }

    #[test]
    fn a_signed_manifest_reads() {
        let manifest = manifest(&signed_manifest("0.2.0"), "m", &test_key()).unwrap();
        assert_eq!(manifest.version, "0.2.0");
    }

    #[test]
    fn a_manifest_with_a_bad_version_is_refused() {
        let error = manifest(&signed_manifest("latest"), "m", &test_key()).unwrap_err();
        assert!(error.contains("not X.Y.Z"), "{error}");
    }

    #[test]
    fn an_unsigned_or_changed_manifest_is_refused() {
        let mut fake = signed_manifest("0.2.0");
        fake.files.remove("m.minisig");
        let error = manifest(&fake, "m", &test_key()).unwrap_err();
        assert!(error.contains("cannot read the signature"), "{error}");

        let mut fake = signed_manifest("0.2.0");
        fake.files
            .insert("m".to_string(), fixture("updater-0.1.0.json"));
        let error = manifest(&fake, "m", &test_key()).unwrap_err();
        assert!(error.contains("does not match its signature"), "{error}");

        let error = manifest(&signed_manifest("0.2.0"), "m", PUBLIC_KEY).unwrap_err();
        assert!(error.contains("updater.json"), "{error}");
    }

    #[test]
    fn a_signed_tar_gz_gives_its_binary() {
        let (manifest, fake) = release(&[LINUX]);
        let binary = download(&fake, &manifest, LINUX, &test_key()).unwrap();
        assert_eq!(binary, b"new twaco binary\n");
    }

    #[test]
    fn a_signed_zip_gives_its_binary() {
        let (manifest, fake) = release(&[WINDOWS]);
        let binary = download(&fake, &manifest, WINDOWS, &test_key()).unwrap();
        assert_eq!(binary, b"new twaco.exe\n");
    }

    #[test]
    fn a_changed_archive_is_refused() {
        let (manifest, mut fake) = release(&[LINUX]);
        let url = &manifest.platforms[LINUX].url;
        let archive = fake.files.get_mut(url).unwrap();
        let last = archive.len() - 1;
        archive[last] ^= 1;
        let error = download(&fake, &manifest, LINUX, &test_key()).unwrap_err();
        assert!(error.contains("does not match its signature"), "{error}");
    }

    #[test]
    fn another_key_is_refused() {
        let (manifest, fake) = release(&[LINUX]);
        let error = download(&fake, &manifest, LINUX, PUBLIC_KEY).unwrap_err();
        assert!(error.contains("nothing was installed"), "{error}");
    }

    #[test]
    fn a_signed_archive_offered_as_another_release_is_refused() {
        // A downgrade: the signed 9.9.9 archive, served as if it were release 9.9.10.
        let (mut manifest, mut fake) = release(&[LINUX]);
        let url = format!("{BASE}/{}", archive_name("9.9.10", LINUX));
        let archive = fake.files.values().next().unwrap().clone();
        fake.files.insert(url.clone(), archive);
        manifest.version = "9.9.10".to_string();
        manifest.platforms.get_mut(LINUX).unwrap().url = url;
        let error = download(&fake, &manifest, LINUX, &test_key()).unwrap_err();
        assert!(error.contains("the signature is for"), "{error}");
    }

    #[test]
    fn a_url_that_is_not_the_release_archive_is_refused_before_download() {
        let (mut manifest, fake) = release(&[LINUX]);
        manifest.platforms.get_mut(LINUX).unwrap().url = format!("{BASE}/other.tar.gz");
        let error = download(&fake, &manifest, LINUX, &test_key()).unwrap_err();
        assert!(error.contains("not twaco-9.9.9"), "{error}");
        assert_eq!(fake.calls.get(), 0);
    }

    #[test]
    fn a_platform_without_a_build_says_so() {
        let (manifest, fake) = release(&[LINUX]);
        let error = download(&fake, &manifest, WINDOWS, &test_key()).unwrap_err();
        assert!(error.contains("has no build for"), "{error}");
    }

    #[test]
    fn package_managed_installs_are_left_alone() {
        assert!(owner(Path::new("/usr/bin/twaco"), None)
            .unwrap()
            .contains(".deb"));
        assert!(
            owner(Path::new("/tmp/x/twaco"), Some("/home/a/twaco.AppImage"))
                .unwrap()
                .contains("AppImage")
        );
        assert!(owner(Path::new("/usr/local/bin/twaco"), None).is_none());
        assert!(owner(Path::new("/usr/local/bin/twaco"), Some("")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn install_replaces_the_file_and_leaves_nothing_beside_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("twaco-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("twaco");
        std::fs::write(&exe, b"old").unwrap();
        install(b"new", &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn install_does_not_write_through_a_link_at_the_staging_name() {
        let dir = std::env::temp_dir().join(format!("twaco-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("twaco");
        let victim = dir.join("victim");
        std::fs::write(&exe, b"old").unwrap();
        std::fs::write(&victim, b"keep").unwrap();
        let staged = dir.join(format!(".twaco-update-{}", std::process::id()));
        std::os::unix::fs::symlink(&victim, &staged).unwrap();
        let error = install(b"new", &exe).unwrap_err();
        assert!(error.contains("cannot write"), "{error}");
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_notice_runs_only_for_a_person_at_a_terminal() {
        let none = |_: &str| None;
        assert!(notice_wanted("check", &none, true));
        assert!(!notice_wanted("check", &none, false));
        assert!(!notice_wanted("mcp", &none, true));
        assert!(!notice_wanted("update", &none, true));
        let ci = |name: &str| (name == "CI").then(|| "true".to_string());
        assert!(!notice_wanted("check", &ci, true));
        let off = |name: &str| (name == OPT_OUT).then(|| "1".to_string());
        assert!(!notice_wanted("check", &off, true));
        let zero = |name: &str| (name == OPT_OUT).then(|| "0".to_string());
        assert!(notice_wanted("check", &zero, true));
    }

    fn cache_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("twaco-notice-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("nested").join("update-check.json")
    }

    #[test]
    fn a_stale_check_asks_once_and_a_fresh_one_does_not_ask() {
        let cache = cache_path("stale");
        let fake = signed_manifest("0.2.0");
        let line = notice(&fake, "m", &test_key(), &cache, 1_000_000, "0.1.0").unwrap();
        assert!(
            line.contains("twaco 0.2.0 is available (this is 0.1.0)"),
            "{line}"
        );
        // The manifest and its signature.
        assert_eq!(fake.calls.get(), 2);
        // Within the day: the record answers, the network is not asked.
        let again = notice(&fake, "m", &test_key(), &cache, 1_000_000 + 3600, "0.1.0");
        assert!(again.is_some());
        assert_eq!(fake.calls.get(), 2);
        // A day later it asks again.
        notice(
            &fake,
            "m",
            &test_key(),
            &cache,
            1_000_000 + CHECK_INTERVAL_SECS,
            "0.1.0",
        );
        assert_eq!(fake.calls.get(), 4);
    }

    #[test]
    fn no_notice_when_current_or_offline_and_offline_still_waits_a_day() {
        let cache = cache_path("offline");
        let current = signed_manifest("0.1.0");
        assert!(notice(&current, "m", &test_key(), &cache, 5_000_000, "0.1.0").is_none());

        let cache = cache_path("offline2");
        let offline = Fake {
            files: HashMap::new(),
            calls: Cell::new(0),
        };
        assert!(notice(&offline, "m", &test_key(), &cache, 5_000_000, "0.1.0").is_none());
        assert!(notice(&offline, "m", &test_key(), &cache, 5_000_100, "0.1.0").is_none());
        assert_eq!(offline.calls.get(), 1);
    }

    #[test]
    fn a_clock_moved_back_asks_again() {
        let cache = cache_path("clock");
        let fake = signed_manifest("0.2.0");
        notice(&fake, "m", &test_key(), &cache, 9_000_000, "0.1.0");
        notice(&fake, "m", &test_key(), &cache, 8_000_000, "0.1.0");
        assert_eq!(fake.calls.get(), 4);
    }

    #[test]
    fn an_older_manifest_does_not_hide_a_newer_release_seen_before() {
        let cache = cache_path("replay");
        notice(
            &signed_manifest("0.2.0"),
            "m",
            &test_key(),
            &cache,
            1_000_000,
            "0.1.0",
        );
        // A day later an old, validly signed manifest arrives.
        let line = notice(
            &signed_manifest("0.1.0"),
            "m",
            &test_key(),
            &cache,
            1_000_000 + CHECK_INTERVAL_SECS,
            "0.1.0",
        )
        .unwrap();
        assert!(line.contains("twaco 0.2.0 is available"), "{line}");
        // Offline: the release seen before still counts.
        let offline = Fake {
            files: HashMap::new(),
            calls: Cell::new(0),
        };
        let line = notice(
            &offline,
            "m",
            &test_key(),
            &cache,
            1_000_000 + 2 * CHECK_INTERVAL_SECS,
            "0.1.0",
        );
        assert!(line.is_some());
    }

    #[test]
    fn update_refuses_a_manifest_older_than_one_seen_before() {
        let cache = cache_path("accept");
        accept(&cache, "0.2.0").unwrap();
        accept(&cache, "0.2.0").unwrap();
        let error = accept(&cache, "0.1.9").unwrap_err();
        assert!(
            error.contains("twaco 0.2.0 was released before it"),
            "{error}"
        );
        accept(&cache, "0.3.0").unwrap();
        assert!(accept(&cache, "0.2.0").is_err());
        // The notice reads the same record.
        let record = read_record(&cache);
        assert_eq!(record.highest.as_deref(), Some("0.3.0"));
    }

    #[test]
    fn a_record_without_highest_still_reads() {
        let cache = cache_path("old-record");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, br#"{"checked":5,"latest":"0.2.0"}"#).unwrap();
        let record = read_record(&cache);
        assert_eq!((record.checked, record.highest), (5, None));
    }

    fn backup_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("twaco-backup-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_failed_swap_puts_the_old_binary_back() {
        let dir = backup_dir("restore");
        let exe = dir.join("twaco.exe");
        std::fs::write(&exe, b"old").unwrap();
        let result = with_backup(&exe, || {
            std::fs::remove_file(&exe)?;
            Err(std::io::Error::other("swap failed"))
        });
        assert_eq!(result.unwrap_err().to_string(), "swap failed");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        assert_eq!(entries(&dir), ["twaco.exe"]);
    }

    #[test]
    fn a_successful_swap_leaves_no_backup() {
        let dir = backup_dir("success");
        let exe = dir.join("twaco.exe");
        std::fs::write(&exe, b"old").unwrap();
        with_backup(&exe, || std::fs::write(&exe, b"new")).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(entries(&dir), ["twaco.exe"]);
    }

    #[test]
    fn a_failed_restore_names_the_backup() {
        let dir = backup_dir("keep");
        let exe = dir.join("twaco.exe");
        std::fs::write(&exe, b"old").unwrap();
        let backup = dir.join(format!(".twaco-backup-{}.exe", std::process::id()));
        // With the backup gone, the rename back fails.
        let error = with_backup(&exe, || {
            std::fs::remove_file(&exe)?;
            std::fs::remove_file(&backup)?;
            Err(std::io::Error::other("swap failed"))
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("is missing. The old twaco is at"), "{error}");
        assert!(error.contains(&backup.display().to_string()), "{error}");
    }
}
