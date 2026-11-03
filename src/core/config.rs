//! The solution: one or more ThingWorx projects in one repository.
//!
//! A repository is rarely one project for long. Work gets split into a backend block and a UI
//! block, or a shared library and the applications that consume it, and a tool that models only
//! a single project is useless the day that happens. So the unit here is a *solution*, and a
//! one-project repository is the one-element case rather than a separate mode.

use super::codes::{Coded, ErrorCode};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// The file a solution is described by, looked for from the working directory upwards.
pub const CONFIG_FILE: &str = "twaco.toml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Solution {
    /// Parts compared and transferred according to the server's ownership.
    #[serde(default)]
    pub(crate) server_owned: Vec<super::server_owned::Rule>,
    /// Committed exceptions for the named server profiles.  Profile credentials remain in their
    /// normal untracked files; this table only describes how the shared source differs there.
    #[serde(default)]
    pub(crate) profiles: BTreeMap<String, ProfileOverrides>,
    #[serde(default)]
    pub solution: SolutionMeta,
    /// One entry per ThingWorx project. A single-project repository declares one.
    #[serde(default, rename = "project")]
    pub projects: Vec<Project>,
    /// Java extension projects whose declared entities are available to this solution.
    #[serde(default, rename = "extension")]
    pub(crate) extensions: Vec<Extension>,
    #[serde(default)]
    pub(crate) bundle: Bundle,
    /// Project-specific gates, run by `twaco check` alongside the built-in ones.
    #[serde(default, rename = "check")]
    pub(crate) checks: Vec<Check>,
    #[serde(default)]
    pub(crate) validate: Validate,
    /// What `twaco unused` treats as in use without anything referring to it.
    #[serde(default)]
    pub(crate) unused: Unused,
    /// What `adopt` should not report as a change. See [`Adopt`].
    #[serde(default)]
    pub(crate) adopt: Adopt,
    /// Services the team declares read-only. See [`Call`].
    #[serde(default)]
    pub(crate) call: Call,
    /// Solution-wide output conventions for generated content.
    #[serde(default)]
    pub(crate) format: Format,
    /// Which of the built-in gates a plain `twaco check` runs. See [`Gates`].
    #[serde(default)]
    pub gates: Gates,
    /// Per-rule script lint policy. Its absence retains the historical checks-v1 behaviour.
    #[serde(default)]
    pub(crate) lint: Option<Lint>,
    /// TypeScript compiler override used by `twaco types --check`.
    #[serde(default)]
    pub(crate) types: Types,
    /// Which ThingWorx Platform help `twaco help` reads, when not the server's own version.
    #[serde(default)]
    pub(crate) help: Help,
    /// Where file repositories' trees are kept: `[repositories] root = "filerepository"`.
    #[serde(default)]
    pub(crate) repositories: Repositories,
    /// Where localization table exports live: `[localization] root = "localization"`.
    #[serde(default)]
    pub(crate) localization: Localization,
    /// What `twaco package extension` writes into each `metadata.xml`.
    #[serde(default)]
    pub(crate) package: Package,
    /// The solution's own markdown that `twaco guide` reads besides its built-in topics.
    #[serde(default)]
    pub(crate) knowledge: Knowledge,
    /// The test users and the cases `twaco test` runs as them. See [`Test`].
    #[serde(default)]
    pub(crate) test: Test,
    #[serde(skip)]
    pub root: PathBuf,
}

/// A source checkout of a Java ThingWorx extension.  It is read-only input, never workspace
/// source: its entities inform analysis but no command discovers it as something to edit.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Extension {
    pub(crate) root: String,
    pub(crate) name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileOverrides {
    #[serde(default)]
    pub(crate) server_owned: Vec<super::server_owned::Rule>,
    #[serde(default)]
    pub(crate) patches: Vec<Patch>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Patch {
    pub(crate) entity: String,
    pub(crate) path: String,
    pub(crate) value: toml::Value,
}

/// How generated content is laid out inside entity XML.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Format {
    /// Preserve the legacy, XML-aligned script layout instead of writing scripts flush left.
    #[serde(default)]
    pub(crate) indent_cdata_payload: bool,
    /// Formatter choices shared by every service script in the solution.
    #[serde(default)]
    pub(crate) script: ScriptFormat,
}

/// Formatter choices whose defaults retain the established service-script layout.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ScriptFormat {
    pub(crate) indent_width: u8,
    pub(crate) line_width: u32,
    pub(crate) quote_style: QuoteStyle,
}

impl Default for ScriptFormat {
    fn default() -> Self {
        Self {
            indent_width: 4,
            line_width: 100_000,
            quote_style: QuoteStyle::Double,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum QuoteStyle {
    Double,
    Single,
}

/// Decisions a project has already made about a designer's export.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Adopt {
    /// Node paths never reported as changes, as `Collection/Entity/node/path` globs: `*` stays
    /// within one path step and `**` crosses them. For values that differ on every export by
    /// design, such as live property values or a password encrypted with the designer's key.
    #[serde(default)]
    pub(crate) ignore_paths: Vec<String>,
    /// `Entity.Service` names whose body is generated here, so a difference from the export is
    /// expected and the repository is authoritative.
    #[serde(default)]
    pub(crate) generated_services: Vec<String>,
}

/// `[call]`: how MCP `call` treats services.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Call {
    /// `Collection/Name.Service` entries (the name may be a glob) that only read, so MCP `call`
    /// runs them without `dry_run: false`. For the repository's own services, which the built-in
    /// platform list never covers.
    #[serde(default)]
    pub(crate) read_only: Vec<String>,
}

/// `[gates]`: named for the built-in gates, since `[[check]]` is the hook list.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gates {
    /// Parse every script on the server on every `check`, as `check --live` does. The gate fails
    /// closed, so a project that sets this needs a reachable server for `check` to pass.
    /// `deploy` parses on the server whatever this says.
    #[serde(default)]
    pub live: bool,
    /// Built-in gates that report without failing the run, for a solution adopting twaco over
    /// code written before these gates existed. A gate that cannot run still fails.
    #[serde(default)]
    pub(crate) advisory: Vec<String>,
    /// An export schema used by the offline structure check. It is deliberately a path: the
    /// vendor schema is not redistributed with a solution or with twaco.
    #[serde(default)]
    pub(crate) structure_xsd: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Severity {
    Error,
    Warn,
    Off,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Lint {
    #[serde(default)]
    pub(crate) extends: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) rules: BTreeMap<String, LintRule>,
    #[serde(default, rename = "override")]
    pub(crate) overrides: Vec<LintOverride>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LintOverride {
    pub(crate) entity: String,
    #[serde(default)]
    pub(crate) service: Option<String>,
    #[serde(default)]
    pub(crate) rules: BTreeMap<String, LintRule>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LintRule {
    pub(crate) severity: Option<Severity>,
    #[serde(flatten)]
    pub(crate) options: toml::Table,
}

/// Rule severities resolved once for a check run. Overrides remain ordered deliberately: the
/// last matching selector wins, without a hidden specificity rule.
pub(crate) struct EffectiveLint<'a> {
    base: BTreeMap<&'static str, Severity>,
    overrides: &'a [LintOverride],
}

impl<'a> EffectiveLint<'a> {
    pub(crate) fn severity(&self, id: &str, entity: Option<&str>, service: &str) -> Severity {
        let mut severity = self.base.get(id).copied().unwrap_or(Severity::Off);
        let Some(entity) = entity else {
            return severity;
        };
        for override_ in self.overrides {
            if super::glob::matches(&override_.entity, entity)
                && override_
                    .service
                    .as_ref()
                    .is_none_or(|pattern| super::glob::matches(pattern, service))
            {
                if let Some(rule) = override_.rules.get(id).and_then(|rule| rule.severity) {
                    severity = rule;
                }
            }
        }
        severity
    }

    /// Whether any rule of the gate can report, here or for an entity an override names.
    pub(crate) fn gate_enabled(&self, gate: &str) -> bool {
        super::rules::RULES
            .iter()
            .filter(|rule| rule.gate == gate)
            .any(|rule| {
                self.base.get(rule.id) != Some(&Severity::Off)
                    || self.overrides.iter().any(|override_| {
                        override_
                            .rules
                            .get(rule.id)
                            .and_then(|configured| configured.severity)
                            .is_some_and(|severity| severity != Severity::Off)
                    })
            })
    }
}

/// Optional TypeScript compiler command. Arguments before twaco's `-p` are allowed.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Types {
    pub(crate) tsc: Option<Vec<String>>,
}

/// `[help] version = "10.1"`: the help center version, instead of the server's.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Help {
    pub(crate) version: Option<String>,
}

/// `[repositories] root`: the folder holding one subfolder per FileRepository.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Repositories {
    pub(crate) root: Option<String>,
}

/// Where localization table exports live.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Localization {
    pub(crate) root: Option<String>,
}

/// `[knowledge] paths`: files or folders of markdown, relative to the solution. Unset, it is
/// AGENTS.md, CLAUDE.md and docs/, where they exist.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Knowledge {
    pub(crate) paths: Option<Vec<String>>,
}

/// `[test]` and `[[test.user]]`: the accounts `twaco users seed` creates on a test server and
/// `twaco test` signs in as, and where the case files are. Their one password is the profile's
/// `test_password`, never this file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Test {
    #[serde(default, rename = "user")]
    pub(crate) users: Vec<TestUser>,
    /// The project the users are created in. Unset, they are in none.
    pub(crate) project: Option<String>,
    /// The folder of case files (`*.toml`), relative to the solution. Default `tests`.
    pub(crate) cases: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TestUser {
    pub(crate) name: String,
    /// The groups the user belongs to: the roles a case runs as.
    #[serde(default)]
    pub(crate) groups: Vec<String>,
}

/// `[package]`: an extension package's metadata. Unset, the version is 1.0.0, the group the
/// solution's name and the minimum ThingWorx 9.0.0.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Package {
    pub(crate) version: Option<String>,
    pub(crate) group: Option<String>,
    pub(crate) vendor: Option<String>,
    pub(crate) minimum_thingworx: Option<String>,
}

/// What the project validator should tolerate.
/// Entities `twaco unused` must never report, because something outside the repository uses them.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Unused {
    /// Names or `Collection/Name` patterns, `*` standing for any text: `Acme.Orders.Database`,
    /// `Things/Acme.Orders.Api.*`. A database connection Thing that the platform calls, an entity
    /// a REST client uses, anything reached from outside, belongs here.
    #[serde(default)]
    pub(crate) keep: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Validate {
    /// Services implemented locally with the definition inherited from a shape or template.
    /// Legitimate, and impossible to tell from a mistake without being told.
    #[serde(default)]
    pub(crate) inherited_overrides: Vec<String>,
}

/// An external command run as part of the gate.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Check {
    pub(crate) name: String,
    /// The program and its arguments. Run from the solution root.
    pub(crate) command: Vec<String>,
    /// Whether a non-zero exit fails the gate. A check may report without blocking.
    #[serde(default = "yes")]
    pub(crate) gate: bool,
    /// Credentials are withheld unless a check says it needs them.
    #[serde(default)]
    pub(crate) needs_credentials: bool,
    #[serde(default = "default_timeout")]
    pub(crate) timeout_seconds: u64,
}

fn yes() -> bool {
    true
}

fn default_timeout() -> u64 {
    120
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolutionMeta {
    #[serde(default)]
    pub name: String,
    /// Where sidecars live, relative to the solution root.
    #[serde(default = "default_src")]
    pub(crate) src: String,
    #[serde(default = "default_dist")]
    pub(crate) dist: String,
}

/// Hand-written rather than derived. `#[serde(default)]` on the `solution` field builds the
/// whole struct with `Default::default()` when the table is absent, which skips every per-field
/// serde default -- and an empty `src` would quietly resolve sidecars to the solution root.
impl Default for SolutionMeta {
    fn default() -> Self {
        SolutionMeta {
            name: String::new(),
            src: default_src(),
            dist: default_dist(),
        }
    }
}

fn default_src() -> String {
    "src".to_string()
}

fn default_dist() -> String {
    "dist".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// The ThingWorx `projectName` this project's entities carry.
    pub name: String,
    /// Where this project's collection folders live, relative to the solution root.
    #[serde(default = "dot")]
    pub(crate) root: String,
    /// Collection folders belonging to this project. Empty means "discover what is there".
    #[serde(default)]
    pub(crate) collections: Vec<String>,
    /// Projects that must be imported before this one.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Calls made after this project's imported entities have been read back successfully.
    #[serde(default)]
    pub(crate) deploy: Deploy,
    /// Token-name prefixes this project owns in shared localization tables.
    #[serde(default)]
    pub(crate) localization: ProjectLocalization,
}

/// Localization token namespaces owned by one project.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectLocalization {
    #[serde(default)]
    pub(crate) prefixes: Vec<String>,
}

/// Optional post-import behaviour for one project.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Deploy {
    /// The project is a Solution Framework building block: after its import, deploy runs the
    /// block's deploy step as the framework does (see `deploy::framework`).
    #[serde(default)]
    pub(crate) framework: bool,
    #[serde(default)]
    pub(crate) entry_point_thing: Option<String>,
    #[serde(default)]
    pub(crate) deploy_service: Option<String>,
    #[serde(default)]
    pub(crate) deploy_parameters: Option<toml::Table>,
    #[serde(default)]
    pub(crate) post_import: Vec<PostImport>,
}

/// The service a building block's deploy step calls.
pub(crate) const DEPLOY_COMPONENT: &str = "DeployComponent";

impl Deploy {
    /// The project's deploy hook: the Thing and the service to call after its import. A
    /// building block's defaults to its entry point's `DeployComponent`.
    pub(crate) fn hook(&self, project: &str) -> Option<(String, String)> {
        if self.framework {
            return Some((
                self.entry_point_thing
                    .clone()
                    .unwrap_or_else(|| format!("{project}.EntryPoint")),
                DEPLOY_COMPONENT.to_string(),
            ));
        }
        self.entry_point_thing
            .clone()
            .zip(self.deploy_service.clone())
    }
}

/// One configured service to run after the project's deploy service.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PostImport {
    pub(crate) thing: String,
    pub(crate) service: String,
    #[serde(default)]
    pub(crate) parameters: Option<toml::Table>,
    /// A qualified `Collection/Name`; defaults to `Things/<thing>`.
    #[serde(default)]
    pub(crate) target: Option<String>,
}

fn dot() -> String {
    ".".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bundle {
    /// Filename for the whole-solution document.
    #[serde(default = "default_bundle_name")]
    pub(crate) name: String,
    /// Filename for the backend-only document.
    #[serde(default = "default_backend_name")]
    pub(crate) backend_name: String,
    /// Collections a designer owns in Composer, excluded by a backend-only build.
    #[serde(default)]
    pub(crate) ui_collections: Vec<String>,
}

impl Default for Bundle {
    fn default() -> Self {
        Bundle {
            name: default_bundle_name(),
            backend_name: default_backend_name(),
            ui_collections: Vec::new(),
        }
    }
}

fn default_bundle_name() -> String {
    "bundle.xml".to_string()
}

fn default_backend_name() -> String {
    "bundle.backend.xml".to_string()
}

#[derive(Debug)]
pub enum ConfigError {
    NotFound {
        from: PathBuf,
    },
    Unreadable {
        path: PathBuf,
        why: String,
    },
    Invalid {
        path: PathBuf,
        why: String,
    },
    NoProjects {
        path: PathBuf,
    },
    DuplicateProject {
        name: String,
    },
    BlankProjectName,
    BlankCheckName,
    EmptyCheckCommand {
        name: String,
    },
    EmptyTypesCompiler,
    /// A path that would reach outside the solution, which nothing here has a reason to do.
    EscapingPath {
        what: &'static str,
        value: String,
    },
    UnknownDependency {
        project: String,
        missing: String,
    },
    DependencyCycle {
        names: Vec<String>,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NotFound { from } => write!(
                f,
                "no {CONFIG_FILE} found in {} or any parent directory",
                from.display()
            ),
            ConfigError::Unreadable { path, why } => write!(f, "cannot read {}: {why}", path.display()),
            ConfigError::Invalid { path, why } => write!(f, "{}: {why}", path.display()),
            ConfigError::NoProjects { path } => write!(
                f,
                "{} declares no [[project]]; a solution needs at least one",
                path.display()
            ),
            ConfigError::DuplicateProject { name } => {
                write!(f, "two projects are both named {name}")
            }
            ConfigError::BlankProjectName => write!(f, "a project has a blank name"),
            ConfigError::BlankCheckName => write!(f, "a [[check]] has a blank name"),
            ConfigError::EmptyCheckCommand { name } => {
                write!(f, "check {name} declares an empty command")
            }
            ConfigError::EmptyTypesCompiler => write!(f, "[types] tsc declares an empty command"),
            ConfigError::EscapingPath { what, value } => write!(
                f,
                "{what} is {value:?}, which leaves the solution directory; paths must stay inside it"
            ),
            ConfigError::UnknownDependency { project, missing } => write!(
                f,
                "project {project} depends on {missing}, which this solution does not declare"
            ),
            ConfigError::DependencyCycle { names } => {
                write!(f, "projects depend on each other in a cycle: {}", names.join(" -> "))
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Refuse a configured path that is absolute or climbs out of the solution.
///
/// Everything twaco reads and writes belongs to the repository it was pointed at. A `..` or a
/// drive letter in a committed config file is either a mistake or someone using the tool to
/// reach somewhere it has no business being.
fn check_contained(what: &'static str, value: &str) -> Result<(), ConfigError> {
    let path = Path::new(value);
    let escapes = path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        || value.contains(':');
    if escapes {
        return Err(ConfigError::EscapingPath {
            what,
            value: value.to_string(),
        });
    }
    Ok(())
}

impl Solution {
    /// `path` as `/`-separated text, relative to the solution root when it is under it: the
    /// form every report and tool result shows.
    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Find and load the solution containing `start`, searching upwards for `twaco.toml`.
    pub fn discover(start: &Path) -> Result<Solution, ConfigError> {
        let mut dir = Some(start);
        while let Some(here) = dir {
            let candidate = here.join(CONFIG_FILE);
            if candidate.is_file() {
                return Solution::load(&candidate);
            }
            dir = here.parent();
        }
        Err(ConfigError::NotFound {
            from: start.to_path_buf(),
        })
    }

    /// A solution at `root` with no `twaco.toml` yet: what `new project` starts from in an
    /// empty folder, writing the file as it creates the project.
    pub fn bare(root: &Path) -> Solution {
        let mut solution: Solution = toml::from_str("").expect("an empty config parses");
        solution.root = root.to_path_buf();
        solution
    }

    pub fn load(path: &Path) -> Result<Solution, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Unreadable {
            path: path.to_path_buf(),
            why: e.to_string(),
        })?;
        let mut solution: Solution = toml::from_str(&text).map_err(|e| ConfigError::Invalid {
            path: path.to_path_buf(),
            why: e.to_string(),
        })?;
        solution.root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        solution.validate(path)?;
        let rule = super::server_owned::Rule::entry_point_state(
            solution
                .projects
                .iter()
                .map(|project| project.name.as_str()),
        );
        solution.server_owned.push(rule);
        Ok(solution)
    }

    fn validate(&self, path: &Path) -> Result<(), ConfigError> {
        if self.projects.is_empty() {
            return Err(ConfigError::NoProjects {
                path: path.to_path_buf(),
            });
        }
        check_contained("solution.src", &self.solution.src)?;
        let script = &self.format.script;
        if !(2..=8).contains(&script.indent_width) {
            return Err(ConfigError::Invalid {
                path: path.to_path_buf(),
                why: format!(
                    "format.script.indent_width = {} must be from 2 to 8",
                    script.indent_width
                ),
            });
        }
        if !(40..=100_000).contains(&script.line_width) {
            return Err(ConfigError::Invalid {
                path: path.to_path_buf(),
                why: format!(
                    "format.script.line_width = {} must be from 40 to 100000",
                    script.line_width
                ),
            });
        }
        for gate in &self.gates.advisory {
            if !super::check::BUILT_IN_GATES.contains(&gate.as_str()) {
                return Err(ConfigError::Invalid {
                    path: path.to_path_buf(),
                    why: format!(
                        "gates.advisory names {gate:?}, which is not a gate; the gates are {}",
                        super::check::BUILT_IN_GATES.join(", ")
                    ),
                });
            }
        }
        check_contained("solution.dist", &self.solution.dist)?;
        // An extension root is only ever read, so it may sit beside the solution
        // (`../Acme_Extension`), unlike the paths twaco writes to.
        let mut seen = BTreeSet::new();
        for project in &self.projects {
            if project.name.trim().is_empty() {
                return Err(ConfigError::BlankProjectName);
            }
            check_contained("project.root", &project.root)?;
            for collection in &project.collections {
                // Checked too: a collection is joined onto the project root and then written
                // to, so `../outside/Things` would reach past the repository.
                check_contained("project.collections", collection)?;
            }
            if !seen.insert(project.name.as_str()) {
                return Err(ConfigError::DuplicateProject {
                    name: project.name.clone(),
                });
            }
        }
        for project in &self.projects {
            for needed in &project.depends_on {
                if !seen.contains(needed.as_str()) {
                    return Err(ConfigError::UnknownDependency {
                        project: project.name.clone(),
                        missing: needed.clone(),
                    });
                }
            }
        }
        for project in &self.projects {
            let deploy = &project.deploy;
            if deploy.framework
                && deploy
                    .deploy_service
                    .as_deref()
                    .is_some_and(|service| service != DEPLOY_COMPONENT)
            {
                return Err(ConfigError::Invalid {
                    path: path.to_path_buf(),
                    why: format!(
                        "project {}: [project.deploy] framework = true runs the building block's {DEPLOY_COMPONENT}, so deploy_service may only be that",
                        project.name
                    ),
                });
            }
            let not_config = deploy
                .deploy_parameters
                .as_ref()
                .is_some_and(|table| table.keys().any(|key| key != "deploymentConfig"));
            if deploy.framework && not_config {
                return Err(ConfigError::Invalid {
                    path: path.to_path_buf(),
                    why: format!(
                        "project {}: with framework = true, deploy_parameters holds only deploymentConfig, the block's deploy parameters",
                        project.name
                    ),
                });
            }
        }
        for check in &self.checks {
            if check.name.trim().is_empty() {
                return Err(ConfigError::BlankCheckName);
            }
            if check.command.is_empty() {
                return Err(ConfigError::EmptyCheckCommand {
                    name: check.name.clone(),
                });
            }
        }
        self.validate_lint(path)?;
        if self.types.tsc.as_ref().is_some_and(Vec::is_empty) {
            return Err(ConfigError::EmptyTypesCompiler);
        }
        for entry in &self.call.read_only {
            super::read_only::check_entry(entry).map_err(|why| ConfigError::Invalid {
                path: path.to_path_buf(),
                why,
            })?;
        }
        for (name, overrides) in &self.profiles {
            super::profile::validate_name(name).map_err(|error| ConfigError::Invalid {
                path: path.to_path_buf(),
                why: error.to_string(),
            })?;
            for patch in &overrides.patches {
                let valid_entity =
                    patch
                        .entity
                        .split_once('/')
                        .is_some_and(|(collection, name)| {
                            collection == "Mashups"
                                && super::entity::key::EntityKey::new(collection, name).is_ok()
                        });
                if !valid_entity {
                    return Err(ConfigError::Invalid {
                        path: path.to_path_buf(),
                        why: format!("invalid patch entity {:?}; use Mashups/Name", patch.entity),
                    });
                }
                if patch.path.is_empty() || patch.path.split('.').any(str::is_empty) {
                    return Err(ConfigError::Invalid {
                        path: path.to_path_buf(),
                        why: format!(
                            "patch for {} has invalid path {:?}; use dotted JSON object keys",
                            patch.entity, patch.path
                        ),
                    });
                }
            }
        }
        self.deploy_order().map(|_| ())
    }

    fn validate_lint(&self, path: &Path) -> Result<(), ConfigError> {
        // Without [lint] the gates are today's, and so is every hook name that loaded before.
        let Some(lint) = &self.lint else {
            return Ok(());
        };
        let extends: Vec<&str> = lint.extends.as_ref().map_or_else(
            || vec!["checks-v1"],
            |values| values.iter().map(String::as_str).collect(),
        );
        if lint.extends.as_ref().is_some_and(Vec::is_empty) {
            return Err(ConfigError::Invalid {
                path: path.to_path_buf(),
                why: "lint.extends must not be empty".to_string(),
            });
        }
        for preset in extends {
            if !super::rules::PRESETS.contains(&preset) {
                return Err(ConfigError::Invalid {
                    path: path.to_path_buf(),
                    why: format!("unknown lint preset {preset:?}"),
                });
            }
        }
        let validate_rules = |rules: &BTreeMap<String, LintRule>| -> Result<(), ConfigError> {
            for (id, rule) in rules {
                if super::rules::by_id(id).is_none() {
                    return Err(ConfigError::Invalid {
                        path: path.to_path_buf(),
                        why: format!("unknown lint rule {id:?}"),
                    });
                }
                if let Some(key) = rule.options.keys().next() {
                    return Err(ConfigError::Invalid {
                        path: path.to_path_buf(),
                        why: format!("rule {id} takes no option {key}"),
                    });
                }
            }
            Ok(())
        };
        validate_rules(&lint.rules)?;
        let mut selectors = BTreeSet::new();
        for override_ in &lint.overrides {
            if override_.entity.trim().is_empty() {
                return Err(ConfigError::Invalid {
                    path: path.to_path_buf(),
                    why: "lint override needs a non-empty entity".to_string(),
                });
            }
            let selector = (
                override_.entity.as_str(),
                override_.service.as_deref().unwrap_or(""),
            );
            if !selectors.insert(selector) {
                return Err(ConfigError::Invalid {
                    path: path.to_path_buf(),
                    why: format!(
                        "two lint overrides use the same entity and service selectors: {:?}, {:?}",
                        selector.0, selector.1
                    ),
                });
            }
            validate_rules(&override_.rules)?;
        }
        let effective = self.effective_lint();
        for check in &self.checks {
            if super::rules::LINT_GATE_LABELS.contains(&check.name.as_str())
                && effective.gate_enabled(&check.name)
            {
                return Err(ConfigError::Invalid {
                    path: path.to_path_buf(),
                    why: format!("[[check]] {:?} has the name of a built-in gate that [lint] enables; rename or remove the hook", check.name),
                });
            }
        }
        Ok(())
    }

    pub(crate) fn effective_lint(&self) -> EffectiveLint<'_> {
        let mut base = super::rules::RULES
            .iter()
            .map(|rule| (rule.id, Severity::Off))
            .collect::<BTreeMap<_, _>>();
        if let Some(lint) = &self.lint {
            let presets: Vec<&str> = lint.extends.as_ref().map_or_else(
                || vec!["checks-v1"],
                |values| values.iter().map(String::as_str).collect(),
            );
            for preset in presets {
                for rule in super::rules::RULES {
                    base.insert(
                        rule.id,
                        super::rules::preset(preset, rule).expect("validated lint preset"),
                    );
                }
            }
            for (id, rule) in &lint.rules {
                if let Some(severity) = rule.severity {
                    base.insert(
                        super::rules::by_id(id).expect("validated lint rule").id,
                        severity,
                    );
                }
            }
            EffectiveLint {
                base,
                overrides: &lint.overrides,
            }
        } else {
            for rule in super::rules::RULES {
                base.insert(rule.id, Severity::Error);
            }
            EffectiveLint {
                base,
                overrides: &[],
            }
        }
    }

    pub(crate) fn lint_doctor_detail(&self) -> String {
        let Some(lint) = &self.lint else {
            return "checks-v1 (default: no [lint]); house-v1 adds the comment rules".to_string();
        };
        let presets = lint
            .extends
            .as_ref()
            .map_or_else(|| "checks-v1".to_string(), |values| values.join(", "));
        let mut detail = format!("{presets} (from [lint])");
        if !lint.rules.is_empty() || !lint.overrides.is_empty() {
            detail.push_str(&format!(
                ", {} rule(s) changed by [lint.rules], {} override(s)",
                lint.rules.len(),
                lint.overrides.len()
            ));
        }
        detail
    }

    /// The profile's patches that name no Mashup among `entities`. A misspelled patch would
    /// otherwise send the shared value to that server without a word.
    pub(crate) fn unknown_patches(
        &self,
        profile: &str,
        entities: &[super::workspace::EntityFile],
    ) -> Vec<&str> {
        self.patches_for(profile)
            .iter()
            .filter(|patch| {
                !entities.iter().any(|entity| {
                    entity.info.collection == "Mashups"
                        && format!("Mashups/{}", entity.info.name) == patch.entity
                })
            })
            .map(|patch| patch.entity.as_str())
            .collect()
    }

    /// Rules applying to `profile`: the shared policy followed by its committed additions.
    pub(crate) fn server_owned_for(&self, profile: &str) -> Vec<super::server_owned::Rule> {
        let mut rules = self.server_owned.clone();
        if let Some(overrides) = self.profiles.get(profile) {
            rules.extend(overrides.server_owned.clone());
        }
        rules
    }

    pub(crate) fn patches_for(&self, profile: &str) -> &[Patch] {
        self.profiles
            .get(profile)
            .map_or(&[], |overrides| &overrides.patches)
    }

    pub fn src_root(&self) -> PathBuf {
        self.root.join(&self.solution.src)
    }

    /// The configured file-repository root, when the solution keeps one.
    pub(crate) fn repositories_root(&self) -> Option<PathBuf> {
        self.repositories
            .root
            .as_ref()
            .map(|root| self.root.join(root))
    }

    /// The configured localization root, or its conventional location.
    pub(crate) fn localization_root(&self) -> PathBuf {
        self.root.join(
            self.localization
                .root
                .as_deref()
                .unwrap_or(super::localization::DEFAULT_ROOT),
        )
    }

    pub fn project(&self, name: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.name == name)
    }

    /// Where one project's entity collection folders live.
    pub fn project_root(&self, project: &Project) -> PathBuf {
        self.root.join(&project.root)
    }

    /// Projects in the order they must be imported.
    ///
    /// A UI project binding a backend service has to import after it, so this is a topological
    /// sort over `depends_on`. A cycle is reported rather than resolved: there is no correct
    /// order, and picking one would produce an import that fails for a reason nobody can see.
    pub fn deploy_order(&self) -> Result<Vec<&Project>, ConfigError> {
        let mut pending: BTreeMap<&str, &Project> =
            self.projects.iter().map(|p| (p.name.as_str(), p)).collect();
        let mut done: BTreeSet<&str> = BTreeSet::new();
        let mut order: Vec<&Project> = Vec::new();

        while !pending.is_empty() {
            // Ready means every dependency is already placed. BTreeMap keeps this deterministic.
            let ready: Vec<&str> = pending
                .iter()
                .filter(|(_, p)| p.depends_on.iter().all(|d| done.contains(d.as_str())))
                .map(|(name, _)| *name)
                .collect();
            if ready.is_empty() {
                let mut names: Vec<String> = pending.keys().map(|n| n.to_string()).collect();
                names.sort();
                return Err(ConfigError::DependencyCycle { names });
            }
            for name in ready {
                let project = pending.remove(name).expect("name came from pending");
                done.insert(project.name.as_str());
                order.push(project);
            }
        }
        Ok(order)
    }
}

impl Coded for ConfigError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound { .. }
            | Self::NoProjects { .. }
            | Self::DuplicateProject { .. }
            | Self::BlankProjectName
            | Self::BlankCheckName
            | Self::EmptyCheckCommand { .. }
            | Self::EmptyTypesCompiler
            | Self::EscapingPath { .. }
            | Self::UnknownDependency { .. }
            | Self::DependencyCycle { .. }
            | Self::Invalid { .. } => ErrorCode::InvalidData,
            Self::Unreadable { .. } => ErrorCode::IoError,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solution(toml_text: &str) -> Result<Solution, ConfigError> {
        let mut s: Solution = toml::from_str(toml_text).map_err(|e| ConfigError::Invalid {
            path: PathBuf::from("x"),
            why: e.to_string(),
        })?;
        s.root = PathBuf::from(".");
        s.validate(Path::new("twaco.toml"))?;
        Ok(s)
    }

    #[test]
    fn lint_presets_rules_and_overrides_resolve_in_order() {
        let configured = solution(
            r#"[lint]
extends = ["traps-only-v1", "checks-v1"]
[lint.rules."traps.for-in"]
severity = "warn"
[[lint.override]]
entity = "Things/P.*"
[lint.override.rules."traps.for-in"]
severity = "off"
[[lint.override]]
entity = "Things/P.*"
service = "Import*"
[lint.override.rules."traps.for-in"]
severity = "error"

[[project]]
name = "P"
"#,
        )
        .unwrap();
        let lint = configured.effective_lint();
        assert_eq!(
            lint.severity("format.unformatted", None, "Run"),
            Severity::Error
        );
        assert_eq!(
            lint.severity("traps.for-in", Some("Things/Else"), "Run"),
            Severity::Warn
        );
        assert_eq!(
            lint.severity("traps.for-in", Some("Things/P.Legacy"), "Run"),
            Severity::Off
        );
        assert_eq!(
            lint.severity("traps.for-in", Some("Things/P.Legacy"), "ImportOld"),
            Severity::Error
        );
        let traps_only =
            solution("[lint]\nextends = [\"traps-only-v1\"]\n[[project]]\nname = \"P\"\n").unwrap();
        assert_eq!(
            traps_only
                .effective_lint()
                .severity("format.unformatted", None, ""),
            Severity::Off
        );
    }

    #[test]
    fn lint_validation_names_every_bad_configuration() {
        for (text, expected) in [
            ("[lint]\nextends = []\n", "extends"),
            ("[lint]\nextends = [\"missing\"]\n", "unknown lint preset"),
            ("[lint.rules.\"nope\"]\nseverity = \"error\"\n", "unknown lint rule"),
            ("[lint.rules.\"traps.for-in\"]\nlimit = 1\n", "takes no option limit"),
            ("[[lint.override]]\nentity = \"\"\n", "non-empty entity"),
            ("[[lint.override]]\nentity = \"Things/P.*\"\n[[lint.override]]\nentity = \"Things/P.*\"\n", "same entity and service"),
        ] {
            let error = solution(&format!("{text}\n[[project]]\nname = \"P\"\n"))
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn lint_hook_names_clash_only_when_enabled() {
        let error = solution("[lint]\nextends = [\"checks-v1\"]\n[[check]]\nname = \"formatting\"\ncommand = [\"x\"]\n[[project]]\nname = \"P\"\n").unwrap_err().to_string();
        assert!(error.contains("has the name of a built-in gate"));
        solution("[lint]\nextends = [\"traps-only-v1\"]\n[[check]]\nname = \"formatting\"\ncommand = [\"x\"]\n[[check]]\nname = \"comments\"\ncommand = [\"x\"]\n[[project]]\nname = \"P\"\n").unwrap();
        // No [lint]: today's gates, and a hook that loaded before still loads.
        solution(
            "[[check]]
name = \"formatting\"
command = [\"x\"]
[[project]]
name = \"P\"
",
        )
        .unwrap();
        // An override that turns a rule on for one entity enables its gate.
        let error = solution(
            "[lint]
extends = [\"traps-only-v1\"]
[[lint.override]]
entity = \"Things/P.*\"
[lint.override.rules.\"format.unformatted\"]
severity = \"warn\"
[[check]]
name = \"formatting\"
command = [\"x\"]
[[project]]
name = \"P\"
",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("has the name of a built-in gate"), "{error}");
    }

    #[test]
    fn a_misspelled_key_is_refused_and_named_rather_than_ignored() {
        // An ignored `[gatez]` or `depends_onn` would run fewer gates or deploy out of order
        // without a word, so every table refuses what it does not declare.
        for (text, key) in [
            (
                "[[project]]\nname = \"P\"\ndepends_onn = [\"X\"]\n",
                "depends_onn",
            ),
            ("[[project]]\nname = \"P\"\n[gatez]\nlive = true\n", "gatez"),
            ("[[project]]\nname = \"P\"\n[gates]\nlve = true\n", "lve"),
            (
                "[[project]]\nname = \"P\"\n[project.deploy]\nentry_point = \"T\"\n",
                "entry_point",
            ),
        ] {
            let error = solution(text).unwrap_err().to_string();
            assert!(error.contains(key), "{key}: {error}");
        }
        // Free-form tables stay free-form.
        solution(
            "[[project]]\nname = \"P\"\n[project.deploy]\ndeploy_parameters = { anything = 1 }\n",
        )
        .unwrap();
    }

    #[test]
    fn profile_overrides_add_rules_and_validate_patches() {
        let text = r#"[[project]]
name = "P"
[[server_owned]]
entities = ["Things/P.*"]
parts = ["property-values"]
[profiles.qa]
[[profiles.qa.server_owned]]
entities = ["Groups/P.Users"]
parts = ["members"]
[[profiles.qa.patches]]
entity = "Mashups/P.Main"
path = "UI.Properties.Master"
value = "P.Qa"
"#;
        let configured = solution(text).unwrap();
        assert_eq!(configured.server_owned_for("qa").len(), 2);
        assert_eq!(configured.server_owned_for("other").len(), 1);
        assert_eq!(configured.patches_for("qa").len(), 1);
        for bad in [
            "[profiles.'../qa']\n",
            "[profiles.qa]\nunknown = 1\n",
            "[profiles.qa]\n[[profiles.qa.patches]]\nentity='Things/P.T'\npath='A'\nvalue=1\n",
        ] {
            let error = solution(&format!("[[project]]\nname='P'\n{bad}"))
                .unwrap_err()
                .to_string();
            assert!(!error.is_empty());
        }
    }

    #[test]
    fn a_patch_naming_no_mashup_in_the_workspace_is_found() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("twaco.toml"),
            "[[project]]\nname='P'\n[profiles.qa]\n[[profiles.qa.patches]]\nentity='Mashups/P.Missing'\npath='UI.Properties.Master'\nvalue='qa'\n",
        )
        .unwrap();
        let solution = Solution::load(&root.path().join("twaco.toml")).unwrap();
        let found = super::super::workspace::discover(&solution);
        assert_eq!(
            solution.unknown_patches("qa", &found.entities),
            ["Mashups/P.Missing"]
        );
        assert!(solution
            .unknown_patches("default", &found.entities)
            .is_empty());
    }

    #[test]
    fn discover_finds_the_solution_from_a_subfolder() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("src/P/services/Run");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.path().join(CONFIG_FILE), "[[project]]\nname = 'P'\n").unwrap();
        assert_eq!(Solution::discover(&nested).unwrap().root, root.path());
    }

    #[test]
    fn a_single_project_solution_needs_no_ceremony() {
        let s = solution("[[project]]\nname = \"Only\"\n").unwrap();
        assert_eq!(s.projects.len(), 1);
        assert_eq!(s.solution.src, "src");
        assert_eq!(s.project_root(&s.projects[0]), PathBuf::from("./."));
        let order: Vec<&str> = s
            .deploy_order()
            .unwrap()
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(order, vec!["Only"]);
        assert_eq!(s.projects[0].deploy, Deploy::default());
    }

    #[test]
    fn project_deploy_configuration_accepts_nested_parameters_and_targets() {
        let s = solution(
            r#"[[project]]
name = "P"

[project.deploy]
entry_point_thing = "P.Entry"
deploy_service = "Deploy"
deploy_parameters = { deploymentConfig = { password = "${profile:database_password}" } }
post_import = [
  { thing = "P.Seed", service = "Seed" },
  { thing = "ignored", target = "Resources/P.Resource", service = "Run", parameters = { n = 2 } }
]
"#,
        )
        .unwrap();
        let deploy = &s.projects[0].deploy;
        assert_eq!(deploy.entry_point_thing.as_deref(), Some("P.Entry"));
        assert_eq!(deploy.deploy_service.as_deref(), Some("Deploy"));
        assert_eq!(
            deploy.deploy_parameters.as_ref().unwrap()["deploymentConfig"]["password"].as_str(),
            Some("${profile:database_password}")
        );
        assert_eq!(
            deploy.post_import[1].target.as_deref(),
            Some("Resources/P.Resource")
        );
        assert_eq!(
            deploy.post_import[1].parameters.as_ref().unwrap()["n"].as_integer(),
            Some(2)
        );
    }

    #[test]
    fn an_absent_solution_table_still_gets_the_real_defaults() {
        // Regression: a derived Default gave src = "", so sidecars resolved to the root.
        let s = solution(
            "[[project]]
name = \"Only\"
",
        )
        .unwrap();
        assert_eq!(s.solution.src, "src");
        assert_eq!(s.solution.dist, "dist");
    }

    #[test]
    fn cdata_payloads_are_flush_left_by_default_and_can_be_indented_solution_wide() {
        let default = solution("[[project]]\nname = \"Only\"\n").unwrap();
        assert!(!default.format.indent_cdata_payload);

        let indented =
            solution("[format]\nindent_cdata_payload = true\n\n[[project]]\nname = \"Only\"\n")
                .unwrap();
        assert!(indented.format.indent_cdata_payload);
    }

    #[test]
    fn script_formatting_defaults_parses_and_refuses_invalid_values() {
        let default = solution("[[project]]\nname = \"Only\"\n").unwrap();
        assert_eq!(default.format.script.indent_width, 4);
        assert_eq!(default.format.script.line_width, 100_000);
        assert_eq!(default.format.script.quote_style, QuoteStyle::Double);

        let configured = solution(
            "[format.script]\nindent_width = 2\nline_width = 80\nquote_style = \"single\"\n\n[[project]]\nname = \"Only\"\n",
        )
        .unwrap();
        assert_eq!(configured.format.script.indent_width, 2);
        assert_eq!(configured.format.script.line_width, 80);
        assert_eq!(configured.format.script.quote_style, QuoteStyle::Single);

        for (key, value, range) in [
            ("indent_width", "1", "from 2 to 8"),
            ("indent_width", "9", "from 2 to 8"),
            ("line_width", "39", "from 40 to 100000"),
            ("line_width", "100001", "from 40 to 100000"),
        ] {
            let error = solution(&format!(
                "[format.script]\n{key} = {value}\n\n[[project]]\nname = \"Only\"\n"
            ))
            .unwrap_err()
            .to_string();
            assert!(error.contains(key), "{error}");
            assert!(error.contains(value), "{error}");
            assert!(error.contains(range), "{error}");
        }

        for (key, value) in [("unknown", "true"), ("quote_style", "\"other\"")] {
            let error = solution(&format!(
                "[format.script]\n{key} = {value}\n\n[[project]]\nname = \"Only\"\n"
            ))
            .unwrap_err()
            .to_string();
            assert!(error.contains(key), "{error}");
            assert!(error.contains(value.trim_matches('\"')), "{error}");
        }
    }

    #[test]
    fn localization_configuration_parses_at_solution_and_project_scope() {
        let s = solution(
            "[localization]\nroot = \"translations\"\n\n[[project]]\nname = \"Acme.App\"\n[project.localization]\nprefixes = [\"Acme.App.\"]\n",
        )
        .unwrap();
        assert_eq!(s.localization.root.as_deref(), Some("translations"));
        assert_eq!(
            s.projects[0].localization.prefixes,
            vec!["Acme.App.".to_string()]
        );
    }

    #[test]
    fn the_live_gate_is_off_unless_the_project_asks_for_it() {
        let off: Solution = toml::from_str("[[project]]\nname = \"P\"\n").unwrap();
        assert!(
            !off.gates.live,
            "the offline loop must stay credential-free by default"
        );
        let on: Solution =
            toml::from_str("[gates]\nlive = true\n\n[[project]]\nname = \"P\"\n").unwrap();
        assert!(on.gates.live);
    }

    #[test]
    fn a_dependency_orders_the_deploy() {
        let s = solution(
            "[[project]]\nname = \"UI\"\ndepends_on = [\"Backend\"]\n\n[[project]]\nname = \"Backend\"\n",
        )
        .unwrap();
        let order: Vec<&str> = s
            .deploy_order()
            .unwrap()
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            order,
            vec!["Backend", "UI"],
            "a dependency must import first"
        );
    }

    #[test]
    fn a_cycle_is_reported_not_resolved() {
        let s = solution(
            "[[project]]\nname = \"A\"\ndepends_on = [\"B\"]\n\n[[project]]\nname = \"B\"\ndepends_on = [\"A\"]\n",
        );
        assert!(matches!(s, Err(ConfigError::DependencyCycle { .. })));
    }

    #[test]
    fn a_dependency_on_something_undeclared_is_refused() {
        let s = solution("[[project]]\nname = \"UI\"\ndepends_on = [\"Ghost\"]\n");
        assert!(matches!(s, Err(ConfigError::UnknownDependency { .. })));
    }

    #[test]
    fn two_projects_may_not_share_a_name() {
        let s = solution("[[project]]\nname = \"Same\"\n\n[[project]]\nname = \"Same\"\n");
        assert!(matches!(s, Err(ConfigError::DuplicateProject { .. })));
    }

    #[test]
    fn a_path_leaving_the_solution_is_refused() {
        let mut cases = vec![
            "[[project]]\nname = \"P\"\nroot = \"../outside\"\n",
            "[solution]\nsrc = \"../elsewhere\"\n[[project]]\nname = \"P\"\n",
        ];
        // A drive path leaves the solution on Windows; elsewhere it is an ordinary file name.
        // A TOML literal string, so its backslash is not an escape.
        if cfg!(windows) {
            cases.push("[[project]]\nname = \"P\"\nroot = 'C:\\outside'\n");
        }
        for bad in cases {
            assert!(
                matches!(solution(bad), Err(ConfigError::EscapingPath { .. })),
                "should have refused {bad:?}"
            );
        }
    }

    #[test]
    fn a_check_needs_a_name_and_a_command() {
        let no_command = "[[project]]\nname = \"P\"\n[[check]]\nname = \"c\"\ncommand = []\n";
        assert!(matches!(
            solution(no_command),
            Err(ConfigError::EmptyCheckCommand { .. })
        ));
        let no_name = "[[project]]\nname = \"P\"\n[[check]]\nname = \" \"\ncommand = [\"x\"]\n";
        assert!(matches!(
            solution(no_name),
            Err(ConfigError::BlankCheckName)
        ));
    }

    #[test]
    fn a_check_gates_and_withholds_credentials_by_default() {
        let text = "[[project]]\nname = \"P\"\n[[check]]\nname = \"c\"\ncommand = [\"x\"]\n";
        let s = solution(text).unwrap();
        assert!(
            s.checks[0].gate,
            "a declared check blocks unless it says otherwise"
        );
        assert!(
            !s.checks[0].needs_credentials,
            "credentials are withheld unless asked for"
        );
        assert_eq!(s.checks[0].timeout_seconds, 120);
    }

    #[test]
    fn a_types_compiler_command_must_not_be_empty() {
        let empty = "[[project]]\nname = \"P\"\n[types]\ntsc = []\n";
        assert!(matches!(
            solution(empty),
            Err(ConfigError::EmptyTypesCompiler)
        ));

        let configured =
            solution("[[project]]\nname = \"P\"\n[types]\ntsc = [\"node\", \"tools/tsc.js\"]\n")
                .unwrap();
        assert_eq!(
            configured.types.tsc.as_deref(),
            Some(["node".to_string(), "tools/tsc.js".to_string()].as_slice())
        );
    }

    #[test]
    fn a_collection_leaving_the_solution_is_refused() {
        let bad = "[[project]]\nname = \"P\"\ncollections = [\"../outside/Things\"]\n";
        assert!(matches!(
            solution(bad),
            Err(ConfigError::EscapingPath { .. })
        ));
    }

    #[test]
    fn a_blank_project_name_is_refused() {
        assert!(matches!(
            solution(
                "[[project]]
name = \"  \"
"
            ),
            Err(ConfigError::BlankProjectName)
        ));
    }

    /// A building block's deploy hook is its entry point's DeployComponent unless named.
    #[test]
    fn a_framework_deploy_defaults_to_the_entry_point_and_takes_only_deployment_config() {
        let s = solution(
            "[[project]]
name = \"Acme.B\"
[project.deploy]
framework = true
",
        )
        .unwrap();
        assert_eq!(
            s.projects[0].deploy.hook("Acme.B"),
            Some((
                "Acme.B.EntryPoint".to_string(),
                "DeployComponent".to_string()
            ))
        );
        let refused = |deploy: &str| match solution(&format!(
            "[[project]]
name = \"Acme.B\"
[project.deploy]
framework = true
{deploy}"
        )) {
            Err(ConfigError::Invalid { why, .. }) => why,
            other => panic!("{deploy}: {other:?}"),
        };
        assert!(refused(
            "deploy_service = \"Other\"
"
        )
        .contains("may only be that"));
        assert!(refused(
            "deploy_parameters = { other = 1 }
"
        )
        .contains("holds only deploymentConfig"));
        assert!(solution(
            "[[project]]
name = \"Acme.B\"
[project.deploy]
framework = true
deploy_service = \"DeployComponent\"
deploy_parameters = { deploymentConfig = { key = \"${profile:key}\" } }
"
        )
        .is_ok());
    }

    #[test]
    fn a_solution_with_no_projects_is_refused() {
        let s = solution("[solution]\nname = \"Empty\"\n");
        assert!(matches!(s, Err(ConfigError::NoProjects { .. })));
    }

    #[test]
    fn ordering_is_deterministic_for_independent_projects() {
        let text = "[[project]]\nname = \"Zeta\"\n\n[[project]]\nname = \"Alpha\"\n";
        let first: Vec<String> = solution(text)
            .unwrap()
            .deploy_order()
            .unwrap()
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let second: Vec<String> = solution(text)
            .unwrap()
            .deploy_order()
            .unwrap()
            .iter()
            .map(|p| p.name.clone())
            .collect();
        assert_eq!(first, second);
    }
}
