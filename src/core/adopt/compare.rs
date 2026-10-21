//! The comparison itself: each entity and service body of an export against the repository, and
//! against a base when one is known.

use super::base::{history_versions, resolve};
use super::classify;
use super::flatten::{attribute, canonical, flatten, glob_matches, is_volatile_id, service_bodies};
use super::read::{entity_from_bytes, export_entities, repository_entities};
use super::{AdoptError, Difference, EntityRef, EntityReport, Report, ServiceReport, Status};
use super::{Change, Kind, Side};
use crate::core::config::Solution;
use crate::core::normalise::{self, Element};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
/// Extra inputs to the three-way comparison. The old `compare` entry point, which the tests
/// still use, keeps its two-way-shaped signature.
#[derive(Debug, Default)]
pub(crate) struct CompareOptions {
    pub(crate) base: Option<String>,
    pub(crate) only_kind: Option<Kind>,
}

/// Compare an export with the solution. `only` retains its established name-fragment meaning.
#[cfg(test)]
pub(crate) fn compare(
    solution: &Solution,
    export: &Path,
    only: &[String],
) -> Result<Report, AdoptError> {
    compare_with(solution, export, only, &CompareOptions::default())
}

/// Compare an export using an explicit base and/or collection kind filter.
pub(crate) fn compare_with(
    solution: &Solution,
    export: &Path,
    only: &[String],
    options: &CompareOptions,
) -> Result<Report, AdoptError> {
    let exported = export_entities(export)?;
    let export_side = Side::from_export(export)?;
    let here = repository_entities(solution)?;
    let ours = Side::from_solution(solution)?;
    let base = resolve(solution, options.base.as_deref(), &export_side)?;
    let mut report = Report {
        base: base.as_ref().map(|base| base.label.clone()),
        base_side: base.as_ref().map(|base| base.side.clone()),
        ..Report::default()
    };

    let generated: BTreeSet<&str> = solution
        .adopt
        .generated_services
        .iter()
        .map(String::as_str)
        .collect();
    let kind_passes = |entity: &EntityRef| {
        options
            .only_kind
            .is_none_or(|kind| kind == kind_of(solution, entity))
    };
    // `only` narrows the entity comparison, not the service check: a revert anywhere in the
    // export matters whichever entity was asked about.
    let passes = |entity: &EntityRef| {
        kind_passes(entity)
            && (only.is_empty()
                || only
                    .iter()
                    .any(|fragment| entity.name.contains(fragment.as_str())))
    };

    for (entity, element) in &exported {
        if !kind_passes(entity) {
            continue;
        }
        for (service, body) in service_bodies(element) {
            let label = format!("{}.{service}", entity.name);
            let sidecar = solution
                .src_root()
                .join(&entity.name)
                .join("services")
                .join(&service)
                .join("script.js");
            let key = (entity.name.clone(), service.clone());
            let mine = ours.services.get(&key);
            let (source, from_sidecar) = if sidecar.is_file() {
                (sidecar, true)
            } else if let Some((_, path)) = here.get(entity) {
                (path.clone(), false)
            } else {
                report.unmatched_services.push(label);
                continue;
            };
            let base_service = base.as_ref().and_then(|base| base.side.services.get(&key));
            let change = match mine {
                Some(mine) => script_change(&body, mine, base_service, || {
                    history_versions(solution, &source, 50).iter().any(|old| {
                        if from_sidecar {
                            std::str::from_utf8(old)
                                .is_ok_and(|old| canonical(&body) == canonical(old))
                        } else {
                            // Without a sidecar the history is the entity file's: its script.
                            entity_from_bytes(old, entity).is_some_and(|old| {
                                service_bodies(&old)
                                    .get(&service)
                                    .is_some_and(|old| canonical(&body) == canonical(old))
                            })
                        }
                    })
                }),
                None => {
                    if base_service.is_some() {
                        Change::WeRemoved
                    } else {
                        Change::Added
                    }
                }
            };
            // Only a difference is reported, as before the three-way states existed.
            if change == Change::Same {
                continue;
            }
            report.services.push(ServiceReport {
                entity: entity.name.clone(),
                service,
                source,
                generated: generated.contains(label.as_str()),
                sidecar: from_sidecar,
                change,
            });
        }
    }

    for (entity, element) in &exported {
        if !passes(entity) {
            continue;
        }
        let project = attribute(element, "projectName").unwrap_or_default();
        let kind = kind_of(solution, entity);
        match here.get(entity) {
            None => report.entities.push(EntityReport {
                entity: entity.clone(),
                project,
                status: Status::New,
                differences: Vec::new(),
                volatile_ids: 0,
                ignored: 0,
                kind,
                change: if base
                    .as_ref()
                    .is_some_and(|base| base.side.entities.contains_key(entity))
                {
                    Change::WeRemoved
                } else {
                    Change::Added
                },
            }),
            Some((repo_element, _)) => {
                let mut compared = compare_owned_entity(solution, entity, element, repo_element);
                compared.project = project;
                compared.kind = kind;
                compared.change = entity_change(
                    solution,
                    entity,
                    element,
                    repo_element,
                    base.as_ref()
                        .and_then(|base| base.side.entities.get(entity)),
                    here.get(entity).map(|(_, path)| path.as_path()),
                );
                report.entities.push(compared);
            }
        }
    }

    let collections: BTreeSet<&str> = exported.keys().map(|e| e.collection.as_str()).collect();
    report.absent = here
        .keys()
        .filter(|e| {
            !exported.contains_key(*e) && collections.contains(e.collection.as_str()) && passes(e)
        })
        .cloned()
        .collect();
    let retypes = super::retype::retypes(solution, &report);
    let (arguments, defaults) = super::retype::report(solution, &retypes, &exported)?;
    report.retyped_arguments = arguments;
    report.retyped_defaults = defaults;
    Ok(report)
}

fn script_change(
    theirs: &str,
    ours: &str,
    base: Option<&String>,
    history_stale: impl FnOnce() -> bool,
) -> Change {
    if canonical(theirs) == canonical(ours) {
        Change::Same
    } else if let Some(base) = base {
        classify::three_way(
            canonical(theirs) == canonical(base),
            canonical(ours) == canonical(base),
        )
    } else if history_stale() {
        Change::Stale
    } else {
        Change::Unknown
    }
}

fn entity_change(
    solution: &Solution,
    entity: &EntityRef,
    theirs: &Element,
    ours: &Element,
    base: Option<&Element>,
    source: Option<&Path>,
) -> Change {
    let same = |left: &Element, right: &Element| {
        compare_owned_entity(solution, entity, left, right)
            .differences
            .is_empty()
    };
    if same(theirs, ours) {
        Change::Same
    } else if let Some(base) = base {
        classify::three_way(same(theirs, base), same(ours, base))
    } else if source.is_some_and(|source| {
        history_versions(solution, source, 50)
            .iter()
            .any(|bytes| entity_from_bytes(bytes, entity).is_some_and(|old| same(theirs, &old)))
    }) {
        Change::Stale
    } else {
        Change::Unknown
    }
}

pub(super) fn compare_owned_entity(
    solution: &Solution,
    entity: &EntityRef,
    left: &Element,
    right: &Element,
) -> EntityReport {
    let parts =
        super::super::server_owned::parts(&solution.server_owned, &entity.collection, &entity.name);
    if parts.is_empty() {
        return compare_entity(entity, left, right, &solution.adopt.ignore_paths);
    }
    let mut left = normalise::comparison_element(left.clone(), true);
    let mut right = normalise::comparison_element(right.clone(), true);
    super::super::server_owned::omit(&mut left, &parts);
    super::super::server_owned::omit(&mut right, &parts);
    compare_entity(entity, &left, &right, &solution.adopt.ignore_paths)
}
fn kind_of(solution: &Solution, entity: &EntityRef) -> Kind {
    const UI: &[&str] = &[
        "Mashups",
        "MediaEntities",
        "StyleThemes",
        "StyleDefinitions",
        "StateDefinitions",
        "Menus",
        "Dashboards",
    ];
    if UI.contains(&entity.collection.as_str())
        || solution
            .bundle
            .ui_collections
            .iter()
            .any(|name| name == &entity.collection)
    {
        Kind::Ui
    } else {
        Kind::Backend
    }
}

fn compare_entity(
    entity: &EntityRef,
    export: &Element,
    repo: &Element,
    ignore: &[String],
) -> EntityReport {
    let (left, right) = (flatten(export), flatten(repo));
    let mut report = EntityReport {
        entity: entity.clone(),
        project: String::new(),
        status: Status::Identical,
        differences: Vec::new(),
        volatile_ids: 0,
        ignored: 0,
        kind: Kind::Backend,
        change: Change::Unknown,
    };
    let entity_path = entity.path();
    let paths: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
    for path in paths {
        let (export_value, repo_value) = (left.get(path), right.get(path));
        if export_value == repo_value {
            continue;
        }
        if is_volatile_id(path) {
            report.volatile_ids += 1;
            continue;
        }
        let full = format!("{entity_path}{path}");
        if ignore.iter().any(|pattern| glob_matches(pattern, &full)) {
            report.ignored += 1;
            continue;
        }
        report.differences.push(Difference {
            path: path.clone(),
            export: export_value.cloned(),
            repo: repo_value.cloned(),
        });
    }
    if !report.differences.is_empty() {
        report.status = Status::Changed;
    }
    report
}
pub(super) fn same_frame(left: &Element, right: &Element) -> bool {
    let frame = |element: &Element| {
        flatten(element)
            .into_iter()
            .filter(|(path, _)| {
                !path.contains("/ServiceDefinitions") && !path.contains("/ServiceImplementations")
            })
            .collect::<BTreeMap<_, _>>()
    };
    frame(left) == frame(right)
}
