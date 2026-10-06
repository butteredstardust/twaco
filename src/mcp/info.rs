use super::*;

pub(crate) fn settings_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let client = client(solution, arguments)?;
    match text(arguments, "action").unwrap_or("list") {
        "list" => {
            let all = settings::summaries(&client).map_err(ToolError::coded)?;
            Ok(json!({ "ok": true, "subsystems": all.iter().map(|s| json!({
                "name": s.name,
                "running": s.running,
                "tables": s.tables,
            })).collect::<Vec<_>>() }))
        }
        "show" => {
            let names = settings::Remote::subsystems(&client).map_err(ToolError::coded)?;
            let name = settings::resolve(&names, required(arguments, "subsystem")?)
                .map_err(ToolError::coded)?;
            let read = settings::read(&client, name).map_err(ToolError::coded)?;
            Ok(
                json!({ "ok": true, "subsystem": read.name, "running": read.running, "tables": read.tables.iter().map(|t| settings::table_json(&read.name, t)).collect::<Vec<_>>() }),
            )
        }
        "search" => {
            let wanted = required(arguments, "text")?;
            let all = settings::read_all(&client).map_err(ToolError::coded)?;
            let found = settings::search(&all, wanted);
            Ok(json!({ "ok": true, "matches": found.iter().map(|f| json!({
                "subsystem": f.subsystem,
                "table": f.table,
                "setting": f.field.name,
                "type": f.field.base_type,
                "values": f.values,
                "description": f.field.description,
            })).collect::<Vec<_>>() }))
        }
        other => Err(ToolError::invalid(format!(
            "action must be list, show or search, not {other:?}"
        ))),
    }
}

/// The repository-derived service catalog, offline and read-only.
pub(crate) fn unused_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let min = match text(arguments, "min_confidence") {
        None => Confidence::Review,
        Some(word) => Confidence::parse(word).ok_or_else(|| {
            ToolError::invalid(format!(
                "`min_confidence` is structural, resolved or review, not {word:?}"
            ))
        })?,
    };
    let report = unused::run(
        solution,
        &unused::Request {
            min,
            collection: text(arguments, "collection").map(str::to_string),
        },
    );
    Ok(report.to_json(flag(arguments, "detail", false)))
}

pub(crate) fn docs_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let detail = flag(arguments, "detail", false);
    let document = docs::build(solution);
    Ok(json!({
        "document": document.to_json(detail),
        "markdown": docs::render_markdown(&document, detail),
    }))
}

pub(crate) fn impact_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let min = match text(arguments, "min_confidence") {
        None => Confidence::Review,
        Some(word) => Confidence::parse(word).ok_or_else(|| {
            ToolError::invalid(format!(
                "`min_confidence` is structural, resolved or review, not {word:?}"
            ))
        })?,
    };
    let depth = match arguments.get("depth") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .filter(|depth| *depth > 0)
                .and_then(|depth| usize::try_from(depth).ok())
                .ok_or_else(|| ToolError::invalid("`depth` must be a positive whole number"))?,
        ),
    };
    let report = impact::run(
        solution,
        &impact::Request {
            entity: required(arguments, "entity")?.to_string(),
            member: text(arguments, "member").map(str::to_string),
            min,
            depth,
        },
    )
    .map_err(ToolError::coded)?;
    if text(arguments, "format") == Some("dot") {
        return Ok(json!({
            "entity": report.entity,
            "dot": impact::render_dot(&report),
            "complete": report.complete,
            "unreadable": report.unreadable,
            "unparsed_scripts": report.unparsed_scripts,
            "limits": report.limits,
        }));
    }
    Ok(report.to_json(flag(arguments, "detail", false)))
}

pub(crate) fn catalog_tool(solution: &Solution, arguments: &Value) -> Result<Value, ToolError> {
    let catalog = catalog::build(
        solution,
        catalog::Query {
            entity: text(arguments, "entity"),
            project: text(arguments, "project"),
            text: text(arguments, "text"),
        },
    )
    .map_err(ToolError::coded)?;
    let service_count = catalog.service_count();
    let entity_count = catalog.entities.len();
    let limit = if flag(arguments, "detail", false) {
        usize::MAX
    } else {
        50
    };
    let mut services = Vec::new();
    for entity in &catalog.entities {
        for service in &entity.services {
            if services.len() == limit {
                break;
            }
            let mut value = serde_json::to_value(service).expect("catalog services serialise");
            let object = value
                .as_object_mut()
                .expect("a catalog service is an object");
            object.insert("collection".to_string(), json!(entity.collection));
            object.insert("entity".to_string(), json!(entity.name));
            object.insert("project".to_string(), json!(entity.project));
            if !entity.inherits.is_empty() {
                object.insert("inherits".to_string(), json!(entity.inherits));
            }
            if !entity.implemented_by.is_empty() {
                object.insert("implemented_by".to_string(), json!(entity.implemented_by));
            }
            services.push(value);
        }
        if services.len() == limit {
            break;
        }
    }
    let mut result = json!({
        "ok": true,
        "service_count": service_count,
        "entity_count": entity_count,
        "services": services,
    });
    if service_count > limit {
        result["note"] = json!(format!(
            "showing 50 of {service_count} services; detail: true lists every one"
        ));
    }
    if !catalog.skipped.is_empty() {
        result["skipped"] = json!(catalog.skipped);
    }
    Ok(result)
}

/// The server's extension packages, read-only.
/// The help version for a call: asked, named in a page address, configured, or the server's.
fn help_version(
    root: &Path,
    arguments: &Value,
    named: Option<String>,
) -> Result<(String, Vec<String>), ToolError> {
    // No twaco.toml means no solution; a broken one is an error.
    let solution = match Solution::discover(root) {
        Ok(solution) => Some(solution),
        Err(crate::core::config::ConfigError::NotFound { .. }) => None,
        Err(error) => return Err(ToolError::coded(error)),
    };
    let mut notes = Vec::new();
    let version = help::choose_version(
        text(arguments, "version"),
        named,
        solution.as_ref(),
        text(arguments, "profile").unwrap_or("default"),
        &mut notes,
    )
    // The only failures are a version that does not parse: the caller's own text when one was
    // given, otherwise the solution's `[help] version`.
    .map_err(|why| {
        let code = if text(arguments, "version").is_some() {
            ErrorCode::InvalidArguments
        } else {
            ErrorCode::InvalidData
        };
        ToolError::with(code, why)
    })?;
    Ok((version, notes))
}

/// Search the help center. Needs no solution; downloads go to the user's cache.
/// The knowledge topics, built in and the solution's own; needs no solution.
pub(crate) fn guide_tool(root: &Path, arguments: &Value) -> Result<Value, ToolError> {
    let solution = match Solution::discover(root) {
        Ok(solution) => Some(solution),
        Err(crate::core::config::ConfigError::NotFound { .. }) => None,
        Err(error) => return Err(ToolError::coded(error)),
    };
    let (topics, problems) = guide::topics(solution.as_ref());
    let mut result = match text(arguments, "action").unwrap_or("search") {
        "list" => json!({ "ok": true, "topics": topics.iter().map(|t| json!({
            "topic": t.id,
            "title": t.title,
            "origin": if t.file.is_some() { "solution" } else { "built in" },
            "sections": guide::sections(&t.text).len(),
        })).collect::<Vec<_>>() }),
        "search" => {
            let query = required(arguments, "text")?;
            let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(8) as usize;
            let hits = guide::search(&topics, query, limit);
            json!({
                "ok": true,
                "results": hits.iter().map(|h| json!({ "topic": h.topic, "section": h.heading, "matched": h.matched, "of": h.of, "line": h.line })).collect::<Vec<_>>(),
                "next": "read a section with action read, topic and section",
            })
        }
        "read" => {
            let topic =
                guide::find(&topics, required(arguments, "topic")?).map_err(ToolError::coded)?;
            match guide::read(topic, text(arguments, "section")).map_err(ToolError::coded)? {
                guide::Reading::Text(markdown) => {
                    json!({ "ok": true, "topic": topic.id, "markdown": markdown })
                }
                guide::Reading::Outline { title, headings } => json!({
                    "ok": true,
                    "topic": topic.id,
                    "title": title,
                    "sections": headings,
                    "note": "too long to read whole; read one section by its heading",
                }),
            }
        }
        other => {
            return Err(ToolError::invalid(format!(
                "action must be list, search or read, not {other:?}"
            )))
        }
    };
    if !problems.is_empty() {
        result["problems"] = json!(problems);
    }
    Ok(result)
}

pub(crate) fn help_search_tool(root: &Path, arguments: &Value) -> Result<Value, ToolError> {
    let query = required(arguments, "query")?;
    let (version, notes) = help_version(root, arguments, None)?;
    let cache = help::cache_root().map_err(ToolError::coded)?;
    let bytes = help::cached(
        &help::Web::default(),
        &cache,
        &version,
        help::INDEX_FILE,
        flag(arguments, "refresh", false),
    )
    .map_err(ToolError::coded)?;
    let index = help::Index::parse(&String::from_utf8_lossy(&bytes)).map_err(ToolError::coded)?;
    let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
    let found = help::search(&index, &version, query, limit);
    let mut result = json!({
        "ok": true,
        "version": version,
        "matched": found.matched,
        "results": found.hits.iter().map(|hit| json!({
            "title": hit.page.title,
            "path": hit.page.path,
            "url": hit.url,
            "summary": hit.page.summary,
        })).collect::<Vec<_>>(),
    });
    if !found.unknown.is_empty() {
        result["unknown_words"] = json!(found.unknown);
        result["note"] = json!("the help never uses these words, so no page holds every word; try fewer or other words");
    }
    if !notes.is_empty() {
        result["notes"] = json!(notes);
    }
    Ok(result)
}

/// Read one help page as Markdown, bounded by max_chars.
pub(crate) fn help_page_tool(root: &Path, arguments: &Value) -> Result<Value, ToolError> {
    let page = required(arguments, "page")?;
    let (named, path) = help::page_path(page).map_err(ToolError::coded)?;
    let (version, notes) = help_version(root, arguments, named)?;
    let cache = help::cache_root().map_err(ToolError::coded)?;
    let html = help::cached(
        &help::Web::default(),
        &cache,
        &version,
        &path,
        flag(arguments, "refresh", false),
    )
    .map_err(ToolError::coded)?;
    let read =
        help::read(&html, &version, &path, text(arguments, "section")).map_err(ToolError::coded)?;
    let max = arguments
        .get("max_chars")
        .and_then(Value::as_u64)
        .unwrap_or(20_000) as usize;
    let total = read.markdown.chars().count();
    let mut result = json!({
        "ok": true,
        "version": version,
        "title": read.title,
        "url": read.url,
        "headings": read.headings,
        "markdown": read.markdown.chars().take(max).collect::<String>(),
    });
    if total > max {
        result["truncated"] = json!(true);
        result["note"] = json!(format!(
            "{total} characters in all; ask for one section by heading, or raise max_chars"
        ));
    }
    if !notes.is_empty() {
        result["notes"] = json!(notes);
    }
    Ok(result)
}

/// Search or read the fixed-version Java API docs. It needs no solution and contacts only the
/// Javadoc site, never a ThingWorx server.
pub(crate) fn javadoc_tool(arguments: &Value) -> Result<Value, ToolError> {
    let action = required(arguments, "action")?;
    let name = required(arguments, "name")?;
    let refresh = flag(arguments, "refresh", false);
    let cache = javadoc::cache_root().map_err(|why| ToolError::with(ErrorCode::IoError, why))?;
    let web = help::Web::new(javadoc::BASE);
    let fetch = |path: &str| javadoc::cached(&web, &cache, path, refresh);
    let types = fetch(javadoc::TYPE_INDEX)?;
    let result = match action {
        "search" => {
            if arguments.get("member").is_some() {
                return Err(ToolError::invalid("`member` is only for action class"));
            }
            let members = fetch(javadoc::MEMBER_INDEX)?;
            let index = javadoc::Index::parse(
                &String::from_utf8_lossy(&types),
                &String::from_utf8_lossy(&members),
            )
            .map_err(|why| ToolError::with(ErrorCode::InvalidData, why))?;
            let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
            let hits = javadoc::search(&index, name, limit);
            json!({
                "ok": true,
                "summary": format!("{} matching Java API class(es) and member(s)", hits.len()),
                "version": javadoc::VERSION,
                "results": hits.iter().map(|hit| json!({
                    "kind": if hit.kind == javadoc::Kind::Class { "class" } else { "member" },
                    "display": hit.display(), "package": hit.package, "class": hit.class,
                    "label": hit.label, "path": hit.path, "url": hit.url,
                })).collect::<Vec<_>>(),
            })
        }
        "class" => {
            let index =
                javadoc::Index::parse(&String::from_utf8_lossy(&types), "memberSearchIndex = []")
                    .map_err(|why| ToolError::with(ErrorCode::InvalidData, why))?;
            // Unknown and ambiguous alike: the name the caller gave does not pick one class.
            let class = javadoc::find_class(&index, name)
                .map_err(|why| ToolError::with(ErrorCode::InvalidArguments, why))?;
            let path = javadoc::class_path(&class)
                .map_err(|why| ToolError::with(ErrorCode::InvalidData, why))?;
            let html = fetch(&path)?;
            let read = javadoc::read(&html, &class, text(arguments, "member"))?;
            json!({
                "ok": true,
                "summary": format!("{} method overload(s) documented for {}", read.methods, read.title),
                "version": javadoc::VERSION,
                "title": read.title,
                "path": read.path,
                "url": read.url,
                "markdown": read.markdown,
            })
        }
        other => {
            return Err(ToolError::invalid(format!(
                "action must be search or class, not {other:?}"
            )))
        }
    };
    Ok(result)
}
