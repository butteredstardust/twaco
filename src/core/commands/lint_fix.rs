//! The command policy around `lint fix-headers`: bring service headers to their definitions.

use super::{lock_workspace, Access, Effects, Mode, Notices};
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::{Severity, Solution};
use crate::core::{catalog, check, header, lint_report, lock, types, workspace};
use std::path::Path;

/// The arguments that affect a header repair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixHeadersRequest {
    pub mode: Mode,
    /// A `Collection/Name` glob, as `twaco lint --entity` takes it.
    pub entity: Option<String>,
    /// A service folder glob.
    pub service: Option<String>,
    pub lock_label: &'static str,
}

/// What the repair did, or would do, to one script.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FixedScript {
    pub file: String,
    pub changes: Vec<String>,
    pub not_fixable: Vec<String>,
}

#[derive(Debug)]
pub struct FixHeadersOutcome {
    /// Every selected script with something to change or to report, in file order.
    pub scripts: Vec<FixedScript>,
    /// How many selected scripts were read.
    pub examined: usize,
    /// Scripts that could not be read or written, with the reason.
    pub failed: Vec<String>,
    #[allow(
        dead_code,
        reason = "the command facade's Effects (COMMAND_FACADE.md); read only by tests"
    )]
    effects: Effects,
}

impl FixHeadersOutcome {
    /// How many scripts change.
    pub fn changed(&self) -> usize {
        self.scripts
            .iter()
            .filter(|script| !script.changes.is_empty())
            .count()
    }

    #[cfg(test)]
    pub(crate) const fn effects(&self) -> Effects {
        self.effects
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FixHeadersError {
    #[error("{0}")]
    Lock(lock::LockError),
}

impl Coded for FixHeadersError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Lock(why) => why.code(),
        }
    }
}

/// Repair headers. A plan reads only and takes no lock; an apply writes each changed script
/// atomically, holding the workspace lock. A script whose header contract rule is off, or which
/// has neither a readable local definition nor a resolved inherited definition, is left alone.
pub fn execute(
    solution: &Solution,
    request: &FixHeadersRequest,
    notices: &mut Notices,
) -> Result<FixHeadersOutcome, FixHeadersError> {
    let _lock = match request.mode {
        Mode::Plan => None,
        Mode::Apply => Some(
            lock_workspace(solution, request.lock_label, notices).map_err(FixHeadersError::Lock)?,
        ),
    };
    let discovery = workspace::discover(solution);
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let lint = solution.effective_lint();
    let mut outcome = FixHeadersOutcome {
        scripts: Vec::new(),
        examined: 0,
        failed: Vec::new(),
        effects: Effects::new(
            match request.mode {
                Mode::Plan => Access::Read,
                Mode::Apply => Access::Write,
            },
            Access::None,
        ),
    };
    let mut paths = lint_report::selected_scripts(
        solution,
        &discovery,
        request.entity.as_deref(),
        request.service.as_deref(),
    );
    paths.sort();
    for path in paths {
        let file = check::relative(solution, &path);
        let Some(service) = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
        else {
            continue;
        };
        let entity = check::script_entity(solution, &discovery, &path)
            .map(|(collection, name)| format!("{collection}/{name}"));
        if lint.severity("comments.header.contract", entity.as_deref(), service) == Severity::Off {
            continue;
        }
        let Some(definition) =
            path.parent()
                .map(|folder| folder.join("definition.xml"))
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|source| header::definition(&source))
                .or_else(|| {
                    let (collection, name) = check::script_entity(solution, &discovery, &path)?;
                    let entity = model
                        .entities
                        .iter()
                        .find(|entity| entity.collection == collection && entity.name == name)?;
                    match catalog::inherited_service(
                        entity,
                        &model.entities,
                        platform.as_ref(),
                        service,
                    ) {
                        catalog::InheritedService::Defined(service) => {
                            Some(header::Definition::from_service(&service))
                        }
                        catalog::InheritedService::Absent
                        | catalog::InheritedService::Unresolved => None,
                    }
                })
        else {
            continue;
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) => {
                outcome.failed.push(format!("{file}: {error}"));
                continue;
            }
        };
        outcome.examined += 1;
        let text = workspace::without_bom(&raw);
        let bom = &raw[..raw.len() - text.len()];
        let repair = header::repair(text, service, &definition, lint.header_policy());
        if repair.changes.is_empty() && repair.not_fixable.is_empty() {
            continue;
        }
        if let (Mode::Apply, Some(fixed)) = (request.mode, &repair.fixed) {
            if let Err(error) = workspace::atomic_replace(&path, format!("{bom}{fixed}").as_bytes())
            {
                outcome.failed.push(format!("{file}: {error}"));
                continue;
            }
        }
        outcome.scripts.push(FixedScript {
            file,
            changes: repair.changes,
            not_fixable: repair.not_fixable,
        });
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFINITION: &str = r#"<ServiceDefinition description="" name="Run"><ResultType baseType="NOTHING" name="result" ordinal="0"></ResultType><ParameterDefinitions><FieldDefinition baseType="STRING" description="" name="b" ordinal="2"></FieldDefinition><FieldDefinition baseType="NUMBER" description="" name="a" ordinal="1"></FieldDefinition></ParameterDefinitions></ServiceDefinition>"#;
    const OUT_OF_ORDER: &str = "/**\n * @function Run\n * @param {string} b - second\n * @param {number} a - first\n * @returns {void}\n */\nresult = 1;";

    fn solution(config: &str) -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("twaco.toml"),
            format!("{config}\n[[project]]\nname = \"P\"\n"),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        for name in ["P.A", "P.B"] {
            std::fs::write(
                root.join(format!("Things/{name}.xml")),
                format!("<Entities><Things><Thing name=\"{name}\" projectName=\"P\"></Thing></Things></Entities>"),
            )
            .unwrap();
            let folder = root.join(format!("src/{name}/services/Run"));
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("definition.xml"), DEFINITION).unwrap();
            std::fs::write(folder.join("script.js"), OUT_OF_ORDER).unwrap();
        }
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn request(mode: Mode, entity: Option<&str>) -> FixHeadersRequest {
        FixHeadersRequest {
            mode,
            entity: entity.map(str::to_string),
            service: None,
            lock_label: "lint fix-headers",
        }
    }

    fn script(solution: &Solution, entity: &str) -> String {
        std::fs::read_to_string(
            solution
                .root
                .join(format!("src/{entity}/services/Run/script.js")),
        )
        .unwrap()
    }

    #[test]
    fn a_plan_writes_nothing_and_an_apply_fixes_once() {
        let (_dir, solution) = solution("[lint]\nextends = [\"house-v1\"]\n");
        let plan = execute(
            &solution,
            &request(Mode::Plan, None),
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(plan.changed(), 2);
        assert_eq!(plan.effects(), Effects::new(Access::Read, Access::None));
        assert_eq!(script(&solution, "P.A"), OUT_OF_ORDER);

        let applied = execute(
            &solution,
            &request(Mode::Apply, Some("Things/P.A")),
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(applied.changed(), 1);
        assert_eq!(
            script(&solution, "P.A"),
            "/**\n * @function Run\n * @param {number} a - first\n * @param {string} b - second\n * @returns {void}\n */\nresult = 1;"
        );
        assert_eq!(script(&solution, "P.B"), OUT_OF_ORDER, "the filter held");
        let again = execute(
            &solution,
            &request(Mode::Apply, Some("Things/P.A")),
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(again.changed(), 0);
    }

    #[test]
    fn an_entity_whose_contract_rule_is_off_is_left_alone() {
        let (_dir, solution) = solution(
            "[lint]\nextends = [\"house-v1\"]\n[[lint.override]]\nentity = \"Things/P.B\"\n[lint.override.rules.\"comments.header.contract\"]\nseverity = \"off\"\n",
        );
        let plan = execute(
            &solution,
            &request(Mode::Plan, None),
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(
            plan.scripts
                .iter()
                .map(|script| script.file.as_str())
                .collect::<Vec<_>>(),
            ["src/P.A/services/Run/script.js"]
        );
    }

    #[test]
    fn fixes_an_override_header_from_its_inherited_definition() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("twaco.toml"),
            "[lint]\nextends = [\"house-v1\"]\n\n[[project]]\nname = \"P\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("ThingTemplates")).unwrap();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(
            root.join("ThingTemplates/Base.xml"),
            r#"<Entities><ThingTemplates><ThingTemplate name="Base" projectName="P"><ThingShape><ServiceDefinitions><ServiceDefinition name="Run"><ParameterDefinitions><FieldDefinition name="who" baseType="STRING"/></ParameterDefinitions><ResultType baseType="NOTHING"/></ServiceDefinition></ServiceDefinitions></ThingShape></ThingTemplate></ThingTemplates></Entities>"#,
        )
        .unwrap();
        std::fs::write(
            root.join("Things/Child.xml"),
            r#"<Entities><Things><Thing name="Child" projectName="P" thingTemplate="Base"><ThingShape><ServiceDefinitions/><ServiceImplementations><ServiceImplementation name="Run" handlerName="Script"><ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code><![CDATA[logger.info(who);]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>"#,
        )
        .unwrap();
        let script = root.join("src/Child/services/Run/script.js");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "logger.info(who);\n").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();

        let fixed = execute(
            &solution,
            &request(Mode::Apply, Some("Things/Child")),
            &mut Notices::default(),
        )
        .unwrap();
        assert_eq!(fixed.changed(), 1);
        let text = std::fs::read_to_string(script).unwrap();
        assert!(text.contains("@function Run"), "{text}");
        assert!(text.contains("@param {string} who"), "{text}");
    }

    #[test]
    fn an_apply_needs_the_lock() {
        let (_dir, solution) = solution("[lint]\nextends = [\"house-v1\"]\n");
        let held = lock::acquire(&solution.root, "holder", &[]).unwrap();
        let error = execute(
            &solution,
            &request(Mode::Apply, None),
            &mut Notices::default(),
        )
        .unwrap_err();
        assert!(matches!(error, FixHeadersError::Lock(_)));
        assert_eq!(script(&solution, "P.A"), OUT_OF_ORDER);
        drop(held);
    }
}
