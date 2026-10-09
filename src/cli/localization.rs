use super::super::*;
use super::style;
use std::collections::BTreeMap;
use std::path::Path;
use twaco::core::commands::localization::{
    self as command, LocalizationAction, LocalizationOutcome, LocalizationRequest,
};
use twaco::core::localization::{self, Problem, State};

/// `twaco localization`: compare, synchronize, and edit localization tables.
pub(crate) fn localization_cmd(solution: &Solution, args: &Args) -> u8 {
    let profile = args.profile.as_deref().unwrap_or("default").to_string();
    let table = args.values.get("--table").cloned();
    let mode = if args.has("--apply") {
        Mode::Apply
    } else {
        Mode::Plan
    };
    let request = match args.names.as_slice() {
        [action] if action == "status" => LocalizationRequest {
            action: LocalizationAction::Status { table },
            profile,
        },
        [action] if action == "pull" => LocalizationRequest {
            action: LocalizationAction::Pull { table, prune: args.has("--prune"), mode },
            profile,
        },
        [action] if action == "push" => LocalizationRequest {
            action: LocalizationAction::Push { table, prune: args.has("--prune"), mode },
            profile,
        },
        [action, table] if action == "new" => LocalizationRequest {
            action: LocalizationAction::New {
                table: table.clone(),
                project: args.project.clone(),
                header: localization::Header {
                    description: args.values.get("--description").cloned(),
                    language_common: args.values.get("--language-common").cloned(),
                    language_native: args.values.get("--language-native").cloned(),
                },
                mode,
            },
            profile,
        },
        [action, name] if action == "set" => {
            let Some(value) = args.values.get("--value") else {
                return usage("localization set needs --value <text>");
            };
            LocalizationRequest {
                action: LocalizationAction::Set {
                    name: name.clone(), value: value.clone(), table,
                    usage: args.values.get("--usage").cloned(),
                    context: args.values.get("--context").cloned(),
                    project: args.project.clone(), mode,
                },
                profile,
            }
        }
        [action, name] if action == "remove" => LocalizationRequest {
            action: LocalizationAction::Remove { name: name.clone(), table, mode },
            profile,
        },
        [action] if action == "new" => return usage("localization new needs <table>"),
        _ => return usage("localization takes: status | pull | push | new <table> | set <token> --value <text> | remove <token>"),
    };
    let progress = super::progress::reporter();
    let mut notices = commands::Notices::default();
    let result = command::execute_with_progress(
        solution,
        &request,
        server::Client::new,
        &mut notices,
        &progress,
    );
    print_notices(&notices);
    match result {
        Ok(LocalizationOutcome::Status { status, .. }) => print_status(solution, &status, args),
        Ok(LocalizationOutcome::Pulled { pulled, .. }) => {
            print_files(
                solution,
                &pulled.files,
                pulled.applied,
                args.has("--detail"),
            );
            OK
        }
        Ok(LocalizationOutcome::Pushed { pushed, .. }) => {
            print_push(
                solution,
                &pushed.tables,
                pushed.applied,
                args.has("--detail"),
            );
            OK
        }
        Ok(LocalizationOutcome::Edited { edited, .. }) => {
            print_files(
                solution,
                &edited.files,
                edited.applied,
                args.has("--detail"),
            );
            OK
        }
        Err(error) => {
            eprintln!("{} localization: {error}", style::prefix());
            FAILED
        }
    }
}

fn usage(message: &str) -> u8 {
    eprintln!("{} {message}", style::prefix());
    FAILED
}

fn relative(solution: &Solution, path: &Path) -> String {
    path.strip_prefix(&solution.root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Same => "same",
        State::Differs => "differs",
        State::LocalOnly => "local only",
        State::ServerOnly => "server only",
    }
}

fn print_status(solution: &Solution, status: &localization::Status, args: &Args) -> u8 {
    if args.has("--json") {
        println!(
            "{}",
            twaco::mcp::localization_status_json(solution, status, true)
        );
    } else {
        let mut tables: BTreeMap<String, [usize; 4]> = BTreeMap::new();
        for item in &status.compared {
            let count = tables.entry(item.table.clone()).or_default();
            count[match item.state {
                State::Same => 0,
                State::Differs => 1,
                State::LocalOnly => 2,
                State::ServerOnly => 3,
            }] += 1;
        }
        let counts: [usize; 4] = tables.values().fold([0; 4], |mut all, one| {
            for i in 0..4 {
                all[i] += one[i];
            }
            all
        });
        println!(
            "{}/: {} tables, {} tokens: {} same, {} differs, {} local only, {} server only",
            relative(solution, &localization::root(solution)),
            tables.len(),
            counts.iter().sum::<usize>(),
            counts[0],
            counts[1],
            counts[2],
            counts[3]
        );
        for (table, count) in &tables {
            let extra = |n: usize, what: &str| {
                if n == 0 {
                    String::new()
                } else {
                    format!(", {n} {what}")
                }
            };
            println!(
                "  {table:<9} {} tokens: {} same, {} differs{}{}{}",
                count.iter().sum::<usize>(),
                count[0],
                count[1],
                extra(count[2], "local only"),
                extra(count[3], "server only"),
                if status.missing_tables.contains(table) {
                    " (not on the server; push creates it)"
                } else {
                    ""
                }
            );
            if args.has("--detail") {
                for item in status
                    .compared
                    .iter()
                    .filter(|x| x.table == *table && x.state != State::Same)
                {
                    println!("      {:<12} {}", state_name(item.state), item.name);
                }
            }
        }
        let mut kinds = [0; 3];
        for problem in &status.problems {
            kinds[match problem {
                Problem::Untranslated { .. } => 0,
                Problem::NotInDefault { .. } => 1,
                Problem::Duplicate { .. } => 2,
            }] += 1;
        }
        println!(
            "problems: {} untranslated, {} not in Default, {} duplicates",
            kinds[0], kinds[1], kinds[2]
        );
        if args.has("--detail") {
            for problem in &status.problems {
                print_problem(solution, problem);
            }
        }
        for (path, why) in &status.unreadable {
            println!("unreadable: {}: {why}", relative(solution, path));
        }
        if !args.has("--detail") && status.compared.iter().any(|x| x.state != State::Same) {
            println!("--detail lists every token");
        }
    }
    status_exit(status)
}

fn status_exit(status: &localization::Status) -> u8 {
    if status.unreadable.is_empty()
        && !status
            .problems
            .iter()
            .any(|p| matches!(p, Problem::Duplicate { .. } | Problem::NotInDefault { .. }))
    {
        OK
    } else {
        DRIFT
    }
}

fn print_problem(solution: &Solution, problem: &Problem) {
    match problem {
        Problem::Untranslated { table, name } => println!("      untranslated  {table}/{name}"),
        Problem::NotInDefault { table, name, file } => println!(
            "      not in Default  {table}/{name} ({})",
            relative(solution, file)
        ),
        Problem::Duplicate { table, name, files } => println!(
            "      duplicate  {table}/{name} ({})",
            files
                .iter()
                .map(|p| relative(solution, p))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn print_files(
    solution: &Solution,
    files: &[localization::FileChange],
    applied: bool,
    detail: bool,
) {
    if files.is_empty() {
        println!("nothing to do");
        return;
    }
    println!(
        "{} {} file(s)",
        if applied { "wrote" } else { "plan:" },
        files.len()
    );
    for file in files {
        println!(
            "  {}: set {}, removed {}{}",
            relative(solution, &file.path),
            file.set.len(),
            file.removed.len(),
            if file.created { " (new file)" } else { "" }
        );
        if detail {
            for name in file.set.iter().chain(&file.removed) {
                println!("    {name}");
            }
        }
    }
    if !applied {
        println!("--apply to make the change");
    }
}

fn print_push(
    _solution: &Solution,
    tables: &[localization::TablePush],
    applied: bool,
    detail: bool,
) {
    if tables.is_empty() {
        println!("nothing to do");
        return;
    }
    println!("{}", push_summary(tables.len(), applied));
    for table in tables {
        // A table whose only change is a prune is not imported.
        let imports = if table.create || !table.set.is_empty() {
            table.files.len()
        } else {
            0
        };
        println!(
            "  {}: import {imports} file(s), set {}, delete {}{}",
            table.table,
            table.set.len(),
            table.delete.len(),
            if table.create {
                " (creates the table)"
            } else {
                ""
            }
        );
        if detail {
            for name in table.set.iter().chain(&table.delete) {
                println!("    {name}");
            }
        }
    }
    if !applied {
        println!("--apply to make the change");
    }
}

fn push_summary(count: usize, applied: bool) -> String {
    format!(
        "{} {count} table(s){}",
        if applied { "pushed" } else { "plan:" },
        if applied { ", read back" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn status(problems: Vec<Problem>, unreadable: Vec<(PathBuf, String)>) -> localization::Status {
        localization::Status {
            compared: Vec::new(),
            problems,
            missing_tables: Vec::new(),
            unreadable,
        }
    }

    #[test]
    fn status_exit_treats_only_push_blockers_as_drift() {
        assert_eq!(
            status_exit(&status(
                vec![Problem::Untranslated {
                    table: "de".into(),
                    name: "Acme.App.Help".into()
                }],
                Vec::new()
            )),
            OK
        );
        assert_eq!(
            status_exit(&status(
                vec![Problem::NotInDefault {
                    table: "de".into(),
                    name: "Acme.App.X".into(),
                    file: PathBuf::from("localization/de.xml")
                }],
                Vec::new()
            )),
            DRIFT
        );
        assert_eq!(
            status_exit(&status(
                Vec::new(),
                vec![(PathBuf::from("localization/x.xml"), "bad XML".into())]
            )),
            DRIFT
        );
    }

    #[test]
    fn push_plan_summary_says_that_it_is_a_plan() {
        assert_eq!(push_summary(1, false), "plan: 1 table(s)");
        assert_eq!(push_summary(2, true), "pushed 2 table(s), read back");
    }
}
