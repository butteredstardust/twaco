//! `twaco doctor`: what resolved, what is reachable, and what is missing, before anything else
//! is blamed.
//!
//! Read-only in every respect. It holds no lock: asking who holds one takes it for an instant,
//! which a writer starting then waits out (see `lock::acquire`). It writes no baseline, and
//! the one server request it makes is the platform's own script parser on an empty script,
//! which proves the server answers and the credentials work without touching anything.

use super::baseline::{Baseline, LEGACY_RELATIVE_PATH};
use super::config::Solution;
use super::permissions::derived::Derived;
use super::{fmt, gitignore, lock, profile, server, workspace};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Info,
    Ok,
    /// Works, but something is worth knowing.
    Warn,
    /// Something needed is broken.
    Fail,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub health: Health,
    pub subject: String,
    pub detail: String,
    /// What the detail names in part, all of it (empty for most items).
    pub entries: Vec<String>,
}

/// The stable report shared by the command line and MCP adapters.
pub fn report_json(items: &[Item]) -> Value {
    let word = |health| match health {
        Health::Info => "info",
        Health::Ok => "ok",
        Health::Warn => "warn",
        Health::Fail => "fail",
    };
    let failed = items
        .iter()
        .filter(|item| item.health == Health::Fail)
        .count();
    json!({
        "ok": failed == 0,
        "failed": failed,
        "items": items.iter().map(|item| {
            let mut value = json!({
                "health": word(item.health),
                "subject": item.subject,
                "detail": item.detail,
            });
            if !item.entries.is_empty() {
                value["entries"] = json!(item.entries);
            }
            value
        }).collect::<Vec<_>>(),
    })
}

fn item(health: Health, subject: impl Into<String>, detail: impl Into<String>) -> Item {
    Item {
        health,
        subject: subject.into(),
        detail: detail.into(),
        entries: Vec::new(),
    }
}

fn agent_guidance_advice(root: &Path) -> Vec<Item> {
    ["AGENTS.md", "CLAUDE.md"]
        .into_iter()
        .filter_map(|file| {
            let text = std::fs::read_to_string(root.join(file)).ok()?;
            super::init::agent_guidance_stale(file, &text).then(|| {
                item(
                    Health::Info,
                    file,
                    "run `twaco init --agents` to add/refresh twaco's agent guidance",
                )
            })
        })
        .collect()
}

fn tooling_source(root: &Path, name: &str, value: Option<&std::ffi::OsStr>) -> Item {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return item(Health::Info, name, "not set (optional)");
    };
    let path = root.join(value);
    if !path.exists() {
        return item(
            Health::Warn,
            name,
            "set; path does not exist; correct the configured path",
        );
    }
    if name == "TWACO_SDK" {
        return item(Health::Ok, name, "set; path exists");
    }
    match super::structure::source_summary(&path) {
        Ok(summary) => item(Health::Ok, name, format!("set; path exists; {summary}")),
        Err(error) => item(Health::Warn, name, format!("set; path exists; {error}")),
    }
}

/// Everything, in the order a person would check it. `root` is where to look for the solution.
pub fn diagnose(root: &Path, profile_name: &str) -> Vec<Item> {
    let mut items = vec![item(
        Health::Ok,
        "twaco",
        format!(
            "{} for {}-{}",
            crate::version(),
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    )];

    for name in ["TWACO_SDK", "TWACO_STRUCTURE_XSD"] {
        items.push(tooling_source(
            root,
            name,
            std::env::var_os(name).as_deref(),
        ));
    }

    let solution = match Solution::discover(root) {
        Ok(solution) => solution,
        Err(error) => {
            items.push(item(
                Health::Fail,
                "solution",
                format!("{error}; `twaco init` scaffolds one"),
            ));
            return items;
        }
    };
    let name = if solution.solution.name.is_empty() {
        "(unnamed)"
    } else {
        &solution.solution.name
    };
    items.push(item(
        Health::Ok,
        "solution",
        format!("{name} at {}", solution.root.display()),
    ));

    items.extend(agent_guidance_advice(&solution.root));

    match solution.deploy_order() {
        Ok(order) => items.push(item(
            Health::Ok,
            "projects",
            order
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(" -> "),
        )),
        Err(error) => items.push(item(Health::Fail, "projects", error.to_string())),
    }

    let found = workspace::discover(&solution);
    items.extend(server_owned_advice(
        &solution,
        &found.entities,
        profile_name,
    ));
    if let Some(overrides) = solution.profiles.get(profile_name) {
        for rule in &overrides.server_owned {
            items.push(item(
                Health::Ok,
                "profile server-owned",
                format!(
                    "{}: {}",
                    rule.entities.join(", "),
                    super::server_owned::labels(&rule.parts).join(", ")
                ),
            ));
        }
        let unknown = solution.unknown_patches(profile_name, &found.entities);
        for patch in &overrides.patches {
            items.push(if unknown.contains(&patch.entity.as_str()) {
                item(
                    Health::Fail,
                    "profile patch",
                    format!(
                        "{} {}: no such Mashup in this workspace",
                        patch.entity, patch.path
                    ),
                )
            } else {
                item(
                    Health::Ok,
                    "profile patch",
                    format!("{} {}", patch.entity, patch.path),
                )
            });
        }
    }
    let misfiled = found.entities.iter().filter(|e| e.is_misfiled()).count();
    let health = if !found.unreadable.is_empty() || misfiled > 0 {
        Health::Warn
    } else {
        Health::Ok
    };
    let mut detail = format!("{} entity document(s)", found.entities.len());
    if !found.unreadable.is_empty() {
        detail.push_str(&format!(", {} unreadable", found.unreadable.len()));
    }
    if misfiled > 0 {
        detail.push_str(&format!(
            ", {misfiled} filed under a project other than their own"
        ));
    }
    items.push(item(health, "entities", detail));

    let src = solution.src_root();
    items.push(if src.is_dir() {
        item(Health::Ok, "sidecars", src.display().to_string())
    } else {
        item(
            Health::Warn,
            "sidecars",
            format!(
                "{} does not exist yet; `twaco extract --all` creates it",
                src.display()
            ),
        )
    });
    if gitignore::in_git_work_tree(&solution.root) {
        let missing = gitignore::missing(&solution.root);
        items.push(if missing.is_empty() {
            item(Health::Ok, "gitignore", "keeps twaco's local state out of git")
        } else {
            item(
                Health::Warn,
                "gitignore",
                format!(
                    "does not ignore {}; backups can hold a server's secrets. `twaco init --agents` adds them",
                    missing.join(", ")
                ),
            )
        });
        items.push(committed_secrets(&solution.root));
    }
    items.push(item(
        Health::Ok,
        "script layout",
        if solution.format.indent_cdata_payload {
            "indented to the <code> element (compatibility)"
        } else {
            "flush left"
        },
    ));
    items.push(script_style_item(&solution));
    items.push(item(Health::Ok, "lint", solution.lint_doctor_detail()));

    match Baseline::profiles(&solution.root) {
        Ok(mut profiles) => {
            if profile::validate_name(profile_name).is_ok()
                && !profiles.iter().any(|name| name == profile_name)
            {
                profiles.push(profile_name.to_string());
                profiles.sort();
            }
            for name in profiles {
                items.push(baseline_item(&solution, &found, &name));
            }
        }
        Err(error) => items.push(item(Health::Fail, "baseline", error.to_string())),
    }
    if solution.root.join(LEGACY_RELATIVE_PATH).is_file()
        && Baseline::path(&solution.root, "default").is_file()
    {
        items.push(item(
            Health::Warn,
            "baseline legacy",
            ".twaco/baseline.json is stale; .twaco/baselines/default.json wins and the legacy file can be deleted",
        ));
    }

    items.push(match lock::holder(&solution.root) {
        Ok(None) => item(Health::Ok, "workspace lock", "free"),
        Ok(Some(holder)) => item(Health::Warn, "workspace lock", format!("held: {holder}")),
        Err(error) => item(Health::Warn, "workspace lock", error.to_string()),
    });

    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            items.push(item(
                Health::Warn,
                "profile",
                format!("{error}. The offline commands work without one; server commands do not"),
            ));
            return items;
        }
    };
    items.push(item(
        Health::Ok,
        "profile",
        format!(
            "{profile_name}: {} {}, from {}",
            profile::shown_url(&profile.url),
            if profile.signs_in_with_app_key() {
                "with its app key".to_string()
            } else {
                format!("as {}", profile.username)
            },
            profile::source(&solution.root, profile_name)
        ),
    ));

    let started = Instant::now();
    let local = is_local_url(&profile.url);
    let client = server::Client::new(profile);
    match client.check_script("") {
        Ok(_) => {
            items.push(item(
                Health::Ok,
                "server",
                format!(
                    "answered and accepted the credentials in {} ms",
                    started.elapsed().as_millis()
                ),
            ));
            items.push(platform_item(super::help::server_version(&client)));
        }
        Err(error) => {
            let mut detail = error.to_string();
            if local && matches!(error, server::ServerError::Http { status: 401, .. }) {
                detail.push_str(PORTABLE_PASSWORD_HINT);
            }
            items.push(item(Health::Fail, "server", detail));
        }
    }
    items
}

/// Said after a 401 from a local server. It names the settings key only, never its value.
const PORTABLE_PASSWORD_HINT: &str = ". On a fresh portable ThingWorx the Administrator password is AdministratorUserSettings.InitialPassword in the instance's platform-settings.json, not the one the start banner prints";

fn is_local_url(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host.starts_with('[') {
        host.split_once(']').map_or(host, |(h, _)| &h[1..])
    } else {
        host.split(':').next().unwrap_or(host)
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn script_style_item(solution: &Solution) -> Item {
    let scripts = workspace::script_files(solution)
        .files
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .map(|source| workspace::without_bom(&source).to_string())
        .collect::<Vec<_>>();
    let survey = fmt::survey(scripts.iter().map(String::as_str));
    let mismatches = fmt::style_mismatches(&survey, &solution.format.script);
    if !mismatches.is_empty() {
        return item(Health::Warn, "script style", mismatches.join("; "));
    }
    if survey.scripts == 0 {
        return item(
            Health::Ok,
            "script style",
            format!(
                "{} spaces, {} quotes; no sidecar scripts",
                solution.format.script.indent_width,
                fmt::quote_name(solution.format.script.quote_style)
            ),
        );
    }
    let indent = survey.majority_indent().map_or_else(
        || format!("{} spaces", solution.format.script.indent_width),
        |(width, _, _)| format!("{width} spaces"),
    );
    let quotes = survey.majority_quote().map_or_else(
        || {
            format!(
                "{} quotes",
                fmt::quote_name(solution.format.script.quote_style)
            )
        },
        |(style, _, _)| format!("{} quotes", fmt::quote_name(style)),
    );
    let evidence = match (survey.majority_indent(), survey.majority_quote()) {
        (Some((_, count, determined)), Some(_)) => {
            format!("as {count} of {determined} scripts")
        }
        (Some((_, count, determined)), None) => {
            format!("as {count} of {determined} indented scripts")
        }
        (None, Some((_, count, determined))) => {
            format!("as {count} of {determined} scripts with strings")
        }
        (None, None) => "with no majority style".to_string(),
    };
    item(
        Health::Ok,
        "script style",
        format!("{indent}, {quotes}, {evidence}"),
    )
}

/// The server's ThingWorx version, against the one the widget catalogue describes: mashup
/// authoring and validation judge widgets by that catalogue.
fn platform_item(version: Result<String, String>) -> Item {
    let catalogue = super::widgets::PLATFORM;
    let version = match version {
        Ok(version) => version,
        Err(why) => {
            return item(
                Health::Warn,
                "platform",
                format!("the server's ThingWorx version is unknown ({why}); the widget catalogue is {catalogue}"),
            )
        }
    };
    let mut parts = version.trim().split(['.', '-']);
    let major_minor = match (parts.next(), parts.next()) {
        (Some(major), Some(minor))
            if major.parse::<u32>().is_ok() && minor.parse::<u32>().is_ok() =>
        {
            format!("{major}.{minor}")
        }
        _ => {
            return item(
                Health::Warn,
                "platform",
                format!("the server says ThingWorx {version:?}, which is not a version; the widget catalogue is {catalogue}"),
            )
        }
    };
    if major_minor == catalogue {
        item(Health::Ok, "platform", format!("ThingWorx {version}"))
    } else {
        item(
            Health::Warn,
            "platform",
            format!("ThingWorx {version}, but the widget catalogue is {catalogue}: mashup add, new and validate judge widgets by {catalogue}, so check new mashups in Composer on this server"),
        )
    }
}

/// How many entries a doctor line names before it says "and N more"; `--json` lists all.
const NAMED: usize = 3;

fn baseline_item(solution: &Solution, found: &workspace::Discovery, profile_name: &str) -> Item {
    let subject = format!("baseline {profile_name}");
    let baseline = match Baseline::load(&solution.root, profile_name) {
        Ok(baseline) => baseline,
        Err(error) => return item(Health::Fail, subject, error.to_string()),
    };
    let derived = match Derived::read(&found.entities) {
        Ok(derived) => derived,
        Err(error) => return item(Health::Fail, subject, error.to_string()),
    };
    let rules = solution.server_owned_for(profile_name);
    let mut recorded = 0;
    let mut incompatible = Vec::new();
    let mut present = BTreeSet::new();
    for entity in &found.entities {
        let (collection, name) = (&entity.info.collection, &entity.info.name);
        present.insert((collection.as_str(), name.as_str()));
        let parts = super::server_owned::parts(&rules, collection, name);
        let scope = super::normalise::scope_with_grants(&parts, &derived.target(collection, name));
        if baseline.get_in_scope(collection, name, &scope).is_some() {
            recorded += 1;
        } else if baseline.contains(collection, name) {
            incompatible.push(format!("{collection}/{name}"));
        }
    }
    let orphans: Vec<String> = baseline
        .addresses()
        .filter(|address| !present.contains(address))
        .map(|(collection, name)| format!("{collection}/{name}"))
        .collect();
    let total = found.entities.len();
    let mut line = format!("{recorded} of {total} entities recorded");
    let mut entries = Vec::new();
    if !incompatible.is_empty() {
        line.push_str(&format!(
            "; {} incompatible entries count as unrecorded (hash version or server-owned rules changed: {}): `twaco entity status --all --record --profile {profile_name}` records again those that match the server, so check they match first",
            incompatible.len(),
            named(&incompatible)
        ));
        entries.extend(incompatible.iter().map(|a| format!("incompatible {a}")));
    }
    if !orphans.is_empty() {
        line.push_str(&format!(
            "; {} entries are for entities not in the repository ({}): `twaco entity status --all --record --profile {profile_name}` prunes those the server no longer has",
            orphans.len(),
            named(&orphans)
        ));
        entries.extend(orphans.iter().map(|a| format!("orphan {a}")));
    }
    let mut result = item(
        if entries.is_empty() {
            Health::Ok
        } else {
            Health::Warn
        },
        subject,
        line,
    );
    result.entries = entries;
    result
}

fn named(addresses: &[String]) -> String {
    let mut text = addresses
        .iter()
        .take(NAMED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if addresses.len() > NAMED {
        text.push_str(&format!(", and {} more", addresses.len() - NAMED));
    }
    text
}

fn server_owned_advice(
    solution: &Solution,
    entities: &[workspace::EntityFile],
    profile_name: &str,
) -> Vec<Item> {
    let mut items = Vec::new();
    let rules = solution.server_owned_for(profile_name);
    // twaco's own rules apply wherever they match, and matching nothing is no mistake.
    for (i, rule) in rules.iter().filter(|rule| !rule.built_in).enumerate() {
        let matched: Vec<_> = entities
            .iter()
            .filter(|e| rule.matches(&e.info.collection, &e.info.name))
            .collect();
        if matched.is_empty() {
            items.push(item(Health::Warn, "server-owned parts", format!("server_owned rule {} matches no entity: {}; check its Collection/Name patterns", i + 1, rule.entities.join(", "))));
            continue;
        }
        // A part some kinds of entity cannot have owns nothing on them: the rule then hides
        // nothing, which its author did not mean.
        for part in &rule.parts {
            let Some(kinds) = super::server_owned::collections(part) else {
                continue;
            };
            if !matched
                .iter()
                .any(|e| kinds.contains(&e.info.collection.as_str()))
            {
                items.push(item(
                    Health::Warn,
                    "server-owned parts",
                    format!(
                        "server_owned rule {}: {} only exists on {}, and the rule matches none",
                        i + 1,
                        part.label(),
                        kinds.join(" and ")
                    ),
                ));
            }
        }
    }
    items
}

/// Profiles, backups or journal backups that git tracks. A committed profile is a leaked
/// credential: untracking it is not enough, because history keeps it.
fn committed_secrets(root: &Path) -> Item {
    let tracked = match gitignore::tracked_secrets(root) {
        Ok(tracked) => tracked,
        Err(error) => return item(Health::Warn, "secrets in git", error),
    };
    if tracked.is_empty() {
        return item(
            Health::Ok,
            "secrets in git",
            "git tracks no profile, backup or journal",
        );
    }
    let profiles = tracked
        .iter()
        .any(|path| path.starts_with(".twaco/profiles/"));
    let shown = tracked
        .iter()
        .take(5)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let more = if tracked.len() > 5 {
        format!(" and {} more", tracked.len() - 5)
    } else {
        String::new()
    };
    let mut detail = format!(
        "git tracks {} file(s) that can hold a secret: {shown}{more}. `git rm --cached` them",
        tracked.len()
    );
    if profiles {
        detail.push_str(
            "; history keeps a committed profile, so change the password and app key it holds",
        );
    }
    item(
        if profiles { Health::Fail } else { Health::Warn },
        "secrets in git",
        detail,
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_platform_item_warns_when_the_server_is_not_the_catalogues_version() {
        let catalogue = crate::core::widgets::PLATFORM;
        let same = super::platform_item(Ok(format!("{catalogue}.0-b12")));
        assert_eq!(same.health, super::Health::Ok);
        assert_eq!(same.detail, format!("ThingWorx {catalogue}.0-b12"));
        let other = super::platform_item(Ok("9.3.6-b123".to_string()));
        assert_eq!(other.health, super::Health::Warn);
        assert!(other
            .detail
            .starts_with("ThingWorx 9.3.6-b123, but the widget catalogue is "));
        // 10.20 is not 10.2.
        let longer = super::platform_item(Ok(format!("{catalogue}0.1")));
        assert_eq!(longer.health, super::Health::Warn);
        for unreadable in [
            Ok("unknown".to_string()),
            Ok("10".to_string()),
            Err("HTTP 403".to_string()),
        ] {
            assert_eq!(super::platform_item(unreadable).health, super::Health::Warn);
        }
    }

    #[test]
    fn server_owned_parts_the_matched_entities_cannot_have_are_advisory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("twaco.toml"),
            "[[project]]
name='Acme.Project'
[[server_owned]]
entities=['Things/Acme.*']
parts=['members', 'property-values', 'run-time-permissions']
",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("Things")).unwrap();
        std::fs::write(dir.path().join("Things/Acme.T.xml"), "<Entities><Things><Thing name=\"Acme.T\" projectName=\"Acme.Project\"/></Things></Entities>").unwrap();
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        let found = workspace::discover(&solution);
        let items = server_owned_advice(&solution, &found.entities, "default");
        let messages: Vec<_> = items.iter().map(|item| item.detail.as_str()).collect();
        assert_eq!(
            messages,
            ["server_owned rule 1: members only exists on Groups and Roles and PermissionGroups and Organizations, and the rule matches none"],
        );
    }

    #[test]
    fn server_owned_rules_matching_no_entity_are_advisory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname='Acme.Project'\n[[server_owned]]\nentities=['Groups/Acme.Missing*']\nparts=['members']\n").unwrap();
        let items = super::diagnose(dir.path(), "missing");
        let advice = items
            .iter()
            .find(|i| i.subject == "server-owned parts")
            .unwrap();
        assert_eq!(advice.health, super::Health::Warn);
        assert!(
            advice.detail.contains("matches no entity")
                && advice.detail.contains("Groups/Acme.Missing*")
        );
        assert!(items.iter().all(|i| i.health != super::Health::Fail));
    }
    use super::*;

    fn git(root: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .stdout(std::process::Stdio::null())
            .status()
            .expect("git is on PATH");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn a_committed_profile_fails_and_a_committed_backup_warns() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-doctor-secrets-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join(".twaco/backups")).unwrap();
        git(&root, &["init", "-q"]);
        assert_eq!(committed_secrets(&root).health, Health::Ok);

        std::fs::write(root.join(".twaco/backups/A.T.xml"), "x").unwrap();
        git(&root, &["add", "-A"]);
        let backup = committed_secrets(&root);
        assert_eq!(backup.health, Health::Warn, "{backup:?}");
        assert!(
            backup.detail.contains(".twaco/backups/A.T.xml"),
            "{backup:?}"
        );
        assert!(!backup.detail.contains("password"), "{backup:?}");

        std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
        std::fs::write(
            root.join(".twaco/profiles/default.toml"),
            "password = \"s3cret\"",
        )
        .unwrap();
        git(&root, &["add", "-A"]);
        let profile = committed_secrets(&root);
        assert_eq!(profile.health, Health::Fail, "{profile:?}");
        assert!(
            profile.detail.contains("change the password"),
            "{profile:?}"
        );
        assert!(
            !profile.detail.contains("s3cret"),
            "names files, never contents"
        );
    }

    #[test]
    fn no_solution_is_a_failure_that_says_what_to_do() {
        let dir_guard = tempfile::Builder::new()
            .prefix("twaco-doctor-empty-")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let items = diagnose(&dir, "default");
        let solution = items.iter().find(|i| i.subject == "solution").unwrap();
        // Unless an ancestor of the temp dir holds a twaco.toml, which a test machine should not.
        if solution.health == Health::Fail {
            assert!(solution.detail.contains("twaco init"));
        }
    }

    #[test]
    fn an_offline_solution_is_healthy_without_a_profile() {
        let dir_guard = tempfile::Builder::new()
            .prefix("twaco-doctor-")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(dir.join("Things")).unwrap();
        std::fs::write(dir.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            dir.join("Things/P.T.xml"),
            "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"></Thing></Things></Entities>",
        )
        .unwrap();
        let items = diagnose(&dir, "no-such-profile");
        assert!(items.iter().all(|i| i.health != Health::Fail), "{items:?}");
        let entities = items.iter().find(|i| i.subject == "entities").unwrap();
        assert_eq!(entities.detail, "1 entity document(s)");
        let profile = items.iter().find(|i| i.subject == "profile").unwrap();
        assert_eq!(profile.health, Health::Warn);
        assert!(
            items.iter().all(|i| i.subject != "server"),
            "no server check without a profile"
        );
    }

    #[test]
    fn script_style_warns_when_the_config_would_reformat_most_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let script = dir.path().join("src/P.T/services/Run/script.js");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(
            script,
            "if (ok) {\n  result = 'one';\n}\nif (again) {\n  result = 'two';\n}",
        )
        .unwrap();

        let item = diagnose(dir.path(), "missing")
            .into_iter()
            .find(|item| item.subject == "script style")
            .unwrap();
        assert_eq!(item.health, Health::Warn);
        assert!(item
            .detail
            .contains("1 of 1 indented scripts use 2 spaces but [format.script] indent_width = 4"));
        assert!(item.detail.contains("1 of 1 scripts with strings use single quotes but [format.script] quote_style = \"double\""));
    }

    #[test]
    fn script_style_without_sidecars_is_healthy() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let item = diagnose(dir.path(), "missing")
            .into_iter()
            .find(|item| item.subject == "script style")
            .unwrap();
        assert_eq!(item.health, Health::Ok);
        assert_eq!(item.detail, "4 spaces, double quotes; no sidecar scripts");
    }

    #[test]
    fn reports_each_profile_baseline_and_a_stale_legacy_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::create_dir_all(dir.path().join("Things")).unwrap();
        std::fs::write(
            dir.path().join("Things/P.T.xml"),
            "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"/></Things></Entities>",
        )
        .unwrap();
        let mut baseline = Baseline::default();
        baseline.set("Things", "P.T", "v7:x".into(), "v7:x".into());
        baseline.write(dir.path(), "default").unwrap();
        baseline.write(dir.path(), "development").unwrap();
        std::fs::write(
            dir.path().join(LEGACY_RELATIVE_PATH),
            baseline.to_bytes(dir.path()).unwrap(),
        )
        .unwrap();

        let items = diagnose(dir.path(), "missing");
        assert!(items.iter().any(|item| item.subject == "baseline default"));
        assert!(items
            .iter()
            .any(|item| item.subject == "baseline development"));
        let legacy = items
            .iter()
            .find(|item| item.subject == "baseline legacy")
            .unwrap();
        assert_eq!(legacy.health, Health::Warn);
        assert!(legacy.detail.contains("can be deleted"));
    }

    #[test]
    fn names_incompatible_and_orphan_entries_and_says_what_fixes_each() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("twaco.toml"),
            "[[project]]
name = \"P\"
",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("Things")).unwrap();
        std::fs::write(
            dir.path().join("Things/P.T.xml"),
            "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"/></Things></Entities>",
        )
        .unwrap();
        let mut baseline = Baseline::default();
        baseline.set("Things", "P.T", "v7:x".into(), "v7:x".into());
        for n in 1..=5 {
            baseline.set(
                "Users",
                &format!("Acme.Gone{n}"),
                "v10:x".into(),
                "v10:x".into(),
            );
        }
        baseline.write(dir.path(), "default").unwrap();

        let items = diagnose(dir.path(), "missing");
        let item = items
            .iter()
            .find(|item| item.subject == "baseline default")
            .unwrap();
        assert_eq!(item.health, Health::Warn);
        assert!(item.detail.contains("Things/P.T"), "{}", item.detail);
        assert!(item
            .detail
            .contains("5 entries are for entities not in the repository"));
        assert!(item.detail.contains("and 2 more"), "{}", item.detail);
        assert!(
            !item.detail.contains("  "),
            "no stray runs of spaces: {}",
            item.detail
        );
        assert!(item.detail.contains("entity status --all --record"));
        assert_eq!(item.entries.len(), 6);
        assert!(item
            .entries
            .contains(&"orphan Users/Acme.Gone5".to_string()));
        let json = report_json(&items);
        let listed = json["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["subject"] == "baseline default")
            .unwrap();
        assert_eq!(listed["entries"].as_array().unwrap().len(), 6);
    }

    #[test]
    fn a_local_address_is_recognised_for_the_portable_password_hint() {
        for local in [
            "http://localhost:8080/Thingworx/",
            "http://127.0.0.1/Thingworx",
            "http://[::1]:8080/Thingworx/",
            "https://user:pw@localhost/Thingworx/",
        ] {
            assert!(is_local_url(local), "{local}");
        }
        for remote in [
            "https://thingworx.example.com/Thingworx/",
            "http://localhost.example.com/Thingworx/",
        ] {
            assert!(!is_local_url(remote), "{remote}");
        }
        assert!(PORTABLE_PASSWORD_HINT.contains("AdministratorUserSettings.InitialPassword"));
    }

    #[test]
    fn a_401_from_a_local_server_names_the_portable_password_setting() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let serving = std::thread::spawn(move || {
            for stream in listener.incoming().take(1) {
                let mut stream = stream.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                let header_end = loop {
                    let read = stream.read(&mut buffer).unwrap();
                    assert!(read > 0);
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .map(|value| value.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                while request.len() < header_end + length {
                    let read = stream.read(&mut buffer).unwrap();
                    assert!(read > 0);
                    request.extend_from_slice(&buffer[..read]);
                }
                stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            }
        });
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("twaco.toml"),
            "[[project]]
name = \"P\"
",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".twaco/profiles")).unwrap();
        std::fs::write(
            dir.path().join(".twaco/profiles/default.toml"),
            format!(
                "url = \"http://127.0.0.1:{port}/Thingworx/\"
username = \"u\"
password = \"p\"
"
            ),
        )
        .unwrap();
        let items = diagnose(dir.path(), "default");
        let server = items.iter().find(|i| i.subject == "server").unwrap();
        assert_eq!(server.health, Health::Fail);
        assert!(server.detail.contains("401"), "{}", server.detail);
        assert!(
            server
                .detail
                .contains("AdministratorUserSettings.InitialPassword"),
            "{}",
            server.detail
        );
        serving.join().unwrap();
    }
}

#[cfg(test)]
mod tooling_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn optional_tooling_reports_unset_missing_broken_and_schema_generation() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            tooling_source(temp.path(), "TWACO_SDK", None).health,
            Health::Info
        );
        assert_eq!(
            tooling_source(
                temp.path(),
                "TWACO_SDK",
                Some(std::ffi::OsStr::new("missing"))
            )
            .health,
            Health::Warn
        );
        let jar = temp.path().join("schema.jar");
        for (schema, version) in [("", "10.0-compatible"), ("<xs:element name=\"Roles\"/><xs:element name=\"PermissionGroups\"/><xs:element name=\"MCPNamespaces\"/>", "10.2-compatible")] {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&jar).unwrap());
            zip.start_file("com/thingworx/documents/schema.xsd", zip::write::SimpleFileOptions::default()).unwrap();
            write!(zip, "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"Entities\"/>{schema}</xs:schema>").unwrap();
            zip.finish().unwrap();
            let report = tooling_source(temp.path(), "TWACO_STRUCTURE_XSD", Some(jar.as_os_str()));
            assert_eq!(report.health, Health::Ok, "{}", report.detail);
            assert!(report.detail.contains(version), "{}", report.detail);
        }
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&jar).unwrap());
        zip.start_file("other.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();
        let report = tooling_source(temp.path(), "TWACO_STRUCTURE_XSD", Some(jar.as_os_str()));
        assert_eq!(report.health, Health::Warn);
        assert!(report.detail.contains("has no"));
    }
}

#[cfg(test)]
mod agent_guidance_tests {
    use super::*;

    #[test]
    fn guidance_advice_reports_only_existing_missing_or_stale_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(agent_guidance_advice(root).is_empty());
        std::fs::write(root.join("AGENTS.md"), "# Acme guidance\n").unwrap();
        std::fs::write(
            root.join("CLAUDE.md"),
            "<!-- twaco:begin -->old<!-- twaco:end -->",
        )
        .unwrap();
        let advice = agent_guidance_advice(root);
        assert_eq!(advice.len(), 2);
        assert!(advice.iter().all(|item| item.health == Health::Info
            && item.detail == "run `twaco init --agents` to add/refresh twaco's agent guidance"));
        super::super::init::write_agent_files(root, "Acme", &[]).unwrap();
        assert!(agent_guidance_advice(root).is_empty());
    }
}
