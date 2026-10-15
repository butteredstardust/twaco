use super::super::*;
use super::style;
use twaco::core::mashup_authoring::{self, Layout};

/// `twaco mashup new <name>`: plan a new mashup, or create it with `--apply`.
pub(crate) fn mashup_new_cmd(solution: &Solution, args: &Args) -> u8 {
    let [name] = args.names.as_slice() else {
        eprintln!("{} mashup new needs one <name>", style::prefix());
        return FAILED;
    };
    let layout = match args.values.get("--layout").map(String::as_str) {
        None => Layout::Blank,
        Some(word) => match Layout::from_word(word) {
            Some(layout) => layout,
            None => {
                eprintln!("{} --layout is blank, not `{word}`", style::prefix());
                return FAILED;
            }
        },
    };
    let apply = args.has("--apply");
    let request = commands::mashup::MashupNewRequest {
        request: mashup_authoring::NewRequest {
            name: name.clone(),
            project: args.project.clone(),
            layout,
            theme: args.values.get("--theme").cloned(),
            master: args.has("--master"),
            visibility: args.visibility.clone(),
        },
        mode: if apply { Mode::Apply } else { Mode::Plan },
        lock_label: "mashup new",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::new(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report = mashup_authoring::new_plan_json(solution, &outcome.plan, outcome.applied);
    if args.has("--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a plan serialises")
        );
        return OK;
    }
    let plan = &outcome.plan;
    let kind = if plan.master {
        "master mashup"
    } else {
        "mashup"
    };
    println!(
        "{} {kind} {} in project {} ({} layout, theme {})",
        if outcome.applied {
            "created"
        } else {
            "would create"
        },
        plan.name,
        plan.project,
        plan.layout.word(),
        plan.theme
    );
    for file in report["files"].as_array().into_iter().flatten() {
        println!("  {}", file.as_str().unwrap_or_default());
    }
    if plan.visibility.is_empty() {
        println!("  visible to: nobody yet (--visibility grants it)");
    } else {
        for principal in &plan.visibility {
            println!("  visible to: {}/{}", principal.kind, principal.name);
        }
    }
    for widget in plan.widgets.iter().skip(1) {
        println!(
            "  widget {} ({}, {})",
            widget.id, widget.type_id, widget.display_name
        );
    }
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

/// `twaco mashup describe <mashup>`: read-only.
pub(crate) fn mashup_describe_cmd(solution: &Solution, args: &Args) -> u8 {
    let [name] = args.names.as_slice() else {
        eprintln!("{} mashup describe needs one <mashup>", style::prefix());
        return FAILED;
    };
    match twaco::core::mashup_describe::describe(solution, name) {
        Ok(description) => {
            if args.has("--json") {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&description).expect("a description serialises")
                );
            } else {
                print!("{}", twaco::core::mashup_describe::render(&description));
            }
            OK
        }
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            FAILED
        }
    }
}
