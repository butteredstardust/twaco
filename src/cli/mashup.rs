use super::flags::{Apply, Json, Project, Selection};
use super::style;
use crate::{print_notices, DRIFT, FAILED, OK};
use twaco::core::commands::{self, Mode};
use twaco::core::config::Solution;
use twaco::core::mashup;
use twaco::core::mashup::edit::{Placement, Side};

/// `twaco mashup <command>`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Mashup {
    Validate(MashupValidate),
    Describe(MashupDescribe),
    Add(MashupAdd),
    AddContained(MashupAddContained),
    Bind(MashupBind),
    On(MashupOn),
    Param(MashupParam),
    Recipe(Box<MashupRecipe>),
    Event(MashupEvent),
    Templates(MashupTemplates),
    New(MashupNew),
}

impl Mashup {
    /// The command's [`USAGE`](super::spec::USAGE) path.
    pub(crate) fn path(&self) -> &'static str {
        match self {
            Mashup::Validate(_) => "mashup validate",
            Mashup::Describe(_) => "mashup describe",
            Mashup::Add(_) => "mashup add",
            Mashup::AddContained(_) => "mashup add-contained",
            Mashup::Bind(_) => "mashup bind",
            Mashup::On(_) => "mashup on",
            Mashup::Param(_) => "mashup param",
            Mashup::Recipe(_) => "mashup recipe",
            Mashup::Event(_) => "mashup event",
            Mashup::Templates(_) => "mashup templates",
            Mashup::New(_) => "mashup new",
        }
    }
}

/// `twaco mashup validate <mashup>|--all`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupValidate {
    #[command(flatten)]
    pub(crate) selection: Selection,
    #[command(flatten)]
    pub(crate) json: Json,
}

/// `twaco mashup describe <mashup>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupDescribe {
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// Where a widget or a contained mashup goes: at most one of these.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Place {
    #[arg(long = "in", value_name = "widget")]
    pub(crate) inside: Option<String>,
    #[arg(long = "right-of", value_name = "widget")]
    pub(crate) right_of: Option<String>,
    #[arg(long = "left-of", value_name = "widget")]
    pub(crate) left_of: Option<String>,
    #[arg(long = "above", value_name = "widget")]
    pub(crate) above: Option<String>,
    #[arg(long = "below", value_name = "widget")]
    pub(crate) below: Option<String>,
}

/// `twaco mashup add <mashup> <widget>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupAdd {
    #[command(flatten)]
    pub(crate) place: Place,
    #[arg(long = "label", value_name = "text")]
    pub(crate) label: Option<String>,
    #[arg(long = "prop", value_name = "name=value")]
    pub(crate) props: Vec<String>,
    #[arg(long = "input", value_name = "name:TYPE")]
    pub(crate) inputs: Vec<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mashup add-contained <parent> <child>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupAddContained {
    #[command(flatten)]
    pub(crate) place: Place,
    #[arg(long = "set", value_name = "parameter=value")]
    pub(crate) props: Vec<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mashup bind <mashup> --from <address> --to <address>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupBind {
    #[arg(long = "from", value_name = "address")]
    pub(crate) from: Option<String>,
    #[arg(long = "to", value_name = "address")]
    pub(crate) to: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mashup on <mashup> --event <address> --invoke <address>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupOn {
    #[arg(long = "event", value_name = "address")]
    pub(crate) event: Option<String>,
    #[arg(long = "invoke", value_name = "address")]
    pub(crate) invoke: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mashup param add|set|remove <mashup> <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupParam {
    #[arg(long = "type", value_name = "value")]
    pub(crate) kind: Option<String>,
    #[arg(long = "direction", value_name = "IN|OUT|BOTH")]
    pub(crate) direction: Option<String>,
    #[arg(long = "description", value_name = "text")]
    pub(crate) description: Option<String>,
    /// Empty when given bare, which the command refuses.
    #[arg(long = "default", value_name = "value", num_args = 0..=1)]
    pub(crate) default: Option<String>,
    #[arg(long = "no-default")]
    pub(crate) no_default: bool,
    #[arg(long = "data-shape", value_name = "DataShape")]
    pub(crate) data_shape: Option<String>,
    #[arg(long = "force")]
    pub(crate) force: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    /// What to do; clap refuses any other word, with the ones it takes.
    #[arg(id = "action", value_name = "action", value_enum)]
    pub(crate) action: Option<ParamVerb>,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// What `mashup param` does to the parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum ParamVerb {
    Add,
    Set,
    Remove,
}

/// `twaco mashup recipe <recipe> <mashup> [<widget>]`: each recipe reads the flags it needs.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupRecipe {
    #[arg(long = "if", value_name = "condition")]
    pub(crate) condition: Option<String>,
    #[arg(long = "expr", value_name = "expression")]
    pub(crate) expression: Option<String>,
    #[arg(long = "input", value_name = "name:TYPE")]
    pub(crate) inputs: Vec<String>,
    #[arg(long = "after", value_name = "address")]
    pub(crate) after: Option<String>,
    #[arg(long = "say", value_name = "text")]
    pub(crate) say: Option<String>,
    #[arg(long = "from", value_name = "address")]
    pub(crate) from: Option<String>,
    #[arg(long = "kind", value_name = "info|warning|error")]
    pub(crate) kind: Option<String>,
    #[arg(long = "then", value_name = "address")]
    pub(crate) then: Option<String>,
    #[arg(long = "title", value_name = "text")]
    pub(crate) title: Option<String>,
    #[arg(long = "on", value_name = "address")]
    pub(crate) on: Option<String>,
    #[arg(long = "mashup", value_name = "name")]
    pub(crate) mashup: Option<String>,
    #[arg(long = "window", value_name = "type")]
    pub(crate) window: Option<String>,
    #[arg(long = "size", value_name = "WxH")]
    pub(crate) size: Option<String>,
    #[arg(long = "pass", value_name = "name=address")]
    pub(crate) pass: Vec<String>,
    #[arg(long = "reload", value_name = "address")]
    pub(crate) reload: Option<String>,
    #[arg(long = "every", value_name = "seconds")]
    pub(crate) every: Option<String>,
    #[arg(long = "interval", value_name = "address")]
    pub(crate) interval: Option<String>,
    #[arg(long = "else-say", value_name = "text")]
    pub(crate) else_say: Option<String>,
    #[arg(long = "data", value_name = "address")]
    pub(crate) data: Option<String>,
    #[arg(long = "x", value_name = "field")]
    pub(crate) x: Option<String>,
    #[arg(long = "value", value_name = "text")]
    pub(crate) value: Option<String>,
    #[arg(long = "column", value_name = "field[:Title[:width]]")]
    pub(crate) columns: Vec<String>,
    #[arg(long = "selected", value_name = "address")]
    pub(crate) selected: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mashup event add <mashup> <name> --when <address>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupEvent {
    #[arg(long = "when", value_name = "address")]
    pub(crate) when: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mashup templates [<name>]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupTemplates {
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mashup new <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupNew {
    #[command(flatten)]
    pub(crate) project: Project,
    #[arg(long = "template", value_name = "name")]
    pub(crate) template: Option<String>,
    #[arg(long = "layout", value_name = "layout")]
    pub(crate) layout: Option<String>,
    #[arg(long = "primary-menu", value_name = "address")]
    pub(crate) primary_menu: Option<String>,
    #[arg(long = "secondary-menu", value_name = "address")]
    pub(crate) secondary_menu: Option<String>,
    #[arg(long = "theme", value_name = "theme")]
    pub(crate) theme: Option<String>,
    #[arg(long = "master")]
    pub(crate) master: bool,
    #[arg(long = "visibility", value_name = "principal")]
    pub(crate) visibility: Vec<String>,
    #[arg(long = "param", value_name = "name:TYPE[:...]")]
    pub(crate) params: Vec<String>,
    #[arg(long = "tab", value_name = "Label=Mashup")]
    pub(crate) tabs: Vec<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mashup new <name>`: plan a new mashup, or create it with `--apply`.
pub(crate) fn mashup_new_cmd(solution: &Solution, args: &MashupNew) -> u8 {
    let [name] = args.operands.as_slice() else {
        eprintln!("{} mashup new needs one <name>", style::prefix());
        return FAILED;
    };
    if args.template.is_some() && args.layout.is_some() {
        eprintln!("{} give --template or --layout, not both", style::prefix());
        return FAILED;
    }
    let template = args
        .template
        .as_ref()
        .or(args.layout.as_ref())
        .cloned()
        .unwrap_or_else(|| "blank".into());
    let apply = args.apply.apply;
    let request = commands::mashup::MashupNewRequest {
        request: mashup::authoring::NewRequest {
            name: name.clone(),
            project: args.project.project.clone(),
            template,
            primary_menu: args.primary_menu.clone(),
            secondary_menu: args.secondary_menu.clone(),
            theme: args.theme.clone(),
            master: args.master,
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
    let report = mashup::authoring::new_plan_json(solution, &outcome.plan, outcome.applied);
    if args.json.json {
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
        mashup::authoring::template_slots(&plan.template).join(", ")
    );
    let parameters =
        twaco::core::mashup::describe::mashup_parameters(&plan.entity).unwrap_or_default();
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
pub(crate) fn mashup_recipe_cmd(solution: &Solution, args: &MashupRecipe) -> u8 {
    let (name, mashup, widget) = match args.operands.as_slice() {
        [name, mashup] => (name, mashup, None),
        [name, mashup, widget] => (name, mashup, Some(widget.clone())),
        _ => {
            eprintln!(
                "{} mashup recipe needs <recipe> <mashup> [<widget>]; the recipes are {}",
                style::prefix(),
                mashup::authoring::RECIPES.join(", ")
            );
            return FAILED;
        }
    };
    let every = match args.every.clone().map(|text| text.parse::<u32>()) {
        None => None,
        Some(Ok(seconds)) => Some(seconds),
        Some(Err(_)) => {
            eprintln!("{} --every takes whole seconds", style::prefix());
            return FAILED;
        }
    };
    let recipe_args = mashup::authoring::RecipeArgs {
        widget,
        condition: args.condition.clone(),
        expression: args.expression.clone(),
        inputs: args.inputs.clone(),
        after: args.after.clone(),
        say: args.say.clone(),
        from: args.from.clone(),
        kind: args.kind.clone(),
        then: args.then.clone(),
        title: args.title.clone(),
        on: args.on.clone(),
        target: args.mashup.clone(),
        window: args.window.clone(),
        size: args.size.clone(),
        pass: args.pass.clone(),
        reload: args.reload.clone(),
        every,
        interval: args.interval.clone(),
        else_say: args.else_say.clone(),
        data: args.data.clone(),
        x: args.x.clone(),
        value: args.value.clone(),
        columns: args.columns.clone(),
        selected: args.selected.clone(),
    };
    let recipe = match mashup::authoring::Recipe::from_args(name, &recipe_args) {
        Ok(recipe) => recipe,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let request = commands::mashup::MashupRecipeRequest {
        request: mashup::authoring::RecipeRequest {
            mashup: mashup.clone(),
            recipe,
        },
        mode: if args.apply.apply {
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
    let report = mashup::authoring::recipe_plan_json(solution, &outcome.plan, outcome.applied);
    if args.json.json {
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
pub(crate) fn mashup_event_cmd(solution: &Solution, args: &MashupEvent) -> u8 {
    let [action, mashup, name] = args.operands.as_slice() else {
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
    let Some(when) = args.when.as_ref() else {
        eprintln!(
            "{} mashup event add needs --when <address>: the event in {mashup} that fires it",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupEventRequest {
        request: mashup::authoring::EventRequest {
            mashup: mashup.clone(),
            name: name.clone(),
            when: when.clone(),
        },
        mode: if args.apply.apply {
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
    let report = mashup::authoring::parameter_plan_json(solution, &outcome.plan, outcome.applied);
    if args.json.json {
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

pub(crate) fn mashup_param_cmd(solution: &Solution, args: &MashupParam) -> u8 {
    let (Some(action), [mashup, name]) = (args.action, args.operands.as_slice()) else {
        eprintln!(
            "{} mashup param needs add|set|remove <mashup> <name>",
            style::prefix()
        );
        return FAILED;
    };
    let action = match action {
        ParamVerb::Add => mashup::authoring::ParameterAction::Add,
        ParamVerb::Set => mashup::authoring::ParameterAction::Set,
        ParamVerb::Remove => mashup::authoring::ParameterAction::Remove,
    };
    // Given bare, `--default` is empty.
    let given = args.default.as_ref().filter(|value| !value.is_empty());
    if args.default.is_some() && given.is_none() {
        eprintln!("{} --default needs a value", style::prefix());
        return FAILED;
    }
    let default = if args.no_default {
        mashup::authoring::ParameterDefault::Clear
    } else if let Some(value) = given {
        mashup::authoring::ParameterDefault::Set(value.clone())
    } else {
        mashup::authoring::ParameterDefault::Unchanged
    };
    if args.no_default && given.is_some() {
        eprintln!(
            "{} give --default or --no-default, not both",
            style::prefix()
        );
        return FAILED;
    }
    let request = commands::mashup::MashupParameterRequest {
        request: mashup::authoring::ParameterRequest {
            mashup: mashup.clone(),
            action,
            name: name.clone(),
            base_type: args.kind.clone(),
            direction: args.direction.clone(),
            description: args.description.clone(),
            default,
            data_shape: args.data_shape.clone(),
            force: args.force,
        },
        mode: if args.apply.apply {
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
    let report = mashup::authoring::parameter_plan_json(solution, &outcome.plan, outcome.applied);
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("parameter plan serialises")
        );
        return OK;
    }
    let verb = match action {
        mashup::authoring::ParameterAction::Add => ("add", "added"),
        mashup::authoring::ParameterAction::Set => ("change", "changed"),
        mashup::authoring::ParameterAction::Remove => ("remove", "removed"),
    };
    println!(
        "{} parameter {}{} in {}",
        if outcome.applied {
            verb.1.to_string()
        } else {
            format!("would {}", verb.0)
        },
        outcome.plan.name,
        if action == mashup::authoring::ParameterAction::Remove {
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
pub(crate) fn mashup_templates_cmd(args: &MashupTemplates) -> u8 {
    let detail = match args.operands.as_slice() {
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
        Some(name) => match twaco::core::mashup::templates_view::detail(name) {
            Ok(Some(template)) => {
                if args.json.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&template).expect("template serialises")
                    );
                } else {
                    println!("{}: {}", template.name, template.summary);
                    if !template.aliases.is_empty() {
                        println!("aliases: {}", template.aliases.join(", "));
                    }
                    for line in twaco::core::mashup::templates_view::tree_lines(&template) {
                        println!("  {line}");
                    }
                    if let Some(slot) = &template.master_slot {
                        println!("with --master, the page mashup container goes in {slot}");
                    }
                }
                OK
            }
            Ok(None) => {
                let names = twaco::core::mashup::templates_view::list()
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
        None => match twaco::core::mashup::templates_view::list() {
            Ok(templates) => {
                if args.json.json {
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
pub(crate) fn mashup_add_cmd(solution: &Solution, args: &MashupAdd) -> u8 {
    let [mashup, widget] = args.operands.as_slice() else {
        eprintln!("{} mashup add needs <mashup> and <widget>", style::prefix());
        return FAILED;
    };
    let placements: Vec<(&str, &String)> = [
        ("--in", args.place.inside.as_ref()),
        ("--right-of", args.place.right_of.as_ref()),
        ("--left-of", args.place.left_of.as_ref()),
        ("--above", args.place.above.as_ref()),
        ("--below", args.place.below.as_ref()),
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
        request: mashup::authoring::AddRequest {
            mashup: mashup.clone(),
            widget: widget.clone(),
            inputs: args.inputs.clone(),
            placement,
            label: args.label.clone(),
            properties,
        },
        mode: if args.apply.apply {
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
    let report = mashup::authoring::add_plan_json(solution, &outcome.plan, outcome.applied);
    if args.json.json {
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
pub(crate) fn mashup_add_contained_cmd(solution: &Solution, args: &MashupAddContained) -> u8 {
    let [mashup, child] = args.operands.as_slice() else {
        eprintln!(
            "{} mashup add-contained needs <parent> and <child>",
            style::prefix()
        );
        return FAILED;
    };
    let placements: Vec<(&str, &String)> = [
        ("--in", args.place.inside.as_ref()),
        ("--right-of", args.place.right_of.as_ref()),
        ("--left-of", args.place.left_of.as_ref()),
        ("--above", args.place.above.as_ref()),
        ("--below", args.place.below.as_ref()),
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
        request: mashup::authoring::AddContainedRequest {
            mashup: mashup.clone(),
            child: child.clone(),
            placement,
            settings: settings.clone(),
        },
        mode: if args.apply.apply {
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
        mashup::authoring::add_contained_plan_json(solution, &outcome.plan, outcome.applied);
    if args.json.json {
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
pub(crate) fn mashup_bind_cmd(solution: &Solution, args: &MashupBind) -> u8 {
    let [mashup] = args.operands.as_slice() else {
        eprintln!("{} mashup bind needs one <mashup>", style::prefix());
        return FAILED;
    };
    let (Some(from), Some(to)) = (args.from.as_ref(), args.to.as_ref()) else {
        eprintln!(
            "{} mashup bind needs --from <address> and --to <address>",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupBindRequest {
        request: mashup::authoring::BindRequest {
            mashup: mashup.clone(),
            from: from.clone(),
            to: to.clone(),
        },
        mode: if args.apply.apply {
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
    if args.json.json {
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
pub(crate) fn mashup_on_cmd(solution: &Solution, args: &MashupOn) -> u8 {
    let [mashup] = args.operands.as_slice() else {
        eprintln!("{} mashup on needs one <mashup>", style::prefix());
        return FAILED;
    };
    let (Some(event), Some(invoke)) = (args.event.as_ref(), args.invoke.as_ref()) else {
        eprintln!(
            "{} mashup on needs --event <address> and --invoke <address>",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupOnRequest {
        request: mashup::authoring::OnRequest {
            mashup: mashup.clone(),
            event: event.clone(),
            invoke: invoke.clone(),
        },
        mode: if args.apply.apply {
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
    if args.json.json {
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
pub(crate) fn mashup_describe_cmd(solution: &Solution, args: &MashupDescribe) -> u8 {
    let [name] = args.operands.as_slice() else {
        eprintln!("{} mashup describe needs one <mashup>", style::prefix());
        return FAILED;
    };
    match twaco::core::mashup::describe::describe(solution, name) {
        Ok(description) => {
            if args.json.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&description).expect("a description serialises")
                );
            } else {
                print!("{}", twaco::core::mashup::describe::render(&description));
            }
            OK
        }
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            FAILED
        }
    }
}

/// `twaco mashup validate <mashup>|--all`: report problems; exit 1 when there are any.
pub(crate) fn mashup_validate_cmd(solution: &Solution, args: &MashupValidate) -> u8 {
    let name = match (args.selection.operands.as_slice(), args.selection.all) {
        ([name], false) => Some(name.as_str()),
        ([], true) => None,
        _ => {
            eprintln!(
                "{} mashup validate needs one <mashup>, or --all",
                style::prefix()
            );
            return FAILED;
        }
    };
    let reports = match twaco::core::mashup::validate::validate(solution, name) {
        Ok(reports) => reports,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let problems: usize = reports.iter().map(|report| report.problems.len()).sum();
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "problems": problems, "mashups": reports })
            )
            .expect("a validation report serialises")
        );
    } else {
        for report in reports.iter().filter(|report| !report.problems.is_empty()) {
            println!("{} ({})", report.mashup, report.file);
            for problem in &report.problems {
                println!("  [{}] {}", problem.rule, problem.message);
            }
        }
        println!(
            "{} mashup(s) validated, {} with problems, {problems} problem(s)",
            reports.len(),
            reports
                .iter()
                .filter(|report| !report.problems.is_empty())
                .count()
        );
    }
    if problems > 0 {
        DRIFT
    } else {
        OK
    }
}
