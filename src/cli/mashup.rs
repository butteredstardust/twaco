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
    Why(MashupWhy),
    Build(MashupBuild),
    Spec(MashupSpec),
    Diff(MashupDiff),
    Add(MashupAdd),
    Collection(MashupCollection),
    StateFormat(MashupStateFormat),
    Set(MashupSet),
    Gap(MashupGap),
    Source(MashupSource),
    AddContained(MashupAddContained),
    Bind(MashupBind),
    On(MashupOn),
    Remove(MashupRemove),
    Move(MashupMove),
    Unbind(MashupUnbind),
    Off(MashupOff),
    Migrate(MashupMigrate),
    Param(MashupParam),
    Recipe(Box<MashupRecipe>),
    Event(MashupEvent),
    Templates(MashupTemplates),
    Tokens(MashupTokens),
    New(MashupNew),
}

impl Mashup {
    /// The command's [`USAGE`](super::spec::USAGE) path.
    pub(crate) fn path(&self) -> &'static str {
        match self {
            Mashup::Validate(_) => "mashup validate",
            Mashup::Describe(_) => "mashup describe",
            Mashup::Why(_) => "mashup why",
            Mashup::Build(_) => "mashup build",
            Mashup::Spec(_) => "mashup spec",
            Mashup::Diff(_) => "mashup diff",
            Mashup::Add(_) => "mashup add",
            Mashup::Collection(collection) => collection.path(),
            Mashup::StateFormat(_) => "mashup state-format set",
            Mashup::Set(_) => "mashup set",
            Mashup::Gap(_) => "mashup gap",
            Mashup::Source(source) => source.path(),
            Mashup::AddContained(_) => "mashup add-contained",
            Mashup::Bind(_) => "mashup bind",
            Mashup::On(_) => "mashup on",
            Mashup::Remove(_) => "mashup remove",
            Mashup::Move(_) => "mashup move",
            Mashup::Unbind(_) => "mashup unbind",
            Mashup::Off(_) => "mashup off",
            Mashup::Migrate(_) => "mashup migrate",
            Mashup::Param(_) => "mashup param",
            Mashup::Recipe(_) => "mashup recipe",
            Mashup::Event(_) => "mashup event",
            Mashup::Templates(_) => "mashup templates",
            Mashup::Tokens(_) => "mashup tokens",
            Mashup::New(_) => "mashup new",
        }
    }
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupCollection {
    #[command(subcommand)]
    pub(crate) command: MashupCollectionCommand,
}
impl MashupCollection {
    fn path(&self) -> &'static str {
        match self.command {
            MashupCollectionCommand::Add(_) => "mashup collection add",
        }
    }
}
#[derive(Debug, clap::Subcommand)]
pub(crate) enum MashupCollectionCommand {
    Add(MashupCollectionAdd),
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupCollectionAdd {
    #[arg(long = "data", value_name = "address")]
    pub(crate) data: Option<String>,
    #[arg(long = "map", value_name = "value")]
    pub(crate) mappings: Option<String>,
    #[arg(long = "uid", value_name = "row-field")]
    pub(crate) uid: Option<String>,
    #[arg(long = "width", value_name = "px")]
    pub(crate) width: Option<u32>,
    #[arg(long = "height", value_name = "px")]
    pub(crate) height: Option<u32>,
    #[arg(long = "columns", value_name = "n")]
    pub(crate) columns: Option<u32>,
    #[arg(long = "across", value_name = "px")]
    pub(crate) across: Option<u32>,
    #[arg(long = "preset", value_name = "name")]
    pub(crate) preset: Option<String>,
    #[arg(long = "menu", value_name = "state-definition")]
    pub(crate) menu: Option<String>,
    #[command(flatten)]
    pub(crate) place: Place,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupStateFormat {
    #[command(subcommand)]
    pub(crate) command: MashupStateFormatCommand,
}
#[derive(Debug, clap::Subcommand)]
pub(crate) enum MashupStateFormatCommand {
    Set(MashupStateFormatSet),
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupStateFormatSet {
    #[arg(long = "state", value_name = "name")]
    pub(crate) state: Option<String>,
    #[arg(long = "property", value_name = "name")]
    pub(crate) property: Option<String>,
    #[arg(long = "column", value_name = "field")]
    pub(crate) column: Option<String>,
    #[arg(long = "field", value_name = "field")]
    pub(crate) field: Option<String>,
    #[arg(long = "icon-only")]
    pub(crate) icon_only: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupSource {
    #[command(subcommand)]
    pub(crate) command: MashupSourceCommand,
}
impl MashupSource {
    fn path(&self) -> &'static str {
        match self.command {
            MashupSourceCommand::Add(_) => "mashup source add",
            MashupSourceCommand::List(_) => "mashup source list",
            MashupSourceCommand::Remove(_) => "mashup source remove",
            MashupSourceCommand::Retarget(_) => "mashup source retarget",
        }
    }
}
#[derive(Debug, clap::Subcommand)]
pub(crate) enum MashupSourceCommand {
    Add(MashupSourceAdd),
    List(MashupSourceList),
    Remove(MashupSourceRemove),
    Retarget(MashupSourceRetarget),
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupSourceAdd {
    #[arg(long = "dynamic")]
    pub(crate) dynamic: bool,
    #[arg(long = "on-load")]
    pub(crate) on_load: bool,
    #[arg(long = "as", value_name = "new")]
    pub(crate) alias: Option<String>,
    #[arg(long = "entity-from", value_name = "address")]
    pub(crate) entity_from: Option<String>,
    #[arg(long = "every", value_name = "seconds")]
    pub(crate) every: Option<u32>,
    #[arg(long = "cache")]
    pub(crate) cache: bool,
    #[arg(long = "cache-key", value_name = "parameter")]
    pub(crate) cache_keys: Vec<String>,
    #[arg(long = "cache-strategy", value_name = "INSTANCE|SESSION|USER")]
    pub(crate) cache_strategy: Option<String>,
    #[arg(long = "max-result-sets", value_name = "n")]
    pub(crate) max_result_sets: Option<u32>,
    #[arg(long = "param-value", value_name = "name=value")]
    pub(crate) param_values: Vec<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupSourceList {
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupSourceRemove {
    #[arg(long = "cascade")]
    pub(crate) cascade: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupSourceRetarget {
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
    pub(crate) detail: super::flags::Detail,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mashup why <mashup> <widget>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupWhy {
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mashup diff <a> <b>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupDiff {
    #[arg(long = "tier", value_name = "tier")]
    pub(crate) tier: Option<String>,
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
    #[arg(long = "end")]
    pub(crate) end: bool,
    #[arg(long = "bottom")]
    pub(crate) bottom: bool,
    #[arg(long = "in-column")]
    pub(crate) in_column: bool,
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

/// `twaco mashup set <mashup> <widget>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupSet {
    #[arg(long = "prop", value_name = "name=value")]
    pub(crate) props: Vec<String>,
    #[arg(long = "style-from", value_name = "mashup:widget")]
    pub(crate) style_from: Option<String>,
    #[arg(long = "style", value_name = "[part[@state]:]property=value")]
    pub(crate) styles: Vec<String>,
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

/// `twaco mashup remove <mashup> <widget> [--cascade]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupRemove {
    #[arg(long = "cascade")]
    pub(crate) cascade: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mashup move <mashup> <widget> --in <container> | --right-of <widget> | ...`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupMove {
    #[command(flatten)]
    pub(crate) place: Place,
    #[arg(long = "end")]
    pub(crate) end: bool,
    #[arg(long = "bottom")]
    pub(crate) bottom: bool,
    #[arg(long = "in-column")]
    pub(crate) in_column: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco mashup unbind <mashup> --from <address> --to <address>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupUnbind {
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

/// `twaco mashup off <mashup> --event <address> --invoke <address>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupOff {
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
    #[arg(long = "set", value_name = "parameter=value")]
    pub(crate) settings: Vec<String>,
    #[arg(long = "fit", value_name = "fill|content")]
    pub(crate) fit: Option<String>,
    #[arg(long = "stacked")]
    pub(crate) stacked: bool,
    #[arg(long = "band")]
    pub(crate) band: bool,
    #[command(flatten)]
    pub(crate) detail: super::flags::Detail,
    #[arg(long = "compact", value_name = "768|960")]
    pub(crate) compact: Option<u32>,
    #[arg(long = "search-param", value_name = "param")]
    pub(crate) search_param: Option<String>,
    #[arg(long = "hint", value_name = "text")]
    pub(crate) hint: Option<String>,
    #[arg(long = "run-on", value_name = "change|enter|both")]
    pub(crate) run_on: Option<String>,
    #[arg(long = "button", value_name = "Label=handler")]
    pub(crate) buttons: Vec<String>,
    #[arg(long = "toggle", value_name = "button")]
    pub(crate) toggle: Option<String>,
    #[arg(long = "add-toggle")]
    pub(crate) add_toggle: bool,
    #[arg(long = "in", value_name = "widget")]
    pub(crate) in_container: Option<String>,
    #[arg(long = "tab", value_name = "inside|outside|none")]
    pub(crate) tab: Option<String>,
    #[arg(long = "start", value_name = "collapsed")]
    pub(crate) start: Option<String>,
    #[arg(long = "if", value_name = "condition")]
    pub(crate) condition: Option<String>,
    #[arg(long = "expr", value_name = "expression")]
    pub(crate) expression: Option<String>,
    #[arg(long = "input", value_name = "name:TYPE")]
    pub(crate) inputs: Vec<String>,
    #[arg(long = "param", value_name = "name:TYPE[:...]")]
    pub(crate) params: Vec<String>,
    #[arg(long = "field-map", value_name = "Property=field")]
    pub(crate) field_maps: Vec<String>,
    #[arg(long = "style", value_name = "style")]
    pub(crate) style: Option<String>,
    #[arg(long = "initial-step", value_name = "n")]
    pub(crate) initial_step: Option<u32>,
    #[arg(long = "step-size", value_name = "small|medium|large")]
    pub(crate) step_size: Option<String>,
    #[arg(long = "host", value_name = "widget")]
    pub(crate) host: Option<String>,
    #[arg(long = "action-label", value_name = "text")]
    pub(crate) action_label: Option<String>,
    #[arg(long = "cancel-label", value_name = "text")]
    pub(crate) cancel_label: Option<String>,
    #[arg(long = "after", value_name = "address")]
    pub(crate) after: Option<String>,
    #[arg(long = "say", value_name = "text")]
    pub(crate) say: Option<String>,
    #[arg(long = "on-failure", value_name = "text")]
    pub(crate) on_failure: Option<String>,
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
    #[arg(long = "window", value_name = "modal-popup|popup|replace|new")]
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
    #[arg(long = "editable-column", value_name = "field")]
    pub(crate) editable: Vec<String>,
    #[arg(long = "column-editor", value_name = "field=editor")]
    pub(crate) column_editors: Vec<String>,
    #[arg(long = "preserve-whitespace", value_name = "field")]
    pub(crate) preserve_whitespace: Vec<String>,
    #[arg(long = "no-none-row")]
    pub(crate) no_none_row: bool,
    #[arg(long = "auto-select-first")]
    pub(crate) auto_select_first: bool,
    #[arg(long = "multiple")]
    pub(crate) multiple: bool,
    #[arg(long = "info", value_name = "text")]
    pub(crate) info: Option<String>,
    #[arg(long = "align", value_name = "start|center")]
    pub(crate) align: Option<String>,
    #[arg(long = "per-row", value_name = "n")]
    pub(crate) per_row: Option<u32>,
    #[arg(long = "cells", value_name = "n")]
    pub(crate) cells: Option<u32>,
    #[arg(long = "variant", value_name = "name")]
    pub(crate) variant: Option<String>,
    #[arg(long = "menu", value_name = "Menu")]
    pub(crate) menu: Option<String>,
    #[arg(long = "no-auto-collapse")]
    pub(crate) no_auto_collapse: bool,
    #[arg(long = "width", value_name = "px")]
    pub(crate) width: Option<String>,
    #[arg(long = "field", value_name = "field")]
    pub(crate) fields: Vec<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
    #[arg(long = "columns", value_name = "n")]
    pub(crate) label_columns: Option<u32>,
    #[arg(long = "date-format", value_name = "pattern")]
    pub(crate) date_format: Option<String>,
    #[arg(long = "row", value_name = "first|selected")]
    pub(crate) row: Option<String>,
    #[arg(long = "layout", value_name = "inline|stacked")]
    pub(crate) layout: Option<String>,
    #[arg(long = "status", value_name = "field")]
    pub(crate) status: Option<String>,
    #[arg(long = "status-state", value_name = "name")]
    pub(crate) status_state: Option<String>,
    #[arg(long = "x-title", value_name = "text")]
    pub(crate) x_title: Option<String>,
    #[arg(long = "y-title", value_name = "text")]
    pub(crate) y_title: Option<String>,
    #[arg(long = "series", value_name = "field:kind[:axis]")]
    pub(crate) series: Vec<String>,

    #[arg(long = "replace", value_name = "widget")]
    pub(crate) replace: Option<String>,
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
    #[arg(long = "spec", value_name = "file")]
    pub(crate) spec: Option<std::path::PathBuf>,
    #[arg(long = "gadget")]
    pub(crate) gadget: bool,
    #[command(flatten)]
    pub(crate) project: Project,
    #[arg(long = "template", value_name = "name")]
    pub(crate) template: Option<String>,
    #[arg(long = "rules", value_name = "legacy|v1")]
    pub(crate) rules: Option<String>,
    #[arg(long = "from", value_name = "address")]
    pub(crate) from: Option<String>,
    #[arg(long = "replace", value_name = "old=new")]
    pub(crate) replacements: Vec<String>,
    #[arg(long = "layout", value_name = "layout")]
    pub(crate) layout: Option<String>,
    #[arg(long = "primary-menu", value_name = "address")]
    pub(crate) primary_menu: Option<String>,
    #[arg(long = "menu-entity", value_name = "Menu")]
    pub(crate) menu_entity: Option<String>,
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
    if let Some(spec) = &args.spec {
        if args.gadget
            || args.template.is_some()
            || args.rules.is_some()
            || args.from.is_some()
            || args.layout.is_some()
            || args.primary_menu.is_some()
            || args.menu_entity.is_some()
            || args.secondary_menu.is_some()
            || args.theme.is_some()
            || args.master
            || !args.visibility.is_empty()
            || !args.params.is_empty()
            || !args.tabs.is_empty()
            || !args.replacements.is_empty()
        {
            eprintln!(
                "{} --spec accepts only --project, --apply and --json",
                style::prefix()
            );
            return FAILED;
        }
        return page_build(
            solution,
            name,
            args.project.project.as_deref(),
            Some(spec),
            args.apply.apply,
            args.json.json,
        );
    }
    if args.gadget && args.template.is_some() {
        eprintln!(
            "{} --gadget cannot be used with --template",
            style::prefix()
        );
        return FAILED;
    }
    if args.template.is_some() && args.layout.is_some() {
        eprintln!("{} give --template or --layout, not both", style::prefix());
        return FAILED;
    }
    if args.from.is_some() {
        for (flag, present) in [
            ("--template", args.template.is_some()),
            ("--layout", args.layout.is_some()),
            ("--master", args.master),
            ("--param", !args.params.is_empty()),
            ("--tab", !args.tabs.is_empty()),
            ("--primary-menu", args.primary_menu.is_some()),
            ("--menu-entity", args.menu_entity.is_some()),
            ("--secondary-menu", args.secondary_menu.is_some()),
            ("--theme", args.theme.is_some()),
        ] {
            if present {
                eprintln!("{} --from cannot be used with {flag}", style::prefix());
                return FAILED;
            }
        }
    }
    let template = args
        .template
        .as_ref()
        .or(args.layout.as_ref().filter(|_| !args.gadget))
        .cloned()
        .unwrap_or_else(|| "blank".into());
    let apply = args.apply.apply;
    let request = commands::mashup::MashupNewRequest {
        request: mashup::authoring::NewRequest {
            name: name.clone(),
            project: args.project.project.clone(),
            template,
            rules: args.rules.clone(),
            gadget: args.gadget,
            layout: args.layout.clone().filter(|_| args.gadget),
            primary_menu: args.primary_menu.clone(),
            menu_entity: args.menu_entity.clone(),
            secondary_menu: args.secondary_menu.clone(),
            theme: args.theme.clone(),
            master: args.master,
            visibility: args.visibility.clone(),
            params: args.params.clone(),
            tabs: args.tabs.clone(),
            from: args.from.clone(),
            replacements: args.replacements.clone(),
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
    if plan.from.is_some() {
        for line in mashup::authoring::copy_summary_lines(solution, plan, outcome.applied) {
            println!("{line}");
        }
        // Visibility and PermissionGroup notices are notes: a copy must show them too.
        for note in &plan.notes {
            println!("  note: {note}");
        }
        if !outcome.applied {
            println!("run again with --apply to write it");
        }
        return OK;
    }
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
    for note in &plan.notes {
        println!("  note: {note}");
    }
    for (source, property) in &plan.menus {
        if property == "PrimaryNavMenu" {
            println!("  menu: Menu {source} -> {property} (DataSource menuentity)");
        } else {
            println!("  menu: {source} -> {property}, loaded with the page");
        }
    }
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

/// `twaco mashup param add|set|remove`: keep Composer's four parameter stores together.
/// `twaco mashup recipe <recipe> <mashup> [<widget>] ...`: plan one recipe, or apply it.
pub(crate) fn mashup_recipe_cmd(solution: &Solution, args: &MashupRecipe) -> u8 {
    if args.operands.first().is_some_and(|s| s == "component") && args.operands.len() != 4 {
        eprintln!("mashup recipe component needs <mashup> <name> <container> and --set id=<id>");
        return FAILED;
    }
    let mut component_kind = None;
    let (name, mashup, widget, button) = match args.operands.as_slice() {
        [name, mashup, component, container] if name == "component" => {
            if args.kind.is_some() {
                eprintln!("component name is already given as an operand; omit --kind");
                return FAILED;
            }
            component_kind = Some(component.clone());
            (name, mashup, Some(container.clone()), None)
        }
        [name, mashup] => (name, mashup, None, None),
        [name, mashup, widget] => (name, mashup, Some(widget.clone()), None),
        [name, mashup, widget, button] if name == "slider-action" => {
            (name, mashup, Some(widget.clone()), Some(button.clone()))
        }
        _ => {
            eprintln!(
                "{} mashup recipe needs <recipe> <mashup> [<widget>] (slider-action needs <slider> <button>); the recipes are {}",
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
    if args.params.len() > 1 {
        eprintln!("{} give --param once", style::prefix());
        return FAILED;
    }
    let width = match args.width.clone().map(|text| text.parse::<u32>()) {
        None => None,
        Some(Ok(pixels)) if pixels > 0 => Some(pixels),
        Some(_) => {
            eprintln!("{} --width takes a whole number of pixels", style::prefix());
            return FAILED;
        }
    };
    if name != "label-pairs" && args.fields.len() > 1 {
        eprintln!("{} give --field once for {name}", style::prefix());
        return FAILED;
    }
    let recipe_args = mashup::authoring::RecipeArgs {
        settings: args.settings.clone(),
        fit: args.fit.clone(),
        stacked: args.stacked,
        band: args.band,
        compact: args.compact,
        button,
        search_param: args.search_param.clone(),
        hint: args.hint.clone(),
        run_on: args.run_on.clone(),
        buttons: args.buttons.clone(),
        toggle: args.toggle.clone(),
        add_toggle: args.add_toggle,
        in_container: args.in_container.clone(),
        tab: args.tab.clone(),
        start: args.start.clone(),
        widget,
        param: args.params.first().cloned(),
        field_maps: args.field_maps.clone(),
        style: args.style.clone(),
        initial_step: args.initial_step,
        step_size: args.step_size.clone(),
        host: args.host.clone(),
        action_label: args.action_label.clone(),
        cancel_label: args.cancel_label.clone(),
        condition: args.condition.clone(),
        expression: args.expression.clone(),
        inputs: args.inputs.clone(),
        after: args.after.clone(),
        say: args.say.clone(),
        on_failure: args.on_failure.clone(),
        from: args.from.clone(),
        kind: component_kind.or_else(|| args.kind.clone()),
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
        editable: args.editable.clone(),
        column_editors: args.column_editors.clone(),
        preserve_whitespace: args.preserve_whitespace.clone(),
        no_none_row: args.no_none_row,
        series: args.series.clone(),
        auto_select_first: args.auto_select_first,
        multiple: args.multiple,
        info: args.info.clone(),
        align: args.align.clone(),
        per_row: args.per_row,
        cells: args.cells,
        variant: args.variant.clone(),
        menu: args.menu.clone(),
        no_auto_collapse: args.no_auto_collapse,
        width,
        field: if name == "label-pairs" {
            None
        } else {
            args.fields.first().cloned()
        },
        fields: if name == "label-pairs" {
            args.fields.clone()
        } else {
            Vec::new()
        },
        label_columns: args.label_columns,
        date_format: args.date_format.clone(),
        replace: args.replace.clone(),
        row: args.row.clone(),
        layout: args.layout.clone(),
        status: args.status.clone(),
        status_state: args.status_state.clone(),
        x_title: args.x_title.clone(),
        y_title: args.y_title.clone(),
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
    if outcome.unchanged {
        println!(
            "recipe {} already in {}; nothing to write",
            outcome.plan.name, mashup
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
    for id in &outcome.plan.removed {
        println!("  remove {id}");
    }
    if args.detail.detail || !["panel", "slider-action"].contains(&outcome.plan.name) {
        for line in &outcome.plan.wired {
            println!("  {line}");
        }
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

/// The placement flags as one [`Placement`]; prints why and fails when they do not fit.
fn placement_from_flags(
    place: &Place,
    end: bool,
    bottom: bool,
    in_column: bool,
) -> Result<Option<Placement>, ()> {
    let placements: Vec<(&str, &String)> = [
        ("--in", place.inside.as_ref()),
        ("--right-of", place.right_of.as_ref()),
        ("--left-of", place.left_of.as_ref()),
        ("--above", place.above.as_ref()),
        ("--below", place.below.as_ref()),
    ]
    .into_iter()
    .filter_map(|(flag, value)| value.map(|value| (flag, value)))
    .collect();
    if placements.len() > 1 {
        eprintln!(
            "{} give one of --in, --right-of, --left-of, --above, --below",
            style::prefix()
        );
        return Err(());
    }
    let placement = placements.first().map(|(flag, value)| match *flag {
        "--in" => Placement::In((*value).clone()),
        "--right-of" => beside(value, Side::Right),
        "--left-of" => beside(value, Side::Left),
        "--above" => beside(value, Side::Above),
        "--below" => beside(value, Side::Below),
        _ => unreachable!("only placement flags are collected"),
    });
    let placement = match (end, placement) {
        (false, placement) => placement,
        (
            true,
            Some(Placement::Beside {
                anchor,
                side: Side::Right,
            }),
        ) => Some(Placement::Beside {
            anchor,
            side: Side::End,
        }),
        (true, _) => {
            eprintln!("{} --end goes with --right-of <widget>", style::prefix());
            return Err(());
        }
    };
    let placement = match (bottom, placement) {
        (false, placement) => placement,
        (true, Some(Placement::In(column))) => Some(Placement::Bottom(column)),
        (true, _) => {
            eprintln!("{} --bottom goes with --in <column>", style::prefix());
            return Err(());
        }
    };
    let placement = match (in_column, placement) {
        (false, placement) => placement,
        (
            true,
            Some(Placement::Beside {
                anchor,
                side: Side::Below,
            }),
        ) => Some(Placement::BelowInColumn(anchor)),
        (true, _) => {
            eprintln!("{} --in-column goes with --below <widget>", style::prefix());
            return Err(());
        }
    };
    Ok(placement)
}

/// The placement flags given, in order.
fn placement_flags(place: &Place) -> Vec<(&'static str, &String)> {
    [
        ("--in", place.inside.as_ref()),
        ("--right-of", place.right_of.as_ref()),
        ("--left-of", place.left_of.as_ref()),
        ("--above", place.above.as_ref()),
        ("--below", place.below.as_ref()),
    ]
    .into_iter()
    .filter_map(|(flag, value)| value.map(|value| (flag, value)))
    .collect()
}

/// What a command with the wrong number of operands says. Extra operands usually mean the shell
/// split one argument at its spaces, or dropped its double quotes (Windows shells do both), so say so.
fn operand_problem(command: &str, needs: &str, operands: &[String], expected: usize) -> String {
    if operands.len() <= expected {
        return format!("{command} needs {needs}");
    }
    let extra: Vec<String> = operands[expected..]
        .iter()
        .map(|operand| format!("`{operand}`"))
        .collect();
    format!(
        "{command} needs {needs} but got {} operands; the extra {} {} probably the rest of a value the shell split at a space or at double quotes it removed. Put the whole argument in double quotes and single quotes inside the CSS value: --style \"font-family='Arial Black'\"",
        operands.len(),
        if extra.len() == 1 { "operand" } else { "operands" },
        extra.join(" ")
            + if extra.len() == 1 { " is" } else { " are" }
    )
}

/// `twaco mashup add <mashup> <widget>`: plan a catalogue widget insertion, or apply it.
pub(crate) fn mashup_add_cmd(solution: &Solution, args: &MashupAdd) -> u8 {
    let [mashup, widget] = args.operands.as_slice() else {
        eprintln!(
            "{} {}",
            style::prefix(),
            operand_problem("mashup add", "<mashup> and <widget>", &args.operands, 2)
        );
        return FAILED;
    };
    let Ok(placement) = placement_from_flags(&args.place, args.end, args.bottom, args.in_column)
    else {
        return FAILED;
    };
    let placements = placement_flags(&args.place);
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
    let label = plan.label.as_str();
    let where_ = match placements.first() {
        None if plan.function => "beside the layout".to_string(),
        None => "in center".to_string(),
        Some(("--in", value)) if args.bottom => format!("at the bottom of {value}"),
        Some(("--in", value)) => format!("in {value}"),
        Some(("--right-of", value)) if args.end => format!("at the end of {value}'s row"),
        Some(("--right-of", value)) => format!("right of {value}"),
        Some(("--left-of", value)) => format!("left of {value}"),
        Some(("--above", value)) => format!("above {value}"),
        Some(("--below", value)) if args.in_column => format!("below {value} in its column"),
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
    for note in &plan.notes {
        println!("  {note}");
    }
    for container in &plan.container_contents {
        println!(
            "  new container {} (holds {})",
            container.id,
            container.holds.join(", ")
        );
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
    for step in report["next_steps"].as_array().into_iter().flatten() {
        println!("{}", step.as_str().expect("a next step is text"));
    }
    OK
}

/// Copy state formatting from a repository StateDefinition.
pub(crate) fn mashup_state_format_set_cmd(solution: &Solution, args: &MashupStateFormatSet) -> u8 {
    if args.operands.len() != 2 || args.state.is_none() {
        eprintln!(
            "{} mashup state-format set needs <mashup> <widget> and --state <name>",
            style::prefix()
        );
        return FAILED;
    }
    let request = commands::mashup::MashupStateFormatSetRequest {
        request: mashup::authoring::StateFormatSetRequest {
            mashup: args.operands[0].clone(),
            widget: args.operands[1].clone(),
            state: args.state.clone().unwrap(),
            property: args.property.clone(),
            column: args.column.clone(),
            field: args.field.clone(),
            icon_only: args.icon_only,
        },
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup state-format set",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::mashup::state_format_set(solution, &request, &mut notices);
    print_notices(&notices);
    match outcome {
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            FAILED
        }
        Ok(outcome) => {
            if args.json.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&mashup::authoring::set_plan_json(
                        solution,
                        &outcome.plan,
                        outcome.applied
                    ))
                    .unwrap()
                );
            } else {
                println!(
                    "{} {} state-format properties on {}",
                    if outcome.applied { "set" } else { "would set" },
                    outcome.plan.changed.len(),
                    outcome.plan.widget
                );
            }
            OK
        }
    }
}

/// `twaco mashup collection add`: insert a collection cell and bind its data in one edit.
pub(crate) fn mashup_collection_add_cmd(solution: &Solution, args: &MashupCollectionAdd) -> u8 {
    let [mashup_name, cell] = args.operands.as_slice() else {
        eprintln!(
            "{} mashup collection add needs <parent> and <cell-mashup>",
            style::prefix()
        );
        return FAILED;
    };
    let Some(data) = &args.data else {
        eprintln!(
            "{} mashup collection add needs --data <address>",
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
    let mappings = match args
        .mappings
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .filter(|text| !text.is_empty())
        .map(|text| {
            text.split_once('=')
                .filter(|(field, parameter)| !field.is_empty() && !parameter.is_empty())
                .map(|(field, parameter)| (field.to_string(), parameter.to_string()))
                .ok_or_else(|| format!("--map {text:?} must be row-field=cell-parameter"))
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(mappings) => mappings,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let preset = match args.preset.as_deref() {
        None => None,
        Some(name) => match mashup::authoring::CollectionPreset::parse(name) {
            Some(preset) => Some(preset),
            None => {
                eprintln!(
                    "{} --preset {name:?} is not a collection preset; the one there is: cards",
                    style::prefix()
                );
                return FAILED;
            }
        },
    };
    let request = commands::mashup::MashupCollectionAddRequest {
        request: mashup::authoring::CollectionAddRequest {
            mashup: mashup_name.clone(),
            cell: cell.clone(),
            data: data.clone(),
            mappings,
            uid: args.uid.clone(),
            width: args.width,
            height: args.height,
            columns: args.columns,
            across: args.across,
            preset,
            menu: args.menu.clone(),
            placement,
        },
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup collection add",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::collection_add(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report =
        mashup::authoring::collection_add_plan_json(solution, &outcome.plan, outcome.applied);
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a collection plan serialises")
        );
        return OK;
    }
    println!(
        "{} collection {} using {}",
        if outcome.applied {
            "added"
        } else {
            "would add"
        },
        outcome.plan.id,
        outcome.plan.cell
    );
    println!(
        "  mappings: {}",
        outcome
            .plan
            .mappings
            .iter()
            .map(|(field, parameter)| format!("{field}={parameter}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if let Some(uid) = &outcome.plan.uid {
        println!("  uid: {uid}");
    }
    println!("  binding: {}", outcome.plan.binding);
    match (outcome.plan.cell_width, outcome.plan.cell_height) {
        (None, None) => {}
        (width, height) => {
            let px = |size: Option<u32>| size.map_or("default".to_string(), |n| format!("{n}px"));
            println!("  cell: {} x {}", px(width), px(height));
        }
    }
    if let Some(preset) = outcome.plan.preset {
        println!("  preset: {}", preset.name());
    }
    if let Some((menu, events)) = &outcome.plan.menu {
        println!("  menu: {menu} ({})", events.join(", "));
    }
    for note in &outcome.plan.notes {
        println!("  note: {note}");
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
    OK
}

/// `twaco mashup set <mashup> <widget>`: set checked widget properties in one edit.
pub(crate) fn mashup_set_cmd(solution: &Solution, args: &MashupSet) -> u8 {
    let [mashup_name, widget] = args.operands.as_slice() else {
        eprintln!(
            "{} {}",
            style::prefix(),
            operand_problem("mashup set", "<mashup> and <widget>", &args.operands, 2)
        );
        return FAILED;
    };
    let properties = match args
        .props
        .iter()
        .map(|text| {
            text.split_once('=')
                .map(|(name, value)| {
                    (
                        name.to_string(),
                        serde_json::Value::String(value.to_string()),
                    )
                })
                .ok_or_else(|| format!("--prop {text:?} must be Name=value"))
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(properties)
            if !properties.is_empty() || args.style_from.is_some() || !args.styles.is_empty() =>
        {
            properties
        }
        Ok(_) => {
            eprintln!(
                "{} mashup set needs --prop <name=value>, --style <property=value> or --style-from <mashup:widget>",
                style::prefix()
            );
            return FAILED;
        }
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let request = commands::mashup::MashupSetRequest {
        request: mashup::authoring::SetRequest {
            mashup: mashup_name.clone(),
            widget: widget.clone(),
            properties,
            style_from: args.style_from.clone(),
            styles: args.styles.clone(),
        },
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup set",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::set(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report = mashup::authoring::set_plan_json(solution, &outcome.plan, outcome.applied);
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a set plan serialises")
        );
    } else if let Some(copy) = &outcome.plan.style_copy {
        if copy.copied {
            println!(
                "{} the style of {}:{} to {}:{}",
                if outcome.applied {
                    "copied"
                } else {
                    "would copy"
                },
                copy.reference_mashup,
                copy.reference_widget,
                outcome.plan.mashup,
                outcome.plan.widget
            );
        } else {
            println!(
                "nothing to copy: {}:{} has no style entry",
                copy.reference_mashup, copy.reference_widget
            );
        }
        for change in &outcome.plan.changed {
            if change.property.starts_with("StyleProperties.data.") {
                continue;
            }
            let after = compact_json(&change.after);
            println!(
                "{} {}.{} = {after}",
                if outcome.applied { "set" } else { "would set" },
                outcome.plan.widget,
                change.property
            );
        }
    } else if outcome.plan.changed.is_empty() {
        println!(
            "unchanged {} in {}",
            outcome.plan.widget, outcome.plan.mashup
        );
    } else {
        for change in &outcome.plan.changed {
            let after = compact_json(&change.after);
            println!(
                "{} {}.{} = {after}",
                if outcome.applied { "set" } else { "would set" },
                outcome.plan.widget,
                change.property
            );
        }
    }
    if !args.json.json {
        for note in &outcome.plan.notes {
            println!("  {note}");
        }
    }
    OK
}

fn compact_json(value: &serde_json::Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= 120 {
        text
    } else {
        format!("{}…", text.chars().take(119).collect::<String>())
    }
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
    for note in &outcome.plan.notes {
        println!("  {note}");
    }
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

pub(crate) fn mashup_source_add_cmd(solution: &Solution, args: &MashupSourceAdd) -> u8 {
    let [mashup, entity, service] = args.operands.as_slice() else {
        eprintln!(
            "{} mashup source add needs <mashup> <entity> <service>",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupSourceAddRequest {
        request: mashup::authoring::SourceAddRequest {
            mashup: mashup.clone(),
            entity: entity.clone(),
            service: service.clone(),
            dynamic: args.dynamic,
            on_load: args.on_load,
            alias: args.alias.clone(),
            entity_from: args.entity_from.clone(),
            refresh: args.every,
            cache: args.cache,
            cache_keys: args.cache_keys.clone(),
            cache_strategy: args.cache_strategy.clone(),
            max_result_sets: args.max_result_sets,
            param_values: args.param_values.clone(),
        },
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup source add",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::source_add(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report = serde_json::json!({"applied":outcome.applied,"mashup":outcome.plan.edit.mashup,"section":outcome.plan.section,"call":outcome.plan.call,"event":outcome.plan.event,"files":edit_files(solution,&outcome.plan.edit)});
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a source plan serialises")
        );
    } else {
        println!(
            "{} source {} in {}",
            if outcome.applied {
                "added"
            } else {
                "would add"
            },
            outcome.plan.call,
            outcome.plan.section
        );
        if let Some(event) = &outcome.plan.event {
            println!("  event {event}");
        }
        if !outcome.applied {
            println!("run again with --apply to write it");
        }
    }
    OK
}

pub(crate) fn mashup_source_retarget_cmd(solution: &Solution, args: &MashupSourceRetarget) -> u8 {
    let [mashup] = args.operands.as_slice() else {
        eprintln!("{} mashup source retarget needs <mashup>", style::prefix());
        return FAILED;
    };
    let (Some(from), Some(to)) = (&args.from, &args.to) else {
        eprintln!(
            "{} mashup source retarget needs --from <Entity> and --to <Entity>",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupSourceRetargetRequest {
        request: mashup::authoring::SourceRetargetRequest {
            mashup: mashup.clone(),
            from: from.clone(),
            to: to.clone(),
        },
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup source retarget",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::source_retarget(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report = serde_json::json!({"applied":outcome.applied,"mashup":mashup,"from":from,"to":to,"sections":outcome.plan.sections.iter().map(|(from_key,to_key)| serde_json::json!({"from_key":from_key,"to_key":to_key})).collect::<Vec<_>>(),"bindings_rewritten":outcome.plan.bindings_rewritten,"events_rewritten":outcome.plan.events_rewritten,"notes":outcome.plan.notes,"files":edit_files(solution,&outcome.plan.edit)});
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a retarget plan serialises")
        );
    } else {
        println!(
            "{} {} source section(s)",
            if outcome.applied {
                "retargeted"
            } else {
                "would retarget"
            },
            outcome.plan.sections.len()
        );
        if outcome.plan.dynamic {
            println!("  dynamic source: set its Thing with `twaco mashup param set {mashup} Manager --default <Thing>`");
        }
        for note in &outcome.plan.notes {
            println!("  note: {note}");
        }
        if !outcome.applied {
            println!("run again with --apply to write it");
        }
    }
    OK
}

pub(crate) fn mashup_source_list_cmd(solution: &Solution, args: &MashupSourceList) -> u8 {
    let [mashup] = args.operands.as_slice() else {
        eprintln!("{} mashup source list needs <mashup>", style::prefix());
        return FAILED;
    };
    let sources = match commands::mashup::source_list(solution, mashup) {
        Ok(sources) => sources,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"mashup": mashup, "sources": sources})
            )
            .expect("sources serialise")
        );
    } else {
        for source in sources {
            println!(
                "{} {}.{} [as {}] entity:{} refresh:{} cache:{}",
                source.kind,
                source.entity,
                source.service,
                source.alias,
                source.entity_from,
                source.refresh,
                if source.cache { "on" } else { "off" }
            );
        }
    }
    OK
}

pub(crate) fn mashup_source_remove_cmd(solution: &Solution, args: &MashupSourceRemove) -> u8 {
    let [mashup, address] = args.operands.as_slice() else {
        eprintln!(
            "{} mashup source remove needs <mashup> <address>",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupSourceRemoveRequest {
        request: mashup::authoring::SourceRemoveRequest {
            mashup: mashup.clone(),
            address: address.clone(),
            cascade: args.cascade,
        },
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup source remove",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::source_remove(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report = serde_json::json!({"applied":outcome.applied,"mashup":mashup,"section":outcome.plan.section,"call":outcome.plan.call,"bindings_removed":outcome.plan.bindings_removed,"events_removed":outcome.plan.events_removed,"files":edit_files(solution,&outcome.plan.edit)});
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("source remove serialises")
        );
    } else {
        println!(
            "{} {}",
            if outcome.applied {
                "removed"
            } else {
                "would remove"
            },
            address
        );
        if !outcome.applied {
            println!("run again with --apply to write it");
        }
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
    let report = serde_json::json!({ "applied": outcome.applied, "mashup": outcome.plan.edit.mashup, "event": outcome.plan.event, "invoke": outcome.plan.invoke, "new_source": outcome.plan.new_source, "warnings": outcome.plan.warnings, "files": files });
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
    for warning in &outcome.plan.warnings {
        println!("  hint: {warning}");
    }
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

/// `twaco mashup remove <mashup> <widget>`: remove a widget, optionally its known wiring.
pub(crate) fn mashup_remove_cmd(solution: &Solution, args: &MashupRemove) -> u8 {
    let [mashup, widget] = args.operands.as_slice() else {
        eprintln!(
            "{} mashup remove needs <mashup> and <widget>",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupRemoveRequest {
        request: mashup::authoring::RemoveRequest {
            mashup: mashup.clone(),
            widget: widget.clone(),
            cascade: args.cascade,
        },
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup remove",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::remove(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let files = outcome
        .plan
        .edit
        .as_ref()
        .map_or_else(Vec::new, |edit| edit_files(solution, edit));
    let report = serde_json::json!({ "applied": outcome.applied, "mashup": mashup, "widget": outcome.plan.widget, "removed": outcome.plan.removed, "bindings_removed": outcome.plan.bindings_removed, "events_removed": outcome.plan.events_removed, "orphaned_data_calls": outcome.plan.orphaned_data_calls, "files": files });
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a remove plan serialises")
        );
        return OK;
    }
    if outcome.plan.removed.is_empty() {
        println!("{} is already absent from {}", widget, mashup);
        return OK;
    }
    println!(
        "{} {} from {}",
        if outcome.applied {
            "removed"
        } else {
            "would remove"
        },
        outcome.plan.removed.join(", "),
        mashup
    );
    if outcome.plan.bindings_removed > 0 || outcome.plan.events_removed > 0 {
        println!(
            "  cascade: {} binding(s), {} event(s)",
            outcome.plan.bindings_removed, outcome.plan.events_removed
        );
    }
    for call in &outcome.plan.orphaned_data_calls {
        println!("  review orphaned data call {call}");
    }
    println!("  files: {}", files.join(", "));
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

/// `twaco mashup move <mashup> <widget>`: take a widget and its subtree to another place.
pub(crate) fn mashup_move_cmd(solution: &Solution, args: &MashupMove) -> u8 {
    let [mashup, widget] = args.operands.as_slice() else {
        eprintln!(
            "{} mashup move needs <mashup> and <widget>",
            style::prefix()
        );
        return FAILED;
    };
    let Ok(placement) = placement_from_flags(&args.place, args.end, args.bottom, args.in_column)
    else {
        return FAILED;
    };
    let request = commands::mashup::MashupMoveRequest {
        request: mashup::authoring::MoveRequest {
            mashup: mashup.clone(),
            widget: widget.clone(),
            placement,
        },
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup move",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::move_widget(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let plan = &outcome.plan;
    let report = mashup::authoring::move_plan_json(solution, plan, outcome.applied);
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a move plan serialises")
        );
        return OK;
    }
    if plan.edit.is_none() {
        println!("{} is already {} in {mashup}", plan.widget, plan.to);
        return OK;
    }
    println!(
        "{} {} from {} {} in {mashup}",
        if outcome.applied {
            "moved"
        } else {
            "would move"
        },
        plan.widget,
        plan.from,
        plan.to,
    );
    if plan.moved.len() > 1 {
        println!("  with {}", plan.moved[1..].join(", "));
    }
    for container in &plan.container_contents {
        println!(
            "  new container {} (holds {})",
            container.id,
            container.holds.join(", ")
        );
    }
    if let Some(flipped) = &plan.placed.flipped {
        println!("  {flipped} now lays out the other way");
    }
    for wrapped in &plan.placed.wrapped {
        println!("  moved {wrapped} into its own container");
    }
    if !plan.removed.is_empty() {
        println!("  removed empty container {}", plan.removed.join(", "));
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

/// `twaco mashup unbind <mashup> --from <address> --to <address>`: remove one binding.
pub(crate) fn mashup_unbind_cmd(solution: &Solution, args: &MashupUnbind) -> u8 {
    let [mashup] = args.operands.as_slice() else {
        eprintln!("{} mashup unbind needs one <mashup>", style::prefix());
        return FAILED;
    };
    let (Some(from), Some(to)) = (args.from.as_ref(), args.to.as_ref()) else {
        eprintln!(
            "{} mashup unbind needs --from <address> and --to <address>",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupUnbindRequest {
        request: mashup::authoring::UnbindRequest {
            mashup: mashup.clone(),
            from: from.clone(),
            to: to.clone(),
        },
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup unbind",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::unbind(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let files = edit_files(solution, &outcome.plan.edit);
    let report = serde_json::json!({ "applied": outcome.applied, "mashup": outcome.plan.edit.mashup, "from": outcome.plan.from, "to": outcome.plan.to, "files": files });
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("an unbind plan serialises")
        );
        return OK;
    }
    println!(
        "{} {} -> {} in {}",
        if outcome.applied {
            "unbound"
        } else {
            "would unbind"
        },
        outcome.plan.from,
        outcome.plan.to,
        outcome.plan.edit.mashup
    );
    println!("  files: {}", files.join(", "));
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

/// `twaco mashup off <mashup> --event <address> --invoke <address>`: remove one event handler.
pub(crate) fn mashup_off_cmd(solution: &Solution, args: &MashupOff) -> u8 {
    let [mashup] = args.operands.as_slice() else {
        eprintln!("{} mashup off needs one <mashup>", style::prefix());
        return FAILED;
    };
    let (Some(event), Some(invoke)) = (args.event.as_ref(), args.invoke.as_ref()) else {
        eprintln!(
            "{} mashup off needs --event <address> and --invoke <address>",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupOffRequest {
        request: mashup::authoring::OffRequest {
            mashup: mashup.clone(),
            event: event.clone(),
            invoke: invoke.clone(),
        },
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup off",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::off(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let files = edit_files(solution, &outcome.plan.edit);
    let report = serde_json::json!({ "applied": outcome.applied, "mashup": outcome.plan.edit.mashup, "event": outcome.plan.event, "invoke": outcome.plan.invoke, "files": files });
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("an off plan serialises")
        );
        return OK;
    }
    println!(
        "{} {} -> {} in {}",
        if outcome.applied {
            "removed"
        } else {
            "would remove"
        },
        outcome.plan.event,
        outcome.plan.invoke,
        outcome.plan.edit.mashup
    );
    println!("  files: {}", files.join(", "));
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}

fn edit_files(solution: &Solution, edit: &mashup::authoring::EditPlan) -> Vec<String> {
    let mut files = vec![edit
        .entity_path
        .strip_prefix(&solution.root)
        .unwrap_or(&edit.entity_path)
        .to_string_lossy()
        .replace('\\', "/")];
    if let Some(sidecars) = &edit.sidecars {
        files.push(
            sidecars
                .content_path
                .strip_prefix(&solution.root)
                .unwrap_or(&sidecars.content_path)
                .to_string_lossy()
                .replace('\\', "/"),
        );
    }
    files
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
    match twaco::core::mashup::describe::describe_detail(solution, name, args.detail.detail) {
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

/// `twaco mashup why <mashup> <widget>`: read-only.
pub(crate) fn mashup_why_cmd(solution: &Solution, args: &MashupWhy) -> u8 {
    let [name, widget] = args.operands.as_slice() else {
        eprintln!("{} mashup why needs <mashup> and <widget>", style::prefix());
        return FAILED;
    };
    match twaco::core::mashup::why::why(solution, name, widget) {
        Ok(why) => {
            if args.json.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&why).expect("an explanation serialises")
                );
            } else {
                print!("{}", twaco::core::mashup::why::render(&why));
            }
            OK
        }
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            FAILED
        }
    }
}

/// `twaco mashup diff <a> <b>`: compare authored semantics, not Composer allocator state.
pub(crate) fn mashup_diff_cmd(solution: &Solution, args: &MashupDiff) -> u8 {
    let [a, b] = args.operands.as_slice() else {
        eprintln!("{} mashup diff needs <a> and <b>", style::prefix());
        return FAILED;
    };
    let value = args.tier.as_deref().unwrap_or("all");
    let Some(tier) = twaco::core::mashup::diff::Tier::parse(value) else {
        eprintln!(
            "{} --tier must be executable, layout, visual or all (got {value})",
            style::prefix()
        );
        return FAILED;
    };
    match twaco::core::mashup::diff::diff(solution, a, b, tier) {
        Ok(result) => {
            if args.json.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).expect("a diff serialises")
                );
            } else {
                print!("{}", twaco::core::mashup::diff::render(&result));
            }
            if result.equal {
                OK
            } else {
                DRIFT
            }
        }
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            FAILED
        }
    }
}

/// `twaco mashup validate <mashup>|--all`: report problems; exit 1 when there are errors.
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
    let counts = twaco::core::mashup::validate::problem_counts(&reports);
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "problems": counts.problems,
                "errors": counts.errors,
                "advisories": counts.advisories,
                "mashups": reports,
            }))
            .expect("a validation report serialises")
        );
    } else {
        for report in reports.iter().filter(|report| !report.problems.is_empty()) {
            println!("{} ({})", report.mashup, report.file);
            for problem in &report.problems {
                let advisory = matches!(
                    problem.severity,
                    twaco::core::mashup::validate::Severity::Advisory
                )
                .then_some("advisory: ")
                .unwrap_or("");
                println!("  {advisory}[{}] {}", problem.rule, problem.message);
            }
        }
        println!(
            "{} mashup(s) validated, {} with errors, {} error(s), {} advisory(ies)",
            reports.len(),
            reports
                .iter()
                .filter(|report| {
                    report.problems.iter().any(|problem| {
                        matches!(
                            problem.severity,
                            twaco::core::mashup::validate::Severity::Error
                        )
                    })
                })
                .count(),
            counts.errors,
            counts.advisories,
        );
    }
    if counts.errors > 0 {
        DRIFT
    } else {
        OK
    }
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupGap {
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

pub(crate) fn mashup_gap_cmd(solution: &Solution, args: &MashupGap) -> u8 {
    let [mashup, container, px] = args.operands.as_slice() else {
        eprintln!(
            "{} mashup gap needs <mashup> <container> <px>",
            style::prefix()
        );
        return FAILED;
    };
    let Ok(px) = px.parse::<i32>() else {
        eprintln!(
            "{} gap px must be an integer between 0 and 200",
            style::prefix()
        );
        return FAILED;
    };
    let request = commands::mashup::MashupGapRequest {
        request: mashup::authoring::GapRequest {
            mashup: mashup.clone(),
            container: container.clone(),
            px,
        },
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup gap",
    };
    let mut notices = commands::Notices::default();
    let result = commands::mashup::gap(solution, &request, &mut notices);
    print_notices(&notices);
    match result {
        Ok(outcome) => {
            if args.json.json {
                println!(
                    "{}",
                    mashup::authoring::gap_plan_json(solution, &outcome.plan, outcome.applied)
                );
            } else {
                println!(
                    "{} {}: {} spacer(s), {}px",
                    if outcome.applied {
                        "updated"
                    } else {
                        "would update"
                    },
                    container,
                    outcome.plan.spacers,
                    px
                );
            }
            OK
        }
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            FAILED
        }
    }
}

/// Diagnose the shared numeric flag before clap discards the recipe context.
pub(crate) fn recipe_columns_hint(args: &[String]) -> Option<&'static str> {
    if !args.starts_with(&["mashup".into(), "recipe".into(), "columns".into()]) {
        return None;
    }
    let bad = args.iter().enumerate().any(|(index, arg)| {
        let value = arg.strip_prefix("--columns=").or_else(|| {
            (arg == "--columns")
                .then(|| args.get(index + 1).map(String::as_str))
                .flatten()
        });
        value.is_some_and(|value| value.parse::<u32>().is_err())
    });
    bad.then_some("recipe columns takes --column field[:Title[:width]] (repeatable); --columns <n> is label-pairs' column count")
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupBuild {
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

pub(crate) fn mashup_build_cmd(solution: &Solution, args: &MashupBuild) -> u8 {
    let [name] = args.operands.as_slice() else {
        eprintln!("{} mashup build needs one <mashup>", style::prefix());
        return FAILED;
    };
    page_build(solution, name, None, None, args.apply.apply, args.json.json)
}

fn page_build(
    solution: &Solution,
    name: &str,
    project: Option<&str>,
    spec: Option<&std::path::Path>,
    apply: bool,
    json: bool,
) -> u8 {
    let mut notices = commands::Notices::default();
    let result = commands::mashup::build(
        solution,
        name,
        project,
        spec,
        if apply { Mode::Apply } else { Mode::Plan },
        &mut notices,
    );
    print_notices(&notices);
    let report = match result {
        Ok(report) => report,
        Err(error) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&mashup::page::error_object(&error.to_string()))
                        .expect("error object")
                );
            } else {
                eprintln!("{} {error}", style::prefix());
            }
            return FAILED;
        }
    };
    let conflicts = report["conflicts"].as_array().expect("conflicts array");
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("build report")
        );
    } else {
        println!(
            "{} {name}: {} nodes, {} hand edits kept, {} conflicts",
            if report["applied"] == true {
                "built"
            } else {
                "planned"
            },
            report["nodes"].as_array().expect("nodes").len(),
            report["hand_edits_kept"].as_array().expect("kept").len(),
            conflicts.len()
        );
        for node in report["nodes"].as_array().expect("nodes") {
            println!(
                "  {}: {}",
                node["node"].as_str().unwrap_or_default(),
                node["status"].as_str().unwrap_or_default()
            );
        }
        for edit in report["hand_edits_kept"].as_array().expect("kept") {
            println!("  hand edit kept: {}", edit.as_str().unwrap_or_default());
        }
        for notice in report["cell_notices"].as_array().expect("cell notices") {
            println!("  {}", notice.as_str().unwrap_or_default());
        }
        for conflict in conflicts {
            println!("  {}", conflict["message"].as_str().unwrap_or_default());
        }
        if !apply {
            let files = report["files"].as_array().map_or(0, Vec::len);
            if files > 0 {
                println!("would write {files} files (nothing is written until --apply)");
            }
        }
        if report["files_state"] == "nothing to write" {
            println!("nothing to write");
        } else if !apply && conflicts.is_empty() {
            println!("run again with --apply to write it");
        }
    }
    if conflicts.is_empty() {
        OK
    } else {
        FAILED
    }
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupSpec {
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(long = "out", value_name = "path")]
    pub(crate) out: Option<String>,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

pub(crate) fn mashup_spec_cmd(solution: &Solution, args: &MashupSpec) -> u8 {
    let [name] = args.operands.as_slice() else {
        eprintln!("mashup spec needs one <mashup>");
        return FAILED;
    };
    match mashup::page::extract::export(
        solution,
        name,
        args.out.as_deref().map(std::path::Path::new),
    ) {
        Ok(result) => {
            if args.json.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).expect("spec report")
                );
            } else {
                let types: Vec<String> = result
                    .raw_types
                    .iter()
                    .map(|(kind, n)| format!("{kind} x{n}"))
                    .collect();
                eprintln!(
                    "{} nodes as elements/components, {} raw widget nodes{}, for {} widgets{}",
                    result.elements,
                    result.raw,
                    if types.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", types.join(", "))
                    },
                    result.widgets,
                    if result.exact { "; exact source" } else { "" }
                );
                if !result.hand_edited.is_empty() {
                    eprintln!("hand-edited: {}", result.hand_edited.join(", "));
                }
                for note in &result.notes {
                    eprintln!("note: {note}");
                }
                if args.out.is_none() {
                    print!("{}", result.spec);
                }
            }
            OK
        }
        Err(e) => {
            eprintln!("{e}");
            FAILED
        }
    }
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Components {
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

pub(crate) fn components_cmd(solution: &Solution, args: &Components) -> u8 {
    let name = match args.operands.as_slice() {
        [action] if action == "list" => None,
        [action, name] if action == "show" => Some(name.as_str()),
        _ => {
            eprintln!("components needs list or show <name>");
            return FAILED;
        }
    };
    match mashup::page::components::inspect(solution, name) {
        Ok(value) => {
            if args.json.json || name.is_some() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).expect("component report")
                );
            } else {
                println!("{} components", value["count"]);
                for component in value["components"].as_array().expect("components") {
                    println!(
                        "{}  source: {}  props: {}",
                        component["name"].as_str().unwrap_or_default(),
                        component["source"].as_str().unwrap_or_default(),
                        component["props"]
                            .as_object()
                            .expect("props")
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            OK
        }
        Err(e) => {
            eprintln!("{e}");
            FAILED
        }
    }
}

#[cfg(test)]
mod operand_tests {
    use super::operand_problem;

    fn operands(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn a_missing_operand_keeps_the_short_message() {
        assert_eq!(
            operand_problem("mashup set", "<mashup> and <widget>", &operands(&["A"]), 2),
            "mashup set needs <mashup> and <widget>"
        );
    }

    #[test]
    fn a_split_value_is_explained_with_the_single_quote_form() {
        let message = operand_problem(
            "mashup set",
            "<mashup> and <widget>",
            &operands(&["Acme.Page_MU", "ptcslabel-3", "Black\""]),
            2,
        );
        assert!(message.contains("got 3 operands"), "{message}");
        assert!(message.contains("`Black\"` is probably"), "{message}");
        assert!(
            message.contains("--style \"font-family='Arial Black'\""),
            "{message}"
        );
    }
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupTokens {
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

pub(crate) fn mashup_tokens_cmd(args: &MashupTokens) -> u8 {
    let source = match args.operands.as_slice() {
        [] => None,
        [source] => Some(source.as_str()),
        _ => {
            eprintln!(
                "{} mashup tokens takes at most one <source>",
                style::prefix()
            );
            return FAILED;
        }
    };
    let base = std::env::current_dir().unwrap_or_default();
    match mashup::tokens::inspect(source, &base) {
        Ok(value) => {
            if args.json.json || source.is_some() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).expect("tokens serialise")
                );
            } else {
                println!("presets: {}", mashup::tokens::PRESETS.join(", "));
            }
            OK
        }
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            FAILED
        }
    }
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct MashupMigrate {
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

pub(crate) fn mashup_migrate_cmd(solution: &Solution, args: &MashupMigrate) -> u8 {
    let [mashup] = args.operands.as_slice() else {
        eprintln!("{} mashup migrate needs one <mashup>", style::prefix());
        return FAILED;
    };
    let request = commands::mashup::MashupMigrateRequest {
        request: mashup::authoring::MigrateRequest {
            mashup: mashup.clone(),
        },
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "mashup migrate",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::mashup::migrate(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&mashup::authoring::migrate_plan_json(
                &outcome.plan,
                outcome.applied
            ))
            .expect("migration plan serialises")
        );
    } else {
        println!(
            "{}: {} widget(s) planned, {} kept as they are; {}",
            mashup,
            outcome.plan.migrations.len(),
            outcome.plan.kept.len(),
            if outcome.applied {
                "applied"
            } else {
                "nothing written"
            }
        );
        for item in &outcome.plan.migrations {
            println!(
                "  {} ({}): {} -> {} [{}]",
                item.name, item.id, item.from, item.to, item.source
            );
        }
        for item in &outcome.plan.kept {
            println!(
                "  kept {} ({}): {}: {}",
                item.name, item.id, item.widget, item.reason
            );
        }
        for warning in &outcome.plan.warnings {
            eprintln!("{} {warning}", style::prefix());
        }
    }
    OK
}
