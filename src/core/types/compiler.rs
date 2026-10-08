use super::super::config::Solution;
use super::model::{load_model, script_declares, Member, Model, Service};
use super::render::{identifiers, type_name};
use super::write::write;
use super::{CheckError, CheckOutcome, TypeFinding};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

/// Captured compiler output, kept independent of `std::process::Output` for no-node tests.
pub struct CompilerOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Injectable boundary around TypeScript. Production uses a child process; tests use canned text.
pub trait CompilerRunner {
    fn run(
        &self,
        program: &OsStr,
        arguments: &[OsString],
        current_dir: &Path,
    ) -> std::io::Result<CompilerOutput>;
}

struct ProcessCompiler;

impl CompilerRunner for ProcessCompiler {
    fn run(
        &self,
        program: &OsStr,
        arguments: &[OsString],
        current_dir: &Path,
    ) -> std::io::Result<CompilerOutput> {
        let started = std::time::Instant::now();
        let output = Command::new(program)
            .args(arguments)
            .current_dir(current_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .inspect_err(|why| {
                tracing::debug!(program = ?program, why = %why, "compiler could not start");
            })?;
        tracing::debug!(
            program = ?program,
            argument_count = arguments.len(),
            exit = output.status.code(),
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "compiler finished"
        );
        Ok(CompilerOutput {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

/// Regenerate editor declarations and type-check every service in one compiler project.
pub fn check(solution: &Solution) -> Result<CheckOutcome, CheckError> {
    check_with(solution, &ProcessCompiler)
}

pub(crate) fn check_with(
    solution: &Solution,
    compiler: &dyn CompilerRunner,
) -> Result<CheckOutcome, CheckError> {
    // This is deliberately the public generation path: checking has exactly the same cache
    // handling and declaration writes as plain `twaco types`.
    let declarations = write(solution)?;
    let (model, _) = load_model(solution);
    let projects = write_check_project(solution, &model)?;
    let command = compiler_command(solution);
    let (program, configured_arguments) = command
        .split_first()
        .expect("compiler discovery always returns a program");
    let config_path = solution.root.join(".twaco/types/check/tsconfig.json");
    let mut arguments = configured_arguments.to_vec();
    arguments.extend([
        OsString::from("-p"),
        config_path.into_os_string(),
        OsString::from("--pretty"),
        OsString::from("false"),
    ]);
    let started = Instant::now();
    let output = compiler
        .run(program, &arguments, &solution.root)
        .map_err(|error| {
            CheckError(format!(
                "cannot start TypeScript compiler {}: {error}; install one with `npm install \
             --save-dev typescript` in the solution root, or set `[types] tsc` in twaco.toml",
                program.to_string_lossy()
            ))
        })?;
    let elapsed = started.elapsed();
    let stdout = std::str::from_utf8(&output.stdout).map_err(|error| {
        CheckError(format!(
            "could not read TypeScript compiler output: {error}"
        ))
    })?;
    let stderr = std::str::from_utf8(&output.stderr).map_err(|error| {
        CheckError(format!(
            "could not read TypeScript compiler output: {error}"
        ))
    })?;
    let combined = if stdout.is_empty() {
        stderr.to_string()
    } else if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    };
    let parsed = parse_compiler_output(&combined);
    if !output.success && parsed.is_empty() {
        let argv = std::iter::once(program.as_os_str())
            .chain(arguments.iter().map(OsString::as_os_str))
            .map(|argument| format!("{:?}", argument.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(" ");
        let detail = stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(str::trim)
            .unwrap_or("(stderr was blank)");
        return Err(CheckError(format!(
            "TypeScript compiler exited non-zero without a parsable finding\n\
             ran: {argv}; stderr: {detail}; install one with `npm install --save-dev typescript` \
             in the solution root, or set `[types] tsc` in twaco.toml"
        )));
    }

    let mut affected = BTreeSet::new();
    let mut findings = Vec::new();
    for finding in parsed {
        let (file, line, service) = map_finding(solution, &projects, &finding);
        if let Some(service) = service {
            affected.insert(service);
        }
        findings.push(TypeFinding {
            file,
            line,
            column: finding.column,
            code: finding.code,
            message: finding.message,
        });
    }
    findings.sort_by(|a, b| {
        (&a.file, a.line, a.column, &a.code, &a.message)
            .cmp(&(&b.file, b.line, b.column, &b.code, &b.message))
    });
    Ok(CheckOutcome {
        declarations,
        findings,
        affected_services: affected.len(),
        services: projects.len(),
        elapsed,
    })
}

#[derive(Debug)]
pub(super) struct CheckProject {
    pub(super) generated_name: String,
    pub(super) script_path: PathBuf,
    pub(super) globals_path: PathBuf,
    pub(super) header_lines: usize,
}

fn check_globals(
    service: &Service,
    entity_id: &str,
    script: &str,
    known_shapes: &BTreeSet<&str>,
    data_shape_ids: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut globals = vec![("me".to_string(), format!("twx.{entity_id}"))];
    globals.extend(
        service
            .parameters
            .iter()
            .filter(|parameter| !script_declares(script, &parameter.name))
            .map(|parameter| {
                (
                    parameter.name.clone(),
                    type_name(&parameter.value, known_shapes, data_shape_ids),
                )
            }),
    );
    if !service.result.base_type.eq_ignore_ascii_case("NOTHING")
        && !script_declares(script, "result")
    {
        globals.push((
            "result".to_string(),
            type_name(&service.result, known_shapes, data_shape_ids),
        ));
    }
    globals
}

pub(super) fn write_check_project(
    solution: &Solution,
    model: &Model,
) -> Result<Vec<CheckProject>, CheckError> {
    struct Pending {
        script_path: PathBuf,
        globals_path: PathBuf,
        script: String,
        globals: Vec<(String, String)>,
    }

    let known_shapes: BTreeSet<&str> = model
        .data_shapes
        .iter()
        .map(|shape| shape.name.as_str())
        .collect();
    let data_shape_ids = identifiers(
        model.data_shapes.iter().map(|shape| shape.name.as_str()),
        "D_",
    );
    let entity_keys: Vec<String> = model
        .entities
        .iter()
        .map(|entity| format!("{}\0{}", entity.name, entity.collection))
        .collect();
    let entity_ids = identifiers(entity_keys.iter().map(String::as_str), "E_");
    let mut pending = Vec::new();
    for entity in &model.entities {
        let entity_key = format!("{}\0{}", entity.name, entity.collection);
        let services_dir = solution.src_root().join(&entity.name).join("services");
        for member in &entity.members {
            let Member::Service(service) = member else {
                continue;
            };
            let directory = services_dir.join(&service.name);
            let script_path = directory.join("script.js");
            if !script_path.is_file() {
                continue;
            }
            let script = std::fs::read_to_string(&script_path)
                .map_err(|error| CheckError(format!("{}: {error}", script_path.display())))?;
            pending.push(Pending {
                globals: check_globals(
                    service,
                    &entity_ids[&entity_key],
                    &script,
                    &known_shapes,
                    &data_shape_ids,
                ),
                globals_path: directory.join("twaco-globals.d.ts"),
                script_path,
                script,
            });
        }
    }
    pending.sort_by(|a, b| a.script_path.cmp(&b.script_path));

    let directory = solution.root.join(".twaco/types/check");
    match std::fs::remove_dir_all(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(CheckError(format!("{}: {error}", directory.display()))),
    }
    std::fs::create_dir_all(&directory)
        .map_err(|error| CheckError(format!("{}: {error}", directory.display())))?;

    let mut projects = Vec::new();
    for (index, item) in pending.into_iter().enumerate() {
        let generated_name = format!("s{index:04}.js");
        let mut source = String::from("export {};\n");
        for (name, value_type) in &item.globals {
            source.push_str(&format!("/** @type {{{value_type}}} */ var {name};\n"));
        }
        let header_lines = 1 + item.globals.len();
        source.push_str(&item.script);
        let path = directory.join(&generated_name);
        std::fs::write(&path, source)
            .map_err(|error| CheckError(format!("{}: {error}", path.display())))?;
        projects.push(CheckProject {
            generated_name,
            script_path: item.script_path,
            globals_path: item.globals_path,
            header_lines,
        });
    }
    let tsconfig = json!({
        "compilerOptions": {
            "allowJs": true,
            "checkJs": true,
            "noEmit": true,
            "target": "ES2015",
            "lib": ["ES2015"],
            "types": [],
            "module": "ES2015",
            "moduleDetection": "force",
            "strict": false
        },
        "include": ["*.js", "../*.d.ts"]
    });
    let config_path = directory.join("tsconfig.json");
    std::fs::write(
        &config_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&tsconfig).expect("tsconfig is JSON encodable")
        ),
    )
    .map_err(|error| CheckError(format!("{}: {error}", config_path.display())))?;
    Ok(projects)
}

pub(super) fn compiler_command(solution: &Solution) -> Vec<OsString> {
    if let Some(configured) = &solution.types.tsc {
        return configured.iter().map(OsString::from).collect();
    }
    let local = solution.root.join("node_modules/typescript/bin/tsc");
    if local.is_file() {
        return vec![OsString::from("node"), local.into_os_string()];
    }
    vec![OsString::from(if cfg!(windows) {
        "tsc.cmd"
    } else {
        "tsc"
    })]
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct CompilerFinding {
    pub(super) file: String,
    pub(super) line: usize,
    pub(super) column: usize,
    pub(super) code: String,
    pub(super) message: String,
}

pub(super) fn parse_compiler_output(output: &str) -> Vec<CompilerFinding> {
    let mut findings: Vec<CompilerFinding> = Vec::new();
    for line in output.lines() {
        if let Some(finding) = parse_compiler_line(line) {
            findings.push(finding);
        } else if line.starts_with([' ', '\t']) {
            if let Some(previous) = findings.last_mut() {
                let continuation = line.trim();
                if !continuation.is_empty() {
                    previous.message.push(' ');
                    previous.message.push_str(continuation);
                }
            }
        }
    }
    findings
}

fn parse_compiler_line(line: &str) -> Option<CompilerFinding> {
    const MARKER: &str = "): error TS";
    let marker = line.find(MARKER)?;
    let before = &line[..marker];
    let coordinates = before.rfind('(')?;
    let file = &before[..coordinates];
    let (line_number, column) = before[coordinates + 1..].split_once(',')?;
    let after_code = &line[marker + MARKER.len()..];
    let (code, message) = after_code.split_once(':')?;
    if file.is_empty() || code.is_empty() || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(CompilerFinding {
        file: file.to_string(),
        line: line_number.parse().ok()?,
        column: column.parse().ok()?,
        code: code.to_string(),
        message: message.trim_start().to_string(),
    })
}

pub(super) fn map_finding(
    solution: &Solution,
    projects: &[CheckProject],
    finding: &CompilerFinding,
) -> (String, usize, Option<usize>) {
    let basename = finding
        .file
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&finding.file);
    if let Some((index, project)) = projects
        .iter()
        .enumerate()
        .find(|(_, project)| project.generated_name == basename)
    {
        if finding.line <= project.header_lines {
            return (
                relative_path(solution, &project.globals_path),
                finding.line,
                Some(index),
            );
        }
        return (
            relative_path(solution, &project.script_path),
            finding.line - project.header_lines,
            Some(index),
        );
    }
    let path = Path::new(&finding.file);
    let file = if path.is_absolute() {
        relative_path(solution, path)
    } else {
        finding.file.replace('\\', "/")
    };
    (file, finding.line, None)
}

fn relative_path(solution: &Solution, path: &Path) -> String {
    path.strip_prefix(&solution.root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
