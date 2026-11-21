use super::flags::Json;
use super::style;
use crate::{FAILED, OK};
use twaco::core::cds_icons;
use twaco::core::widgets_view::{self, WidgetDetail, WidgetSummary};

/// `twaco widgets [<widget>] [--search <text>] [--json]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Widgets {
    #[arg(long = "search", value_name = "text")]
    pub(crate) search: Option<String>,
    #[arg(long = "icons")]
    pub(crate) icons: bool,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco widgets [<widget>] [--search <text>]`: read the embedded catalogue.
pub(crate) fn widgets_cmd(args: &Widgets) -> u8 {
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
        println!(
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
    }
    println!("events: {}", detail.events.join(", "));
    println!("services: {}", detail.services.join(", "));
    if let Some(note) = &detail.note {
        println!("{note}");
    }
}
