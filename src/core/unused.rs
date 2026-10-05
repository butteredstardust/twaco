//! Entities nothing reaches.
//!
//! An entity is reported when no chain of references leads to it from an **entry point**: the
//! things a deploy runs (`twaco.toml`), every mashup, anything that runs on events (it declares
//! subscriptions, inherits something that does, or is a Timer or Scheduler), and whatever
//! `[unused] keep` names. The answer is advisory. A REST client, a connected system or a person
//! with a bookmark may use an entity the repository never mentions, so nothing here deletes
//! anything, and `keep` is how a solution says "this one is used from outside".
//!
//! Only entities that hold code or types are judged (Things, templates, shapes and DataShapes):
//! users, groups and the like are used by being present.

use super::config::Solution;
use super::entity_key::EntityKey;
use super::index::{Confidence, Index, Unreadable};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The collections whose members are judged.
pub const JUDGED: &[&str] = &["Things", "ThingTemplates", "ThingShapes", "DataShapes"];

/// What the index cannot see, said wherever the answer is shown.
pub const LIMITS: &str = "an entity used only from outside this repository (a REST client, a connected system) looks unused; list it under [unused] keep";

pub struct Request {
    /// The weakest reference that counts as use. `review` (the default) counts a string that
    /// merely looks like the name; `resolved` or `structural` report more.
    pub min: Confidence,
    /// Judge one collection only, such as `Things`.
    pub collection: Option<String>,
}

/// Why a group of entities is an entry point.
#[derive(Debug, Serialize)]
pub struct RootGroup {
    pub reason: &'static str,
    pub entities: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Unused {
    /// `Collection/Name`.
    pub entity: String,
    pub project: String,
    /// Its file, relative to the solution root.
    pub file: Option<String>,
    /// What still names it although nothing reaches that either: a cluster of dead entities names
    /// itself, and removing one entity at a time would break the rest.
    pub named_by_unreached: Vec<String>,
    /// What names it and is reached, but by a reference weaker than the minimum asked for. A
    /// stronger reading of the same repository would not call this entity unused.
    pub named_weakly_by: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub min_confidence: Confidence,
    pub roots: Vec<RootGroup>,
    /// How many entities were judged, and how many of them something reaches.
    pub judged: usize,
    pub reachable: usize,
    pub unused: Vec<Unused>,
    /// `keep` patterns that match no entity: a typo keeps nothing.
    pub keep_unmatched: Vec<String>,
    pub complete: bool,
    pub unreadable: Vec<Unreadable>,
    pub unparsed_scripts: Vec<String>,
    pub limits: &'static str,
}

/// Whether `pattern` matches `text`, `*` standing for any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (at, part) in parts.iter().enumerate() {
        if at == 0 {
            let Some(after) = rest.strip_prefix(part) else {
                return false;
            };
            rest = after;
        } else if at == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            let Some(found) = rest.find(part) else {
                return false;
            };
            rest = &rest[found + part.len()..];
        }
    }
    true
}

/// A keep pattern names an entity by `Collection/Name` or by its full name.
fn kept(patterns: &[String], key: &EntityKey) -> bool {
    patterns
        .iter()
        .any(|pattern| glob(pattern, &key.to_string()) || glob(pattern, key.name()))
}

pub fn run(solution: &Solution, request: &Request) -> Report {
    let index = Index::build(solution);
    let keep = &solution.unused.keep;
    let mut groups: BTreeMap<&'static str, Vec<EntityKey>> = BTreeMap::new();
    for target in index.deploy_targets() {
        groups
            .entry("deployed by twaco.toml")
            .or_default()
            .push(target);
    }
    for (key, _) in index.entities() {
        if key.collection() == "Mashups" {
            groups.entry("a mashup").or_default().push(key.clone());
        }
        if index.runs_on_events(key) {
            groups
                .entry("runs on events")
                .or_default()
                .push(key.clone());
        }
        if kept(keep, key) {
            groups
                .entry("kept by [unused] keep")
                .or_default()
                .push(key.clone());
        }
    }
    let roots: Vec<EntityKey> = groups
        .values()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let reached = index.reachable_from(&roots, request.min);
    let judged: Vec<EntityKey> = index
        .entities()
        .filter(|(key, _)| JUDGED.contains(&key.collection()))
        .filter(|(key, _)| {
            request
                .collection
                .as_deref()
                .is_none_or(|collection| key.collection() == collection)
        })
        .map(|(key, _)| key.clone())
        .collect();
    let mut unused: Vec<Unused> = judged
        .iter()
        .filter(|key| !reached.contains(*key))
        .filter_map(|key| {
            let node = index.node(key)?;
            let (reached_by, unreached_by): (Vec<String>, Vec<String>) =
                index.referrers(key).into_iter().partition(|label| {
                    label
                        .split_once('/')
                        .and_then(|(collection, name)| EntityKey::new(collection, name).ok())
                        .is_none_or(|referrer| reached.contains(&referrer))
                });
            Some(Unused {
                entity: key.to_string(),
                project: node.project.clone(),
                file: node
                    .file
                    .as_ref()
                    .map(|path| path.to_string_lossy().replace('\\', "/")),
                named_by_unreached: unreached_by,
                named_weakly_by: reached_by,
            })
        })
        .collect();
    unused.sort_by(|a, b| a.entity.cmp(&b.entity));
    let keep_unmatched = keep
        .iter()
        .filter(|pattern| {
            !index
                .entities()
                .any(|(key, _)| glob(pattern, &key.to_string()) || glob(pattern, key.name()))
        })
        .cloned()
        .collect();
    Report {
        min_confidence: request.min,
        roots: groups
            .into_iter()
            .map(|(reason, mut keys)| {
                keys.sort();
                keys.dedup();
                RootGroup {
                    reason,
                    entities: keys.iter().map(EntityKey::to_string).collect(),
                }
            })
            .collect(),
        reachable: judged.iter().filter(|key| reached.contains(*key)).count(),
        judged: judged.len(),
        unused,
        keep_unmatched,
        complete: index.is_complete(),
        unreadable: index.unreadable().to_vec(),
        unparsed_scripts: index.unparsed_scripts().to_vec(),
        limits: LIMITS,
    }
}

impl Report {
    /// The report as JSON. The entry points are listed only when `detail` asks (a mashup-heavy
    /// solution has hundreds).
    pub fn to_json(&self, detail: bool) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("a report serialises");
        if !detail {
            if let Some(groups) = value["roots"].as_array_mut() {
                for group in groups {
                    if let Some(object) = group.as_object_mut() {
                        let count = object["entities"].as_array().map_or(0, Vec::len);
                        object.remove("entities");
                        object.insert("count".to_string(), count.into());
                    }
                }
            }
        }
        value
    }
}

/// The report as text. A summary lists at most thirty unused entities; `detail` lists all and the
/// entry points.
pub fn render_text(report: &Report, detail: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "unused: {} of {} entities are not reached from any entry point (at {} confidence or stronger)\n",
        report.unused.len(),
        report.judged,
        report.min_confidence.word()
    ));
    out.push_str("  entry points  ");
    let groups: Vec<String> = report
        .roots
        .iter()
        .map(|group| format!("{} {}", group.entities.len(), group.reason))
        .collect();
    out.push_str(&if groups.is_empty() {
        "none".to_string()
    } else {
        groups.join(", ")
    });
    out.push('\n');
    if detail {
        for group in &report.roots {
            out.push_str(&format!("  {}:\n", group.reason));
            for entity in &group.entities {
                out.push_str(&format!("    {entity}\n"));
            }
        }
    }
    if !report.unused.is_empty() {
        out.push('\n');
    }
    let shown = if detail {
        report.unused.len()
    } else {
        30.min(report.unused.len())
    };
    for item in &report.unused[..shown] {
        out.push_str(&format!("  {}  ({})\n", item.entity, item.project));
        if !item.named_by_unreached.is_empty() {
            out.push_str(&format!(
                "      still named by {} (itself unreached)\n",
                item.named_by_unreached.join(", ")
            ));
        }
        if !item.named_weakly_by.is_empty() {
            out.push_str(&format!(
                "      named by {}, by a reference weaker than {}\n",
                item.named_weakly_by.join(", "),
                report.min_confidence.word()
            ));
        }
        if detail {
            if let Some(file) = &item.file {
                out.push_str(&format!("      {file}\n"));
            }
        }
    }
    if shown < report.unused.len() {
        out.push_str(&format!(
            "  ... and {} more (--detail lists all)\n",
            report.unused.len() - shown
        ));
    }
    out.push('\n');
    out.push_str(
        "advisory: nothing is deleted, and a name in this list may be used from outside\n",
    );
    out.push_str(&format!("limits: {}\n", report.limits));
    for pattern in &report.keep_unmatched {
        out.push_str(&format!(
            "note: [unused] keep entry {pattern:?} matches no entity\n"
        ));
    }
    if !report.complete {
        out.push_str(&format!(
            "partial: {} input(s) could not be read, so what they refer to is missing and more may look unused:\n",
            report.unreadable.len()
        ));
        for entry in &report.unreadable {
            if entry.why.is_empty() {
                out.push_str(&format!("  {}\n", entry.what));
            } else {
                out.push_str(&format!("  {}: {}\n", entry.what, entry.why));
            }
        }
    }
    if !report.unparsed_scripts.is_empty() {
        out.push_str(&format!(
            "note: {} script(s) could not be parsed; what they refer to counts only as a review-level mention\n",
            report.unparsed_scripts.len()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    fn bundled() -> Solution {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/acme-orders");
        Solution::load(&root.join("twaco.toml")).unwrap()
    }

    fn unused_in(solution: &Solution, min: Confidence) -> Vec<String> {
        run(
            solution,
            &Request {
                min,
                collection: None,
            },
        )
        .unused
        .into_iter()
        .map(|item| item.entity)
        .collect()
    }

    #[test]
    fn what_no_entry_point_reaches_is_reported_with_what_still_names_it() {
        let report = run(
            &bundled(),
            &Request {
                min: Confidence::Review,
                collection: None,
            },
        );
        let names: Vec<&str> = report
            .unused
            .iter()
            .map(|item| item.entity.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "DataShapes/Acme.Orders.Retired_DS",
                "ThingShapes/Acme.Orders.Legacy_TS"
            ]
        );
        let retired = &report.unused[0];
        assert_eq!(
            retired.named_by_unreached,
            ["ThingShapes/Acme.Orders.Legacy_TS"],
            "a dead cluster names itself"
        );
        assert!(retired.named_weakly_by.is_empty());
        assert_eq!(
            retired.file.as_deref(),
            Some("DataShapes/Acme.Orders.Retired_DS.xml")
        );
        assert_eq!((report.judged, report.reachable), (10, 8));
        assert!(report.complete && report.keep_unmatched.is_empty());
    }

    #[test]
    fn the_entry_points_are_mashups_timers_and_what_keep_names() {
        let report = run(
            &bundled(),
            &Request {
                min: Confidence::Review,
                collection: None,
            },
        );
        let group = |reason: &str| -> Vec<&str> {
            report
                .roots
                .iter()
                .find(|g| g.reason == reason)
                .map(|g| g.entities.iter().map(String::as_str).collect())
                .unwrap_or_default()
        };
        assert_eq!(group("a mashup"), ["Mashups/Acme.Orders.Dashboard"]);
        assert_eq!(group("runs on events"), ["Things/Acme.Orders.Poller"]);
        assert_eq!(
            group("kept by [unused] keep"),
            ["Things/Acme.Orders.Database"]
        );
        // Without the keep rule the database Thing, which only the platform calls, is reported.
        let mut solution = bundled();
        solution.unused.keep.clear();
        assert!(unused_in(&solution, Confidence::Review)
            .contains(&"Things/Acme.Orders.Database".to_string()));
        // A pattern that matches nothing is said so: a typo keeps nothing.
        solution.unused.keep = vec!["Acme.Nope.*".into(), "Things/Acme.Orders.*".into()];
        let report = run(
            &solution,
            &Request {
                min: Confidence::Review,
                collection: None,
            },
        );
        assert_eq!(report.keep_unmatched, ["Acme.Nope.*"]);
        assert!(
            !report
                .unused
                .iter()
                .any(|item| item.entity.starts_with("Things/")),
            "{:?}",
            report.unused
        );
    }

    #[test]
    fn a_stronger_minimum_counts_fewer_references_as_use_and_reports_more() {
        let solution = bundled();
        let any = unused_in(&solution, Confidence::Review);
        let resolved = unused_in(&solution, Confidence::Resolved);
        let structural = unused_in(&solution, Confidence::Structural);
        assert!(
            any.len() <= resolved.len() && resolved.len() < structural.len(),
            "{any:?} {resolved:?} {structural:?}"
        );
        // The mashup binds the Manager only by a resolved reference, so structurally it is unreached.
        assert!(structural.contains(&"Things/Acme.Orders.Manager".to_string()));
        assert!(!resolved.contains(&"Things/Acme.Orders.Manager".to_string()));
    }

    #[test]
    fn a_reference_weaker_than_the_minimum_is_not_called_a_dead_cluster() {
        let report = run(
            &bundled(),
            &Request {
                min: Confidence::Structural,
                collection: Some("Things".into()),
            },
        );
        let manager = report
            .unused
            .iter()
            .find(|item| item.entity == "Things/Acme.Orders.Manager")
            .expect("the mashup binds it only by a resolved reference");
        // The mashup is an entry point, so it is reached; only its reference is too weak.
        assert!(manager.named_by_unreached.is_empty(), "{manager:?}");
        assert_eq!(manager.named_weakly_by, ["Mashups/Acme.Orders.Dashboard"]);
        let audit = report
            .unused
            .iter()
            .find(|item| item.entity == "Things/Acme.Orders.Audit")
            .unwrap();
        assert_eq!(
            audit.named_by_unreached,
            ["Things/Acme.Orders.Manager"],
            "the Manager is itself unreached here"
        );
        let text = render_text(&report, false);
        assert!(
            text.contains("by a reference weaker than structural"),
            "{text}"
        );
        assert!(
            text.contains("still named by Things/Acme.Orders.Manager (itself unreached)"),
            "{text}"
        );
    }

    #[test]
    fn one_collection_can_be_judged_alone_and_the_report_says_what_it_could_not_read() {
        let only = run(
            &bundled(),
            &Request {
                min: Confidence::Review,
                collection: Some("DataShapes".into()),
            },
        );
        assert_eq!(only.judged, 2);
        assert_eq!(only.unused.len(), 1);
        let text = render_text(&only, false);
        assert!(
            text.starts_with("unused: 1 of 2 entities are not reached"),
            "{text}"
        );
        assert!(text.contains("advisory: nothing is deleted"), "{text}");
        let compact = only.to_json(false);
        assert!(
            compact["roots"][0].get("entities").is_none()
                && compact["roots"][0]["count"].is_number()
        );
        assert!(only.to_json(true)["roots"][0]["entities"].is_array());
    }

    #[test]
    fn a_star_matches_any_run_and_nothing_else_does() {
        assert!(glob("Acme.Orders.Manager", "Acme.Orders.Manager"));
        assert!(!glob("Acme.Orders.Manager", "Acme.Orders.Manager2"));
        assert!(glob("Acme.*", "Acme.Orders.Manager"));
        assert!(glob("Things/Acme.*.Manager", "Things/Acme.Orders.Manager"));
        assert!(glob("*Manager", "Acme.Orders.Manager"));
        assert!(glob("*", "anything"));
        assert!(!glob("Acme.*.Manager", "Acme.Orders.Audit"));
        assert!(!glob("Things/*", "ThingShapes/Acme.X"));
        assert!(glob("A*B*C", "A--B--C") && !glob("A*B*C", "A--C--B"));
    }
}
