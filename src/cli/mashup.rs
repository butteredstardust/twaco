use super::super::*;
use super::style;
use twaco::core::mashup_authoring::{self, Layout};
use twaco::core::mashup_edit::{Placement, Side};

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

/// `twaco mashup add <mashup> <widget>`: plan a catalogue widget insertion, or apply it.
pub(crate) fn mashup_add_cmd(solution: &Solution, args: &Args) -> u8 {
    let [mashup, widget] = args.names.as_slice() else {
        eprintln!("{} mashup add needs <mashup> and <widget>", style::prefix());
        return FAILED;
    };
    let placements: Vec<(&str, &String)> = [
        ("--in", args.values.get("--in")),
        ("--right-of", args.values.get("--right-of")),
        ("--left-of", args.values.get("--left-of")),
        ("--above", args.values.get("--above")),
        ("--below", args.values.get("--below")),
    ]
    .into_iter()
    .filter_map(|(flag, value)| value.map(|value| (flag, value)))
    .collect();
    if placements.len() > 1 {
        eprintln!(
            "{} give one of --in, --right-of, --left-of, --above, --below",
            style::prefix()
        );
        return FAILED;
    }
    let placement = placements.first().map(|(flag, value)| match *flag {
        "--in" => Placement::In((*value).clone()),
        "--right-of" => beside(value, Side::Right),
        "--left-of" => beside(value, Side::Left),
        "--above" => beside(value, Side::Above),
        "--below" => beside(value, Side::Below),
        _ => unreachable!("only placement flags are collected"),
    });
    let properties = match args
        .props
        .iter()
        .map(|text| {
            text.split_once('=')
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .ok_or_else(|| format!("--prop {text:?} must be Name=value"))
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(properties) => properties,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let request = commands::mashup::MashupAddRequest {
        request: mashup_authoring::AddRequest {
            mashup: mashup.clone(),
            widget: widget.clone(),
            placement,
            label: args.values.get("--label").cloned(),
            properties,
        },
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup add",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::add(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report = mashup_authoring::add_plan_json(solution, &outcome.plan, outcome.applied);
    if args.has("--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("an add plan serialises")
        );
        return OK;
    }
    let plan = &outcome.plan;
    let label = twaco::core::widgets::catalog()
        .by_type(&plan.widget)
        .expect("the planned widget came from the catalogue")
        .type_display_name
        .as_str();
    let where_ = match placements.first() {
        None => "in center".to_string(),
        Some(("--in", value)) => format!("in {value}"),
        Some(("--right-of", value)) => format!("right of {value}"),
        Some(("--left-of", value)) => format!("left of {value}"),
        Some(("--above", value)) => format!("above {value}"),
        Some(("--below", value)) => format!("below {value}"),
        Some(_) => unreachable!("only placement flags are collected"),
    };
    println!(
        "{} {} ({label}) to {} {where_}",
        if outcome.applied {
            "added"
        } else {
            "would add"
        },
        plan.placed.id,
        plan.edit.mashup
    );
    for container in &plan.placed.containers {
        println!("  new container {container}");
    }
    if let Some(flipped) = &plan.placed.flipped {
        let direction = match placements.first() {
            Some(("--above" | "--below", _)) => "column",
            _ => "row",
        };
        println!("  {flipped} now lays out as a {direction}");
    }
    for wrapped in &plan.placed.wrapped {
        println!("  moved {wrapped} into its own container");
    }
    let files = report["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| file.as_str())
        .collect::<Vec<_>>();
    println!("  files: {}", files.join(", "));
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

fn beside(anchor: &str, side: Side) -> Placement {
    Placement::Beside {
        anchor: anchor.to_string(),
        side,
    }
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
