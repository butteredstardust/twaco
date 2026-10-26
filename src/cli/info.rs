use super::flags::{Apply, Detail, Json, Profile, Project};
use super::style;
use crate::{DRIFT, FAILED, OK};
use std::path::PathBuf;
use std::time::Duration;
use twaco::core::config::Solution;
use twaco::core::index::Confidence;
use twaco::core::{catalog, docs, impact, profile, server, unused};

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
                        eprintln!("{} .gitignore: {error}", style::prefix());
                        return FAILED;
                    }
                }
            }
            OK
        }
        Err(error) => {
            eprintln!("{} agent files: {error}", style::prefix());
            FAILED
        }
    }
}

/// `twaco update [--apply]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Update {
    #[command(flatten)]
    pub(crate) apply: Apply,
}

/// `twaco update [--apply]`: compare with the latest release, and with `--apply` replace this
/// binary with it. No MCP tool: an agent must not replace the server it is talking to.
pub(crate) fn update_cmd(parsed: &Update) -> u8 {
    use twaco::core::update;
    let current = env!("CARGO_PKG_VERSION");
    let web = update::Web::new(Duration::from_secs(120));
    let manifest = match update::manifest(&web, update::MANIFEST_URL, update::PUBLIC_KEY) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("{} update: {error}", style::prefix());
            return FAILED;
        }
    };
    // A manifest older than one seen before is an old copy: refuse it rather than report that
    // this twaco is current.
    if let Some(cache) = update::cache_file() {
        if let Err(error) = update::accept(&cache, &manifest.version) {
            eprintln!("{} update: {error}", style::prefix());
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
            eprintln!(
                "{} update: cannot find this executable: {error}",
                style::prefix()
            );
            return FAILED;
        }
    };
    // A package manager or an AppImage owns some installs: say how to update those instead.
    let appimage = std::env::var("APPIMAGE").ok();
    let owner = update::owner(&exe, appimage.as_deref());
    if !parsed.apply.apply {
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
        eprintln!("{} update: {why}", style::prefix());
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
            eprintln!("{} update: {error}", style::prefix());
            FAILED
        }
    }
}

/// `twaco guide [<topic> [--section <heading>]] [--search <words>]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Guide {
    #[arg(long = "section", value_name = "heading")]
    pub(crate) section: Option<String>,
    #[arg(long = "search", value_name = "text")]
    pub(crate) search: Option<String>,
    #[arg(long = "limit", value_name = "n")]
    pub(crate) limit: Option<String>,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco guide [<topic> [--section <heading>]] [--search <words>]`: the knowledge an agent needs
/// besides the CLI, built in and the solution's own. Works outside a solution too.
pub(crate) fn guide_cmd(parsed: &Guide) -> u8 {
    use twaco::core::guide;
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let solution = match Solution::discover(&here) {
        Ok(solution) => Some(solution),
        Err(twaco::core::config::ConfigError::NotFound { .. }) => None,
        Err(error) => {
            eprintln!("{} guide: {error}", style::prefix());
            return FAILED;
        }
    };
    let (topics, problems) = guide::topics(solution.as_ref());
    for problem in &problems {
        eprintln!("{} guide: {problem}", style::prefix());
    }
    let result: Result<(), String> = (|| {
        if let Some(query) = parsed.search.as_ref() {
            if !parsed.operands.is_empty() {
                return Err("--search searches every topic; pass no topic with it".to_string());
            }
            let limit = match parsed.limit.as_ref() {
                Some(n) => n
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--limit takes a positive number")?,
                None => 10,
            };
            let hits = guide::search(&topics, query, limit);
            for hit in &hits {
                if parsed.json.json {
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
            } else if !parsed.json.json {
                eprintln!("read one: twaco guide <topic> --section \"<heading>\"");
            }
            return Ok(());
        }
        match parsed.operands.as_slice() {
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
                match guide::read(topic, parsed.section.as_deref()).map_err(|e| e.to_string())? {
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
            eprintln!("{} guide: {why}", style::prefix());
            FAILED
        }
    }
}

/// `twaco help search <words> | page <page>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Help {
    #[arg(long = "version", value_name = "release")]
    pub(crate) version: Option<String>,
    #[arg(long = "limit", value_name = "n")]
    pub(crate) limit: Option<String>,
    #[arg(long = "section", value_name = "heading")]
    pub(crate) section: Option<String>,
    #[arg(long = "refresh")]
    pub(crate) refresh: bool,
    #[command(flatten)]
    pub(crate) json: Json,
    #[command(flatten)]
    pub(crate) profile: Profile,
    /// What to do; clap refuses any other word, with the ones it takes.
    #[arg(id = "action", value_name = "action", value_enum)]
    pub(crate) action: Option<HelpVerb>,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// What `help` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum HelpVerb {
    Search,
    Page,
}

pub(crate) fn help_cmd(parsed: &Help) -> u8 {
    use twaco::core::help;
    let (Some(action), rest) = (parsed.action, parsed.operands.as_slice()) else {
        eprintln!(
            "{} help needs `search <words>` or `page <page>`",
            style::prefix()
        );
        return FAILED;
    };
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // No twaco.toml means no solution, and the newest help; a broken one is an error, not
    // a reason to skip the configured and the server's versions.
    let solution = match Solution::discover(&here) {
        Ok(solution) => Some(solution),
        Err(twaco::core::config::ConfigError::NotFound { .. }) => None,
        Err(error) => {
            eprintln!("{} help: {error}", style::prefix());
            return FAILED;
        }
    };
    let mut notes = Vec::new();
    let named = match action {
        HelpVerb::Page => match rest {
            [page] => match help::page_path(page) {
                Ok((version, _)) => version,
                Err(error) => {
                    eprintln!("{} help: {error}", style::prefix());
                    return FAILED;
                }
            },
            _ => {
                eprintln!("{} help page needs one page: a path such as ThingWorx/Welcome.html, or its address", style::prefix());
                return FAILED;
            }
        },
        HelpVerb::Search => None,
    };
    let version = match help::choose_version(
        parsed.version.as_deref(),
        named,
        solution.as_ref(),
        parsed.profile.name(),
        &mut notes,
    ) {
        Ok(version) => version,
        Err(why) => {
            eprintln!("{} help: {why}", style::prefix());
            return FAILED;
        }
    };
    for note in &notes {
        eprintln!("{} {note}", style::prefix());
    }
    let cache = match help::cache_root() {
        Ok(cache) => cache,
        Err(error) => {
            eprintln!("{} help: {error}", style::prefix());
            return FAILED;
        }
    };
    let web = help::Web::default();
    let refresh = parsed.refresh;
    match action {
        HelpVerb::Search => {
            if rest.is_empty() {
                eprintln!("{} help search needs words", style::prefix());
                return FAILED;
            }
            let limit = match parsed.limit.as_ref().map(|n| n.parse::<usize>()) {
                None => 10,
                Some(Ok(n)) if n > 0 => n,
                Some(_) => {
                    eprintln!(
                        "{} help: --limit needs a positive whole number",
                        style::prefix()
                    );
                    return FAILED;
                }
            };
            let index = match help::cached(&web, &cache, &version, help::INDEX_FILE, refresh)
                .and_then(|bytes| help::Index::parse(&String::from_utf8_lossy(&bytes)))
            {
                Ok(index) => index,
                Err(error) => {
                    eprintln!("{} help: {error}", style::prefix());
                    return FAILED;
                }
            };
            let found = help::search(&index, &version, &rest.join(" "), limit);
            if parsed.json.json {
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
                    "{} the {version} help never uses: {}",
                    style::prefix(),
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
        HelpVerb::Page => {
            let section = parsed.section.as_deref();
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
                    eprintln!("{} help: {error}", style::prefix());
                    FAILED
                }
            }
        }
    }
}

/// `twaco javadoc search <name> | class <Name> | service <ref>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Javadoc {
    #[arg(long = "sdk", value_name = "path")]
    pub(crate) sdk: Option<PathBuf>,
    #[arg(long = "member", value_name = "name")]
    pub(crate) member: Option<String>,
    #[arg(long = "limit", value_name = "n")]
    pub(crate) limit: Option<String>,
    #[arg(long = "refresh")]
    pub(crate) refresh: bool,
    #[command(flatten)]
    pub(crate) json: Json,
    #[command(flatten)]
    pub(crate) profile: Profile,
    /// What to do; clap refuses any other word, with the ones it takes.
    #[arg(id = "action", value_name = "action", value_enum)]
    pub(crate) action: Option<JavadocVerb>,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// What `javadoc` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum JavadocVerb {
    Search,
    Class,
    Service,
}

/// `twaco javadoc search <name> | class <Name> | service <ref>`: the public ThingWorx Java API
/// docs.
/// Read-only and usable outside a solution; downloads go to the user's cache.
pub(crate) fn javadoc_cmd(parsed: &Javadoc) -> u8 {
    use twaco::core::javadoc;
    let (Some(action), rest) = (parsed.action, parsed.operands.as_slice()) else {
        eprintln!(
            "{} javadoc needs `search <name>`, `class <Name>` or `service <ref>`",
            style::prefix()
        );
        return FAILED;
    };
    match action {
        JavadocVerb::Search if rest.is_empty() => {
            eprintln!("{} javadoc search needs a name", style::prefix());
            return FAILED;
        }
        JavadocVerb::Class if rest.len() != 1 => {
            eprintln!("{} javadoc class needs one class name", style::prefix());
            return FAILED;
        }
        JavadocVerb::Service if rest.is_empty() => {
            eprintln!(
                "{} javadoc service needs a reference, such as 'Resources[\"InfoTableFunctions\"].Sort'",
                style::prefix()
            );
            return FAILED;
        }
        JavadocVerb::Search | JavadocVerb::Class | JavadocVerb::Service => {}
    }
    if parsed.member.is_some() && action != JavadocVerb::Class {
        eprintln!("{} javadoc: --member is only for `class`", style::prefix());
        return FAILED;
    }
    let source = match javadoc_source(parsed) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("{} javadoc: {error}", style::prefix());
            return FAILED;
        }
    };
    let fetch = |path: &str| source.fetch(path);
    if action == JavadocVerb::Service {
        return match javadoc::read_service_at(&fetch, &rest.join(" "), &|path| source.url(path)) {
            Ok(found) => {
                if parsed.json.json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "version": source.version(), "asked": found.asked,
                            "declared_in": found.declared_in, "inherited": found.inherited,
                            "title": found.reading.title, "path": found.reading.path,
                            "url": found.reading.url, "methods": found.reading.methods,
                            "markdown": found.reading.markdown, "note": found.note,
                        })
                    );
                } else {
                    println!("{}", found.reading.markdown);
                    let inherited = if found.inherited { ", inherited" } else { "" };
                    eprintln!(
                        "{} is declared in {}{inherited}; from {}",
                        found.asked,
                        found.declared_in,
                        source.display(&found.reading.path)
                    );
                    if let Some(note) = &found.note {
                        eprintln!("note: {note}");
                    }
                }
                OK
            }
            Err(error) => {
                eprintln!("{} javadoc: {error}", style::prefix());
                FAILED
            }
        };
    }
    let types = match fetch(javadoc::TYPE_INDEX) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{} javadoc: {error}", style::prefix());
            return FAILED;
        }
    };
    match action {
        JavadocVerb::Service => unreachable!("answered above"),
        JavadocVerb::Search => {
            let limit = match parsed.limit.as_ref().map(|n| n.parse::<usize>()) {
                None => 10,
                Some(Ok(n)) if n > 0 => n,
                Some(_) => {
                    eprintln!(
                        "{} javadoc: --limit needs a positive whole number",
                        style::prefix()
                    );
                    return FAILED;
                }
            };
            let members = match fetch(javadoc::MEMBER_INDEX) {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!("{} javadoc: {error}", style::prefix());
                    return FAILED;
                }
            };
            let index = match javadoc::Index::parse(
                &String::from_utf8_lossy(&types),
                &String::from_utf8_lossy(&members),
            ) {
                Ok(index) => index,
                Err(error) => {
                    eprintln!("{} javadoc: {error}", style::prefix());
                    return FAILED;
                }
            };
            let hits = javadoc::search(&index, &rest.join(" "), limit);
            for hit in &hits {
                if parsed.json.json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "kind": if hit.kind == javadoc::Kind::Class { "class" } else { "member" },
                            "package": hit.package, "class": hit.class, "label": hit.label,
                            "path": hit.path, "url": source.url(&hit.path), "rank": hit.rank,
                        })
                    );
                } else {
                    println!("{}", hit.display());
                }
            }
            eprintln!(
                "{} result(s) from the ThingWorx Platform API {} Javadoc",
                hits.len(),
                source.version()
            );
            OK
        }
        JavadocVerb::Class => {
            let index = match javadoc::Index::parse(
                &String::from_utf8_lossy(&types),
                "memberSearchIndex = []",
            ) {
                Ok(index) => index,
                Err(error) => {
                    eprintln!("{} javadoc: {error}", style::prefix());
                    return FAILED;
                }
            };
            let class = match javadoc::find_class(&index, &rest[0]) {
                Ok(class) => class,
                Err(error) => {
                    eprintln!("{} javadoc: {error}", style::prefix());
                    return FAILED;
                }
            };
            let path = match javadoc::class_path(&class) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!("{} javadoc: {error}", style::prefix());
                    return FAILED;
                }
            };
            match fetch(&path).map_err(|e| e.to_string()).and_then(|html| {
                javadoc::read_at(&html, &class, parsed.member.as_deref(), source.url(&path))
                    .map_err(|e| e.to_string())
            }) {
                Ok(read) => {
                    if parsed.json.json {
                        println!(
                            "{}",
                            serde_json::json!({
                                "version": source.version(), "title": read.title, "path": read.path,
                                "url": read.url, "methods": read.methods, "markdown": read.markdown,
                            })
                        );
                    } else {
                        println!("{}", read.markdown);
                        eprintln!("from {}", source.display(&read.path));
                    }
                    OK
                }
                Err(error) => {
                    eprintln!("{} javadoc: {error}", style::prefix());
                    FAILED
                }
            }
        }
    }
}

fn javadoc_source(parsed: &Javadoc) -> Result<twaco::core::javadoc::Source, String> {
    use twaco::core::javadoc;
    let environment = std::env::var_os("TWACO_SDK").map(PathBuf::from);
    let mut configured_profile = None;
    let here = std::env::current_dir().map_err(|error| error.to_string())?;
    if let Ok(solution) = Solution::discover(&here) {
        match profile::load(&solution.root, parsed.profile.name()) {
            Ok(profile) => {
                if let Some(path) = profile.sdk() {
                    configured_profile = Some(path.to_path_buf());
                }
            }
            Err(profile::ProfileError::Missing { .. }) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    javadoc::Source::configured(
        parsed.sdk.as_deref(),
        environment.as_deref(),
        configured_profile.as_deref(),
        parsed.profile.name(),
        parsed.refresh,
    )
}

/// `twaco settings [<Subsystem> [<Table>]] [--search <text>] [--json]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Settings {
    #[arg(long = "search", value_name = "text")]
    pub(crate) search: Option<String>,
    #[command(flatten)]
    pub(crate) json: Json,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco settings [<Subsystem> [<Table>]] [--search <text>]`: the server's subsystem
/// settings, read-only.
pub(crate) fn settings_cmd(solution: &Solution, args: &Settings) -> u8 {
    use twaco::core::settings;
    let profile_name = args.profile.name();
    let client = match profile::load(&solution.root, profile_name) {
        Ok(profile) => server::Client::new(profile),
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
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
        if let Some(text) = args.search.as_ref() {
            let all = settings::read_all(&client).map_err(|e| e.to_string())?;
            let found = settings::search(&all, text);
            for f in &found {
                let values: Vec<String> = f.values.iter().map(settings::shown).collect();
                if args.json.json {
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
        match args.operands.as_slice() {
            [] => {
                let all = settings::summaries(&client).map_err(|e| e.to_string())?;
                for settings::Summary {
                    name,
                    running,
                    tables,
                } in &all
                {
                    if args.json.json {
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
                let chosen = settings::tables(&read, rest.first().map(String::as_str))
                    .map_err(|e| e.to_string())?;
                for table in chosen {
                    if args.json.json {
                        println!("{}", settings::table_json(&read.name, table));
                    } else {
                        print_table(&read.name, table);
                    }
                }
                Ok(())
            }
            _ => Err("settings takes: [<Subsystem> [<Table>]] or --search <text>".to_string()),
        }
    })();
    match result {
        Ok(()) => OK,
        Err(why) => {
            eprintln!("{} settings: {why}", style::prefix());
            FAILED
        }
    }
}

/// `twaco trace <Entity>.<Service> [--param <name>] [--json]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Trace {
    #[arg(long = "param", value_name = "name:TYPE[:...]")]
    pub(crate) params: Vec<String>,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco trace <Entity>.<Service>`: how each caller uses a service. Read-only and offline.
pub(crate) fn trace_cmd(solution: &Solution, args: &Trace) -> u8 {
    let [service] = args.operands.as_slice() else {
        eprintln!("{} trace needs one <Entity>.<Service>", style::prefix());
        return FAILED;
    };
    if args.params.len() > 1 {
        eprintln!("{} give --param once", style::prefix());
        return FAILED;
    }
    let param = args.params.first().map(String::as_str);
    match twaco::core::trace::run(solution, service, param) {
        Ok(trace) if args.json.json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&trace).expect("a trace serialises")
            );
            OK
        }
        Ok(trace) => {
            print!("{}", twaco::core::trace::render(&trace));
            OK
        }
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            FAILED
        }
    }
}

/// `twaco impact <entity> [--member <name>] [--min-confidence <value>] [--depth <n>]
/// [--server [--profile <name>]]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Impact {
    #[arg(long = "member", value_name = "name")]
    pub(crate) member: Option<String>,
    #[arg(long = "min-confidence", value_name = "value")]
    pub(crate) min_confidence: Option<String>,
    #[arg(long = "depth", value_name = "n")]
    pub(crate) depth: Option<String>,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(long = "dot")]
    pub(crate) dot: bool,
    #[arg(long = "server")]
    pub(crate) server: bool,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// What changing an entity, or one service, property or field of it, would reach. Read-only: it
/// reads the solution into the index and asks it, and with `--server` the server as well.
pub(crate) fn impact_cmd(solution: &Solution, args: &Impact) -> u8 {
    let [entity] = args.operands.as_slice() else {
        eprintln!(
            "{} impact needs one <entity>, such as Acme.Orders.Manager",
            style::prefix()
        );
        return FAILED;
    };
    if args.json.json && args.dot {
        eprintln!(
            "{} --json and --dot are different outputs; pick one",
            style::prefix()
        );
        return FAILED;
    }
    if args.server && args.dot {
        eprintln!(
            "{} --dot draws the repository's graph only; --server adds to the text and --json reports",
            style::prefix()
        );
        return FAILED;
    }
    let min = match args.min_confidence.as_ref() {
        None => Confidence::Review,
        Some(word) => match Confidence::parse(word) {
            Some(confidence) => confidence,
            None => {
                eprintln!(
                    "{} --min-confidence is structural, resolved or review, not {word:?}",
                    style::prefix()
                );
                return FAILED;
            }
        },
    };
    let depth = match args.depth.as_ref() {
        None => None,
        Some(text) => match text.parse::<usize>() {
            Ok(depth) if depth > 0 => Some(depth),
            _ => {
                eprintln!(
                    "{} --depth is a positive whole number, not {text:?}",
                    style::prefix()
                );
                return FAILED;
            }
        },
    };
    let request = impact::Request {
        entity: entity.clone(),
        member: args.member.clone(),
        min,
        depth,
    };
    let client = if args.server {
        match profile::load(&solution.root, args.profile.name()) {
            Ok(profile) => Some(server::Client::new(profile)),
            Err(error) => {
                eprintln!("{} {error}", style::prefix());
                return FAILED;
            }
        }
    } else {
        None
    };
    let remote = client
        .as_ref()
        .map(|client| client as &dyn impact::ServerRemote);
    let report = match impact::run_with(solution, &request, remote) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    if args.json.json {
        let value = report.to_json(args.detail.detail);
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("a report serialises")
        );
    } else if args.dot {
        print!("{}", impact::render_dot(&report));
    } else {
        print!("{}", impact::render_text(&report, args.detail.detail));
    }
    OK
}

/// `twaco unused [--min-confidence <value>] [--collection <name>] [--fields]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Unused {
    #[arg(long = "min-confidence", value_name = "value")]
    pub(crate) min_confidence: Option<String>,
    #[arg(long = "collection", value_name = "name")]
    pub(crate) collection: Option<String>,
    #[arg(long = "fields")]
    pub(crate) fields: bool,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) json: Json,
}

/// Entities no entry point reaches. Offline, advisory and read-only: it deletes nothing.
pub(crate) fn unused_cmd(solution: &Solution, args: &Unused) -> u8 {
    let fields = args.fields;
    if fields && (args.min_confidence.is_some() || args.collection.is_some()) {
        eprintln!(
            "{} --fields cannot be used with --collection or --min-confidence",
            style::prefix()
        );
        return FAILED;
    }
    if fields {
        let report = unused::fields(solution);
        if args.json.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("a report serialises")
            );
        } else {
            print!("{}", unused::render_fields(&report));
        }
        return OK;
    }
    let min = match args.min_confidence.as_ref() {
        None => Confidence::Review,
        Some(word) => match Confidence::parse(word) {
            Some(confidence) => confidence,
            None => {
                eprintln!(
                    "{} --min-confidence is structural, resolved or review, not {word:?}",
                    style::prefix()
                );
                return FAILED;
            }
        },
    };
    let collection = args.collection.clone();
    if let Some(name) = &collection {
        if !unused::JUDGED.contains(&name.as_str()) {
            eprintln!(
                "{} --collection is one of {}, not {name:?}",
                style::prefix(),
                unused::JUDGED.join(", ")
            );
            return FAILED;
        }
    }
    let report = unused::run(solution, &unused::Request { min, collection });
    if args.json.json {
        let value = report.to_json(args.detail.detail);
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("a report serialises")
        );
    } else {
        print!("{}", unused::render_text(&report, args.detail.detail));
    }
    OK
}

/// `twaco docs [--detail] [--json] [--out <file> [--force|--check]]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Docs {
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(long = "out", value_name = "path", value_parser = super::flags::path())]
    pub(crate) out: Option<std::path::PathBuf>,
    #[arg(long = "force")]
    pub(crate) force: bool,
    #[arg(long = "check")]
    pub(crate) check: bool,
}

/// The solution written down. Offline; with `--out` it writes exactly one file.
pub(crate) fn docs_cmd(solution: &Solution, args: &Docs) -> u8 {
    if args.force && args.out.is_none() {
        eprintln!("{} --force only applies to --out", style::prefix());
        return FAILED;
    }
    if args.check && args.out.is_none() {
        eprintln!(
            "{} --check needs --out <file>: the document to compare",
            style::prefix()
        );
        return FAILED;
    }
    if args.check && args.force {
        eprintln!(
            "{} --check writes nothing; leave out --force",
            style::prefix()
        );
        return FAILED;
    }
    let document = docs::build(solution);
    let detail = args.detail.detail;
    let text = if args.json.json {
        let mut json =
            serde_json::to_string_pretty(&document.to_json(detail)).expect("a document serialises");
        json.push('\n');
        json
    } else {
        docs::render_markdown(&document, detail)
    };
    match &args.out {
        None => print!("{text}"),
        Some(path) if args.check => {
            let format = if args.json.json { " --json" } else { "" };
            let detail = if detail { " --detail" } else { "" };
            match docs::is_current(path, &text) {
                Ok(true) => println!("{} is up to date", path.display()),
                Ok(false) if !path.exists() => {
                    println!(
                        "{} does not exist; write it with `twaco docs{detail}{format} --out {}`",
                        path.display(),
                        path.display()
                    );
                    return DRIFT;
                }
                Ok(false) => {
                    println!(
                        "{} is out of date; regenerate it with `twaco docs{detail}{format} --out {} --force`",
                        path.display(),
                        path.display()
                    );
                    return DRIFT;
                }
                Err(error) => {
                    eprintln!("{} docs: {error}", style::prefix());
                    return FAILED;
                }
            }
        }
        Some(path) => {
            if let Err(error) = docs::write(path, &text, args.force) {
                eprintln!("{} docs: {error}", style::prefix());
                return FAILED;
            }
            println!("{} bytes to {}", text.len(), path.display());
        }
    }
    OK
}

/// `twaco catalog [<entity>] [--project P] [--search <text>] [--json]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Catalog {
    #[command(flatten)]
    pub(crate) project: Project,
    #[arg(long = "search", value_name = "text")]
    pub(crate) search: Option<String>,
    #[arg(long = "body")]
    pub(crate) body: bool,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

pub(crate) fn catalog_cmd(solution: &Solution, args: &Catalog) -> u8 {
    if args.operands.len() > 1 {
        eprintln!(
            "{} catalog takes at most one entity, Entity.Service or service name",
            style::prefix()
        );
        return FAILED;
    }
    let query = catalog::Query {
        project: args.project.project.as_deref(),
        entity: args.operands.first().map(String::as_str),
        text: args.search.as_deref(),
        body: args.body,
    };
    let result = match catalog::build(solution, query) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("{} catalog: {error}", style::prefix());
            return FAILED;
        }
    };
    for skipped in &result.skipped {
        eprintln!("{} skipped {skipped}", style::prefix());
    }
    for entity in &result.entities {
        if !args.json.json {
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
            if args.json.json {
                let mut line = serde_json::json!({
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
                });
                if args.body {
                    line["body"] = serde_json::json!(service.body);
                }
                println!("{line}");
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
                if args.body {
                    match &service.body {
                        Some(body) => {
                            for line in body.lines() {
                                println!("    {line}");
                            }
                        }
                        None => println!("    (no script: not a script service)"),
                    }
                }
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
