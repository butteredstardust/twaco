use super::super::*;

/// `twaco help search <words> | page <page>`: the ThingWorx Platform help center, for the
/// server's own version unless told otherwise. Read-only; downloads go to the user's cache.
/// AGENTS.md and CLAUDE.md for the solution, each only where none exists, and the `.gitignore`
/// lines twaco's local state needs.
pub(crate) fn write_agent_files(solution: &Solution) -> u8 {
    let projects: Vec<String> = match solution.deploy_order() {
        Ok(order) => order.iter().map(|p| p.name.clone()).collect(),
        Err(_) => solution.projects.iter().map(|p| p.name.clone()).collect(),
    };
    match twaco::core::init::write_agent_files(&solution.root, &solution.solution.name, &projects) {
        Ok((wrote, kept)) => {
            for file in &wrote {
                match file.as_str() {
                    "AGENTS.md" => println!("wrote AGENTS.md: fill in its project context; the next agent starts from it"),
                    other => println!("wrote {other}"),
                }
            }
            for file in &kept {
                println!("{file} exists and is left alone");
            }
            if twaco::core::gitignore::in_git_work_tree(&solution.root) {
                match twaco::core::gitignore::add_missing(&solution.root) {
                    Ok(added) if added.is_empty() => {}
                    Ok(added) => println!(
                        "added {} line(s) to .gitignore: twaco's local state, backups included, is never committed",
                        added.len()
                    ),
                    Err(error) => {
                        eprintln!("twaco: .gitignore: {error}");
                        return FAILED;
                    }
                }
            }
            OK
        }
        Err(error) => {
            eprintln!("twaco: agent files: {error}");
            FAILED
        }
    }
}

/// `twaco update [--apply]`: compare with the latest release, and with `--apply` replace this
/// binary with it. No MCP tool: an agent must not replace the server it is talking to.
pub(crate) fn update_cmd(parsed: &Args) -> u8 {
    use twaco::core::update;
    if !parsed.names.is_empty() {
        eprintln!("twaco: update takes only --apply");
        return FAILED;
    }
    let current = env!("CARGO_PKG_VERSION");
    let web = update::Web::new(Duration::from_secs(120));
    let manifest = match update::manifest(&web, update::MANIFEST_URL, update::PUBLIC_KEY) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("twaco: update: {error}");
            return FAILED;
        }
    };
    // A manifest older than one seen before is an old copy: refuse it rather than report that
    // this twaco is current.
    if let Some(cache) = update::cache_file() {
        if let Err(error) = update::accept(&cache, &manifest.version) {
            eprintln!("twaco: update: {error}");
            return FAILED;
        }
    }
    if !update::is_newer(&manifest.version, current) {
        println!("twaco {current} is the latest release");
        return OK;
    }
    println!(
        "twaco {} is available (this is {current})",
        manifest.version
    );
    if !manifest.notes.trim().is_empty() {
        println!("\n{}\n", manifest.notes.trim());
    }
    // The real file, not a link to it: a link is replaced by a file otherwise.
    let exe = match std::env::current_exe().and_then(std::fs::canonicalize) {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("twaco: update: cannot find this executable: {error}");
            return FAILED;
        }
    };
    // A package manager or an AppImage owns some installs: say how to update those instead.
    let appimage = std::env::var("APPIMAGE").ok();
    let owner = update::owner(&exe, appimage.as_deref());
    if !parsed.flags.iter().any(|flag| flag == "--apply") {
        match owner {
            Some(why) => println!("nothing installed; {why}"),
            None => println!(
                "nothing installed; `twaco update --apply` replaces {}",
                exe.display()
            ),
        }
        return OK;
    }
    if let Some(why) = owner {
        eprintln!("twaco: update: {why}");
        return FAILED;
    }
    let installed = update::download(&web, &manifest, update::TARGET, update::PUBLIC_KEY)
        .and_then(|binary| update::install(&binary, &exe));
    match installed {
        Ok(()) => {
            println!("installed twaco {} at {}", manifest.version, exe.display());
            OK
        }
        Err(error) => {
            eprintln!("twaco: update: {error}");
            FAILED
        }
    }
}

/// `twaco guide [<topic> [--section <heading>]] [--search <words>]`: the knowledge an agent needs
/// besides the CLI, built in and the solution's own. Works outside a solution too.
pub(crate) fn guide_cmd(parsed: &Args) -> u8 {
    use twaco::core::guide;
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let solution = match Solution::discover(&here) {
        Ok(solution) => Some(solution),
        Err(twaco::core::config::ConfigError::NotFound { .. }) => None,
        Err(error) => {
            eprintln!("twaco: guide: {error}");
            return FAILED;
        }
    };
    let (topics, problems) = guide::topics(solution.as_ref());
    for problem in &problems {
        eprintln!("twaco: guide: {problem}");
    }
    let result: Result<(), String> = (|| {
        if let Some(query) = parsed.values.get("--search") {
            if !parsed.names.is_empty() {
                return Err("--search searches every topic; pass no topic with it".to_string());
            }
            let limit = match parsed.values.get("--limit") {
                Some(n) => n
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--limit takes a positive number")?,
                None => 10,
            };
            let hits = guide::search(&topics, query, limit);
            for hit in &hits {
                if parsed.has("--json") {
                    println!(
                        "{}",
                        serde_json::json!({ "topic": hit.topic, "section": hit.heading, "matched": hit.matched, "of": hit.of, "line": hit.line })
                    );
                } else {
                    let heading = if hit.heading.is_empty() {
                        "(introduction)"
                    } else {
                        hit.heading.as_str()
                    };
                    println!(
                        "{}: {heading}  [{}/{} words]",
                        hit.topic, hit.matched, hit.of
                    );
                    if !hit.line.is_empty() {
                        println!("    {}", hit.line);
                    }
                }
            }
            if hits.is_empty() {
                eprintln!("nothing matches {query:?}");
            } else if !parsed.has("--json") {
                eprintln!("read one: twaco guide <topic> --section \"<heading>\"");
            }
            return Ok(());
        }
        match parsed.names.as_slice() {
            [] => {
                for topic in &topics {
                    let origin = if topic.file.is_some() {
                        "solution"
                    } else {
                        "built in"
                    };
                    println!(
                        "{:<width$}  {:<9} {}",
                        topic.id,
                        origin,
                        topic.title,
                        width = topics.iter().map(|t| t.id.len()).max().unwrap_or(0)
                    );
                }
                eprintln!(
                    "read one: twaco guide <topic>; search all: twaco guide --search <words>"
                );
                Ok(())
            }
            [wanted] => {
                let topic = guide::find(&topics, wanted).map_err(|e| e.to_string())?;
                match guide::read(topic, parsed.values.get("--section").map(String::as_str))
                    .map_err(|e| e.to_string())?
                {
                    guide::Reading::Text(text) => print!("{text}"),
                    guide::Reading::Outline { title, headings } => {
                        println!("# {title}\n");
                        for heading in &headings {
                            println!("- {heading}");
                        }
                        eprintln!("{} sections; read one: twaco guide {} --section \"<heading or part of it>\"", headings.len(), topic.id);
                    }
                }
                Ok(())
            }
            _ => Err("guide takes one topic".to_string()),
        }
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: guide: {why}");
            FAILED
        }
    }
}

pub(crate) fn help_cmd(parsed: &Args) -> u8 {
    use twaco::core::help;
    let (Some(action), rest) = (
        parsed.names.first().map(String::as_str),
        parsed.names.get(1..).unwrap_or_default(),
    ) else {
        eprintln!("twaco: help needs `search <words>` or `page <page>`");
        return FAILED;
    };
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // No twaco.toml means no solution, and the newest help; a broken one is an error, not
    // a reason to skip the configured and the server's versions.
    let solution = match Solution::discover(&here) {
        Ok(solution) => Some(solution),
        Err(twaco::core::config::ConfigError::NotFound { .. }) => None,
        Err(error) => {
            eprintln!("twaco: help: {error}");
            return FAILED;
        }
    };
    let mut notes = Vec::new();
    let named = match action {
        "page" => match rest {
            [page] => match help::page_path(page) {
                Ok((version, _)) => version,
                Err(error) => {
                    eprintln!("twaco: help: {error}");
                    return FAILED;
                }
            },
            _ => {
                eprintln!("twaco: help page needs one page: a path such as ThingWorx/Welcome.html, or its address");
                return FAILED;
            }
        },
        _ => None,
    };
    let version = match help::choose_version(
        parsed.values.get("--version").map(String::as_str),
        named,
        solution.as_ref(),
        parsed.profile.as_deref().unwrap_or("default"),
        &mut notes,
    ) {
        Ok(version) => version,
        Err(why) => {
            eprintln!("twaco: help: {why}");
            return FAILED;
        }
    };
    for note in &notes {
        eprintln!("twaco: {note}");
    }
    let cache = match help::cache_root() {
        Ok(cache) => cache,
        Err(error) => {
            eprintln!("twaco: help: {error}");
            return FAILED;
        }
    };
    let web = help::Web::default();
    let refresh = parsed.has("--refresh");
    match action {
        "search" => {
            if rest.is_empty() {
                eprintln!("twaco: help search needs words");
                return FAILED;
            }
            let limit = match parsed.values.get("--limit").map(|n| n.parse::<usize>()) {
                None => 10,
                Some(Ok(n)) if n > 0 => n,
                Some(_) => {
                    eprintln!("twaco: help: --limit needs a positive whole number");
                    return FAILED;
                }
            };
            let index = match help::cached(&web, &cache, &version, help::INDEX_FILE, refresh)
                .and_then(|bytes| help::Index::parse(&String::from_utf8_lossy(&bytes)))
            {
                Ok(index) => index,
                Err(error) => {
                    eprintln!("twaco: help: {error}");
                    return FAILED;
                }
            };
            let found = help::search(&index, &version, &rest.join(" "), limit);
            if parsed.has("--json") {
                for hit in &found.hits {
                    println!(
                        "{}",
                        serde_json::json!({ "title": hit.page.title, "path": hit.page.path, "url": hit.url, "summary": hit.page.summary, "score": hit.score })
                    );
                }
            } else {
                for hit in &found.hits {
                    println!(
                        "{}
  {}
  {}
",
                        hit.page.title, hit.page.path, hit.page.summary
                    );
                }
            }
            if !found.unknown.is_empty() {
                eprintln!(
                    "twaco: the {version} help never uses: {}",
                    found.unknown.join(", ")
                );
            }
            eprintln!(
                "{} of {} matching page(s) from the ThingWorx Platform {version} help",
                found.hits.len(),
                found.matched
            );
            OK
        }
        "page" => {
            let section = parsed.values.get("--section").map(String::as_str);
            let (_, path) = help::page_path(&rest[0]).expect("checked above");
            match help::cached(&web, &cache, &version, &path, refresh)
                .and_then(|html| help::read(&html, &version, &path, section))
            {
                Ok(read) => {
                    println!("{}", read.markdown);
                    eprintln!("\nfrom {}", read.url);
                    OK
                }
                Err(error) => {
                    eprintln!("twaco: help: {error}");
                    FAILED
                }
            }
        }
        other => {
            eprintln!("twaco: help has `search` and `page`, not {other:?}");
            FAILED
        }
    }
}

/// `twaco javadoc search <name> | class <Name>`: the public ThingWorx Java API docs.
/// Read-only and usable outside a solution; downloads go to the user's cache.
pub(crate) fn javadoc_cmd(parsed: &Args) -> u8 {
    use twaco::core::{help, javadoc};
    let (Some(action), rest) = (
        parsed.names.first().map(String::as_str),
        parsed.names.get(1..).unwrap_or_default(),
    ) else {
        eprintln!("twaco: javadoc needs `search <name>` or `class <Name>`");
        return FAILED;
    };
    match action {
        "search" if rest.is_empty() => {
            eprintln!("twaco: javadoc search needs a name");
            return FAILED;
        }
        "class" if rest.len() != 1 => {
            eprintln!("twaco: javadoc class needs one class name");
            return FAILED;
        }
        "search" | "class" => {}
        other => {
            eprintln!("twaco: javadoc has `search` and `class`, not {other:?}");
            return FAILED;
        }
    }
    let cache = match javadoc::cache_root() {
        Ok(cache) => cache,
        Err(error) => {
            eprintln!("twaco: javadoc: {error}");
            return FAILED;
        }
    };
    let web = help::Web::new(javadoc::BASE);
    let refresh = parsed.has("--refresh");
    let fetch = |path: &str| javadoc::cached(&web, &cache, path, refresh);
    let types = match fetch(javadoc::TYPE_INDEX) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("twaco: javadoc: {error}");
            return FAILED;
        }
    };
    match action {
        "search" => {
            if parsed.values.contains_key("--member") {
                eprintln!("twaco: javadoc: --member is only for `class`");
                return FAILED;
            }
            let limit = match parsed.values.get("--limit").map(|n| n.parse::<usize>()) {
                None => 10,
                Some(Ok(n)) if n > 0 => n,
                Some(_) => {
                    eprintln!("twaco: javadoc: --limit needs a positive whole number");
                    return FAILED;
                }
            };
            let members = match fetch(javadoc::MEMBER_INDEX) {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!("twaco: javadoc: {error}");
                    return FAILED;
                }
            };
            let index = match javadoc::Index::parse(
                &String::from_utf8_lossy(&types),
                &String::from_utf8_lossy(&members),
            ) {
                Ok(index) => index,
                Err(error) => {
                    eprintln!("twaco: javadoc: {error}");
                    return FAILED;
                }
            };
            let hits = javadoc::search(&index, &rest.join(" "), limit);
            for hit in &hits {
                if parsed.has("--json") {
                    println!(
                        "{}",
                        serde_json::json!({
                            "kind": if hit.kind == javadoc::Kind::Class { "class" } else { "member" },
                            "package": hit.package, "class": hit.class, "label": hit.label,
                            "path": hit.path, "url": hit.url, "rank": hit.rank,
                        })
                    );
                } else {
                    println!("{}", hit.display());
                }
            }
            eprintln!(
                "{} result(s) from the ThingWorx Platform API {} Javadoc",
                hits.len(),
                javadoc::VERSION
            );
            OK
        }
        "class" => {
            let index = match javadoc::Index::parse(
                &String::from_utf8_lossy(&types),
                "memberSearchIndex = []",
            ) {
                Ok(index) => index,
                Err(error) => {
                    eprintln!("twaco: javadoc: {error}");
                    return FAILED;
                }
            };
            let class = match javadoc::find_class(&index, &rest[0]) {
                Ok(class) => class,
                Err(error) => {
                    eprintln!("twaco: javadoc: {error}");
                    return FAILED;
                }
            };
            let path = match javadoc::class_path(&class) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!("twaco: javadoc: {error}");
                    return FAILED;
                }
            };
            match fetch(&path).and_then(|html| {
                javadoc::read(
                    &html,
                    &class,
                    parsed.values.get("--member").map(String::as_str),
                )
            }) {
                Ok(read) => {
                    if parsed.has("--json") {
                        println!(
                            "{}",
                            serde_json::json!({
                                "version": javadoc::VERSION, "title": read.title, "path": read.path,
                                "url": read.url, "methods": read.methods, "markdown": read.markdown,
                            })
                        );
                    } else {
                        println!("{}", read.markdown);
                        eprintln!("from {}", read.url);
                    }
                    OK
                }
                Err(error) => {
                    eprintln!("twaco: javadoc: {error}");
                    FAILED
                }
            }
        }
        _ => unreachable!("the action was checked above"),
    }
}

/// `twaco settings [<Subsystem> [<Table>]] [--search <text>]`: the server's subsystem
/// settings, read-only.
pub(crate) fn settings_cmd(solution: &Solution, args: &Args) -> u8 {
    use twaco::core::settings;
    let profile_name = args.profile.as_deref().unwrap_or("default");
    let client = match profile::load(&solution.root, profile_name) {
        Ok(profile) => server::Client::new(profile),
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    let print_table = |subsystem: &str, table: &settings::Table| {
        println!("{subsystem}.{}", table.name);
        if table.rows.is_empty() {
            println!("  (no rows)");
        }
        for (index, row) in table.rows.iter().enumerate() {
            if table.rows.len() > 1 {
                println!("  row {}", index + 1);
            }
            for field in &table.fields {
                let value = row
                    .get(&field.name)
                    .map(settings::shown)
                    .unwrap_or_default();
                println!("  {:<40} {:<24} {}", field.name, value, field.description);
            }
        }
    };
    let result: Result<(), String> = (|| {
        if let Some(text) = args.values.get("--search") {
            let all = settings::read_all(&client).map_err(|e| e.to_string())?;
            let found = settings::search(&all, text);
            for f in &found {
                let values: Vec<String> = f.values.iter().map(settings::shown).collect();
                if args.has("--json") {
                    println!(
                        "{}",
                        serde_json::json!({ "subsystem": f.subsystem, "table": f.table, "setting": f.field.name, "type": f.field.base_type, "values": f.values, "description": f.field.description })
                    );
                } else {
                    println!(
                        "{}.{}.{} = {}  ({})",
                        f.subsystem,
                        f.table,
                        f.field.name,
                        values.join(" | "),
                        f.field.description
                    );
                }
            }
            eprintln!("{} setting(s) match {text:?}", found.len());
            return Ok(());
        }
        let names = settings::Remote::subsystems(&client).map_err(|e| e.to_string())?;
        match args.names.as_slice() {
            [] => {
                let all = settings::summaries(&client).map_err(|e| e.to_string())?;
                for settings::Summary {
                    name,
                    running,
                    tables,
                } in &all
                {
                    if args.has("--json") {
                        println!(
                            "{}",
                            serde_json::json!({ "subsystem": name, "running": running, "tables": tables })
                        );
                    } else {
                        println!(
                            "{name:<34} {:<8} {}",
                            if *running { "running" } else { "stopped" },
                            tables.join(", ")
                        );
                    }
                }
                eprintln!("{} subsystem(s)", all.len());
                Ok(())
            }
            [subsystem, rest @ ..] if rest.len() <= 1 => {
                let name = settings::resolve(&names, subsystem).map_err(|e| e.to_string())?;
                let read = settings::read(&client, name).map_err(|e| e.to_string())?;
                let mut shown = 0;
                for table in &read.tables {
                    if rest
                        .first()
                        .is_some_and(|wanted| !table.name.eq_ignore_ascii_case(wanted))
                    {
                        continue;
                    }
                    shown += 1;
                    if args.has("--json") {
                        println!("{}", settings::table_json(&read.name, table));
                    } else {
                        print_table(&read.name, table);
                    }
                }
                if shown == 0 {
                    let tables: Vec<&str> = read.tables.iter().map(|t| t.name.as_str()).collect();
                    return Err(format!(
                        "{} has no table {:?}; it has: {}",
                        read.name,
                        rest.first().map(String::as_str).unwrap_or(""),
                        tables.join(", ")
                    ));
                }
                Ok(())
            }
            _ => Err("settings takes: [<Subsystem> [<Table>]] or --search <text>".to_string()),
        }
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("twaco: settings: {why}");
            FAILED
        }
    }
}

/// `twaco catalog [<entity>] [--project P] [--search <text>] [--json]`.
/// What changing an entity, or one service, property or field of it, would reach. Offline and
/// read-only: it reads the solution into the index and asks it.
pub(crate) fn impact_cmd(solution: &Solution, args: &Args) -> u8 {
    let [entity] = args.names.as_slice() else {
        eprintln!("twaco: impact needs one <entity>, such as Acme.Orders.Manager");
        return FAILED;
    };
    if args.has("--json") && args.has("--dot") {
        eprintln!("twaco: --json and --dot are different outputs; pick one");
        return FAILED;
    }
    let min = match args.values.get("--min-confidence") {
        None => Confidence::Review,
        Some(word) => match Confidence::parse(word) {
            Some(confidence) => confidence,
            None => {
                eprintln!(
                    "twaco: --min-confidence is structural, resolved or review, not {word:?}"
                );
                return FAILED;
            }
        },
    };
    let depth = match args.values.get("--depth") {
        None => None,
        Some(text) => match text.parse::<usize>() {
            Ok(depth) if depth > 0 => Some(depth),
            _ => {
                eprintln!("twaco: --depth is a positive whole number, not {text:?}");
                return FAILED;
            }
        },
    };
    let request = impact::Request {
        entity: entity.clone(),
        member: args.values.get("--member").cloned(),
        min,
        depth,
    };
    let report = match impact::run(solution, &request) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("twaco: {error}");
            return FAILED;
        }
    };
    if args.has("--json") {
        let value = report.to_json(args.has("--detail"));
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("a report serialises")
        );
    } else if args.has("--dot") {
        print!("{}", impact::render_dot(&report));
    } else {
        print!("{}", impact::render_text(&report, args.has("--detail")));
    }
    OK
}

/// Entities no entry point reaches. Offline, advisory and read-only: it deletes nothing.
pub(crate) fn unused_cmd(solution: &Solution, args: &Args) -> u8 {
    if !args.names.is_empty() {
        eprintln!("twaco: unused takes no entity; use --collection to narrow it");
        return FAILED;
    }
    let min = match args.values.get("--min-confidence") {
        None => Confidence::Review,
        Some(word) => match Confidence::parse(word) {
            Some(confidence) => confidence,
            None => {
                eprintln!(
                    "twaco: --min-confidence is structural, resolved or review, not {word:?}"
                );
                return FAILED;
            }
        },
    };
    let collection = args.values.get("--collection").cloned();
    if let Some(name) = &collection {
        if !unused::JUDGED.contains(&name.as_str()) {
            eprintln!(
                "twaco: --collection is one of {}, not {name:?}",
                unused::JUDGED.join(", ")
            );
            return FAILED;
        }
    }
    let report = unused::run(solution, &unused::Request { min, collection });
    if args.has("--json") {
        let value = report.to_json(args.has("--detail"));
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("a report serialises")
        );
    } else {
        print!("{}", unused::render_text(&report, args.has("--detail")));
    }
    OK
}

/// The solution written down. Offline; with `--out` it writes exactly one file.
pub(crate) fn docs_cmd(solution: &Solution, args: &Args) -> u8 {
    if !args.names.is_empty() {
        eprintln!("twaco: docs takes no entity name");
        return FAILED;
    }
    if args.has("--force") && args.out.is_none() {
        eprintln!("twaco: --force only applies to --out");
        return FAILED;
    }
    let document = docs::build(solution);
    let detail = args.has("--detail");
    let text = if args.has("--json") {
        let mut json =
            serde_json::to_string_pretty(&document.to_json(detail)).expect("a document serialises");
        json.push('\n');
        json
    } else {
        docs::render_markdown(&document, detail)
    };
    match &args.out {
        None => print!("{text}"),
        Some(path) => {
            if let Err(error) = docs::write(path, &text, args.has("--force")) {
                eprintln!("twaco: docs: {error}");
                return FAILED;
            }
            println!("{} bytes to {}", text.len(), path.display());
        }
    }
    OK
}

pub(crate) fn catalog_cmd(solution: &Solution, args: &Args) -> u8 {
    if args.names.len() > 1 {
        eprintln!("twaco: catalog takes at most one entity name");
        return FAILED;
    }
    let query = catalog::Query {
        project: args.project.as_deref(),
        entity: args.names.first().map(String::as_str),
        text: args.values.get("--search").map(String::as_str),
    };
    let result = match catalog::build(solution, query) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("twaco: catalog: {error}");
            return FAILED;
        }
    };
    for skipped in &result.skipped {
        eprintln!("twaco: skipped {skipped}");
    }
    for entity in &result.entities {
        if !args.has("--json") {
            let mut notes = Vec::new();
            if !entity.inherits.is_empty() {
                notes.push(format!("inherits {}", entity.inherits.join(", ")));
            }
            if !entity.implemented_by.is_empty() {
                notes.push(format!(
                    "implemented by {}",
                    entity.implemented_by.join(", ")
                ));
            }
            let notes = if notes.is_empty() {
                String::new()
            } else {
                format!("  [{}]", notes.join("; "))
            };
            println!("{}/{}{}", entity.collection, entity.name, notes);
        }
        for service in &entity.services {
            if args.has("--json") {
                println!(
                    "{}",
                    serde_json::json!({
                        "collection": entity.collection,
                        "entity": entity.name,
                        "project": entity.project,
                        "inherits": entity.inherits,
                        "implemented_by": entity.implemented_by,
                        "service": service.name,
                        "parameters": service.parameters,
                        "result": service.result,
                        "description": service.description,
                        "from": service.from,
                        "has_script": service.has_script,
                    })
                );
            } else {
                let parameters = service
                    .parameters
                    .iter()
                    .map(|parameter| {
                        format!(
                            "{}: {}",
                            parameter.name,
                            catalog_type(&parameter.base_type, parameter.data_shape.as_deref())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let origin = if service.from == "own" {
                    "[own]".to_string()
                } else {
                    format!("[from {}]", service.from)
                };
                let description = if service.description.is_empty() {
                    String::new()
                } else {
                    format!(
                        "  {}",
                        service
                            .description
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                };
                println!(
                    "  {}({}) -> {}  {}{}",
                    service.name,
                    parameters,
                    catalog_type(
                        &service.result.base_type,
                        service.result.data_shape.as_deref()
                    ),
                    origin,
                    description
                );
            }
        }
    }
    eprintln!(
        "{} service(s) on {} entities",
        result.service_count(),
        result.entities.len()
    );
    OK
}

fn catalog_type(base_type: &str, data_shape: Option<&str>) -> String {
    match data_shape {
        Some(shape) => format!("{base_type}<{shape}>"),
        None => base_type.to_string(),
    }
}
