//! `twaco doctor`: what resolved, what is reachable, and what is missing, before anything else
//! is blamed.
//!
//! Read-only in every respect. It holds no lock: asking who holds one takes it for an instant,
//! which a writer starting then waits out (see `lock::acquire`). It writes no baseline, and
//! the one server request it makes is the platform's own script parser on an empty script,
//! which proves the server answers and the credentials work without touching anything.

use super::baseline::{Baseline, LEGACY_RELATIVE_PATH};
use super::config::Solution;
use super::{gitignore, lock, profile, server, workspace};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
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
}

/// The stable report shared by the command line and MCP adapters.
pub fn report_json(items: &[Item]) -> Value {
    let word = |health| match health {
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
        "items": items.iter().map(|item| json!({
            "health": word(item.health),
            "subject": item.subject,
            "detail": item.detail,
        })).collect::<Vec<_>>(),
    })
}

fn item(health: Health, subject: impl Into<String>, detail: impl Into<String>) -> Item {
    Item {
        health,
        subject: subject.into(),
        detail: detail.into(),
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
            "{profile_name}: {} as {}, from {}",
            profile::shown_url(&profile.url),
            profile.username,
            profile::source(&solution.root, profile_name)
        ),
    ));

    let started = Instant::now();
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
        Err(error) => items.push(item(Health::Fail, "server", error.to_string())),
    }
    items
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

fn baseline_item(solution: &Solution, found: &workspace::Discovery, profile_name: &str) -> Item {
    let subject = format!("baseline {profile_name}");
    let baseline = match Baseline::load(&solution.root, profile_name) {
        Ok(baseline) => baseline,
        Err(error) => return item(Health::Fail, subject, error.to_string()),
    };
    let mut recorded = 0;
    let mut outdated = baseline.outdated();
    let rules = solution.server_owned_for(profile_name);
    for entity in &found.entities {
        let (collection, name) = (&entity.info.collection, &entity.info.name);
        let parts = super::server_owned::parts(&rules, collection, name);
        let hash = format!("{}:", super::normalise::hash_scope(&parts));
        if baseline.get_for(collection, name, &hash).is_some() {
            recorded += 1;
        } else if baseline.get(collection, name).is_some() {
            outdated += 1;
        }
    }
    let total = found.entities.len();
    if outdated == 0 {
        return item(
            Health::Ok,
            subject,
            format!("{recorded} of {total} entities recorded"),
        );
    }
    item(
        Health::Warn,
        subject,
        format!(
            "{recorded} of {total} entities recorded; {outdated} incompatible entries count as              unrecorded (hash version or server-owned rules changed): `twaco entity status --all              --record --profile {profile_name}` records again those that match the server"
        ),
    )
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
            ["server_owned rule 1: members only exists on Groups and Organizations, and the rule matches none"],
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
}
