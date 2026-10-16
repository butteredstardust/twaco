use super::super::*;
use super::style;
use twaco::core::mashup_authoring;
use twaco::core::mashup_edit::{Placement, Side};

/// `twaco mashup new <name>`: plan a new mashup, or create it with `--apply`.
pub(crate) fn mashup_new_cmd(solution: &Solution, args: &Args) -> u8 {
    let [name] = args.names.as_slice() else {
        eprintln!("{} mashup new needs one <name>", style::prefix());
        return FAILED;
    };
    if args.values.contains_key("--template") && args.values.contains_key("--layout") {
        eprintln!("{} give --template or --layout, not both", style::prefix());
        return FAILED;
    }
    let template = args
        .values
        .get("--template")
        .or_else(|| args.values.get("--layout"))
        .cloned()
        .unwrap_or_else(|| "blank".into());
    let apply = args.has("--apply");
    let request = commands::mashup::MashupNewRequest {
        request: mashup_authoring::NewRequest {
            name: name.clone(),
            project: args.project.clone(),
            template,
            primary_menu: args.values.get("--primary-menu").cloned(),
            secondary_menu: args.values.get("--secondary-menu").cloned(),
            theme: args.values.get("--theme").cloned(),
            master: args.has("--master"),
            visibility: args.visibility.clone(),
            params: args.params.clone(),
            tabs: args.tabs.clone(),
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
        "{} {kind} {} in project {} ({} template, theme {})",
        if outcome.applied {
            "created"
        } else {
            "would create"
        },
        plan.name,
        plan.project,
        plan.template.name,
        plan.theme
    );
    println!(
        "  slots: {}",
        mashup_authoring::template_slots(&plan.template).join(", ")
    );
    let parameters =
        twaco::core::mashup_describe::mashup_parameters(&plan.entity).unwrap_or_default();
    if !parameters.is_empty() {
        println!(
            "  parameters: {}",
            parameters
                .iter()
                .map(|p| format!("{} {} ({})", p.name, p.base_type, p.direction))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
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
    for (index, (label, child)) in plan.tabs.iter().enumerate() {
        match child {
            Some(child) => println!("  tab {}: {label} shows {child}", index + 1),
            None => println!("  tab {}: {label} (empty)", index + 1),
        }
    }
    for (service, property) in &plan.menus {
        println!("  menu: {service} -> {property}, loaded with the page");
    }
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

/// `twaco mashup param add|set|remove`: keep Composer's four parameter stores together.
/// `twaco mashup recipe <recipe> <mashup> [<widget>] ...`: plan one recipe, or apply it.
pub(crate) fn mashup_recipe_cmd(solution: &Solution, args: &Args) -> u8 {
    let (name, mashup, widget) = match args.names.as_slice() {
        [name, mashup] => (name, mashup, None),
        [name, mashup, widget] => (name, mashup, Some(widget.clone())),
        _ => {
            eprintln!(
                "{} mashup recipe needs <recipe> <mashup> [<widget>]; the recipes are {}",
                style::prefix(),
                mashup_authoring::RECIPES.join(", ")
            );
            return FAILED;
        }
    };
    let value = |flag: &str| args.values.get(flag).cloned();
    let every = match value("--every").map(|text| text.parse::<u32>()) {
        None => None,
        Some(Ok(seconds)) => Some(seconds),
        Some(Err(_)) => {
            eprintln!("{} --every takes whole seconds", style::prefix());
            return FAILED;
        }
    };
    let recipe_args = mashup_authoring::RecipeArgs {
        widget,
        condition: value("--if"),
        expression: value("--expr"),
        inputs: args.inputs.clone(),
        after: value("--after"),
        say: value("--say"),
        from: value("--from"),
        kind: value("--kind"),
        then: value("--then"),
        title: value("--title"),
        on: value("--on"),
        target: value("--mashup"),
        window: value("--window"),
        size: value("--size"),
        pass: args.pass.clone(),
        reload: value("--reload"),
        every,
        interval: value("--interval"),
        else_say: value("--else-say"),
    };
    let recipe = match mashup_authoring::Recipe::from_args(name, &recipe_args) {
        Ok(recipe) => recipe,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let request = commands::mashup::MashupRecipeRequest {
        request: mashup_authoring::RecipeRequest {
            mashup: mashup.clone(),
            recipe,
        },
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup recipe",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::recipe(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report = mashup_authoring::recipe_plan_json(solution, &outcome.plan, outcome.applied);
    if args.has("--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a recipe plan serialises")
        );
        return OK;
    }
    println!(
        "{} recipe {} in {}",
        if outcome.applied {
            "wrote"
        } else {
            "would write"
        },
        outcome.plan.name,
        mashup
    );
    for id in &outcome.plan.added {
        println!("  added {id}");
    }
    for line in &outcome.plan.wired {
        println!("  {line}");
    }
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

/// `twaco mashup event add <mashup> <name> --when <address>`: plan a child event, or apply it.
pub(crate) fn mashup_event_cmd(solution: &Solution, args: &Args) -> u8 {
    let [action, mashup, name] = args.names.as_slice() else {
        eprintln!(
            "{} mashup event needs add <mashup> <name> --when <address>",
            style::prefix()
        );
        return FAILED;
    };
    if action != "add" {
        eprintln!("{} mashup event takes add", style::prefix());
        return FAILED;
    }
    let Some(when) = args.values.get("--when") else {
        eprintln!(
            "{} mashup event add needs --when <address>: the event in {mashup} that fires it",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupEventRequest {
        request: mashup_authoring::EventRequest {
            mashup: mashup.clone(),
            name: name.clone(),
            when: when.clone(),
        },
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup event",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::event(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report = mashup_authoring::parameter_plan_json(solution, &outcome.plan, outcome.applied);
    if args.has("--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("event plan serialises")
        );
        return OK;
    }
    println!(
        "{} event {name} in {mashup}, fired on {when}",
        if outcome.applied {
            "added"
        } else {
            "would add"
        }
    );
    for (widget, parent) in &outcome.plan.refreshed {
        println!("  copy of the parameters refreshed: {widget} in {parent}");
    }
    println!(
        "  a parent reacts with: twaco mashup on <parent> --event ui:<container>.{name} --invoke <handler>"
    );
    println!(
        "  files: {}",
        report["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

pub(crate) fn mashup_param_cmd(solution: &Solution, args: &Args) -> u8 {
    let [action, mashup, name] = args.names.as_slice() else {
        eprintln!(
            "{} mashup param needs add|set|remove <mashup> <name>",
            style::prefix()
        );
        return FAILED;
    };
    let action = match action.as_str() {
        "add" => mashup_authoring::ParameterAction::Add,
        "set" => mashup_authoring::ParameterAction::Set,
        "remove" => mashup_authoring::ParameterAction::Remove,
        _ => {
            eprintln!("{} action must be add, set or remove", style::prefix());
            return FAILED;
        }
    };
    let default = if args.has("--no-default") {
        mashup_authoring::ParameterDefault::Clear
    } else if let Some(value) = args.values.get("--default") {
        mashup_authoring::ParameterDefault::Set(value.clone())
    } else {
        mashup_authoring::ParameterDefault::Unchanged
    };
    if args.has("--no-default") && args.values.contains_key("--default") {
        eprintln!(
            "{} give --default or --no-default, not both",
            style::prefix()
        );
        return FAILED;
    }
    let request = commands::mashup::MashupParameterRequest {
        request: mashup_authoring::ParameterRequest {
            mashup: mashup.clone(),
            action,
            name: name.clone(),
            base_type: args.values.get("--type").cloned(),
            direction: args.values.get("--direction").cloned(),
            description: args.values.get("--description").cloned(),
            default,
            data_shape: args.values.get("--data-shape").cloned(),
            force: args.has("--force"),
        },
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup param",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::parameter(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report = mashup_authoring::parameter_plan_json(solution, &outcome.plan, outcome.applied);
    if args.has("--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("parameter plan serialises")
        );
        return OK;
    }
    let verb = match action {
        mashup_authoring::ParameterAction::Add => ("add", "added"),
        mashup_authoring::ParameterAction::Set => ("change", "changed"),
        mashup_authoring::ParameterAction::Remove => ("remove", "removed"),
    };
    println!(
        "{} parameter {}{} in {}",
        if outcome.applied {
            verb.1.to_string()
        } else {
            format!("would {}", verb.0)
        },
        outcome.plan.name,
        if action == mashup_authoring::ParameterAction::Remove {
            String::new()
        } else {
            format!(" ({}, {})", outcome.plan.base_type, outcome.plan.direction)
        },
        outcome.plan.mashup
    );
    for (widget, parent) in &outcome.plan.refreshed {
        println!("  copy of the parameters refreshed: {widget} in {parent}");
    }
    for binding in &outcome.plan.retyped {
        println!("  bindings retyped: {binding}");
    }
    for binding in &outcome.plan.removed {
        println!("  bindings removed: {binding}");
    }
    println!(
        "  files: {}",
        report["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|file| file.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

/// `twaco mashup templates [<name>]`: read the embedded layout templates.
pub(crate) fn mashup_templates_cmd(args: &Args) -> u8 {
    let detail = match args.names.as_slice() {
        [] => None,
        [name] => Some(name),
        _ => {
            eprintln!(
                "{} mashup templates takes at most one <name>",
                style::prefix()
            );
            return FAILED;
        }
    };
    match detail {
        Some(name) => match twaco::core::mashup_templates_view::detail(name) {
            Ok(Some(template)) => {
                if args.has("--json") {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&template).expect("template serialises")
                    );
                } else {
                    println!("{}: {}", template.name, template.summary);
                    if !template.aliases.is_empty() {
                        println!("aliases: {}", template.aliases.join(", "));
                    }
                    for line in twaco::core::mashup_templates_view::tree_lines(&template) {
                        println!("  {line}");
                    }
                    if let Some(slot) = &template.master_slot {
                        println!("with --master, the page mashup container goes in {slot}");
                    }
                }
                OK
            }
            Ok(None) => {
                let names = twaco::core::mashup_templates_view::list()
                    .map(|all| {
                        all.into_iter()
                            .map(|t| t.name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                eprintln!(
                    "{} unknown template `{name}`; the templates are {names}",
                    style::prefix()
                );
                FAILED
            }
            Err(error) => {
                eprintln!("{} {error}", style::prefix());
                FAILED
            }
        },
        None => match twaco::core::mashup_templates_view::list() {
            Ok(templates) => {
                if args.has("--json") {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&templates).expect("templates serialise")
                    );
                } else {
                    println!("Mashup templates");
                    for template in templates {
                        let aliases = if template.aliases.is_empty() {
                            String::new()
                        } else {
                            format!(" (also {})", template.aliases.join(", "))
                        };
                        println!("  {:<15} {}{aliases}", template.name, template.summary);
                        println!("  {:<15} slots: {}", "", template.slots.join(", "));
                    }
                }
                OK
            }
            Err(error) => {
                eprintln!("{} {error}", style::prefix());
                FAILED
            }
        },
    }
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
            inputs: args.inputs.clone(),
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
    let widget = twaco::core::widgets::catalog()
        .by_type(&plan.widget)
        .expect("the planned widget came from the catalogue");
    let label = widget.type_display_name.as_str();
    let where_ = match placements.first() {
        None if widget.function => "beside the layout".to_string(),
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

/// `twaco mashup add-contained <parent> <child>`: place a child mashup as a container.
pub(crate) fn mashup_add_contained_cmd(solution: &Solution, args: &Args) -> u8 {
    let [mashup, child] = args.names.as_slice() else {
        eprintln!(
            "{} mashup add-contained needs <parent> and <child>",
            style::prefix()
        );
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
        _ => unreachable!(),
    });
    let settings = match args
        .props
        .iter()
        .map(|text| {
            text.split_once('=')
                .map(|(name, value)| (name.into(), value.into()))
                .ok_or_else(|| format!("--set {text:?} must be parameter=value"))
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let request = commands::mashup::MashupAddContainedRequest {
        request: mashup_authoring::AddContainedRequest {
            mashup: mashup.clone(),
            child: child.clone(),
            placement,
            settings: settings.clone(),
        },
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup add-contained",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::add_contained(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report =
        mashup_authoring::add_contained_plan_json(solution, &outcome.plan, outcome.applied);
    if args.has("--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a contained plan serialises")
        );
        return OK;
    }
    let where_ = match placements.first() {
        None => "in center".to_string(),
        Some(("--in", value)) => format!("in {value}"),
        Some(("--right-of", value)) => format!("right of {value}"),
        Some(("--left-of", value)) => format!("left of {value}"),
        Some(("--above", value)) => format!("above {value}"),
        Some(("--below", value)) => format!("below {value}"),
        Some(_) => unreachable!(),
    };
    println!(
        "{} {} showing {} {where_}",
        if outcome.applied {
            "added"
        } else {
            "would add"
        },
        outcome.plan.placed.id,
        outcome.plan.child
    );
    let parameters = outcome
        .plan
        .parameters
        .iter()
        .map(
            |parameter| match settings.iter().find(|(name, _)| *name == parameter.name) {
                Some((_, value)) => format!(
                    "{} {} ({}, set to {value})",
                    parameter.name, parameter.base_type, parameter.direction
                ),
                None => format!(
                    "{} {} ({})",
                    parameter.name, parameter.base_type, parameter.direction
                ),
            },
        )
        .collect::<Vec<_>>();
    if !parameters.is_empty() {
        println!("  parameters: {}", parameters.join(", "));
    }
    println!(
        "  files: {}",
        report["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

/// `twaco mashup bind <mashup> --from <address> --to <address>`: plan a data binding or apply it.
pub(crate) fn mashup_bind_cmd(solution: &Solution, args: &Args) -> u8 {
    let [mashup] = args.names.as_slice() else {
        eprintln!("{} mashup bind needs one <mashup>", style::prefix());
        return FAILED;
    };
    let (Some(from), Some(to)) = (args.values.get("--from"), args.values.get("--to")) else {
        eprintln!(
            "{} mashup bind needs --from <address> and --to <address>",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupBindRequest {
        request: mashup_authoring::BindRequest {
            mashup: mashup.clone(),
            from: from.clone(),
            to: to.clone(),
        },
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup bind",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::bind(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let mut files = vec![outcome
        .plan
        .edit
        .entity_path
        .strip_prefix(&solution.root)
        .unwrap_or(&outcome.plan.edit.entity_path)
        .to_string_lossy()
        .replace('\\', "/")];
    if let Some(sidecars) = &outcome.plan.edit.sidecars {
        files.push(
            sidecars
                .content_path
                .strip_prefix(&solution.root)
                .unwrap_or(&sidecars.content_path)
                .to_string_lossy()
                .replace('\\', "/"),
        );
    }
    let report = serde_json::json!({ "applied": outcome.applied, "mashup": outcome.plan.edit.mashup, "from": outcome.plan.from, "to": outcome.plan.to, "new_source": outcome.plan.new_source, "warnings": outcome.plan.warnings, "refreshed": outcome.plan.refreshed, "files": files });
    if args.has("--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a bind plan serialises")
        );
        return OK;
    }
    println!(
        "{} {} -> {} in {}",
        if outcome.applied {
            "bound"
        } else {
            "would bind"
        },
        outcome.plan.from,
        outcome.plan.to,
        outcome.plan.edit.mashup
    );
    if let Some(source) = &outcome.plan.new_source {
        println!("  new data source {source}");
    }
    for warning in &outcome.plan.warnings {
        println!("  warning: {warning}");
    }
    for container in &outcome.plan.refreshed {
        println!("  container cache refreshed: {container}");
    }
    println!(
        "  files: {}",
        report["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

/// `twaco mashup on <mashup> --event <address> --invoke <address>`: plan an event handler or apply it.
pub(crate) fn mashup_on_cmd(solution: &Solution, args: &Args) -> u8 {
    let [mashup] = args.names.as_slice() else {
        eprintln!("{} mashup on needs one <mashup>", style::prefix());
        return FAILED;
    };
    let (Some(event), Some(invoke)) = (args.values.get("--event"), args.values.get("--invoke"))
    else {
        eprintln!(
            "{} mashup on needs --event <address> and --invoke <address>",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupOnRequest {
        request: mashup_authoring::OnRequest {
            mashup: mashup.clone(),
            event: event.clone(),
            invoke: invoke.clone(),
        },
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup on",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::on(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let mut files = vec![outcome
        .plan
        .edit
        .entity_path
        .strip_prefix(&solution.root)
        .unwrap_or(&outcome.plan.edit.entity_path)
        .to_string_lossy()
        .replace('\\', "/")];
    if let Some(sidecars) = &outcome.plan.edit.sidecars {
        files.push(
            sidecars
                .content_path
                .strip_prefix(&solution.root)
                .unwrap_or(&sidecars.content_path)
                .to_string_lossy()
                .replace('\\', "/"),
        );
    }
    let report = serde_json::json!({ "applied": outcome.applied, "mashup": outcome.plan.edit.mashup, "event": outcome.plan.event, "invoke": outcome.plan.invoke, "new_source": outcome.plan.new_source, "files": files });
    if args.has("--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("an event plan serialises")
        );
        return OK;
    }
    println!(
        "{} {} on {} in {}",
        if outcome.applied { "runs" } else { "would run" },
        outcome.plan.invoke,
        outcome.plan.event,
        outcome.plan.edit.mashup
    );
    if let Some(source) = &outcome.plan.new_source {
        println!("  new data source {source}");
    }
    println!(
        "  files: {}",
        report["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
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
