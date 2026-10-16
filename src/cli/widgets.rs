use super::args::Args;
use super::style;
use crate::{FAILED, OK};
use twaco::core::widgets_view::{self, WidgetDetail, WidgetSummary};

/// `twaco widgets [<widget>] [--search <text>]`: read the embedded catalogue.
pub(crate) fn widgets_cmd(args: &Args) -> u8 {
    if !args.names.is_empty() && args.values.contains_key("--search") {
        eprintln!("{} give a widget or --search, not both", style::prefix());
        return FAILED;
    }
    match args.names.as_slice() {
        [] => list_cmd(args),
        [word] => match widgets_view::detail(word) {
            Ok(detail) => {
                if args.has("--json") {
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

fn list_cmd(args: &Args) -> u8 {
    let search = args.values.get("--search").map(String::as_str);
    let widgets = widgets_view::list(search);
    if widgets.is_empty() && !args.has("--json") {
        println!(
            "no widget matches \"{}\"; twaco widgets lists them all",
            search.unwrap_or_default()
        );
        return OK;
    }
    if args.has("--json") {
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
}

fn print_detail(detail: &WidgetDetail) {
    println!("{} ({})", detail.type_id, detail.type_display_name);
    println!("aliases: {}", detail.aliases.join(", "));
    println!("properties:");
    for property in &detail.properties {
        println!(
            "  {} {} {}",
            property.name, property.base_type, property.direction
        );
    }
    println!("events: {}", detail.events.join(", "));
    println!("services: {}", detail.services.join(", "));
    if let Some(note) = &detail.note {
        println!("{note}");
    }
}
