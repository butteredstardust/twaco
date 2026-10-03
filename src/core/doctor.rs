//! `twaco doctor`: what resolved, what is reachable, and what is missing, before anything else
//! is blamed.
//!
//! Read-only in every respect. It holds no lock: asking who holds one takes it for an instant,
//! which a writer starting then waits out (see `lock::acquire`). It writes no baseline, and
//! the one server request it makes is the platform's own script parser on an empty script,
//! which proves the server answers and the credentials work without touching anything.

use super::baseline::Baseline;
use super::config::Solution;
use super::{lock, profile, server, workspace};
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
    pub subject: &'static str,
    pub detail: String,
}

fn item(health: Health, subject: &'static str, detail: impl Into<String>) -> Item {
    Item { health, subject, detail: detail.into() }
}

/// Everything, in the order a person would check it. `root` is where to look for the solution.
pub fn diagnose(root: &Path, profile_name: &str) -> Vec<Item> {
    let mut items = vec![item(
        Health::Ok,
        "twaco",
        format!("{} for {}-{}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH),
    )];

    let solution = match Solution::discover(root) {
        Ok(solution) => solution,
        Err(error) => {
            items.push(item(Health::Fail, "solution", format!("{error}; `twaco init` scaffolds one")));
            return items;
        }
    };
    let name = if solution.solution.name.is_empty() { "(unnamed)" } else { &solution.solution.name };
    items.push(item(Health::Ok, "solution", format!("{name} at {}", solution.root.display())));

    match solution.deploy_order() {
        Ok(order) => items.push(item(
            Health::Ok,
            "projects",
            order.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(" -> "),
        )),
        Err(error) => items.push(item(Health::Fail, "projects", error.to_string())),
    }

    let found = workspace::discover(&solution);
    let misfiled = found.entities.iter().filter(|e| e.is_misfiled()).count();
    let health = if !found.unreadable.is_empty() || misfiled > 0 { Health::Warn } else { Health::Ok };
    let mut detail = format!("{} entity document(s)", found.entities.len());
    if !found.unreadable.is_empty() {
        detail.push_str(&format!(", {} unreadable", found.unreadable.len()));
    }
    if misfiled > 0 {
        detail.push_str(&format!(", {misfiled} filed under a project other than their own"));
    }
    items.push(item(health, "entities", detail));

    let src = solution.src_root();
    items.push(if src.is_dir() {
        item(Health::Ok, "sidecars", src.display().to_string())
    } else {
        item(Health::Warn, "sidecars", format!("{} does not exist yet; `twaco extract --all` creates it", src.display()))
    });
    items.push(item(
        Health::Ok,
        "script layout",
        if solution.format.indent_cdata_payload {
            "indented to the <code> element (compatibility)"
        } else {
            "flush left"
        },
    ));

    items.push(match Baseline::load(&solution.root) {
        Ok(baseline) => {
            let recorded = found.entities.iter().filter(|e| baseline.get(&e.info.collection, &e.info.name).is_some()).count();
            item(Health::Ok, "baseline", format!("{recorded} of {} entities recorded", found.entities.len()))
        }
        Err(error) => item(Health::Fail, "baseline", error.to_string()),
    });

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
        format!("{profile_name}: {} as {}, from {}", profile.url, profile.username, profile::source(&solution.root, profile_name)),
    ));

    let started = Instant::now();
    items.push(match server::Client::new(profile).check_script("") {
        Ok(_) => item(Health::Ok, "server", format!("answered and accepted the credentials in {} ms", started.elapsed().as_millis())),
        Err(error) => item(Health::Fail, "server", error.to_string()),
    });
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_solution_is_a_failure_that_says_what_to_do() {
        let dir = std::env::temp_dir().join(format!("twaco-doctor-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let items = diagnose(&dir, "default");
        let solution = items.iter().find(|i| i.subject == "solution").unwrap();
        // Unless an ancestor of the temp dir holds a twaco.toml, which a test machine should not.
        if solution.health == Health::Fail {
            assert!(solution.detail.contains("twaco init"));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_offline_solution_is_healthy_without_a_profile() {
        let dir = std::env::temp_dir().join(format!(
            "twaco-doctor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
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
        assert!(items.iter().all(|i| i.subject != "server"), "no server check without a profile");
        let _ = std::fs::remove_dir_all(dir);
    }
}
