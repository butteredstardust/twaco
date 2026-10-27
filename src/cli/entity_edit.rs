use super::flags::{Apply, Json};
use super::style;
use crate::{print_notices, FAILED, OK};
use std::path::PathBuf;
use twaco::core::commands::{self, Mode};
use twaco::core::config::Solution;
use twaco::core::entity;
use twaco::core::entity::edit::{Action, AgentChange, Property, Request, Service, StyleChange};

/// `twaco thing property|service|subscription|shape`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Thing {
    Property(ThingProperty),
    Service(ThingService),
    Subscription(ThingSubscription),
    Shape(ThingShape),
}

/// `twaco thing service add|remove <Entity> <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct ThingService {
    #[arg(long = "param", value_name = "name:TYPE[:...]")]
    pub(crate) parameters: Vec<String>,
    #[arg(long = "returns", value_name = "TYPE[:DataShape]")]
    pub(crate) returns: Option<String>,
    #[arg(long = "description", value_name = "text")]
    pub(crate) description: Option<String>,
    #[arg(long = "category", value_name = "text")]
    pub(crate) category: Option<String>,
    #[arg(long = "private")]
    pub(crate) private: bool,
    #[arg(long = "then", value_name = "address")]
    pub(crate) then: Option<String>,
    #[arg(long = "script", value_name = "file.js", value_parser = super::flags::path())]
    pub(crate) script: Option<PathBuf>,
    #[arg(long = "force")]
    pub(crate) force: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco thing property add|set|remove <Entity> <name[:TYPE]>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct ThingProperty {
    /// Empty when given bare, which the command refuses.
    #[arg(long = "default", value_name = "value", num_args = 0..=1)]
    pub(crate) default: Option<String>,
    #[arg(long = "no-default")]
    pub(crate) no_default: bool,
    #[arg(long = "persistent")]
    pub(crate) persistent: bool,
    #[arg(long = "no-persistent")]
    pub(crate) no_persistent: bool,
    #[arg(long = "logged")]
    pub(crate) logged: bool,
    #[arg(long = "no-logged")]
    pub(crate) no_logged: bool,
    #[arg(long = "read-only")]
    pub(crate) read_only: bool,
    #[arg(long = "no-read-only")]
    pub(crate) no_read_only: bool,
    #[arg(long = "category", value_name = "text")]
    pub(crate) category: Option<String>,
    #[arg(long = "units", value_name = "text")]
    pub(crate) units: Option<String>,
    #[arg(long = "data-shape", value_name = "DataShape")]
    pub(crate) data_shape: Option<String>,
    #[arg(long = "description", value_name = "text")]
    pub(crate) description: Option<String>,
    #[arg(long = "force")]
    pub(crate) force: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco thing subscription add|remove <Entity> <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct ThingSubscription {
    #[arg(long = "event", value_name = "address")]
    pub(crate) event: Option<String>,
    #[arg(long = "source", value_name = "Thing")]
    pub(crate) source: Option<String>,
    #[arg(long = "source-property", value_name = "property")]
    pub(crate) source_property: Option<String>,
    #[arg(long = "then", value_name = "address")]
    pub(crate) then: Option<String>,
    #[arg(long = "script", value_name = "file.js", value_parser = super::flags::path())]
    pub(crate) script: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco thing shape add|remove <Thing|ThingTemplate> <ThingShape>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct ThingShape {
    #[arg(long = "force")]
    pub(crate) force: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco timer set`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Timer {
    Set(TimerSet),
}

/// `twaco timer set <Timer>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct TimerSet {
    #[arg(long = "every", value_name = "seconds")]
    pub(crate) every: Option<String>,
    #[arg(long = "run-as", value_name = "user")]
    pub(crate) run_as: Option<String>,
    #[arg(long = "enabled")]
    pub(crate) enabled: bool,
    #[arg(long = "disabled")]
    pub(crate) disabled: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco scheduler set`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Scheduler {
    Set(SchedulerSet),
}

/// `twaco scheduler set <Scheduler>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct SchedulerSet {
    #[arg(long = "cron", value_name = "quartz expression")]
    pub(crate) cron: Option<String>,
    #[arg(long = "run-as", value_name = "user")]
    pub(crate) run_as: Option<String>,
    #[arg(long = "enabled")]
    pub(crate) enabled: bool,
    #[arg(long = "disabled")]
    pub(crate) disabled: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco style set`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Style {
    Set(StyleSet),
}

/// `twaco style set <StyleDefinition>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct StyleSet {
    #[arg(long = "background", value_name = "color")]
    pub(crate) background: Option<String>,
    #[arg(long = "foreground", value_name = "color")]
    pub(crate) foreground: Option<String>,
    #[arg(long = "secondary-background", value_name = "color")]
    pub(crate) secondary_background: Option<String>,
    #[arg(long = "line-color", value_name = "color")]
    pub(crate) line_color: Option<String>,
    #[arg(long = "line-style", value_name = "solid|dashed|dotted|none")]
    pub(crate) line_style: Option<String>,
    #[arg(long = "line-thickness", value_name = "n")]
    pub(crate) line_thickness: Option<String>,
    #[arg(long = "bold")]
    pub(crate) bold: bool,
    #[arg(long = "no-bold")]
    pub(crate) no_bold: bool,
    #[arg(long = "italic")]
    pub(crate) italic: bool,
    #[arg(long = "no-italic")]
    pub(crate) no_italic: bool,
    #[arg(long = "underline")]
    pub(crate) underline: bool,
    #[arg(long = "no-underline")]
    pub(crate) no_underline: bool,
    #[arg(long = "text-size", value_name = "size")]
    pub(crate) text_size: Option<String>,
    #[arg(long = "image", value_name = "MediaEntity")]
    pub(crate) image: Option<String>,
    #[arg(long = "display-string", value_name = "text")]
    pub(crate) display_string: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco media set`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Media {
    Set(MediaSet),
}

/// `twaco media set <MediaEntity> --file <path>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MediaSet {
    #[arg(long = "file", value_name = "path", value_parser = super::flags::path())]
    pub(crate) file: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco menu item`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Menu {
    Item(MenuItem),
}

/// `twaco menu item add|remove <Menu> <Title=Mashup|Title>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MenuItem {
    /// A switch here: it is empty when given bare, and the command refuses a value.
    #[arg(long = "default", value_name = "value", num_args = 0..=1)]
    pub(crate) default: Option<String>,
    #[arg(long = "before", value_name = "Title")]
    pub(crate) before: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco state set`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum State {
    Set(StateSet),
}

/// `twaco state set <StateDefinition> --type numeric|string --state <spec>...`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct StateSet {
    #[arg(long = "type", value_name = "value")]
    pub(crate) kind: Option<String>,
    #[arg(long = "state", value_name = "spec")]
    pub(crate) states: Vec<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// What an edit command line asks for, as [`entity_edit_cmd`] runs it: the entity and the
/// action, or why the line does not say, and how to answer.
pub(crate) struct Edit {
    found: Result<(String, Action), String>,
    apply: bool,
    json: bool,
}

impl Edit {
    fn new(found: Result<(String, Action), String>, apply: &Apply, json: &Json) -> Edit {
        Edit {
            found,
            apply: apply.apply,
            json: json.json,
        }
    }
}

impl Thing {
    /// The edit the line asks for. `--script` is read here, so it is read where the command
    /// runs, after the solution is found.
    pub(crate) fn edit(&self) -> Edit {
        match self {
            Thing::Property(args) => Edit::new(property(args), &args.apply, &args.json),
            Thing::Service(args) => Edit::new(service(args), &args.apply, &args.json),
            Thing::Subscription(args) => Edit::new(subscription(args), &args.apply, &args.json),
            Thing::Shape(args) => Edit::new(shape(args), &args.apply, &args.json),
        }
    }
}

/// `thing service add|remove`.
fn service(args: &ThingService) -> Result<(String, Action), String> {
    let [action, entity, name] = args.operands.as_slice() else {
        return Err("thing service needs <add|remove> <Entity> <name>".into());
    };
    let action = match action.as_str() {
        "add" => {
            let code = match (&args.then, &args.script) {
                (Some(code), None) => code.clone(),
                (None, Some(path)) => std::fs::read_to_string(path)
                    .map_err(|error| format!("{}: {error}", path.display()))?,
                (None, None) => String::new(),
                _ => return Err("give the script as --then <code> or --script <file>".into()),
            };
            Action::ServiceAdd(Service {
                name: name.clone(),
                parameters: args.parameters.clone(),
                returns: args.returns.clone(),
                description: args.description.clone(),
                category: args.category.clone(),
                private: args.private,
                code,
            })
        }
        "remove" => Action::ServiceRemove {
            name: name.clone(),
            force: args.force,
        },
        other => return Err(format!("thing service {other}: say add or remove")),
    };
    Ok((entity.clone(), action))
}

impl Timer {
    /// The edit the line asks for.
    pub(crate) fn edit(&self) -> Edit {
        let Timer::Set(args) = self;
        let found = (|| {
            let [entity] = args.operands.as_slice() else {
                return Err("timer set needs one <Thing>".to_string());
            };
            let action = Action::TimerSet {
                every: args.every.clone(),
                run_as: args.run_as.clone(),
                enabled: switch((args.enabled, "--enabled"), (args.disabled, "--disabled"))?,
            };
            Ok((entity.clone(), action))
        })();
        Edit::new(found, &args.apply, &args.json)
    }
}

impl Scheduler {
    /// The edit the line asks for.
    pub(crate) fn edit(&self) -> Edit {
        let Scheduler::Set(args) = self;
        let found = (|| {
            let [entity] = args.operands.as_slice() else {
                return Err("scheduler set needs one <Thing>".to_string());
            };
            let action = Action::SchedulerSet {
                cron: args.cron.clone(),
                run_as: args.run_as.clone(),
                enabled: switch((args.enabled, "--enabled"), (args.disabled, "--disabled"))?,
            };
            Ok((entity.clone(), action))
        })();
        Edit::new(found, &args.apply, &args.json)
    }
}

impl Style {
    /// The edit the line asks for.
    pub(crate) fn edit(&self) -> Edit {
        let Style::Set(args) = self;
        let found = (|| {
            let [entity] = args.operands.as_slice() else {
                return Err("style set needs one <StyleDefinition>".to_string());
            };
            let change = StyleChange {
                background: args.background.clone(),
                foreground: args.foreground.clone(),
                secondary_background: args.secondary_background.clone(),
                line_color: args.line_color.clone(),
                line_style: args.line_style.clone(),
                line_thickness: args.line_thickness.clone(),
                bold: switch((args.bold, "--bold"), (args.no_bold, "--no-bold"))?,
                italic: switch((args.italic, "--italic"), (args.no_italic, "--no-italic"))?,
                underline: switch(
                    (args.underline, "--underline"),
                    (args.no_underline, "--no-underline"),
                )?,
                text_size: args.text_size.clone(),
                image: args.image.clone(),
                display_string: args.display_string.clone(),
            };
            Ok((entity.clone(), Action::StyleSet(change)))
        })();
        Edit::new(found, &args.apply, &args.json)
    }
}

impl Media {
    /// The edit the line asks for.
    pub(crate) fn edit(&self) -> Edit {
        let Media::Set(args) = self;
        let found = (|| {
            let [entity] = args.operands.as_slice() else {
                return Err("media set needs one <MediaEntity>".to_string());
            };
            let file = args.file.clone().ok_or("media set needs --file <path>")?;
            Ok((entity.clone(), Action::MediaSet { file }))
        })();
        Edit::new(found, &args.apply, &args.json)
    }
}

impl Menu {
    /// The edit the line asks for.
    pub(crate) fn edit(&self) -> Edit {
        let Menu::Item(args) = self;
        Edit::new(menu_item(args), &args.apply, &args.json)
    }
}

impl State {
    /// The edit the line asks for.
    pub(crate) fn edit(&self) -> Edit {
        let State::Set(args) = self;
        let found = (|| {
            let [entity] = args.operands.as_slice() else {
                return Err("state set needs one <StateDefinition>".to_string());
            };
            let state_type = args
                .kind
                .clone()
                .ok_or("state set needs --type numeric|string")?;
            if args.states.is_empty() {
                return Err("state set needs one or more --state".into());
            }
            let action = Action::StateSet {
                state_type,
                states: args.states.clone(),
            };
            Ok((entity.clone(), action))
        })();
        Edit::new(found, &args.apply, &args.json)
    }
}

/// The edit commands: `thing property|subscription|shape`, `timer set`, `scheduler set`,
/// `style set`, `media set`, `menu item` and `state set`. Each plans until `--apply`.
pub(crate) fn entity_edit_cmd(solution: &Solution, edit: Edit) -> u8 {
    let (entity, action) = match edit.found {
        Ok(found) => found,
        Err(why) => {
            eprintln!("{} {why}", style::prefix());
            return FAILED;
        }
    };
    let request = commands::entity_edit::EditRequest {
        request: Request { entity, action },
        mode: if edit.apply { Mode::Apply } else { Mode::Plan },
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
    if edit.json {
        let report = entity::edit::plan_json(solution, plan, outcome.applied);
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
        println!("  {}", solution.relative(&change.path));
    }
    for warning in &plan.warnings {
        println!("  warning: {warning}");
    }
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

/// `--on` / `--off` style switches as one optional value; both together is an error.
fn switch(
    (on, on_name): (bool, &str),
    (off, off_name): (bool, &str),
) -> Result<Option<bool>, String> {
    match (on, off) {
        (true, true) => Err(format!("give {on_name} or {off_name}, not both")),
        (true, false) => Ok(Some(true)),
        (false, true) => Ok(Some(false)),
        (false, false) => Ok(None),
    }
}

/// `thing property add|set|remove`.
fn property(args: &ThingProperty) -> Result<(String, Action), String> {
    let [action, entity, member] = args.operands.as_slice() else {
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
            // Given bare, `--default` is empty.
            let default = args.default.clone().filter(|value| !value.is_empty());
            if args.default.is_some() && default.is_none() {
                return Err("--default needs a value".into());
            }
            if args.no_default && default.is_some() {
                return Err("give --default or --no-default, not both".into());
            }
            let property = Property {
                name: name.to_string(),
                base_type,
                default,
                no_default: args.no_default,
                persistent: switch(
                    (args.persistent, "--persistent"),
                    (args.no_persistent, "--no-persistent"),
                )?,
                logged: switch((args.logged, "--logged"), (args.no_logged, "--no-logged"))?,
                read_only: switch(
                    (args.read_only, "--read-only"),
                    (args.no_read_only, "--no-read-only"),
                )?,
                category: args.category.clone(),
                units: args.units.clone(),
                data_shape: args.data_shape.clone(),
                description: args.description.clone(),
            };
            if action == "add" {
                Action::PropertyAdd(property)
            } else {
                Action::PropertySet {
                    property,
                    force: args.force,
                }
            }
        }
        "remove" => Action::PropertyRemove {
            name: member.clone(),
            force: args.force,
        },
        other => return Err(format!("thing property {other}: say add, set or remove")),
    };
    Ok((entity.clone(), action))
}

/// `thing subscription add|remove`.
fn subscription(args: &ThingSubscription) -> Result<(String, Action), String> {
    let [action, entity, name] = args.operands.as_slice() else {
        return Err("thing subscription needs <add|remove> <Entity> <name>".into());
    };
    let action = match action.as_str() {
        "add" => {
            let event = args
                .event
                .clone()
                .ok_or("thing subscription add needs --event <Event>")?;
            let code = match (&args.then, &args.script) {
                (Some(code), None) => code.clone(),
                (None, Some(path)) => std::fs::read_to_string(path)
                    .map_err(|error| format!("{}: {error}", path.display()))?,
                _ => return Err("give the script as --then <code> or --script <file>".into()),
            };
            Action::SubscriptionAdd {
                name: name.clone(),
                event,
                source: args.source.clone(),
                source_property: args.source_property.clone(),
                code,
            }
        }
        "remove" => Action::SubscriptionRemove { name: name.clone() },
        other => return Err(format!("thing subscription {other}: say add or remove")),
    };
    Ok((entity.clone(), action))
}

/// `thing shape add|remove`.
fn shape(args: &ThingShape) -> Result<(String, Action), String> {
    let [action, entity, shape] = args.operands.as_slice() else {
        return Err("thing shape needs <add|remove> <Thing|ThingTemplate> <ThingShape>".into());
    };
    let shape = shape.clone();
    let action = match action.as_str() {
        "add" => Action::ShapeAdd { shape },
        "remove" => Action::ShapeRemove {
            shape,
            force: args.force,
        },
        other => return Err(format!("thing shape {other}: say add or remove")),
    };
    Ok((entity.clone(), action))
}

/// `menu item add|remove`.
fn menu_item(args: &MenuItem) -> Result<(String, Action), String> {
    let [action, entity, item] = args.operands.as_slice() else {
        return Err("menu item needs <add|remove> <Menu> <Title=Mashup|Title>".into());
    };
    let action = match action.as_str() {
        "add" => {
            let (title, mashup) = item
                .split_once('=')
                .ok_or("menu item add needs <Title>=<Mashup>")?;
            if args.default.as_ref().is_some_and(|value| !value.is_empty()) {
                return Err("--default takes no value here".into());
            }
            Action::MenuItemAdd {
                title: title.to_string(),
                mashup: mashup.to_string(),
                default: args.default.is_some(),
                before: args.before.clone(),
            }
        }
        "remove" => Action::MenuItemRemove {
            title: item.clone(),
        },
        other => return Err(format!("menu item {other}: say add or remove")),
    };
    Ok((entity.clone(), action))
}

/// `twaco mcp-namespace add|remove`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum McpNamespace {
    Add(McpNamespaceAdd),
    Remove(McpNamespaceRemove),
}

/// `twaco mcp-namespace add <Namespace> tool|service-resource|file-resource|prompt <spec>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct McpNamespaceAdd {
    #[arg(long = "description", value_name = "text")]
    pub(crate) description: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mcp-namespace remove <Namespace> <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct McpNamespaceRemove {
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

impl McpNamespace {
    /// The edit the line asks for.
    pub(crate) fn edit(&self) -> Edit {
        match self {
            McpNamespace::Add(args) => {
                let found = match args.operands.as_slice() {
                    [entity, kind, spec] => Ok((
                        entity.clone(),
                        Action::McpAdd {
                            kind: kind.clone(),
                            spec: spec.clone(),
                            describe: args.description.clone(),
                        },
                    )),
                    _ => Err("mcp-namespace add needs <Namespace> <tool|service-resource|file-resource|prompt> <spec>".to_string()),
                };
                Edit::new(found, &args.apply, &args.json)
            }
            McpNamespace::Remove(args) => {
                let found = match args.operands.as_slice() {
                    [entity, name] => {
                        Ok((entity.clone(), Action::McpRemove { name: name.clone() }))
                    }
                    _ => Err("mcp-namespace remove needs <Namespace> <name>".to_string()),
                };
                Edit::new(found, &args.apply, &args.json)
            }
        }
    }
}

/// `twaco agent allow|disallow|taxonomy|skill|set`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Agent {
    Allow(AgentOperands),
    Disallow(AgentOperands),
    Taxonomy(AgentOperands),
    Skill(AgentOperands),
    Set(AgentSet),
}

/// The agent commands that take operands only.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct AgentOperands {
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco agent set <Agent> [--resolver <Thing>] [--instructions <text>|--instructions-file <file>]
/// [--agent-description <text>] [--display-name <text>]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct AgentSet {
    #[arg(long = "resolver", value_name = "Thing")]
    pub(crate) resolver: Option<String>,
    #[arg(long = "instructions", value_name = "text")]
    pub(crate) instructions: Option<String>,
    #[arg(long = "instructions-file", value_name = "file")]
    pub(crate) instructions_file: Option<PathBuf>,
    #[arg(long = "agent-description", value_name = "text")]
    pub(crate) agent_description: Option<String>,
    #[arg(long = "display-name", value_name = "text")]
    pub(crate) display_name: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

impl Agent {
    /// The edit the line asks for.
    pub(crate) fn edit(&self) -> Edit {
        match self {
            Agent::Allow(args) | Agent::Disallow(args) => {
                let allow = matches!(self, Agent::Allow(_));
                let found = match args.operands.as_slice() {
                    [entity, spec] => Ok((
                        entity.clone(),
                        if allow {
                            Action::AgentAllow { spec: spec.clone() }
                        } else {
                            Action::AgentDisallow { spec: spec.clone() }
                        },
                    )),
                    _ => Err(format!(
                        "agent {} needs <Agent> <Namespace[=item,...]>",
                        if allow { "allow" } else { "disallow" }
                    )),
                };
                Edit::new(found, &args.apply, &args.json)
            }
            Agent::Taxonomy(args) | Agent::Skill(args) => {
                let taxonomy = matches!(self, Agent::Taxonomy(_));
                let found = match args.operands.as_slice() {
                    [action, entity, value] => {
                        let value = value.clone();
                        match (taxonomy, action.as_str()) {
                            (true, "add") => Ok(Action::AgentTaxonomyAdd { spec: value }),
                            (true, "remove") => Ok(Action::AgentTaxonomyRemove { name: value }),
                            (false, "add") => Ok(Action::AgentSkillAdd { spec: value }),
                            (false, "remove") => Ok(Action::AgentSkillRemove { spec: value }),
                            (_, other) => Err(format!("{other}: say add or remove")),
                        }
                        .map(|action| (entity.clone(), action))
                    }
                    _ if taxonomy => Err(
                        "agent taxonomy needs <add|remove> <Agent> <TemplateOrShape[=aliases]>"
                            .to_string(),
                    ),
                    _ => {
                        Err("agent skill needs <add|remove> <Agent> <Repository:/path>".to_string())
                    }
                };
                Edit::new(found, &args.apply, &args.json)
            }
            Agent::Set(args) => {
                let found = match args.operands.as_slice() {
                    [entity] => Ok((
                        entity.clone(),
                        Action::AgentSet(AgentChange {
                            resolver: args.resolver.clone(),
                            instructions: args.instructions.clone(),
                            instructions_file: args.instructions_file.clone(),
                            agent_description: args.agent_description.clone(),
                            display_name: args.display_name.clone(),
                        }),
                    )),
                    _ => Err("agent set needs one <Agent>".to_string()),
                };
                Edit::new(found, &args.apply, &args.json)
            }
        }
    }
}
