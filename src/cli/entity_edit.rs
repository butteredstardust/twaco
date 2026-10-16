use super::super::*;
use super::style;
use twaco::core::entity_edit::{self, Action, Property, Request, StyleChange};

/// The edit commands: `thing property|subscription|shape`, `timer set`, `scheduler set`,
/// `style set`, `media set`, `menu item` and `state set`. Each plans until `--apply`.
pub(crate) fn entity_edit_cmd(solution: &Solution, route: &str, args: &Args) -> u8 {
    let (entity, action) = match edit_action(route, args) {
        Ok(found) => found,
        Err(why) => {
            eprintln!("{} {why}", style::prefix());
            return FAILED;
        }
    };
    let request = commands::entity_edit::EditRequest {
        request: Request { entity, action },
        mode: if args.has("--apply") {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "entity edit",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::entity_edit::execute(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let plan = &outcome.plan;
    if args.has("--json") {
        let report = entity_edit::plan_json(solution, plan, outcome.applied);
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("entity edit plan serialises")
        );
        return OK;
    }
    let verb = if outcome.applied {
        "changed"
    } else {
        "would change"
    };
    println!("{verb} {}: {}", plan.entity, plan.summary);
    for change in &plan.changes {
        println!("  {}", entity_edit::relative(solution, &change.path));
    }
    for warning in &plan.warnings {
        println!("  warning: {warning}");
    }
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

fn value(args: &Args, flag: &str) -> Option<String> {
    args.values.get(flag).cloned()
}

/// `--on` / `--off` style switches as one optional value; both together is an error.
fn switch(args: &Args, on: &str, off: &str) -> Result<Option<bool>, String> {
    match (args.has(on), args.has(off)) {
        (true, true) => Err(format!("give {on} or {off}, not both")),
        (true, false) => Ok(Some(true)),
        (false, true) => Ok(Some(false)),
        (false, false) => Ok(None),
    }
}

/// The entity and the action a command line asks for.
fn edit_action(route: &str, args: &Args) -> Result<(String, Action), String> {
    let names = args.names.as_slice();
    match route {
        "thing property" => {
            let [action, entity, member] = names else {
                return Err("thing property needs <add|set|remove> <Entity> <name[:TYPE]>".into());
            };
            let action = match action.as_str() {
                "add" | "set" => {
                    let (name, base_type) = match member.split_once(':') {
                        Some((name, base_type)) => (name, Some(base_type.to_uppercase())),
                        None => (member.as_str(), None),
                    };
                    if action == "add" && base_type.is_none() {
                        return Err("thing property add needs <name>:<TYPE>".into());
                    }
                    if args.has("--default") && !args.values.contains_key("--default") {
                        return Err("--default needs a value".into());
                    }
                    if args.has("--no-default") && args.values.contains_key("--default") {
                        return Err("give --default or --no-default, not both".into());
                    }
                    let property = Property {
                        name: name.to_string(),
                        base_type,
                        default: value(args, "--default"),
                        no_default: args.has("--no-default"),
                        persistent: switch(args, "--persistent", "--no-persistent")?,
                        logged: switch(args, "--logged", "--no-logged")?,
                        read_only: switch(args, "--read-only", "--no-read-only")?,
                        category: value(args, "--category"),
                        units: value(args, "--units"),
                        data_shape: value(args, "--data-shape"),
                        description: value(args, "--description"),
                    };
                    if action == "add" {
                        Action::PropertyAdd(property)
                    } else {
                        Action::PropertySet {
                            property,
                            force: args.has("--force"),
                        }
                    }
                }
                "remove" => Action::PropertyRemove {
                    name: member.clone(),
                    force: args.has("--force"),
                },
                other => return Err(format!("thing property {other}: say add, set or remove")),
            };
            Ok((entity.clone(), action))
        }
        "thing subscription" => {
            let [action, entity, name] = names else {
                return Err("thing subscription needs <add|remove> <Entity> <name>".into());
            };
            let action = match action.as_str() {
                "add" => {
                    let event = value(args, "--event")
                        .ok_or("thing subscription add needs --event <Event>")?;
                    let code = match (value(args, "--then"), value(args, "--script")) {
                        (Some(code), None) => code,
                        (None, Some(path)) => std::fs::read_to_string(&path)
                            .map_err(|error| format!("{path}: {error}"))?,
                        _ => {
                            return Err("give the script as --then <code> or --script <file>".into())
                        }
                    };
                    Action::SubscriptionAdd {
                        name: name.clone(),
                        event,
                        source: value(args, "--source"),
                        source_property: value(args, "--source-property"),
                        code,
                    }
                }
                "remove" => Action::SubscriptionRemove { name: name.clone() },
                other => return Err(format!("thing subscription {other}: say add or remove")),
            };
            Ok((entity.clone(), action))
        }
        "thing shape" => {
            let [action, entity, shape] = names else {
                return Err(
                    "thing shape needs <add|remove> <Thing|ThingTemplate> <ThingShape>".into(),
                );
            };
            let shape = shape.clone();
            let action = match action.as_str() {
                "add" => Action::ShapeAdd { shape },
                "remove" => Action::ShapeRemove {
                    shape,
                    force: args.has("--force"),
                },
                other => return Err(format!("thing shape {other}: say add or remove")),
            };
            Ok((entity.clone(), action))
        }
        "timer set" | "scheduler set" => {
            let [entity] = names else {
                return Err(format!("{route} needs one <Thing>"));
            };
            let enabled = switch(args, "--enabled", "--disabled")?;
            let run_as = value(args, "--run-as");
            let action = if route == "timer set" {
                Action::TimerSet {
                    every: value(args, "--every"),
                    run_as,
                    enabled,
                }
            } else {
                Action::SchedulerSet {
                    cron: value(args, "--cron"),
                    run_as,
                    enabled,
                }
            };
            Ok((entity.clone(), action))
        }
        "style set" => {
            let [entity] = names else {
                return Err("style set needs one <StyleDefinition>".into());
            };
            let change = StyleChange {
                background: value(args, "--background"),
                foreground: value(args, "--foreground"),
                secondary_background: value(args, "--secondary-background"),
                line_color: value(args, "--line-color"),
                line_style: value(args, "--line-style"),
                line_thickness: value(args, "--line-thickness"),
                bold: switch(args, "--bold", "--no-bold")?,
                italic: switch(args, "--italic", "--no-italic")?,
                underline: switch(args, "--underline", "--no-underline")?,
                text_size: value(args, "--text-size"),
                image: value(args, "--image"),
                display_string: value(args, "--display-string"),
            };
            Ok((entity.clone(), Action::StyleSet(change)))
        }
        "media set" => {
            let [entity] = names else {
                return Err("media set needs one <MediaEntity>".into());
            };
            let file = value(args, "--file").ok_or("media set needs --file <path>")?;
            Ok((entity.clone(), Action::MediaSet { file: file.into() }))
        }
        "menu item" => {
            let [action, entity, item] = names else {
                return Err("menu item needs <add|remove> <Menu> <Title=Mashup|Title>".into());
            };
            let action = match action.as_str() {
                "add" => {
                    let (title, mashup) = item
                        .split_once('=')
                        .ok_or("menu item add needs <Title>=<Mashup>")?;
                    if args.values.contains_key("--default") {
                        return Err("--default takes no value here".into());
                    }
                    Action::MenuItemAdd {
                        title: title.to_string(),
                        mashup: mashup.to_string(),
                        default: args.has("--default"),
                        before: value(args, "--before"),
                    }
                }
                "remove" => Action::MenuItemRemove {
                    title: item.clone(),
                },
                other => return Err(format!("menu item {other}: say add or remove")),
            };
            Ok((entity.clone(), action))
        }
        "state set" => {
            let [entity] = names else {
                return Err("state set needs one <StateDefinition>".into());
            };
            let state_type =
                value(args, "--type").ok_or("state set needs --type numeric|string")?;
            if args.states.is_empty() {
                return Err("state set needs one or more --state".into());
            }
            let action = Action::StateSet {
                state_type,
                states: args.states.clone(),
            };
            Ok((entity.clone(), action))
        }
        other => unreachable!("{other} is not an edit route"),
    }
}
