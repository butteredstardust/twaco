//! Two-input status, without profile, baseline or adopt planning.

use super::{Notices, StatusCommandError, StatusTarget};
use crate::core::adopt::{self, EntityRef};
use crate::core::config::Solution;
use crate::core::normalise::Element;
use crate::core::workspace;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug)]
pub struct Entry {
    pub entity: String,
    pub verdict: &'static str,
    pub paths: Vec<String>,
}

#[derive(Debug)]
pub struct Report {
    pub entries: Vec<Entry>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.entries.iter().all(|entry| entry.verdict == "same")
    }

    pub fn counts(&self) -> BTreeMap<&str, usize> {
        let mut counts = BTreeMap::from([
            ("same", 0),
            ("different", 0),
            ("only-in-export", 0),
            ("only-in-repository", 0),
        ]);
        for entry in &self.entries {
            *counts.get_mut(entry.verdict).expect("known verdict") += 1;
        }
        counts
    }

    pub fn json(&self, detail: bool) -> Value {
        let listed: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| detail || entry.verdict != "same")
            .map(|entry| {
                let mut value = json!({"entity": entry.entity, "verdict": entry.verdict});
                if detail {
                    value["paths"] = json!(entry.paths);
                }
                value
            })
            .collect();
        let mut result = json!({
            "ok": self.ok(), "entities": self.entries.len(), "counts": self.counts(),
            "failures": [], "unreadable": [],
            "notes": ["Only-in-repository means absent from this export; a partial export does not prove deletion."],
        });
        result[if detail { "statuses" } else { "attention" }] = json!(listed);
        result
    }
}

fn project(element: &Element) -> String {
    element
        .attributes
        .iter()
        .find(|(name, _)| name == b"projectName")
        .map(|(_, value)| String::from_utf8_lossy(value).into_owned())
        .unwrap_or_default()
}

/// Compare entity XML in the export's scope. Never reads profiles or stored baselines, and
/// never writes the workspace; sidecar edits are only reported as an advisory.
pub fn execute(
    solution: &Solution,
    path: &Path,
    target: &StatusTarget,
    narrowed: Option<&str>,
    notices: &mut Notices,
) -> Result<Report, StatusCommandError> {
    if let Some(name) = narrowed {
        if solution.project(name).is_none() {
            return Err(StatusCommandError::Project(name.to_string()));
        }
    }
    let found = workspace::discover(solution);
    if !found.unreadable.is_empty() {
        return Err(StatusCommandError::Unreadable(found.unreadable));
    }
    let bad = |error: adopt::AdoptError| StatusCommandError::Ancestor {
        path: path.to_path_buf(),
        why: error.to_string(),
    };
    let mut exported = adopt::export_entities(path).map_err(bad)?;
    let mut repository = adopt::repository_entities(solution).map_err(bad)?;
    if let Some(name) = narrowed {
        exported.retain(|_, element| project(element) == name);
        repository.retain(|_, (_, path)| {
            found
                .entities
                .iter()
                .any(|file| &file.path == path && file.found_under == name)
        });
    }
    let coverage: BTreeSet<_> = exported
        .iter()
        .map(|(key, element)| (key.collection.clone(), project(element)))
        .collect();
    let mut keys: BTreeSet<EntityRef> = exported.keys().cloned().collect();
    keys.extend(
        repository
            .iter()
            .filter(|(key, (element, _))| {
                coverage.contains(&(key.collection.clone(), project(element)))
            })
            .map(|(key, _)| key.clone()),
    );
    match target {
        StatusTarget::All => {}
        StatusTarget::Names(names) => {
            if names.is_empty() {
                return Err(StatusCommandError::Target);
            }
            let mut selected = BTreeSet::new();
            for name in names {
                let matches: Vec<_> = keys
                    .iter()
                    .filter(|key| {
                        key.path() == *name
                            || key.name == *name
                            || key.name.rsplit('.').next() == Some(name.as_str())
                    })
                    .cloned()
                    .collect();
                if matches.len() != 1 {
                    return Err(StatusCommandError::Ancestor {
                        path: path.to_path_buf(),
                        why: format!("{name}: expected one entity in the export scope, found {}; use Collection/Name", matches.len()),
                    });
                }
                selected.extend(matches);
            }
            keys = selected;
        }
        StatusTarget::Keys(selected) => keys.retain(|key| {
            selected
                .iter()
                .any(|wanted| wanted.collection() == key.collection && wanted.name() == key.name)
        }),
    }
    let entries: Vec<Entry> = keys
        .into_iter()
        .map(|key| {
            let (verdict, paths) = match (exported.get(&key), repository.get(&key)) {
                (Some(left), Some((right, _))) => {
                    let report = adopt::compare_owned_entity(solution, &key, left, right);
                    let paths: Vec<_> = report
                        .differences
                        .into_iter()
                        .map(|diff| diff.path)
                        .collect();
                    (
                        if paths.is_empty() {
                            "same"
                        } else {
                            "different"
                        },
                        paths,
                    )
                }
                (Some(_), None) => ("only-in-export", Vec::new()),
                (None, Some(_)) => ("only-in-repository", Vec::new()),
                (None, None) => unreachable!("union of inputs"),
            };
            Entry {
                entity: key.path(),
                verdict,
                paths,
            }
        })
        .collect();
    let compared = StatusTarget::Names(entries.iter().map(|entry| entry.entity.clone()).collect());
    super::behind_sidecars(solution, &compared, narrowed, notices);
    Ok(Report { entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(config: &str) -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("twaco.toml"),
            format!("[[project]]\nname = \"Acme.Project\"\n{config}"),
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("Things")).unwrap();
        std::fs::create_dir(dir.path().join("Mashups")).unwrap();
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn thing(name: &str, description: &str) -> String {
        format!("<Thing name=\"Acme.{name}\" projectName=\"Acme.Project\" description=\"{description}\"/>")
    }

    fn write_thing(solution: &Solution, name: &str, description: &str) {
        std::fs::write(
            solution.root.join(format!("Things/Acme.{name}.xml")),
            format!(
                "<Entities><Things>{}</Things></Entities>",
                thing(name, description)
            ),
        )
        .unwrap();
    }

    #[test]
    fn offline_scope_and_paths_without_profiles_or_usable_baselines() {
        let (_dir, solution) = fixture("");
        for (name, description) in [("Same", "same"), ("Different", "repo"), ("Missing", "")] {
            write_thing(&solution, name, description);
        }
        std::fs::write(solution.root.join("Mashups/Acme.Other.xml"),
            "<Entities><Mashups><Mashup name=\"Acme.Other\" projectName=\"Acme.Project\"/></Mashups></Entities>").unwrap();
        std::fs::create_dir_all(solution.root.join(".twaco/baselines")).unwrap();
        let baseline = solution.root.join(".twaco/baselines/default.json");
        std::fs::write(&baseline, "unreadable baseline").unwrap();
        let path = solution.root.join("export.xml");
        std::fs::write(
            &path,
            format!(
                "<Entities><Things>{}{}{}</Things></Entities>",
                thing("Same", "same"),
                thing("Different", "export"),
                thing("New", "")
            ),
        )
        .unwrap();
        let report = execute(
            &solution,
            &path,
            &StatusTarget::All,
            None,
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(report.entries.len(), 4);
        assert!(!report.ok());
        for verdict in ["same", "different", "only-in-export", "only-in-repository"] {
            assert_eq!(report.counts()[verdict], 1);
        }
        let different = report
            .entries
            .iter()
            .find(|entry| entry.verdict == "different")
            .unwrap();
        assert!(different
            .paths
            .iter()
            .any(|path| path.ends_with("@description")));
        assert_eq!(report.json(false)["attention"].as_array().unwrap().len(), 3);
        assert_eq!(report.json(true)["statuses"].as_array().unwrap().len(), 4);
        assert_eq!(
            std::fs::read_to_string(&baseline).unwrap(),
            "unreadable baseline"
        );
        assert!(!solution.root.join(".twaco/profiles").exists());
        assert!(!solution.root.join(".twaco/lock").exists());
    }

    #[test]
    fn single_entity_and_ignores_use_adopt_semantics() {
        let (_dir, solution) = fixture("\n[adopt]\nignore_paths = [\"**@description\"]\n");
        write_thing(&solution, "Same", "repo");
        let path = solution.root.join("export.xml");
        std::fs::write(&path, thing("Same", "export")).unwrap();
        let report = execute(
            &solution,
            &path,
            &StatusTarget::All,
            None,
            &mut Notices::default(),
        )
        .unwrap();
        assert!(report.ok());
        let report = execute(
            &solution,
            &path,
            &StatusTarget::Names(vec!["Same".into()]),
            None,
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(report.entries.len(), 1);
        std::fs::write(&path, "<invalid/>").unwrap();
        assert!(execute(
            &solution,
            &path,
            &StatusTarget::All,
            None,
            &mut Notices::default()
        )
        .is_err());
    }

    #[test]
    fn server_owned_values_are_omitted_and_comparison_writes_nothing() {
        let (_dir, solution) = fixture(
            "\n[[server_owned]]\nentities = [\"Things/Acme.*\"]\nparts = [\"property-values\"]\n",
        );
        let xml = |value| {
            format!("<Entities><Things><Thing name=\"Acme.Values\" projectName=\"Acme.Project\"><ThingProperties><speed><Value>{value}</Value></speed></ThingProperties></Thing></Things></Entities>")
        };
        let repository = solution.root.join("Things/Acme.Values.xml");
        std::fs::write(&repository, xml(1)).unwrap();
        let path = solution.root.join("export.xml");
        std::fs::write(&path, xml(2)).unwrap();
        let report = execute(
            &solution,
            &path,
            &StatusTarget::All,
            None,
            &mut Notices::default(),
        )
        .unwrap();
        assert!(report.ok());
        assert_eq!(std::fs::read_to_string(&repository).unwrap(), xml(1));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), xml(2));
        assert!(!solution.root.join(".twaco").exists());
    }

    #[test]
    fn coverage_is_per_collection_and_project() {
        let (_dir, solution) = fixture("\n[[project]]\nname = \"Acme.Other\"\n");
        write_thing(&solution, "Same", "");
        std::fs::write(solution.root.join("Things/Acme.Other.xml"),
            "<Entities><Things><Thing name=\"Acme.Other\" projectName=\"Acme.Other\"/></Things></Entities>").unwrap();
        let path = solution.root.join("export.xml");
        std::fs::write(&path, thing("Same", "")).unwrap();
        let report = execute(
            &solution,
            &path,
            &StatusTarget::All,
            None,
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(report.entries.len(), 1);
        assert!(report.ok());
        let report = execute(
            &solution,
            &path,
            &StatusTarget::All,
            Some("Acme.Other"),
            &mut Notices::default(),
        )
        .unwrap();
        assert!(report.entries.is_empty());
    }

    #[test]
    fn stale_sidecar_advisory_does_not_sync_the_xml() {
        let (_dir, solution) = fixture("");
        let xml = "<Entities><Things><Thing name=\"Acme.Script\" projectName=\"Acme.Project\"><ThingShape>\
<ServiceDefinitions><ServiceDefinition name=\"Run\"/></ServiceDefinitions>\
<ServiceImplementations><ServiceImplementation name=\"Run\" handlerName=\"Script\">\
<ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>\
<code><![CDATA[old();]]></code></Row></Rows></ConfigurationTable></ConfigurationTables>\
</ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>";
        let entity = solution.root.join("Things/Acme.Script.xml");
        std::fs::write(&entity, xml).unwrap();
        let path = solution.root.join("export.xml");
        std::fs::write(&path, xml).unwrap();
        let sidecar = solution.root.join("src/Acme.Script/services/Run");
        std::fs::create_dir_all(&sidecar).unwrap();
        std::fs::write(
            sidecar.join("definition.xml"),
            "<ServiceDefinition name=\"Run\"/>\n",
        )
        .unwrap();
        std::fs::write(sidecar.join("script.js"), "changed();").unwrap();
        let mut notices = Notices::default();
        let report = execute(&solution, &path, &StatusTarget::All, None, &mut notices).unwrap();
        assert!(report.ok());
        assert!(notices
            .lines()
            .iter()
            .any(|line| line.contains("behind their sidecars")));
        assert_eq!(std::fs::read_to_string(&entity).unwrap(), xml);
    }
}
