//! Advice that never participates in adopt's write decisions.

use super::flatten::attribute;
use super::{EntityRef, EntityReport, Status};
use crate::core::config::Solution;
use crate::core::normalise::Element;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) fn mashup_names(
    solution: &Solution,
    incoming: &[EntityReport],
    here: &BTreeMap<EntityRef, (Element, PathBuf)>,
) -> Vec<String> {
    let mut names = BTreeMap::new();
    for (entity, (element, path)) in here {
        if entity.collection != "Mashups" {
            continue;
        }
        let project = attribute(element, "projectName")
            .filter(|name| !name.is_empty())
            .or_else(|| layout_project(solution, path));
        if let Some((project, short)) = project.and_then(|p| short_name(&entity.name, p)) {
            names.insert(entity.name.clone(), (project, short, false));
        }
    }
    for entry in incoming {
        if entry.entity.collection != "Mashups" || entry.status != Status::New {
            continue;
        }
        if let Some((project, short)) = short_name(&entry.entity.name, entry.project.clone()) {
            names.insert(entry.entity.name.clone(), (project, short, true));
        }
    }
    let names: Vec<_> = names.into_iter().collect();
    let mut notes = Vec::new();
    for (index, (name, (project, short, new))) in names.iter().enumerate() {
        for (other, (other_project, other_short, other_new)) in &names[index + 1..] {
            if (*new || *other_new)
                && project != other_project
                && short == other_short
                && !name.eq_ignore_ascii_case(other)
            {
                notes.push(format!(
                    "warning: mashups {name} (project {project}) and {other} (project {other_project}) share the short name {short}; adopt continues unchanged"
                ));
            }
        }
    }
    notes
}

fn short_name(name: &str, project: String) -> Option<(String, String)> {
    if project.is_empty() {
        return None;
    }
    let short = name.strip_prefix(&project)?.strip_prefix('.')?;
    if short.is_empty() {
        return None;
    }
    Some((project, short.to_string()))
}

fn layout_project(solution: &Solution, path: &Path) -> Option<String> {
    let mut projects: Vec<_> = solution
        .projects
        .iter()
        .filter_map(|project| {
            let root = solution.project_root(project);
            path.starts_with(&root).then_some((root, project))
        })
        .collect();
    projects.sort_by_key(|(root, _)| std::cmp::Reverse(root.components().count()));
    let (root, project) = projects.first()?;
    if projects.get(1).is_some_and(|(other, _)| other == root) {
        return None;
    }
    path.starts_with(root.join("Mashups"))
        .then(|| project.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::adopt::compare::compare;

    fn fixture(existing: &str, incoming: &str) -> (tempfile::TempDir, Solution, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("twaco.toml");
        std::fs::write(
            &config,
            "[[project]]\nname='Acme.Sales'\nroot='sales'\n[[project]]\nname='Acme.Billing'\nroot='billing'\n",
        )
        .unwrap();
        for root in ["sales", "billing"] {
            std::fs::create_dir_all(dir.path().join(root).join("Mashups")).unwrap();
        }
        if !existing.is_empty() {
            std::fs::write(
                dir.path().join("sales/Mashups/Acme.Sales.Orders_MU.xml"),
                existing,
            )
            .unwrap();
        }
        let export = dir.path().join("export.xml");
        std::fs::write(&export, incoming).unwrap();
        let solution = Solution::load(&config).unwrap();
        (dir, solution, export)
    }

    fn mashup(name: &str, project: &str) -> String {
        format!(
            "<Entities><Mashups><Mashup name='{name}' projectName='{project}'/></Mashups></Entities>"
        )
    }

    fn warnings(report: &super::super::Report) -> Vec<&str> {
        report
            .notes
            .iter()
            .filter(|n| n.starts_with("warning:"))
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn a_new_mashup_names_both_projects_and_keeps_its_plan() {
        let (_dir, solution, export) = fixture(
            &mashup("Acme.Sales.Orders_MU", "Acme.Sales"),
            &mashup("Acme.Billing.Orders_MU", "Acme.Billing"),
        );
        let report = compare(&solution, &export, &[]).unwrap();
        assert_eq!(warnings(&report), ["warning: mashups Acme.Billing.Orders_MU (project Acme.Billing) and Acme.Sales.Orders_MU (project Acme.Sales) share the short name Orders_MU; adopt continues unchanged"]);
        assert_eq!(report.entities[0].status, Status::New);
        assert_eq!(report.entities[0].change, super::super::Change::Added);
        assert!(report.entities[0].differences.is_empty());
        let filtered = compare(&solution, &export, &["Other".into()]).unwrap();
        assert!(warnings(&filtered).is_empty());
    }

    #[test]
    fn repository_layout_supplies_a_missing_project_but_never_overrides_one() {
        for (project, expected) in [("", 1), ("Acme.Other", 0)] {
            let (_dir, solution, export) = fixture(
                &mashup("Acme.Sales.Orders_MU", project),
                &mashup("Acme.Billing.Orders_MU", "Acme.Billing"),
            );
            assert_eq!(
                warnings(&compare(&solution, &export, &[]).unwrap()).len(),
                expected
            );
        }
    }

    #[test]
    fn unrelated_names_collections_and_unknown_prefixes_have_no_advisory() {
        for incoming in [
            mashup("Acme.Sales.Orders_MU", "Acme.Sales"),
            mashup("Acme.Billing.Order_MU", "Acme.Billing"),
            mashup("Acme.Other.Orders_MU", "Acme.Billing"),
            mashup("Acme.Billing.Orders_MU", ""),
            mashup("Acme.Billing.Orders_MU", "Acme.Sales"),
            mashup("Acme.Billing.Orders_MU", "Acme.Billing")
                .replace("Mashups", "Things")
                .replace("Mashup", "Thing"),
        ] {
            let (_dir, solution, export) =
                fixture(&mashup("Acme.Sales.Orders_MU", "Acme.Sales"), &incoming);
            assert!(
                warnings(&compare(&solution, &export, &[]).unwrap()).is_empty(),
                "{incoming}"
            );
        }
    }

    #[test]
    fn two_incoming_mashups_warn_once_and_keep_dots_in_the_short_name() {
        let incoming = mashup("Acme.Sales.Admin.Orders_MU", "Acme.Sales").replace(
            "</Mashups>",
            "<Mashup name='Acme.Billing.Admin.Orders_MU' projectName='Acme.Billing'/></Mashups>",
        );
        let (_dir, solution, export) = fixture("", &incoming);
        let report = compare(&solution, &export, &[]).unwrap();
        assert_eq!(warnings(&report).len(), 1);
        assert!(warnings(&report)[0].contains("short name Admin.Orders_MU;"));
    }

    #[test]
    fn layout_supports_collection_subfolders_and_refuses_ambiguous_roots() {
        let (_dir, mut solution, _export) = fixture("", &mashup("Acme.New", "Acme"));
        let path = solution
            .root
            .join("sales/Mashups/nested/Acme.Sales.Orders_MU.xml");
        assert_eq!(
            layout_project(&solution, &path).as_deref(),
            Some("Acme.Sales")
        );
        solution.projects[1].root = "sales".into();
        assert!(layout_project(&solution, &path).is_none());
    }

    #[test]
    fn an_advisory_does_not_change_the_writes() {
        let existing = mashup("Acme.Sales.Orders_MU", "Acme.Sales");
        let incoming = mashup("Acme.Billing.Orders_MU", "Acme.Billing").replace(
            "/>",
            "><ParameterDefinitions></ParameterDefinitions><mashupContent><![CDATA[{\"New\":true}]]></mashupContent></Mashup>",
        );
        let (_dir, solution, export) = fixture(&existing, &incoming);
        let skeleton = "<Entities><Mashups><Mashup\n         name=\"Acme.Billing.Skeleton\"\n         projectName=\"Acme.Billing\">\n            <ParameterDefinitions></ParameterDefinitions><mashupContent><![CDATA[{}]]></mashupContent></Mashup></Mashups></Entities>";
        std::fs::write(
            solution
                .root
                .join("billing/Mashups/Acme.Billing.Skeleton.xml"),
            skeleton,
        )
        .unwrap();
        let mut report = compare(&solution, &export, &[]).unwrap();
        assert_eq!(warnings(&report).len(), 1);
        let lock = crate::core::lock::acquire(&solution.root, "test", &[]).unwrap();
        let outcome = super::super::apply(&solution, &export, &report, &[], &lock).unwrap();
        let target = solution
            .root
            .join("billing/Mashups/Acme.Billing.Orders_MU.xml");
        assert!(target.is_file());
        assert_eq!(
            std::fs::read_to_string(solution.root.join("sales/Mashups/Acme.Sales.Orders_MU.xml"))
                .unwrap(),
            existing
        );
        // Replaying the same plan without advice must produce the same files.
        let before = std::fs::read(&target).unwrap();
        report.notes.clear();
        let repeated = super::super::apply(&solution, &export, &report, &[], &lock).unwrap();
        assert!(outcome
            .lines
            .iter()
            .any(|line| line.starts_with("created  ")));
        assert!(!repeated
            .lines
            .iter()
            .any(|line| line.starts_with("created  ")));
        assert_eq!(std::fs::read(&target).unwrap(), before);
    }
}
