//! The `project dependencies` gate: a Project entity's `dependsOn` should name every project of
//! the solution whose ThingTemplate or ThingShape its own Things and ThingTemplates build on.
//!
//! Verified on ThingWorx 10.2: importing a Thing or ThingTemplate whose template or implemented
//! shape belongs to another project adds that project to its own Project's `dependsOn`, so the
//! Project reads back "not kept". A base DataShape adds nothing. Importing the Project alone keeps
//! what its file declares, so a file that names every such project is read back as sent.

use super::config::Solution;
use super::{normalise, types, workspace};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const GATE: &str = "project dependencies";

/// One project a Project's `dependsOn` lacks, with the first use that needs it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Missing {
    /// The Project entity file, relative to the solution root.
    pub(crate) file: String,
    pub(crate) project: String,
    pub(crate) needs: String,
    /// `<Collection>/<Name> uses <Collection>/<Name> as its template`, or `... implements ...`.
    pub(crate) because: String,
    /// The `dependsOn` entry the server writes for it: `<project>:<packageVersion>`.
    pub(crate) entry: String,
}

/// The Project files examined, every missing dependency, and the files that could not be read.
pub(crate) fn check(
    solution: &Solution,
    discovered: &workspace::Discovery,
    model: &types::Model,
) -> (usize, Vec<Missing>, Vec<(String, String)>) {
    let mut unreadable = Vec::new();
    // Each Project file of the solution: its declared dependencies and its package version.
    let mut projects: BTreeMap<String, (String, BTreeSet<String>, String)> = BTreeMap::new();
    for entry in discovered
        .entities
        .iter()
        .filter(|entry| entry.info.collection == "Projects")
    {
        let file = solution.relative(&entry.path);
        let element = match std::fs::read(&entry.path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| normalise::entity_of(&bytes).map_err(|error| error.to_string()))
        {
            Ok(element) => element,
            Err(why) => {
                unreadable.push((file, why));
                continue;
            }
        };
        let written = element
            .attributes
            .iter()
            .find(|(name, _)| name == b"dependsOn")
            .map(|(_, value)| String::from_utf8_lossy(value).into_owned());
        if let Some(written) = written.filter(|text| !text.trim().is_empty()) {
            if let Err(error) = serde_json::from_str::<serde_json::Value>(&written) {
                unreadable.push((file, format!("its dependsOn is not JSON: {error}")));
                continue;
            }
        }
        let declared = normalise::project_dependencies(&element)
            .into_iter()
            .collect();
        let version = element
            .attributes
            .iter()
            .find(|(name, _)| name == b"packageVersion")
            .map(|(_, value)| String::from_utf8_lossy(value).into_owned())
            .filter(|version| !version.is_empty())
            .unwrap_or_else(|| "1.0.0".to_string());
        projects.insert(entry.info.name.clone(), (file, declared, version));
    }
    // The project the server files each entity in: its document's projectName, which can differ
    // from the folder it sits in (the project gate reports that).
    let project_of: BTreeMap<(&str, &str), &str> = discovered
        .entities
        .iter()
        .filter(|entry| !entry.info.project.is_empty())
        .map(|entry| {
            (
                (entry.info.collection.as_str(), entry.info.name.as_str()),
                entry.info.project.as_str(),
            )
        })
        .collect();
    let project = |entity: &types::Entity| -> String {
        project_of
            .get(&(entity.collection.as_str(), entity.name.as_str()))
            .map_or_else(|| entity.project.clone(), |project| (*project).to_string())
    };
    // Where each of the solution's own templates and shapes lives.
    let owner: BTreeMap<(&str, &str), String> = model
        .entities
        .iter()
        .filter(|entity| entity.extension.is_none())
        .filter(|entity| matches!(entity.collection.as_str(), "ThingTemplates" | "ThingShapes"))
        .map(|entity| {
            (
                (entity.collection.as_str(), entity.name.as_str()),
                project(entity),
            )
        })
        .collect();
    let mut missing: BTreeMap<(String, String), Missing> = BTreeMap::new();
    for entity in model.entities.iter().filter(|entity| {
        entity.extension.is_none()
            && matches!(entity.collection.as_str(), "Things" | "ThingTemplates")
    }) {
        let own = project(entity);
        let Some((file, declared, _)) = projects.get(&own) else {
            continue;
        };
        let uses = entity
            .template
            .iter()
            .map(|template| ("ThingTemplates", template.as_str(), "as its template"))
            .chain(
                entity
                    .shapes
                    .iter()
                    .map(|shape| ("ThingShapes", shape.as_str(), "as a shape it implements")),
            );
        for (collection, name, how) in uses {
            let Some(needs) = owner.get(&(collection, name)).map(String::as_str) else {
                continue;
            };
            if needs.is_empty() || needs == own || declared.contains(needs) {
                continue;
            }
            let version = projects
                .get(needs)
                .map(|(_, _, version)| version.as_str())
                .unwrap_or("1.0.0");
            missing
                .entry((own.clone(), needs.to_string()))
                .or_insert_with(|| Missing {
                    file: file.clone(),
                    project: own.clone(),
                    needs: needs.to_string(),
                    because: format!(
                        "{}/{} uses {collection}/{name} {how}",
                        entity.collection, entity.name
                    ),
                    entry: format!("{needs}:{version}"),
                });
        }
    }
    (projects.len(), missing.into_values().collect(), unreadable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &std::path::Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn project(name: &str, depends: &str) -> String {
        format!(
            "<Entities><Projects><Project name=\"{name}\" projectName=\"{name}\" packageVersion=\"2.1.0\" dependsOn=\"{{&quot;extensions&quot;:&quot;&quot;,&quot;projects&quot;:&quot;{depends}&quot;}}\"/></Projects></Entities>"
        )
    }

    #[test]
    fn a_template_or_shape_of_another_project_must_be_in_dependson() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "twaco.toml",
            "[[project]]\nname = \"A\"\nroot = \"a\"\n[[project]]\nname = \"B\"\nroot = \"b\"\n[[project]]\nname = \"C\"\nroot = \"c\"\n",
        );
        write(root, "a/Projects/A.xml", &project("A", ""));
        write(root, "b/Projects/B.xml", &project("B", ""));
        write(root, "c/Projects/C.xml", &project("C", "A:2.1.0"));
        write(root, "a/ThingTemplates/A.TT.xml", "<Entities><ThingTemplates><ThingTemplate name=\"A.TT\" baseThingTemplate=\"GenericThing\" projectName=\"A\"/></ThingTemplates></Entities>");
        write(root, "a/ThingShapes/A.TS.xml", "<Entities><ThingShapes><ThingShape name=\"A.TS\" projectName=\"A\"/></ThingShapes></Entities>");
        // B needs A twice, and is told once; C declares A already.
        write(root, "b/Things/B.T.xml", "<Entities><Things><Thing name=\"B.T\" thingTemplate=\"A.TT\" projectName=\"B\"><ImplementedShapes><ImplementedShape name=\"A.TS\"/></ImplementedShapes></Thing></Things></Entities>");
        write(root, "c/Things/C.T.xml", "<Entities><Things><Thing name=\"C.T\" thingTemplate=\"A.TT\" projectName=\"C\"/></Things></Entities>");
        // Filed under A but in project C by its projectName, which already declares A.
        write(root, "a/Things/C.Misfiled.xml", "<Entities><Things><Thing name=\"C.Misfiled\" thingTemplate=\"A.TT\" projectName=\"C\"/></Things></Entities>");
        // A platform template is no project of the solution.
        write(root, "a/Things/A.T.xml", "<Entities><Things><Thing name=\"A.T\" thingTemplate=\"GenericThing\" projectName=\"A\"/></Things></Entities>");
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let discovered = workspace::discover(&solution);
        let (model, _) = types::load_model(&solution);
        let (examined, missing, unreadable) = check(&solution, &discovered, &model);
        assert_eq!(examined, 3);
        assert!(unreadable.is_empty(), "{unreadable:?}");
        assert_eq!(
            missing,
            [Missing {
                file: "b/Projects/B.xml".into(),
                project: "B".into(),
                needs: "A".into(),
                because: "Things/B.T uses ThingTemplates/A.TT as its template".into(),
                entry: "A:2.1.0".into(),
            }]
        );

        // A dependsOn that is not JSON is unreadable, not empty.
        write(root, "b/Projects/B.xml", "<Entities><Projects><Project name=\"B\" projectName=\"B\" dependsOn=\"{projects\"/></Projects></Entities>");
        let discovered = workspace::discover(&solution);
        let (_, missing, unreadable) = check(&solution, &discovered, &model);
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(unreadable.len(), 1, "{unreadable:?}");
        assert!(unreadable[0].1.starts_with("its dependsOn is not JSON"));
    }
}
