//! One gate, one exit code.
//!
//! A check either passes or produces findings. Findings are structured — a gate, a file, a line,
//! a rule, a message — so a person reads a list and an agent acts on it without re-reading the
//! file. Structured findings keep large check results compact and machine-readable.

use super::config::Solution;
use super::{fmt as format_js, sync, workspace};
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// One thing wrong, in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub gate: String,
    /// Relative to the solution root where possible, so findings are comparable between machines.
    pub file: String,
    /// 1-based. Zero means the finding is about the file rather than a line in it.
    pub line: usize,
    pub rule: String,
    pub message: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(
                f,
                "{}:{} [{}/{}] {}",
                self.file, self.line, self.gate, self.rule, self.message
            )
        } else {
            write!(
                f,
                "{} [{}/{}] {}",
                self.file, self.gate, self.rule, self.message
            )
        }
    }
}

/// What one gate did.
#[derive(Debug)]
pub struct GateResult {
    pub name: String,
    /// What the gate looked at, for a summary line that means something.
    pub examined: usize,
    pub findings: Vec<Finding>,
    /// Whatever the gate printed that was not a finding. Kept so a passing hook's own summary
    /// -- "123 scripts, 0 failing" -- stays readable rather than being silently discarded.
    pub prose: Vec<String>,
    /// The gate could not run. Distinct from finding something wrong.
    pub broken: Option<String>,
    /// Whether this gate's findings fail the run. A `[[check]]` may report without blocking.
    pub gates_the_run: bool,
}

impl GateResult {
    fn passed(name: &str, examined: usize) -> Self {
        GateResult {
            name: name.to_string(),
            examined,
            findings: Vec::new(),
            prose: Vec::new(),
            broken: None,
            gates_the_run: true,
        }
    }

    /// Whether the gate passed outright.
    pub fn ok(&self) -> bool {
        self.findings.is_empty() && self.broken.is_none()
    }

    /// Whether the gate should fail the run. A non-gating check reports and steps aside; a gate
    /// that could not run always counts, because nobody knows what it would have said.
    pub fn blocks(&self) -> bool {
        self.broken.is_some() || (self.gates_the_run && !self.findings.is_empty())
    }
}

/// Everything the gate found.
#[derive(Debug, Default)]
pub struct CheckReport {
    pub gates: Vec<GateResult>,
}

impl CheckReport {
    pub fn findings(&self) -> usize {
        self.gates.iter().map(|g| g.findings.len()).sum()
    }

    pub fn broken(&self) -> usize {
        self.gates.iter().filter(|g| g.broken.is_some()).count()
    }

    /// Whether every gate passed.
    pub fn ok(&self) -> bool {
        self.gates.iter().all(GateResult::ok)
    }

    /// Whether anything found should fail the run.
    pub fn blocks(&self) -> bool {
        self.gates.iter().any(GateResult::blocks)
    }
}

/// The built-in gates, by the names `[gates] advisory` uses.
pub const BUILT_IN_GATES: &[&str] = &[
    "line endings",
    "sidecars",
    "formatting",
    "script traps",
    "code order",
    "project",
];

/// A built-in gate as the solution wants it: blocking, or advisory when `[gates] advisory`
/// names it.
fn as_configured(solution: &Solution, mut gate: GateResult) -> GateResult {
    if solution.gates.advisory.contains(&gate.name) {
        gate.gates_the_run = false;
    }
    gate
}

/// Run every gate the solution declares, built in and configured alike.
pub fn run(solution: &Solution) -> CheckReport {
    let mut report = CheckReport::default();
    for gate in [
        line_endings(solution),
        sidecars_in_sync(solution),
        formatting(solution),
        script_traps(solution),
        code_order(solution),
        project_validation(solution),
    ] {
        report.gates.push(as_configured(solution, gate));
    }
    for hook in &solution.checks {
        report.gates.push(run_hook(solution, hook));
    }
    report
}

/// The server's own parser, the one gate that needs a server.
///
/// A trait so the gate is testable without one; [`Client`](super::server::Client) is the real
/// one. `Sync` because the scripts are sent in parallel.
pub trait ScriptChecker: Sync {
    fn check_script(
        &self,
        script: &str,
    ) -> Result<super::server::ScriptCheck, super::server::ServerError>;
}

impl ScriptChecker for super::server::Client {
    fn check_script(
        &self,
        script: &str,
    ) -> Result<super::server::ScriptCheck, super::server::ServerError> {
        super::server::Client::check_script(self, script)
    }
}

/// Every service script, parsed by ThingWorx itself (`check --live`).
///
/// Offline lint catches the traps it knows by pattern. This catches everything the platform's
/// Rhino would refuse, which an import will not: it reports success and the service fails at its
/// first call with "No service handler defined".
///
/// **Fails closed**: no profile, or a server that cannot be reached, makes the gate
/// broken, and a broken gate fails the run. An unreachable server must never quietly remove the
/// strongest check there is. `checker` is an `Err` when no client could be built.
///
/// A finding points at the service's `script.js` sidecar when it exists. The sidecar holds the
/// script exactly as extraction reads it, which is what was sent, so the server's line number
/// is that file's line number.
pub fn live_parse(solution: &Solution, checker: Result<&dyn ScriptChecker, String>) -> GateResult {
    as_configured(solution, live_parse_gate(solution, checker))
}

fn live_parse_gate(solution: &Solution, checker: Result<&dyn ScriptChecker, String>) -> GateResult {
    const GATE: &str = "live parse";
    let mut result = GateResult::passed(GATE, 0);
    let checker = match checker {
        Ok(checker) => checker,
        Err(why) => {
            result.broken = Some(why);
            return result;
        }
    };

    struct Script {
        entity_file: PathBuf,
        sidecar: PathBuf,
        service: String,
        source: String,
    }
    let mut scripts = Vec::new();
    for entity in workspace::discover(solution).entities {
        let Ok(bytes) = std::fs::read(&entity.path) else {
            continue;
        };
        // A document that will not parse is the project gate's finding, not this one's.
        let Ok(services) = super::sidecar::script_services(&bytes) else {
            continue;
        };
        let services_dir = workspace::services_dir(solution, &entity);
        for service in services {
            scripts.push(Script {
                entity_file: entity.path.clone(),
                sidecar: services_dir.join(&service.name).join("script.js"),
                service: service.name,
                source: service.script,
            });
        }
    }

    let answers = super::parallel::map(&scripts, |script| checker.check_script(&script.source));
    for (script, answer) in scripts.iter().zip(answers) {
        let checked = match answer {
            Ok(checked) => checked,
            Err(error) => {
                result.broken = Some(format!("the server could not parse scripts: {error}"));
                result.findings.clear();
                return result;
            }
        };
        result.examined += 1;
        if checked.status {
            continue;
        }
        let message = without_embedded_position(&checked.message);
        let finding = if script.sidecar.is_file() {
            Finding {
                gate: GATE.to_string(),
                file: relative(solution, &script.sidecar),
                line: checked.line_number,
                rule: "rhino".to_string(),
                message: format!("column {}: {message}", checked.column_number),
            }
        } else {
            Finding {
                gate: GATE.to_string(),
                file: relative(solution, &script.entity_file),
                line: 0,
                rule: "rhino".to_string(),
                message: format!(
                    "service {} line {} column {}: {message}",
                    script.service, checked.line_number, checked.column_number
                ),
            }
        };
        result.findings.push(finding);
    }
    result
}

/// Drop the position the parser writes into its message. It is one line too high,
/// and printed beside the correct one it would contradict it. The error and its source excerpt
/// stay.
fn without_embedded_position(message: &str) -> String {
    let Some(at) = message.find(" at line ") else {
        return message.to_string();
    };
    match message[at..].find(" source:") {
        Some(source) => format!("{}{}", &message[..at], &message[at + source..]),
        None => message[..at].to_string(),
    }
}

/// Files whose bytes hold both CRLF and lone LF.
///
/// A file with both is a tool's half-rewrite. It stays silent until a later edit that matches on
/// a newline finds nothing.
fn line_endings(solution: &Solution) -> GateResult {
    let mut result = GateResult::passed("line endings", 0);
    for path in text_files(solution) {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        result.examined += 1;
        let crlf = bytes.windows(2).filter(|w| w == b"\r\n").count();
        let lone_lf = bytes.iter().filter(|&&b| b == b'\n').count() - crlf;
        if crlf > 0 && lone_lf > 0 {
            result.findings.push(Finding {
                gate: "line endings".to_string(),
                file: relative(solution, &path),
                line: 0,
                rule: "mixed".to_string(),
                message: format!(
                    "{crlf} CRLF and {lone_lf} LF lines; rewrite it in whichever style it mostly uses"
                ),
            });
        }
    }
    result
}

/// Text files worth checking, by extension, skipping generated and foreign trees, and whatever
/// git ignores: a file nobody commits is not the repository's to fix.
fn text_files(solution: &Solution) -> Vec<PathBuf> {
    const SUFFIXES: &[&str] = &[
        "xml", "js", "json", "md", "toml", "css", "txt", "yml", "yaml",
    ];
    let mut out = Vec::new();
    for path in walk_files(solution) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let generated = ["jsconfig.json", "twaco-globals.d.ts"].contains(&name.as_str())
            && path
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|parent| parent == "services");
        let suffix = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| SUFFIXES.contains(&e.to_ascii_lowercase().as_str()));
        if suffix && !generated {
            out.push(path);
        }
    }
    out
}

/// Every repository file considered by source checks, in stable order.
///
/// The walk honours the solution's `.gitignore` and `.ignore`, skips generated and foreign
/// directory trees, and never follows links.
pub(crate) fn walk_files(solution: &Solution) -> Vec<PathBuf> {
    const SKIP: &[&str] = &[
        ".git",
        ".hg",
        ".svn",
        ".venv",
        "venv",
        "__pycache__",
        ".mypy_cache",
        ".pytest_cache",
        ".idea",
        ".vscode",
        "target",
        "dist",
        "build",
        "out",
        "coverage",
        "vendor",
        "node_modules",
        "distribution-backend",
        // twaco's own state: the baseline, profiles and generated types, among them copies of
        // every script made for `types --check`, which would report a script's endings twice.
        ".twaco",
    ];
    // Symlinks are not followed (the walker's default): one can point out of the solution, or
    // at an ancestor, and the walk would never end. Hidden files are checked like any other.
    let walker = ignore::WalkBuilder::new(&solution.root)
        .hidden(false)
        // .gitignore is honoured before `git init` too, as a later commit would honour it. Only
        // the solution's own ignore files count: not a parent folder's, not the user's global
        // excludes, not .git/info/exclude, none of which travel with the repository.
        .require_git(false)
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .filter_entry(|entry| {
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            !(is_dir
                && entry.depth() > 0
                && SKIP.contains(&entry.file_name().to_string_lossy().as_ref()))
        })
        .build();
    let mut out = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        out.push(entry.into_path());
    }
    out.sort();
    out
}

/// Every repository directory considered by source checks, in stable order.
///
/// This has the same ignore, foreign-tree and link rules as [`walk_files`], and omits the
/// solution root itself.
pub(crate) fn walk_dirs(solution: &Solution) -> Vec<PathBuf> {
    const SKIP: &[&str] = &[
        ".git",
        ".hg",
        ".svn",
        ".venv",
        "venv",
        "__pycache__",
        ".mypy_cache",
        ".pytest_cache",
        ".idea",
        ".vscode",
        "target",
        "dist",
        "build",
        "out",
        "coverage",
        "vendor",
        "node_modules",
        "distribution-backend",
        ".twaco",
    ];
    let walker = ignore::WalkBuilder::new(&solution.root)
        .hidden(false)
        .require_git(false)
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .filter_entry(|entry| {
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            !(is_dir
                && entry.depth() > 0
                && SKIP.contains(&entry.file_name().to_string_lossy().as_ref()))
        })
        .build();
    let mut out = Vec::new();
    for entry in walker.flatten() {
        if entry.depth() == 0 || !entry.file_type().is_some_and(|t| t.is_dir()) {
            continue;
        }
        out.push(entry.into_path());
    }
    out.sort();
    out
}

/// Entity documents that disagree with their sidecars.
fn sidecars_in_sync(solution: &Solution) -> GateResult {
    let mut result = GateResult::passed("sidecars", 0);
    let found = workspace::discover(solution);
    // Entities with a sidecar of any kind. A set, so an entity carrying both services and
    // fields is one thing examined rather than two, and a DataShape is not left out of the
    // count merely because its sidecar is not a services tree.
    let mut examined: BTreeSet<String> = BTreeSet::new();
    for problem in &found.unreadable {
        result.findings.push(Finding {
            gate: "sidecars".to_string(),
            file: problem.split(':').next().unwrap_or(problem).to_string(),
            line: 0,
            rule: "unreadable".to_string(),
            message: problem.clone(),
        });
    }

    for entity in &found.entities {
        let dir = workspace::services_dir(solution, entity);
        // read_sidecars skips a service folder missing either file, so a half-deleted sidecar
        // would otherwise read as no sidecar at all, and the entity as unmanaged.
        for service in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let missing: Vec<&str> = ["definition.xml", "script.js"]
                .into_iter()
                .filter(|file| !service.path().join(file).is_file())
                .collect();
            if service.path().is_dir() && !missing.is_empty() {
                result.findings.push(Finding {
                    gate: "sidecars".to_string(),
                    file: relative(solution, &service.path()),
                    line: 0,
                    rule: "incomplete".to_string(),
                    message: format!("service sidecar without {}", missing.join(" or ")),
                });
            }
        }
        let sidecars = workspace::read_sidecars(&dir);
        if sidecars.is_empty() {
            continue;
        }
        let Ok(src) = std::fs::read(&entity.path) else {
            continue;
        };
        examined.insert(entity.info.name.clone());
        match sync::sync(
            &src,
            &sidecars,
            true,
            solution.format.indent_cdata_payload,
            false,
        ) {
            Ok((out, report)) if out != src => result.findings.push(Finding {
                gate: "sidecars".to_string(),
                file: relative(solution, &entity.path),
                line: 0,
                rule: "out-of-sync".to_string(),
                message: format!(
                    "{} service(s) differ: {}",
                    report.changed.len(),
                    report.changed.join(", ")
                ),
            }),
            Ok(_) => {}
            Err(e) => result.findings.push(Finding {
                gate: "sidecars".to_string(),
                file: relative(solution, &entity.path),
                line: 0,
                rule: "will-not-sync".to_string(),
                message: e.to_string(),
            }),
        }
    }

    // DataShape fields are a sidecar too, and for a persisted shape they are the database's
    // column list, so a drift here matters more than most.
    for entity in &found.entities {
        if entity.info.collection != "DataShapes" {
            continue;
        }
        let path = workspace::fields_path(solution, entity);
        if !path.exists() {
            // Not under management. Nothing to compare, and not a defect.
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                result.findings.push(Finding {
                    gate: "sidecars".to_string(),
                    file: relative(solution, &path),
                    line: 0,
                    rule: "unreadable".to_string(),
                    message: e.to_string(),
                });
                continue;
            }
        };
        let Ok(src) = std::fs::read(&entity.path) else {
            continue;
        };
        examined.insert(entity.info.name.clone());
        match super::datashape::from_sidecar(&text.replace("\r\n", "\n"))
            .and_then(|desired| super::datashape::sync(&src, &desired, true))
        {
            Ok((out, changes)) if out != src => result.findings.push(Finding {
                gate: "sidecars".to_string(),
                file: relative(solution, &entity.path),
                line: 0,
                rule: "fields-out-of-sync".to_string(),
                message: format!("{} field change(s): {}", changes.len(), changes.join(", ")),
            }),
            Ok(_) => {}
            Err(e) => result.findings.push(Finding {
                gate: "sidecars".to_string(),
                file: relative(solution, &path),
                line: 0,
                rule: "fields-will-not-sync".to_string(),
                message: e.to_string(),
            }),
        }
    }

    // A mashup's content and stylesheet, and a DataTable's configuration. Neither is a service
    // tree, so neither is reached by the loop above.
    for entity in &found.entities {
        let Ok(src) = std::fs::read(&entity.path) else {
            continue;
        };

        if entity.info.collection == "Mashups" {
            let dir = workspace::mashup_dir(solution, entity);
            // A sidecar that will not read is a finding, not an absence. Treating it as "no
            // sidecars here" would let the gate pass a mashup nobody can sync.
            let read = match workspace::read_mashup(&dir) {
                Ok(read) => read,
                Err(e) => {
                    result.findings.push(Finding {
                        gate: "sidecars".to_string(),
                        file: relative(solution, &dir),
                        line: 0,
                        rule: "unreadable".to_string(),
                        message: e.to_string(),
                    });
                    None
                }
            };
            if let Some(assets) = read {
                examined.insert(entity.info.name.clone());
                match super::mashup::sync(&src, &assets) {
                    Ok((out, changes)) if out != src => result.findings.push(Finding {
                        gate: "sidecars".to_string(),
                        file: relative(solution, &entity.path),
                        line: 0,
                        rule: "mashup-out-of-sync".to_string(),
                        message: format!("{} differ: {}", changes.len(), changes.join(", ")),
                    }),
                    Ok(_) => {}
                    Err(e) => result.findings.push(Finding {
                        gate: "sidecars".to_string(),
                        file: relative(solution, &entity.path),
                        line: 0,
                        rule: "mashup-will-not-sync".to_string(),
                        message: e.to_string(),
                    }),
                }
            }
        }

        let path = workspace::datatable_path(solution, entity);
        if super::datatable::is_data_table(&src) {
            let text = match workspace::read_datatable(&path) {
                Ok(Some(text)) => text,
                // No sidecar: this DataTable is not under management.
                Ok(None) => continue,
                Err(e) => {
                    result.findings.push(Finding {
                        gate: "sidecars".to_string(),
                        file: relative(solution, &path),
                        line: 0,
                        rule: "unreadable".to_string(),
                        message: e.to_string(),
                    });
                    continue;
                }
            };
            examined.insert(entity.info.name.clone());
            match super::datatable::from_sidecar(&text)
                .and_then(|desired| super::datatable::sync(&src, &desired))
            {
                Ok((out, changes)) if out != src => result.findings.push(Finding {
                    gate: "sidecars".to_string(),
                    file: relative(solution, &entity.path),
                    line: 0,
                    rule: "datatable-out-of-sync".to_string(),
                    message: format!(
                        "{} configuration change(s): {}",
                        changes.len(),
                        changes.join(", ")
                    ),
                }),
                Ok(_) => {}
                Err(e) => result.findings.push(Finding {
                    gate: "sidecars".to_string(),
                    file: relative(solution, &path),
                    line: 0,
                    rule: "datatable-will-not-sync".to_string(),
                    message: e.to_string(),
                }),
            }
        }
    }

    result.examined = examined.len();

    // A sidecar tree for an entity that no longer exists is worth knowing about: a sync would
    // never visit it, so it would sit there being wrong indefinitely.
    let known: BTreeSet<&str> = found
        .entities
        .iter()
        .map(|e| e.info.name.as_str())
        .collect();
    if let Ok(entries) = std::fs::read_dir(solution.src_root()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let looks_like_sidecars = entry.path().join("services").is_dir()
                || entry.path().join("mashup").is_dir()
                || entry.path().join("fields.json").is_file()
                || entry.path().join("datatable.json").is_file();
            let claimed = known.iter().any(|k| k.eq_ignore_ascii_case(&name));
            if entry.path().is_dir() && looks_like_sidecars && !claimed {
                result.findings.push(Finding {
                    gate: "sidecars".to_string(),
                    file: relative(solution, &entry.path()),
                    line: 0,
                    rule: "orphan".to_string(),
                    message: format!(
                        "sidecars for {name}, which is not an entity in this solution"
                    ),
                });
            }
        }
    }
    result
}

/// Service scripts that the formatter would change.
fn formatting(solution: &Solution) -> GateResult {
    let mut result = GateResult::passed("formatting", 0);
    let style = format_js::Style::default();
    for path in workspace::script_files(solution) {
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        result.examined += 1;
        match format_js::format(&source.replace("\r\n", "\n"), &style) {
            Ok(None) => {}
            Ok(Some(_)) => result.findings.push(Finding {
                gate: "formatting".to_string(),
                file: relative(solution, &path),
                line: 0,
                rule: "unformatted".to_string(),
                message: "run `twaco fmt`".to_string(),
            }),
            Err(e) => result.findings.push(Finding {
                gate: "formatting".to_string(),
                file: relative(solution, &path),
                line: 0,
                rule: "will-not-parse".to_string(),
                message: e,
            }),
        }
    }
    result
}

/// How much of a hook's output is kept. Beyond this it is drained and discarded, so the hook
/// can still finish rather than blocking forever on a full pipe.
const OUTPUT_CAP: usize = 1024 * 1024;

/// Traps in the script engine that compile cleanly and then misbehave at runtime.
fn script_traps(solution: &Solution) -> GateResult {
    let mut result = GateResult::passed("script traps", 0);
    for path in workspace::script_files(solution) {
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        result.examined += 1;
        for trap in super::lint::lint(&source.replace("\r\n", "\n")) {
            result.findings.push(Finding {
                gate: "script traps".to_string(),
                file: relative(solution, &path),
                line: trap.line,
                rule: trap.rule.to_string(),
                message: trap.message,
            });
        }
    }
    result
}

/// Main code above its helpers, so a service reads in the order it runs.
fn code_order(solution: &Solution) -> GateResult {
    let mut result = GateResult::passed("code order", 0);
    for path in workspace::script_files(solution) {
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        result.examined += 1;
        for misplaced in super::order::check(&source.replace("\r\n", "\n")) {
            result.findings.push(Finding {
                gate: "code order".to_string(),
                file: relative(solution, &path),
                line: misplaced.line,
                rule: misplaced.rule.to_string(),
                message: misplaced.message,
            });
        }
    }
    result
}

/// Whether the entity files agree with themselves and with each other.
fn project_validation(solution: &Solution) -> GateResult {
    let mut result = GateResult::passed("project", 0);
    result.examined = workspace::discover(solution).entities.len();
    for problem in super::validate::check(solution) {
        result.findings.push(Finding {
            gate: "project".to_string(),
            file: problem.file,
            line: 0,
            rule: problem.rule.to_string(),
            message: problem.message,
        });
    }
    result
}

/// Run one declared external check.
fn run_hook(solution: &Solution, hook: &super::config::Check) -> GateResult {
    let mut result = GateResult::passed(&hook.name, 0);
    let Some((program, arguments)) = hook.command.split_first() else {
        result.broken = Some("declares no command".to_string());
        return result;
    };

    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(&solution.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Credentials are never handed to a declared check. A hook that genuinely needs them says
    // so, and until then the environment it sees carries none.
    if !hook.needs_credentials {
        for (key, _) in std::env::vars() {
            let upper = key.to_ascii_uppercase();
            if upper.starts_with("TWACO_") || upper.starts_with("TWX_") {
                command.env_remove(key);
            }
        }
    }

    let child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            result.broken = Some(format!("cannot run {program}: {e}"));
            return result;
        }
    };

    let finished = match run_to_completion(child, Duration::from_secs(hook.timeout_seconds)) {
        Ok(f) => f,
        Err(e) => {
            result.broken = Some(e);
            return result;
        }
    };

    if finished.timed_out {
        result.broken = Some(format!(
            "did not finish within {} seconds, and was killed",
            hook.timeout_seconds
        ));
        return result;
    }
    if finished.truncated {
        result.broken = Some(format!("printed more than {OUTPUT_CAP} bytes"));
        return result;
    }

    // A hook given credentials may print them; twaco prints what a hook says, so they are taken
    // out first, whatever form the hook printed.
    let secrets = if hook.needs_credentials {
        credential_values()
    } else {
        Vec::new()
    };
    let hide = |text: &str| {
        let mut out = text.to_string();
        for secret in &secrets {
            out = out.replace(secret.as_str(), "<redacted>");
        }
        out
    };

    // Findings protocol: JSON Lines that parse become findings; anything else is kept as text
    // and attributed to the hook, so a check that prints its own summary is still readable.
    let stdout = hide(&String::from_utf8_lossy(&finished.stdout));
    let mut prose = Vec::new();
    for line in stdout.lines() {
        result.examined += 1;
        match parse_finding(&hook.name, line) {
            // Scrubbed again once decoded: JSON can spell a secret with escapes (`e` for
            // `e`) that the raw line does not show.
            Some(finding) => result.findings.push(Finding {
                gate: hide(&finding.gate),
                file: hide(&finding.file),
                rule: hide(&finding.rule),
                message: hide(&finding.message),
                ..finding
            }),
            None if !line.trim().is_empty() => prose.push(sanitise(line)),
            None => {}
        }
    }
    result.prose = prose.clone();

    let succeeded = finished.status.map(|s| s.success()).unwrap_or(false);
    if !succeeded && result.findings.is_empty() {
        let detail = if prose.is_empty() {
            sanitise(hide(&String::from_utf8_lossy(&finished.stderr)).trim())
        } else {
            prose.join("; ")
        };
        result.findings.push(Finding {
            gate: hook.name.clone(),
            file: hook.command.join(" "),
            line: 0,
            rule: "failed".to_string(),
            message: if detail.is_empty() {
                "exited non-zero".to_string()
            } else {
                detail
            },
        });
    }

    // A check declared `gate = false` reports without blocking. Its findings are kept so they
    // are still read; they simply do not fail the run.
    result.gates_the_run = hook.gate;
    result
}

/// What running a child produced.
struct Finished {
    status: Option<std::process::ExitStatus>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
    truncated: bool,
}

/// Run a child to completion, killing it if it outstays the deadline.
///
/// Both pipes are drained on their own threads from the moment the child starts. Waiting first
/// and reading afterwards deadlocks as soon as a hook writes more than the operating system's
/// pipe buffer, which is far smaller than the output cap: the hook blocks writing, the wait
/// blocks on the hook, and the timeout fires on a program that was only being chatty.
fn run_to_completion(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<Finished, String> {
    let out_reader = spawn_reader(child.stdout.take());
    let err_reader = spawn_reader(child.stderr.take());

    let deadline = std::time::Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // Killed rather than abandoned: a detached hook would outlive the run and
                    // overlap whatever ran next.
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e.to_string()),
        }
    };

    let (stdout, out_truncated) = out_reader
        .join()
        .map_err(|_| "reader thread panicked".to_string())?;
    let (stderr, err_truncated) = err_reader
        .join()
        .map_err(|_| "reader thread panicked".to_string())?;
    Ok(Finished {
        status,
        stdout,
        stderr,
        timed_out,
        truncated: out_truncated || err_truncated,
    })
}

/// Drain one pipe on its own thread, keeping at most `OUTPUT_CAP` bytes.
///
/// Past the cap the bytes are read and discarded rather than left in the pipe, so a hook that
/// will not stop talking still reaches its own exit instead of blocking there forever.
fn spawn_reader<R: std::io::Read + Send + 'static>(
    source: Option<R>,
) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut truncated = false;
        let Some(mut source) = source else {
            return (kept, truncated);
        };
        let mut buffer = [0u8; 8192];
        loop {
            match source.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if kept.len() < OUTPUT_CAP {
                        let room = OUTPUT_CAP - kept.len();
                        kept.extend_from_slice(&buffer[..n.min(room)]);
                        if n > room {
                            truncated = true;
                        }
                    } else {
                        truncated = true;
                    }
                }
            }
        }
        (kept, truncated)
    })
}

/// The secret values a hook with `needs_credentials` inherits: the password, app key, token or
/// secret variables twaco and its `TWX_` spellings read, as set in this process.
fn credential_values() -> Vec<String> {
    let mut values: Vec<String> = std::env::vars()
        .filter(|(key, value)| {
            let key = key.to_ascii_uppercase();
            (key.starts_with("TWACO_") || key.starts_with("TWX_"))
                && ["PASSWORD", "KEY", "SECRET", "TOKEN"]
                    .iter()
                    .any(|word| key.contains(word))
                && value.len() >= 4
        })
        .map(|(_, value)| value)
        .collect();
    // Longest first, so a secret that contains another is replaced whole.
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values
}

/// Strip the control characters a hook could use to forge output.
///
/// A finding's text is written by someone else's program and printed to a terminal. A carriage
/// return or an escape sequence in it can overwrite the lines around it, so a hostile or merely
/// careless check could make the gate appear to say something it did not.
fn sanitise(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c == '\t' {
                ' '
            } else if c.is_control() {
                '?'
            } else {
                c
            }
        })
        .take(2000)
        .collect()
}

/// A JSON Lines finding, parsed without pulling in a JSON dependency for five fields.
pub(crate) fn parse_finding(gate: &str, line: &str) -> Option<Finding> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let object = value.as_object()?;
    // A finding must at least say something; anything else is just JSON the hook printed.
    let message = object.get("message")?.as_str()?.to_string();
    Some(Finding {
        gate: sanitise(object.get("gate").and_then(|v| v.as_str()).unwrap_or(gate)),
        file: sanitise(
            object
                .get("file")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
        ),
        line: object.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        rule: sanitise(
            object
                .get("rule")
                .and_then(|v| v.as_str())
                .unwrap_or("finding"),
        ),
        message: sanitise(&message),
    })
}

/// A path as the user would recognise it: relative to the solution when it is inside one.
fn relative(solution: &Solution, path: &Path) -> String {
    path.strip_prefix(&solution.root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::server::{Method, ScriptCheck, ServerError};
    use std::sync::Mutex;

    /// A solution on disk: one Thing with two script services, one of them with its sidecar.
    fn live_solution() -> (PathBuf, Solution) {
        let nonce = crate::test_nonce();
        let root = std::env::temp_dir().join(format!("twaco-live-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        let service = |name: &str, code: &str| {
            format!(
                "<ServiceImplementation name=\"{name}\" handlerName=\"Script\"><ConfigurationTables>\
                 <ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[{code}]]></code></Row>\
                 </Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation>"
            )
        };
        let xml = format!(
            "<Entities><Things><Thing name=\"P.T\" projectName=\"P\"><ThingShape><ServiceDefinitions>\
             <ServiceDefinition name=\"Good\"/><ServiceDefinition name=\"Bad\"/></ServiceDefinitions>\
             <ServiceImplementations>{}{}</ServiceImplementations></ThingShape></Thing></Things></Entities>",
            service("Good", "result = 1;"),
            service("Bad", "var a = 1;\nvar b = ;")
        );
        std::fs::write(root.join("Things/P.T.xml"), xml).unwrap();
        let bad = root.join("src/P.T/services/Bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("script.js"), "var a = 1;\nvar b = ;").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root, solution)
    }

    /// A declared check is third-party code run from the repository, so what it can read from the
    /// environment is a boundary: credentials reach it only if it says it needs them.
    #[test]
    fn a_secret_spelled_with_json_escapes_is_hidden_once_decoded() {
        let (root, solution) = live_solution();
        std::env::set_var("TWACO_ESCAPE_PROBE_PASSWORD", "probe-escape-91c2");
        // `e` is `e`: the raw line does not hold the secret, the decoded message does.
        std::fs::write(
            root.join("finding.json"),
            "{\"message\":\"leaked probe-\\u0065scape-91c2\",\"rule\":\"probe-\\u0065scape-91c2\"}\n",
        )
        .unwrap();
        let printer: Vec<String> = if cfg!(windows) {
            vec![
                "cmd".into(),
                "/C".into(),
                "type".into(),
                "finding.json".into(),
            ]
        } else {
            vec!["cat".into(), "finding.json".into()]
        };
        let result = run_hook(
            &solution,
            &super::super::config::Check {
                name: "escapes".to_string(),
                command: printer,
                gate: false,
                needs_credentials: true,
                timeout_seconds: 30,
            },
        );
        std::env::remove_var("TWACO_ESCAPE_PROBE_PASSWORD");
        let _ = std::fs::remove_dir_all(root);
        assert_eq!(result.findings.len(), 1, "{result:?}");
        let finding = &result.findings[0];
        assert_eq!(finding.message, "leaked <redacted>");
        assert_eq!(finding.rule, "<redacted>");
    }

    #[test]
    fn a_hook_sees_no_credentials_unless_it_says_it_needs_them() {
        let (root, solution) = live_solution();
        std::env::set_var("TWACO_HOOK_PROBE_SECRET", "probe-value-7f3a");
        std::env::set_var("TWX_HOOK_PROBE_SECRET", "probe-value-7f3a");
        let printer: Vec<String> = if cfg!(windows) {
            vec!["cmd".into(), "/C".into(), "set".into()]
        } else {
            vec!["env".into()]
        };
        let run = |needs_credentials: bool| {
            run_hook(
                &solution,
                &super::super::config::Check {
                    name: "environment".to_string(),
                    command: printer.clone(),
                    gate: false,
                    needs_credentials,
                    timeout_seconds: 30,
                },
            )
        };
        let without = run(false);
        let with = run(true);
        std::env::remove_var("TWACO_HOOK_PROBE_SECRET");
        std::env::remove_var("TWX_HOOK_PROBE_SECRET");
        let _ = std::fs::remove_dir_all(root);

        assert!(without.broken.is_none(), "{:?}", without.broken);
        assert!(
            !without.prose.is_empty(),
            "the hook printed its environment"
        );
        assert!(
            without
                .prose
                .iter()
                .all(|line| !line.contains("probe-value-7f3a")),
            "a hook that did not ask for credentials saw one: {:?}",
            without.prose
        );
        // It got them (the variables are set), and twaco does not print what it printed of them.
        assert!(
            with.prose
                .iter()
                .any(|line| line.contains("TWACO_HOOK_PROBE_SECRET=<redacted>"))
                && with
                    .prose
                    .iter()
                    .any(|line| line.contains("TWX_HOOK_PROBE_SECRET=<redacted>")),
            "a hook that declared needs_credentials gets them: {:?}",
            with.prose
        );
        assert!(
            with.prose
                .iter()
                .all(|line| !line.contains("probe-value-7f3a")),
            "what a hook prints of its credentials is not printed: {:?}",
            with.prose
        );
    }

    /// Answers as the server was observed to: the fields right, the message's line one too high.
    struct Parser {
        unreachable: bool,
        seen: Mutex<Vec<String>>,
    }

    impl ScriptChecker for Parser {
        fn check_script(&self, script: &str) -> Result<ScriptCheck, ServerError> {
            self.seen.lock().unwrap().push(script.to_string());
            if self.unreachable {
                return Err(ServerError::Transport {
                    method: Method::Post,
                    url: "http://server/Thingworx".to_string(),
                    why: "connection refused".to_string(),
                });
            }
            Ok(if script.contains("= ;") {
                ScriptCheck {
                    status: false,
                    line_number: 2,
                    column_number: 9,
                    message: "syntax error at line 3 column 9 source: [var b = ;]".to_string(),
                }
            } else {
                ScriptCheck {
                    status: true,
                    line_number: 0,
                    column_number: 0,
                    message: String::new(),
                }
            })
        }
    }

    #[test]
    fn a_rejected_script_is_a_finding_on_its_sidecar_line() {
        let (root, solution) = live_solution();
        let parser = Parser {
            unreachable: false,
            seen: Mutex::new(Vec::new()),
        };
        let result = live_parse(&solution, Ok(&parser));
        assert_eq!(result.examined, 2);
        assert!(result.broken.is_none());
        assert_eq!(result.findings.len(), 1);
        let finding = &result.findings[0];
        assert_eq!(finding.file, "src/P.T/services/Bad/script.js");
        assert_eq!(finding.line, 2);
        assert_eq!(
            finding.message,
            "column 9: syntax error source: [var b = ;]"
        );
        assert!(result.blocks());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_unreachable_server_breaks_the_gate_rather_than_passing_it() {
        let (root, solution) = live_solution();
        let parser = Parser {
            unreachable: true,
            seen: Mutex::new(Vec::new()),
        };
        let result = live_parse(&solution, Ok(&parser));
        assert!(result.broken.is_some());
        assert!(result.findings.is_empty());
        assert!(result.blocks(), "fail closed");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn no_profile_breaks_the_gate_too() {
        let (root, solution) = live_solution();
        let result = live_parse(
            &solution,
            Err("profile \"default\" was not found".to_string()),
        );
        assert!(result.blocks());
        assert!(result.broken.unwrap().contains("not found"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_misleading_position_in_the_parser_message_is_dropped() {
        assert_eq!(
            without_embedded_position(
                "missing ) after condition at line 4 column 7 source: [if (a {]"
            ),
            "missing ) after condition source: [if (a {]"
        );
        assert_eq!(
            without_embedded_position("TypeError: x at line 3 column 6"),
            "TypeError: x"
        );
        assert_eq!(
            without_embedded_position("no position here"),
            "no position here"
        );
    }

    #[test]
    fn a_json_line_becomes_a_finding() {
        let line = r#"{"file":"a.js","line":7,"rule":"length","message":"too long"}"#;
        let finding = parse_finding("comments", line).unwrap();
        assert_eq!(finding.gate, "comments");
        assert_eq!(finding.file, "a.js");
        assert_eq!(finding.line, 7);
        assert_eq!(finding.rule, "length");
        assert_eq!(finding.message, "too long");
    }

    #[test]
    fn a_finding_may_name_its_own_gate() {
        let line = r#"{"gate":"other","message":"m"}"#;
        assert_eq!(parse_finding("comments", line).unwrap().gate, "other");
    }

    #[test]
    fn ordinary_output_is_not_a_finding() {
        assert!(parse_finding("g", "123 scripts, 0 failing").is_none());
        assert!(parse_finding("g", "").is_none());
        // JSON, but saying nothing.
        assert!(parse_finding("g", r#"{"count": 3}"#).is_none());
    }

    #[test]
    fn a_finding_renders_with_and_without_a_line() {
        let with = Finding {
            gate: "g".into(),
            file: "f.js".into(),
            line: 4,
            rule: "r".into(),
            message: "m".into(),
        };
        assert_eq!(with.to_string(), "f.js:4 [g/r] m");
        let without = Finding { line: 0, ..with };
        assert_eq!(without.to_string(), "f.js [g/r] m");
    }

    #[test]
    fn a_gate_with_findings_has_not_passed() {
        let mut gate = GateResult::passed("g", 3);
        assert!(gate.ok());
        gate.findings.push(Finding {
            gate: "g".into(),
            file: "f".into(),
            line: 0,
            rule: "r".into(),
            message: "m".into(),
        });
        assert!(!gate.ok());
    }

    #[test]
    fn a_broken_gate_has_not_passed_either() {
        let mut gate = GateResult::passed("g", 0);
        gate.broken = Some("could not run".to_string());
        assert!(
            !gate.ok(),
            "a gate that could not run is not a gate that passed"
        );
    }
}
