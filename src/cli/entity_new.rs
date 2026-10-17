use super::flags::{Apply, Json, Project};
use super::style;
use crate::{print_notices, FAILED, OK};
use std::path::PathBuf;
use twaco::core::commands::{self, Mode};
use twaco::core::config::Solution;
use twaco::core::entity;
use twaco::core::entity::new::{Kind, NewRequest};

/// `twaco new <kind> <name>`: a building block, or one of the common entities.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum New {
    BuildingBlock(super::refactor::NewBuildingBlock),
    #[command(flatten)]
    Entity(NewEntity),
}

/// `twaco new <kind> <name>` for a common entity, one variant per kind.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum NewEntity {
    Thing(NewThing),
    ThingTemplate(NewThingTemplate),
    ThingShape(NewThingShape),
    DataShape(NewDataShape),
    Timer(NewTimer),
    Scheduler(NewScheduler),
    DataTable(NewDataTable),
    Stream(NewStream),
    ValueStream(NewValueStream),
    Media(NewMedia),
    Style(NewStyle),
    State(NewState),
    Menu(NewMenu),
    Theme(NewTheme),
    Network(NewNetwork),
}

/// What every `new <kind>` takes: the name, `--project`, `--description`, `--apply` and
/// `--json`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Base {
    #[command(flatten)]
    pub(crate) project: Project,
    #[arg(long = "description", value_name = "text")]
    pub(crate) description: Option<String>,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco new thing <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewThing {
    #[command(flatten)]
    pub(crate) base: Base,
    #[arg(long = "visibility", value_name = "principal")]
    pub(crate) visibility: Vec<String>,
    #[arg(long = "template", value_name = "name")]
    pub(crate) template: Option<String>,
    #[arg(long = "shape", value_name = "ThingShape")]
    pub(crate) shapes: Vec<String>,
    #[arg(long = "value-stream", value_name = "ValueStream")]
    pub(crate) value_stream: Option<String>,
}

/// `twaco new thing-template <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewThingTemplate {
    #[command(flatten)]
    pub(crate) base: Base,
    #[arg(long = "visibility", value_name = "principal")]
    pub(crate) visibility: Vec<String>,
    #[arg(long = "base", value_name = "file|handoff|git rev")]
    pub(crate) parent: Option<String>,
    #[arg(long = "shape", value_name = "ThingShape")]
    pub(crate) shapes: Vec<String>,
}

/// `twaco new thing-shape <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewThingShape {
    #[command(flatten)]
    pub(crate) base: Base,
    #[arg(long = "visibility", value_name = "principal")]
    pub(crate) visibility: Vec<String>,
}

/// `twaco new data-shape <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewDataShape {
    #[command(flatten)]
    pub(crate) base: Base,
    #[arg(long = "visibility", value_name = "principal")]
    pub(crate) visibility: Vec<String>,
    #[arg(long = "base", value_name = "file|handoff|git rev")]
    pub(crate) parent: Option<String>,
    #[arg(long = "field", value_name = "name:TYPE[:primary]")]
    pub(crate) fields: Vec<String>,
}

/// `twaco new timer <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewTimer {
    #[command(flatten)]
    pub(crate) base: Base,
    #[arg(long = "visibility", value_name = "principal")]
    pub(crate) visibility: Vec<String>,
    #[arg(long = "every", value_name = "seconds")]
    pub(crate) every: Option<String>,
    #[command(flatten)]
    pub(crate) event: Event,
}

/// `twaco new scheduler <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewScheduler {
    #[command(flatten)]
    pub(crate) base: Base,
    #[arg(long = "visibility", value_name = "principal")]
    pub(crate) visibility: Vec<String>,
    #[arg(long = "cron", value_name = "quartz expression")]
    pub(crate) cron: Option<String>,
    #[command(flatten)]
    pub(crate) event: Event,
}

/// What a new timer or scheduler runs, as whom, and whether it starts disabled.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Event {
    #[arg(long = "run-as", value_name = "user")]
    pub(crate) run_as: Option<String>,
    #[arg(long = "disabled")]
    pub(crate) disabled: bool,
    #[arg(long = "then", value_name = "address")]
    pub(crate) then: Option<String>,
    #[arg(long = "script", value_name = "file.js", value_parser = super::flags::path())]
    pub(crate) script: Option<PathBuf>,
}

/// `twaco new data-table <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewDataTable {
    #[command(flatten)]
    pub(crate) base: Base,
    #[arg(long = "data-shape", value_name = "DataShape")]
    pub(crate) data_shape: Option<String>,
    #[arg(long = "index", value_name = "name=field1,field2")]
    pub(crate) indexes: Vec<String>,
}

/// `twaco new stream <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewStream {
    #[command(flatten)]
    pub(crate) base: Base,
    #[arg(long = "data-shape", value_name = "DataShape")]
    pub(crate) data_shape: Option<String>,
}

/// `twaco new value-stream <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewValueStream {
    #[command(flatten)]
    pub(crate) base: Base,
}

/// `twaco new media <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewMedia {
    #[command(flatten)]
    pub(crate) base: Base,
    #[arg(long = "file", value_name = "path", value_parser = super::flags::path())]
    pub(crate) file: Option<PathBuf>,
}

/// `twaco new style <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewStyle {
    #[command(flatten)]
    pub(crate) base: Base,
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
    #[arg(long = "italic")]
    pub(crate) italic: bool,
    #[arg(long = "underline")]
    pub(crate) underline: bool,
    #[arg(long = "text-size", value_name = "size")]
    pub(crate) text_size: Option<String>,
    #[arg(long = "image", value_name = "MediaEntity")]
    pub(crate) image: Option<String>,
    #[arg(long = "display-string", value_name = "text")]
    pub(crate) display_string: Option<String>,
}

/// `twaco new state <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewState {
    #[command(flatten)]
    pub(crate) base: Base,
    #[arg(long = "type", value_name = "value")]
    pub(crate) kind: Option<String>,
    #[arg(long = "state", value_name = "spec")]
    pub(crate) states: Vec<String>,
}

/// `twaco new menu <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewMenu {
    #[command(flatten)]
    pub(crate) base: Base,
    #[arg(long = "item", value_name = "Title=Mashup")]
    pub(crate) items: Vec<String>,
    /// A switch here: empty when given bare; a value given to it is not read.
    #[arg(long = "default", value_name = "value", num_args = 0..=1)]
    pub(crate) default: Option<String>,
    #[arg(long = "label", value_name = "text")]
    pub(crate) label: Option<String>,
}

/// `twaco new theme <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewTheme {
    #[command(flatten)]
    pub(crate) base: Base,
    #[arg(long = "from", value_name = "address")]
    pub(crate) from: Option<String>,
    #[arg(long = "prop", value_name = "name=value")]
    pub(crate) props: Vec<String>,
}

/// `twaco new network <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct NewNetwork {
    #[command(flatten)]
    pub(crate) base: Base,
}

impl NewEntity {
    /// The command's [`USAGE`](super::spec::USAGE) path.
    pub(crate) fn path(&self) -> &'static str {
        match self {
            NewEntity::Thing(_) => "new thing",
            NewEntity::ThingTemplate(_) => "new thing-template",
            NewEntity::ThingShape(_) => "new thing-shape",
            NewEntity::DataShape(_) => "new data-shape",
            NewEntity::Timer(_) => "new timer",
            NewEntity::Scheduler(_) => "new scheduler",
            NewEntity::DataTable(_) => "new data-table",
            NewEntity::Stream(_) => "new stream",
            NewEntity::ValueStream(_) => "new value-stream",
            NewEntity::Media(_) => "new media",
            NewEntity::Style(_) => "new style",
            NewEntity::State(_) => "new state",
            NewEntity::Menu(_) => "new menu",
            NewEntity::Theme(_) => "new theme",
            NewEntity::Network(_) => "new network",
        }
    }

    /// What every kind takes.
    pub(crate) fn base(&self) -> &Base {
        match self {
            NewEntity::Thing(new) => &new.base,
            NewEntity::ThingTemplate(new) => &new.base,
            NewEntity::ThingShape(new) => &new.base,
            NewEntity::DataShape(new) => &new.base,
            NewEntity::Timer(new) => &new.base,
            NewEntity::Scheduler(new) => &new.base,
            NewEntity::DataTable(new) => &new.base,
            NewEntity::Stream(new) => &new.base,
            NewEntity::ValueStream(new) => &new.base,
            NewEntity::Media(new) => &new.base,
            NewEntity::Style(new) => &new.base,
            NewEntity::State(new) => &new.base,
            NewEntity::Menu(new) => &new.base,
            NewEntity::Theme(new) => &new.base,
            NewEntity::Network(new) => &new.base,
        }
    }

    /// The request for an entity named `name`: what every kind takes, then the kind's own.
    fn request(&self, name: &str) -> NewRequest {
        let kind = Kind::parse(self.path().strip_prefix("new ").unwrap_or_default())
            .expect("new routes have known kinds");
        let base = self.base();
        let mut request = NewRequest::blank(kind, name);
        request.project.clone_from(&base.project.project);
        request.description = base.description.clone().unwrap_or_default();
        match self {
            NewEntity::Thing(new) => {
                request.visibility.clone_from(&new.visibility);
                request.template.clone_from(&new.template);
                request.shapes.clone_from(&new.shapes);
                request.value_stream.clone_from(&new.value_stream);
            }
            NewEntity::ThingTemplate(new) => {
                request.visibility.clone_from(&new.visibility);
                request.base.clone_from(&new.parent);
                request.shapes.clone_from(&new.shapes);
            }
            NewEntity::ThingShape(new) => request.visibility.clone_from(&new.visibility),
            NewEntity::DataShape(new) => {
                request.visibility.clone_from(&new.visibility);
                request.base.clone_from(&new.parent);
                request.fields.clone_from(&new.fields);
            }
            NewEntity::Timer(new) => {
                request.visibility.clone_from(&new.visibility);
                request.every.clone_from(&new.every);
                new.event.fill(&mut request);
            }
            NewEntity::Scheduler(new) => {
                request.visibility.clone_from(&new.visibility);
                request.cron.clone_from(&new.cron);
                new.event.fill(&mut request);
            }
            NewEntity::DataTable(new) => {
                request.data_shape.clone_from(&new.data_shape);
                request.indexes.clone_from(&new.indexes);
            }
            NewEntity::Stream(new) => request.data_shape.clone_from(&new.data_shape),
            NewEntity::ValueStream(_) => {}
            NewEntity::Media(new) => request.file.clone_from(&new.file),
            NewEntity::Style(new) => {
                request.background.clone_from(&new.background);
                request.foreground.clone_from(&new.foreground);
                request
                    .secondary_background
                    .clone_from(&new.secondary_background);
                request.line_color.clone_from(&new.line_color);
                request.line_style.clone_from(&new.line_style);
                request.line_thickness.clone_from(&new.line_thickness);
                request.bold = new.bold;
                request.italic = new.italic;
                request.underline = new.underline;
                request.text_size.clone_from(&new.text_size);
                request.image.clone_from(&new.image);
                request.display_string.clone_from(&new.display_string);
            }
            NewEntity::State(new) => {
                request.state_type.clone_from(&new.kind);
                request.states.clone_from(&new.states);
            }
            NewEntity::Menu(new) => {
                request.items.clone_from(&new.items);
                request.default = new.default.is_some();
                request.label.clone_from(&new.label);
            }
            NewEntity::Theme(new) => {
                request.from.clone_from(&new.from);
                request.props.clone_from(&new.props);
            }
            NewEntity::Network(_) => {}
        }
        request
    }
}

impl Event {
    /// Set what a timer and a scheduler share.
    fn fill(&self, request: &mut NewRequest) {
        request.run_as.clone_from(&self.run_as);
        request.disabled = self.disabled;
        request.then.clone_from(&self.then);
        request.script.clone_from(&self.script);
    }
}

/// `twaco new <kind> <name>`: plan a common entity, or create its XML and sidecars atomically.
pub(crate) fn entity_new_cmd(solution: &Solution, new: &NewEntity) -> u8 {
    let base = new.base();
    let [name] = base.operands.as_slice() else {
        eprintln!("{} {} needs one <name>", style::prefix(), new.path());
        return FAILED;
    };
    let request = new.request(name);
    let request = commands::entity_new::Request {
        request,
        mode: if base.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "new entity",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::entity_new::execute(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report = entity::new::plan_json(solution, &outcome.plan, outcome.applied);
    if base.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("new entity plan serialises")
        );
        return OK;
    }
    let plan = &outcome.plan;
    match plan.request.kind {
        Kind::Timer => println!(
            "{} timer {} in project {} (every {} as {})",
            if outcome.applied {
                "created"
            } else {
                "would create"
            },
            plan.request.name,
            plan.project,
            plan.request.every.as_deref().unwrap_or("60s"),
            plan.request.run_as.as_deref().unwrap()
        ),
        Kind::Scheduler => println!(
            "{} scheduler {} in project {} (cron {} as {})",
            if outcome.applied {
                "created"
            } else {
                "would create"
            },
            plan.request.name,
            plan.project,
            plan.cron.as_deref().unwrap(),
            plan.request.run_as.as_deref().unwrap()
        ),
        _ => println!(
            "{} {} {} in project {}",
            if outcome.applied {
                "created"
            } else {
                "would create"
            },
            plan.request.kind.word(),
            plan.request.name,
            plan.project
        ),
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
    if let Some(code) = &plan.script {
        println!(
            "  runs on its {} event: {}",
            if plan.request.kind == Kind::Timer {
                "Timer"
            } else {
                "ScheduledEvent"
            },
            code.lines().next().unwrap_or_default()
        );
    }
    if !outcome.applied {
        println!("run again with --apply to write it");
    }
    OK
}
