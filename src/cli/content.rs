use super::super::*;

/// `twaco export entity | collection | project | source-control`.
pub(crate) fn export_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::commands::export::{self as command, ExportAction, ExportRequest};
    use twaco::core::export;
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let value = |flag: &str| args.values.get(flag).cloned();
    let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
    let result: Result<(), String> = (|| {
        let action = match names.as_slice() {
            ["entity", entity] => ExportAction::Xml {
                what: export::What::entity(entity).map_err(|e| e.to_string())?,
                out: args
                    .out
                    .clone()
                    .ok_or("an export needs --out <file>")?,
                force: args.has("--force"),
            },
            ["collection", collection] => ExportAction::Xml {
                what: export::What::Collection {
                    collection: collection.to_string(),
                    project: args.project.clone(),
                },
                out: args
                    .out
                    .clone()
                    .ok_or("an export needs --out <file>")?,
                force: args.has("--force"),
            },
            ["project", project] => ExportAction::Xml {
                what: export::What::Project {
                    project: project.to_string(),
                },
                out: args
                    .out
                    .clone()
                    .ok_or("an export needs --out <file>")?,
                force: args.has("--force"),
            },
            ["source-control"] => {
                let repository = value("--repository").ok_or("source-control needs --repository")?;
                let path = value("--path").ok_or("source-control needs --path, a folder of the repository")?;
                let filters = export::Filters {
                    project: args.project.clone(),
                    collection: value("--collection"),
                    tags: value("--tags"),
                    include_dependents: args.has("--with-dependents"),
                };
                let zip = value("--zip");
                let apply = args.has("--apply");
                let request = ExportRequest {
                    action: ExportAction::SourceControl {
                        repository,
                        path,
                        filters,
                        zip,
                        mode: if apply {
                            commands::Mode::Apply
                        } else {
                            commands::Mode::Plan
                        },
                    },
                    profile: profile_name.to_string(),
                };
                let mut notices = commands::Notices::default();
                let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                    .map_err(|e| e.to_string())?;
                print_notices(&notices);
                let command::ExportOutcome::SourceControl { plan, download: link, .. } = outcome else {
                    unreachable!()
                };
                if apply {
                    println!("done: {plan}");
                    if let Some(link) = link {
                        println!("download: {link}");
                    }
                } else {
                    println!("would {plan}; nothing sent (pass --apply)");
                }
                return Ok(());
            }
            _ => return Err("export takes: entity <Coll/Name> | collection <Coll> | project <P> | source-control".to_string()),
        };
        let request = ExportRequest {
            action,
            profile: profile_name.to_string(),
        };
        let mut notices = commands::Notices::default();
        let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
            .map_err(|e| e.to_string())?;
        print_notices(&notices);
        let command::ExportOutcome::Xml { out, exported, .. } = outcome else {
            unreachable!()
        };
        let counts: Vec<String> = exported
            .counts
            .iter()
            .map(|(c, n)| format!("{n} {c}"))
            .collect();
        println!(
            "{} bytes to {}: {}",
            exported.xml.len(),
            out.display(),
            if counts.is_empty() {
                "no entities".to_string()
            } else {
                counts.join(", ")
            }
        );
        Ok(())
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: export: {why}");
            FAILED
        }
    }
}

/// `twaco import <file> | import source-control`: into the server, as plans unless applied.
pub(crate) fn import_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::commands::imports::{self as command, ImportAction, ImportRequest};
    use twaco::core::imports;
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let apply = args.has("--apply");
    let (properties, tables) = (
        args.has("--overwrite-properties"),
        args.has("--overwrite-tables"),
    );
    let differs_line =
        |d: &imports::Differs| format!("{} {}: {}", d.entity_type, d.name, d.what.join("; "));
    let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
    let result: Result<(), String> = (|| match names.as_slice() {
        ["source-control"] => {
            let repository = args
                .values
                .get("--repository")
                .ok_or("import source-control needs --repository")?;
            let path = args
                .values
                .get("--path")
                .ok_or("import source-control needs --path")?;
            let request = ImportRequest {
                action: ImportAction::SourceControl {
                    repository: repository.clone(),
                    path: path.clone(),
                },
                mode: if apply {
                    commands::Mode::Apply
                } else {
                    commands::Mode::Plan
                },
                overwrite_properties: properties,
                overwrite_tables: tables,
                profile: profile_name.to_string(),
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(|e| e.to_string())?;
            print_notices(&notices);
            let command::ImportOutcome::SourceControl {
                report: imported, ..
            } = outcome
            else {
                unreachable!()
            };
            let (total, before, after) = (imported.total, imported.differ, imported.still_differ);
            let shown = if args.has("--detail") { usize::MAX } else { 20 };
            println!(
                "{total} entities in {repository}:{path}; {} differ from the server",
                before.len()
            );
            for d in before.iter().take(shown) {
                println!("  {}", differs_line(d));
            }
            match after {
                None => println!("nothing sent (pass --apply to import them)"),
                Some(after) => {
                    println!(
                        "imported; {} still differ{}",
                        after.len(),
                        if after.is_empty() { "" } else { ":" }
                    );
                    for d in after.iter().take(shown) {
                        println!("  {}", differs_line(d));
                    }
                }
            }
            Ok(())
        }
        [file] => {
            let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            let file_name = std::path::Path::new(file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "import.xml".into());
            let request = ImportRequest {
                action: ImportAction::File { file_name, bytes },
                mode: if apply {
                    commands::Mode::Apply
                } else {
                    commands::Mode::Plan
                },
                overwrite_properties: properties,
                overwrite_tables: tables,
                profile: profile_name.to_string(),
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(|e| e.to_string())?;
            print_notices(&notices);
            let command::ImportOutcome::File { plan, .. } = outcome else {
                unreachable!()
            };
            let shown = if args.has("--detail") { usize::MAX } else { 20 };
            for key in plan.replaced.iter().take(shown) {
                println!("  replaces {key}");
            }
            for key in plan.new.iter().take(shown) {
                println!("  adds     {key}");
            }
            println!(
                "{} {} new and {} replaced entities{}",
                if plan.applied {
                    "imported:"
                } else {
                    "would import:"
                },
                plan.new.len(),
                plan.replaced.len(),
                if plan.applied {
                    "; every one is on the server"
                } else {
                    "; nothing sent (pass --apply)"
                }
            );
            Ok(())
        }
        _ => Err(
            "import takes: <file.xml|.zip> | source-control --repository R --path <p>".to_string(),
        ),
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: import: {why}");
            FAILED
        }
    }
}

/// `twaco package bundle | source-control | extension`: the repository packaged for release,
/// offline. Never replaces an existing `--out` file without `--force`.
pub(crate) fn package_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::commands::package::{self, PackageAction, PackageRequest};
    let Some(out) = args.out.clone() else {
        eprintln!("twaco: package: package needs --out <file>");
        return FAILED;
    };
    let action = match args.names.as_slice() {
        [name] if name == "bundle" => {
            match (args.has("--backend-only"), args.has("--frontend-only")) {
                (true, true) => {
                    eprintln!(
                        "twaco: package: --backend-only and --frontend-only say different things"
                    );
                    return FAILED;
                }
                (true, false) => PackageAction::Bundle {
                    part: twaco::core::package::Part::Backend,
                },
                (false, true) => PackageAction::Bundle {
                    part: twaco::core::package::Part::Frontend,
                },
                (false, false) => PackageAction::Bundle {
                    part: twaco::core::package::Part::All,
                },
            }
        }
        [name] if name == "source-control" => PackageAction::SourceControl,
        [name] if name == "extension" => PackageAction::Extension {
            editable: args.has("--editable"),
        },
        _ => {
            eprintln!("twaco: package: package takes: bundle | source-control | extension");
            return FAILED;
        }
    };
    match package::execute(
        solution,
        &PackageRequest {
            action,
            project: args.project.clone(),
            out,
            force: args.has("--force"),
        },
    ) {
        Ok(outcome) => {
            println!(
                "{} bytes to {}: {}",
                outcome.bytes,
                outcome.out.display(),
                outcome.summary
            );
            OK
        }
        Err(error) => {
            eprintln!("twaco: package: {error}");
            FAILED
        }
    }
}

/// `twaco ext list | show | import | remove`: the server's extension packages.
pub(crate) fn ext_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::commands::extensions::{self as command, ExtensionAction, ExtensionRequest};
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
    let result: Result<(), String> = (|| match names.as_slice() {
        ["list"] => {
            let request = ExtensionRequest {
                action: ExtensionAction::List,
                profile: profile_name.to_string(),
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(|e| e.to_string())?;
            print_notices(&notices);
            let command::ExtensionOutcome::Listed { packages, .. } = outcome else {
                unreachable!()
            };
            for p in &packages {
                if args.has("--json") {
                    println!(
                        "{}",
                        serde_json::json!({ "name": p.name, "version": p.version, "vendor": p.vendor, "minimumThingWorxVersion": p.minimum_thingworx, "group": p.group, "artifact": p.artifact })
                    );
                } else {
                    println!("{:<48} {:<20} {}", p.name, p.version, p.vendor);
                }
            }
            eprintln!("{} package(s)", packages.len());
            Ok(())
        }
        ["show", name] => {
            let request = ExtensionRequest {
                action: ExtensionAction::Show {
                    name: name.to_string(),
                },
                profile: profile_name.to_string(),
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(|e| e.to_string())?;
            print_notices(&notices);
            let command::ExtensionOutcome::Shown { shown, .. } = outcome else {
                unreachable!()
            };
            let p = &shown.package;
            println!(
                "{} {} by {} (needs ThingWorx {})",
                p.name,
                p.version,
                if p.vendor.is_empty() { "?" } else { &p.vendor },
                p.minimum_thingworx
            );
            if !p.description.is_empty() {
                println!("  {}", p.description);
            }
            println!("{} extension(s):", shown.extensions.len());
            for row in &shown.extensions {
                let name = row
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?");
                let kind = row
                    .get("extensionType")
                    .or_else(|| row.get("type"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                println!("  {name} {kind}");
            }
            println!("{} in use", shown.in_use.len());
            Ok(())
        }
        ["import", file] => {
            let zip = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            let file_name = std::path::Path::new(file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "package.zip".into());
            let request = ExtensionRequest {
                action: ExtensionAction::Import {
                    file_name,
                    zip,
                    mode: if args.has("--apply") {
                        commands::Mode::Apply
                    } else {
                        commands::Mode::Plan
                    },
                },
                profile: profile_name.to_string(),
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(|e| e.to_string())?;
            print_notices(&notices);
            let command::ExtensionOutcome::Imported { imported, .. } = outcome else {
                unreachable!()
            };
            if imported.applied {
                println!("done: {}; the package list now shows it", imported.plan);
            } else {
                println!(
                    "would {}; the server validated it and installed nothing (pass --apply)",
                    imported.plan
                );
            }
            Ok(())
        }
        ["remove", name] => {
            let request = ExtensionRequest {
                action: ExtensionAction::Remove {
                    name: name.to_string(),
                    mode: if args.has("--apply") {
                        commands::Mode::Apply
                    } else {
                        commands::Mode::Plan
                    },
                },
                profile: profile_name.to_string(),
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(|e| e.to_string())?;
            print_notices(&notices);
            let command::ExtensionOutcome::Removed { plan, .. } = outcome else {
                unreachable!()
            };
            if args.has("--apply") {
                println!("done: {plan}; it is gone from the package list");
            } else {
                println!("would {plan}; nothing sent (pass --apply)");
            }
            Ok(())
        }
        _ => Err("ext takes: list | show <package> | import <zip> | remove <package>".to_string()),
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: ext: {why}");
            FAILED
        }
    }
}

/// `twaco repo list | ls | get | status`: the server's file repositories, read-only.
pub(crate) fn repo_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::commands::repo::{self as command, RepoAction, RepoRequest};
    use twaco::core::repo;
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
    let result: Result<(), String> = (|| {
        match names.as_slice() {
        ["list"] => {
            let request = RepoRequest {
                action: RepoAction::List,
                profile: profile_name.to_string(),
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(|e| e.to_string())?;
            print_notices(&notices);
            let command::RepoOutcome::Listed { repositories, .. } = outcome else {
                unreachable!()
            };
            for name in repositories {
                println!("{name}");
            }
            Ok(())
        }
        ["ls", repository, rest @ ..] if rest.len() <= 1 => {
            let folder = rest.first().copied().unwrap_or("/");
            let request = RepoRequest {
                action: RepoAction::Ls {
                    repository: repository.to_string(),
                    path: folder.to_string(),
                    recursive: args.has("--recursive"),
                },
                profile: profile_name.to_string(),
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(|e| e.to_string())?;
            print_notices(&notices);
            let command::RepoOutcome::Ls { listing, .. } = outcome else {
                unreachable!()
            };
            if args.has("--json") {
                for folder in &listing.folders {
                    println!("{}", serde_json::json!({ "path": folder, "type": "folder" }));
                }
                for file in &listing.files {
                    println!("{}", serde_json::json!({ "path": file.path, "type": "file", "size": file.size, "modified": twaco::core::logs::iso(file.modified) }));
                }
            } else {
                for folder in &listing.folders {
                    println!("{folder}/");
                }
                for file in &listing.files {
                    println!("{:>12}  {}  {}", file.size, twaco::core::logs::local(file.modified), file.path);
                }
            }
            eprintln!("{} folder(s), {} file(s)", listing.folders.len(), listing.files.len());
            Ok(())
        }
        ["get", repository, path] => {
            let request = RepoRequest {
                action: RepoAction::Get {
                    repository: repository.to_string(),
                    path: path.to_string(),
                    out: args.out.clone(),
                    force: args.has("--force"),
                },
                profile: profile_name.to_string(),
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(|e| e.to_string())?;
            print_notices(&notices);
            let command::RepoOutcome::Got { bytes, out, .. } = outcome else {
                unreachable!()
            };
            match out {
                None => {
                    use std::io::Write;
                    std::io::stdout().write_all(&bytes).map_err(|e| e.to_string())
                }
                Some(out) => {
                    eprintln!("{} bytes to {}", bytes.len(), out.display());
                    Ok(())
                }
            }
        }
        ["status", repository] => {
            let request = RepoRequest {
                action: RepoAction::Status {
                    repository: repository.to_string(),
                },
                profile: profile_name.to_string(),
            };
            let mut notices = commands::Notices::default();
            let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
                .map_err(|e| e.to_string())?;
            print_notices(&notices);
            let command::RepoOutcome::Status { local: root, compared, .. } = outcome else {
                unreachable!()
            };
            let mut counts = std::collections::BTreeMap::new();
            for item in &compared {
                *counts.entry(item.state.label()).or_insert(0usize) += 1;
                if args.has("--json") {
                    println!(
                        "{}",
                        serde_json::json!({ "path": item.path, "state": item.state.label(), "local_size": item.local_size, "remote_size": item.remote_size })
                    );
                } else if item.state != repo::State::Same {
                    println!("{:<11} {}", item.state.label(), item.path);
                }
            }
            let summary: Vec<String> = counts.iter().map(|(state, n)| format!("{n} {state}")).collect();
            eprintln!(
                "{} against {}: {}",
                repository,
                root.display(),
                if summary.is_empty() { "both empty".to_string() } else { summary.join(", ") }
            );
            Ok(())
        }
        ["put", repository, file, path] => {
            let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            let path = repo::remote_path(path).map_err(|e| e.to_string())?;
            repo_change(solution, repository, repo::Change::Put { path, bytes, overwrite: args.has("--overwrite") }, profile_name, args)
        }
        ["mkdir", repository, path] => {
            let path = repo::remote_path(path).map_err(|e| e.to_string())?;
            repo_change(solution, repository, repo::Change::Mkdir { path }, profile_name, args)
        }
        ["rm", repository, path] => {
            let path = repo::remote_path(path).map_err(|e| e.to_string())?;
            if path == "/" {
                return Err("the repository root cannot be deleted".to_string());
            }
            repo_change(solution, repository, repo::Change::Remove { path, recursive: args.has("--recursive") }, profile_name, args)
        }
        [direction @ ("push" | "pull"), repository] => {
            let way = if *direction == "push" { repo::Direction::Push } else { repo::Direction::Pull };
            let apply = args.has("--apply");
            let request = RepoRequest {
                action: RepoAction::Sync {
                    repository: (*repository).to_string(), direction: way, overwrite: args.has("--overwrite"),
                    mode: if apply { commands::Mode::Apply } else { commands::Mode::Plan },
                },
                profile: profile_name.to_string(),
            };
            let mut notices = commands::Notices::default();
            let result = command::execute(solution, &request, server::Client::new, &mut notices);
            print_notices(&notices);
            let outcome = result.map_err(|e| e.to_string())?;
            let command::RepoOutcome::Synced { synced, .. } = outcome else { unreachable!() };
            let verb = match (way, apply) {
                (repo::Direction::Push, true) => "uploaded",
                (repo::Direction::Push, false) => "would upload",
                (repo::Direction::Pull, true) => "downloaded",
                (repo::Direction::Pull, false) => "would download",
            };
            for path in &synced.copied {
                println!("{verb} {path}");
            }
            for path in &synced.left {
                println!("left alone (only {}) {path}", if way == repo::Direction::Push { "on the server" } else { "here" });
            }
            println!(
                "{verb} {} file(s); {} the same; {} left alone{}",
                synced.copied.len(),
                synced.same,
                synced.left.len(),
                if apply || synced.copied.is_empty() { "" } else { "; nothing sent (pass --apply)" }
            );
            Ok(())
        }
        ["mv", repository, from, to] => {
            let from = repo::remote_path(from).map_err(|e| e.to_string())?;
            let to = repo::remote_path(to).map_err(|e| e.to_string())?;
            repo_change(solution, repository, repo::Change::Move { from, to, overwrite: args.has("--overwrite") }, profile_name, args)
        }
        _ => Err("repo takes: list | ls <repo> [<path>] | get <repo> <path> | status <repo> | put <repo> <file> <path> | mkdir <repo> <path> | rm <repo> <path> | mv <repo> <from> <to>".to_string()),
    }
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: repo: {why}");
            FAILED
        }
    }
}

/// One repository change: a plan unless --apply, applied and read back with it.
fn repo_change(
    solution: &Solution,
    repository: &str,
    change: twaco::core::repo::Change,
    profile: &str,
    args: &Args,
) -> Result<(), String> {
    use twaco::core::commands::repo::{self as command, RepoAction, RepoRequest};
    let request = RepoRequest {
        action: RepoAction::Change {
            repository: repository.to_string(),
            change,
            mode: if args.has("--apply") {
                commands::Mode::Apply
            } else {
                commands::Mode::Plan
            },
        },
        profile: profile.to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome = command::execute(solution, &request, server::Client::new, &mut notices)
        .map_err(|e| e.to_string())?;
    print_notices(&notices);
    let command::RepoOutcome::Changed { planned, .. } = outcome else {
        unreachable!()
    };
    if planned.nothing {
        println!("{}: nothing to do", planned.plan);
    } else if planned.applied {
        println!("done: {} in {repository}, read back", planned.plan);
    } else {
        println!(
            "would {} in {repository}; nothing sent (pass --apply)",
            planned.plan
        );
    }
    Ok(())
}
