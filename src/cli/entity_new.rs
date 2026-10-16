use super::super::*;
use super::style;
use twaco::core::entity_new::{self, Kind, NewRequest};

/// `twaco new <kind> <name>`: plan a common entity, or create its XML and sidecars atomically.
pub(crate) fn entity_new_cmd(solution: &Solution, route: &str, args: &Args) -> u8 {
    let [name] = args.names.as_slice() else {
        eprintln!("{} {route} needs one <name>", style::prefix());
        return FAILED;
    };
    let kind = Kind::parse(route.strip_prefix("new ").unwrap_or_default())
        .expect("new routes have known kinds");
    let request = NewRequest {
        kind,
        name: name.clone(),
        project: args.project.clone(),
        description: args
            .values
            .get("--description")
            .cloned()
            .unwrap_or_default(),
        visibility: args.visibility.clone(),
        template: args.values.get("--template").cloned(),
        shapes: args.shapes.clone(),
        value_stream: args.values.get("--value-stream").cloned(),
        base: args.values.get("--base").cloned(),
        fields: args.fields.clone(),
        every: args.values.get("--every").cloned(),
        cron: args.values.get("--cron").cloned(),
        run_as: args.values.get("--run-as").cloned(),
        disabled: args.has("--disabled"),
        then: args.values.get("--then").cloned(),
        script: args.values.get("--script").map(Into::into),
        data_shape: args.values.get("--data-shape").cloned(),
        indexes: args.indexes.clone(),
        file: args.values.get("--file").map(Into::into),
        background: args.values.get("--background").cloned(),
        foreground: args.values.get("--foreground").cloned(),
        secondary_background: args.values.get("--secondary-background").cloned(),
        line_color: args.values.get("--line-color").cloned(),
        line_style: args.values.get("--line-style").cloned(),
        line_thickness: args.values.get("--line-thickness").cloned(),
        bold: args.has("--bold"),
        italic: args.has("--italic"),
        underline: args.has("--underline"),
        text_size: args.values.get("--text-size").cloned(),
        image: args.values.get("--image").cloned(),
        display_string: args.values.get("--display-string").cloned(),
        state_type: args.values.get("--type").cloned(),
        states: args.states.clone(),
        items: args.items.clone(),
        default: args.has("--default"),
        label: args.values.get("--label").cloned(),
        from: args.values.get("--from").cloned(),
        props: args.props.clone(),
        theme_content: None,
    };
    let request = commands::entity_new::Request {
        request,
        mode: if args.has("--apply") {
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
    let report = entity_new::plan_json(solution, &outcome.plan, outcome.applied);
    if args.has("--json") {
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
