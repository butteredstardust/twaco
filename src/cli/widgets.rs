use super::flags::{Json, Profile};
use super::style;
use crate::{FAILED, OK};
use std::path::PathBuf;
use twaco::core::config::Solution;
use twaco::core::widget_cache::Outcome;
use twaco::core::widgets_view::{self, WidgetDetail, WidgetSummary};
use twaco::core::{cds_icons, commands, server};

/// `twaco widgets [<widget>] [--search <text>] [--json]`, and `widgets --platform|--webapp`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Widgets {
    #[arg(long = "search", value_name = "text")]
    pub(crate) search: Option<String>,
    #[arg(long = "icons")]
    pub(crate) icons: bool,
    #[arg(long = "platform")]
    pub(crate) platform: bool,
    #[arg(long = "webapp", value_name = "webapp folder")]
    pub(crate) from: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco widgets --platform` or `--webapp <folder>`: read the installation's widget files into
/// `.twaco/widgets.json`.
pub(crate) fn refresh_cmd(solution: &Solution, args: &Widgets) -> u8 {
    if args.platform && args.from.is_some() {
        eprintln!("{} give --platform or --webapp, not both", style::prefix());
        return FAILED;
    }
    if args.icons || args.search.is_some() || !args.operands.is_empty() {
        eprintln!(
            "{} --platform and --webapp rebuild the widget cache and take no widget, --search or --icons",
            style::prefix()
        );
        return FAILED;
    }
    let source = match &args.from {
        Some(folder) => commands::widgets::WidgetSource::Folder(folder.clone()),
        None => commands::widgets::WidgetSource::Server {
            profile: args.profile.name().to_string(),
        },
    };
    let request = commands::widgets::WidgetsRefreshRequest {
        source,
        lock_label: "widgets",
    };
    let mut notices = commands::Notices::default();
    let result = commands::widgets::execute(solution, &request, server::Client::new, &mut notices);
    for line in notices.lines() {
        eprintln!("{} {line}", style::prefix());
    }
    match result {
        Ok(outcome) => {
            if args.json.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&outcome).expect("the outcome serialises")
                );
            } else {
                print_refresh(&outcome);
            }
            OK
        }
        Err(error) => {
            eprintln!("{} widgets: {error}", style::prefix());
            FAILED
        }
    }
}

fn print_refresh(outcome: &Outcome) {
    for skipped in &outcome.skipped {
        eprintln!("{} skipped {skipped}", style::prefix());
    }
    let version = outcome
        .platform_version
        .as_deref()
        .map(|version| format!(" ThingWorx {version},"))
        .unwrap_or_default();
    println!(
        "read widget contracts from the {}{version} into {} ({})",
        outcome.source, outcome.cache, outcome.fetched_at
    );
    println!(
        "{} platform widgets in the embedded catalogue (it wins), {} legacy, {} extension, {} unknown",
        outcome.platform.len(),
        outcome.legacy.len(),
        outcome.extension.len(),
        outcome.unknown.len()
    );
    if !outcome.legacy.is_empty() {
        println!("legacy: {}", outcome.legacy.join(", "));
    }
    if !outcome.extension.is_empty() {
        println!("extension: {}", outcome.extension.join(", "));
    }
    for unknown in &outcome.unknown {
        println!(
            "unknown: {} ({}): {}",
            unknown.type_id, unknown.origin, unknown.why
        );
    }
}

/// `twaco widgets [<widget>] [--search <text>]`: read the embedded catalogue, and the widget
/// cache of the solution it runs in, when there is one.
pub(crate) fn widgets_cmd(args: &Widgets) -> u8 {
    if args.profile.profile.is_some() {
        eprintln!("{} --profile needs --platform", style::prefix());
        return FAILED;
    }
    // Finding the solution loads its widget cache, which adds legacy and extension widgets.
    let _ = std::env::current_dir().map(|here| Solution::discover(&here));
    if args.icons {
        return icons_cmd(args);
    }
    if !args.operands.is_empty() && args.search.is_some() {
        eprintln!("{} give a widget or --search, not both", style::prefix());
        return FAILED;
    }
    match args.operands.as_slice() {
        [] => list_cmd(args),
        [word] => match widgets_view::detail(word) {
            Ok(detail) => {
                if args.json.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&detail).expect("a widget serialises")
                    );
                } else {
                    print_detail(&detail);
                }
                OK
            }
            Err(error) => {
                eprintln!("{} {error}", style::prefix());
                FAILED
            }
        },
        _ => {
            eprintln!("{} widgets takes at most one <widget>", style::prefix());
            FAILED
        }
    }
}

/// `twaco widgets --icons [<filter>]`: the CDS icon names a `cds:` value may take.
fn icons_cmd(args: &Widgets) -> u8 {
    if args.search.is_some() || args.operands.len() > 1 {
        eprintln!("{} --icons takes at most one <filter>", style::prefix());
        return FAILED;
    }
    let icons = cds_icons::list(args.operands.first().map(String::as_str));
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&icons).expect("icon names serialise")
        );
    } else if icons.is_empty() {
        println!("no CDS icon matches; twaco widgets --icons lists them all");
    } else {
        for icon in &icons {
            println!("{icon}");
        }
    }
    OK
}

fn list_cmd(args: &Widgets) -> u8 {
    let search = args.search.as_deref();
    let widgets = widgets_view::list(search);
    if widgets.is_empty() && !args.json.json {
        println!(
            "no widget matches \"{}\"; twaco widgets lists them all",
            search.unwrap_or_default()
        );
        if let Some(note) = search.and_then(widgets_view::absent_note) {
            println!("{note}");
        }
        return OK;
    }
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&widgets).expect("widgets serialise")
        );
        return OK;
    }
    print!("ThingWorx 10.2 widgets");
    if let Some(search) = search {
        print!(" matching \"{search}\"");
    }
    println!();
    for widget in &widgets {
        print_summary(widget);
    }
    OK
}

fn print_summary(widget: &WidgetSummary) {
    print!(
        "  {} ({})  aliases: {}",
        widget.type_id,
        widget.type_display_name,
        widget.aliases.join(", ")
    );
    if widget.container {
        print!(" [container]");
    }
    if !widget.bindable {
        print!(" [no contract]");
    }
    println!();
    if let Some(hint) = &widget.hint {
        println!("    {hint}");
    }
}

fn print_detail(detail: &WidgetDetail) {
    println!("{} ({})", detail.type_id, detail.type_display_name);
    println!("aliases: {}", detail.aliases.join(", "));
    println!("properties:");
    for property in &detail.properties {
        print!(
            "  {} {}{}{}",
            property.name,
            if property.base_type.is_empty() {
                String::new()
            } else {
                format!("{} ", property.base_type)
            },
            property.direction,
            property
                .default
                .as_ref()
                .map(|value| format!(" (default {value})"))
                .unwrap_or_default()
        );
        if !property.values.is_empty() {
            let values = property
                .values
                .iter()
                .map(|entry| {
                    let value = entry
                        .value
                        .as_str()
                        .map_or_else(|| entry.value.to_string(), str::to_owned);
                    format!("{value} ({:?})", entry.label)
                })
                .collect::<Vec<_>>()
                .join(", ");
            print!("; allowed: {values}");
        }
        println!();
    }
    println!("events: {}", detail.events.join(", "));
    println!("services: {}", detail.services.join(", "));
    if detail.bindable {
        println!(
            "in a page spec: {}
  props take the properties above (name and base type are checked), events the events above, and services are targets of `invoke:` or `show:`",
            detail.page_spec
        );
    }
    if let Some(note) = &detail.note {
        println!("{note}");
    }
}
