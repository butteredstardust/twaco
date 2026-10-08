use super::requests::info as tool;
use super::*;

/// What resolved, what is reachable and what is missing: `twaco doctor`. Read-only.
pub(crate) fn doctor_tool(root: &Path, arguments: tool::DoctorRequest) -> Result<Value, ToolError> {
    let items = crate::core::doctor::diagnose(root, &arguments.profile);
    let word = |health: crate::core::doctor::Health| match health {
        crate::core::doctor::Health::Ok => "ok",
        crate::core::doctor::Health::Warn => "warn",
        crate::core::doctor::Health::Fail => "fail",
    };
    let failed = items
        .iter()
        .filter(|item| item.health == crate::core::doctor::Health::Fail)
        .count();
    Ok(json!({
        "ok": failed == 0,
        "failed": failed,
        "items": items.iter().map(|item| json!({
            "health": word(item.health),
            "subject": item.subject,
            "detail": item.detail,
        })).collect::<Vec<_>>(),
    }))
}

pub(crate) fn settings_tool(
    solution: &Solution,
    arguments: tool::SettingsRequest,
) -> Result<Value, ToolError> {
    let client = client(solution, &arguments.profile)?;
    match arguments.action {
        tool::SettingsAction::List => {
            let all = settings::summaries(&client).map_err(ToolError::coded)?;
            Ok(json!({ "ok": true, "subsystems": all.iter().map(|s| json!({
                "name": s.name,
                "running": s.running,
                "tables": s.tables,
            })).collect::<Vec<_>>() }))
        }
        tool::SettingsAction::Show => {
            let names = settings::Remote::subsystems(&client).map_err(ToolError::coded)?;
            let name = settings::resolve(&names, required_text(&arguments.subsystem, "subsystem")?)
                .map_err(ToolError::coded)?;
            let read = settings::read(&client, name).map_err(ToolError::coded)?;
            let chosen =
                settings::tables(&read, arguments.table.as_deref()).map_err(ToolError::coded)?;
            Ok(
                json!({ "ok": true, "subsystem": read.name, "running": read.running, "tables": chosen.iter().map(|t| settings::table_json(&read.name, t)).collect::<Vec<_>>() }),
            )
        }
        tool::SettingsAction::Search => {
            let wanted = required_text(&arguments.text, "text")?;
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
    }
}

/// The repository-derived service catalog, offline and read-only.
pub(crate) fn unused_tool(
    solution: &Solution,
    arguments: tool::UnusedRequest,
) -> Result<Value, ToolError> {
    let min = match arguments.min_confidence {
        tool::UnusedMinConfidence::Structural => Confidence::Structural,
        tool::UnusedMinConfidence::Resolved => Confidence::Resolved,
        tool::UnusedMinConfidence::Review => Confidence::Review,
    };
    let report = unused::run(
        solution,
        &unused::Request {
            min,
            collection: arguments
                .collection
                .map(|collection| collection.as_str().to_string()),
        },
    );
    Ok(report.to_json(arguments.detail))
}

pub(crate) fn docs_tool(
    solution: &Solution,
    arguments: tool::DocsRequest,
) -> Result<Value, ToolError> {
    let detail = arguments.detail;
    let document = docs::build(solution);
    Ok(json!({
        "document": document.to_json(detail),
        "markdown": docs::render_markdown(&document, detail),
    }))
}

pub(crate) fn impact_tool(
    solution: &Solution,
    arguments: tool::ImpactRequest,
) -> Result<Value, ToolError> {
    let min = match arguments.min_confidence {
        tool::ImpactMinConfidence::Structural => Confidence::Structural,
        tool::ImpactMinConfidence::Resolved => Confidence::Resolved,
        tool::ImpactMinConfidence::Review => Confidence::Review,
    };
    let depth = arguments
        .depth
        .map(usize::try_from)
        .transpose()
        .map_err(|_| ToolError::invalid("`depth` must be a positive whole number"))?;
    let report = impact::run(
        solution,
        &impact::Request {
            entity: nonempty(&arguments.entity, "entity")?.to_string(),
            member: arguments.member.as_ref().cloned(),
            min,
            depth,
        },
    )
    .map_err(ToolError::coded)?;
    if arguments.format == tool::ImpactFormat::Dot {
        return Ok(json!({
            "entity": report.entity,
            "dot": impact::render_dot(&report),
            "complete": report.complete,
            "unreadable": report.unreadable,
            "unparsed_scripts": report.unparsed_scripts,
            "limits": report.limits,
        }));
    }
    Ok(report.to_json(arguments.detail))
}

pub(crate) fn catalog_tool(
    solution: &Solution,
    arguments: tool::CatalogRequest,
) -> Result<Value, ToolError> {
    let catalog = catalog::build(
        solution,
        catalog::Query {
            entity: arguments.entity.as_deref(),
            project: arguments.project.as_deref(),
            text: arguments.text.as_deref(),
        },
    )
    .map_err(ToolError::coded)?;
    let service_count = catalog.service_count();
    let entity_count = catalog.entities.len();
    let limit = if arguments.detail { usize::MAX } else { 50 };
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
    version: Option<&str>,
    profile: &str,
    named: Option<String>,
) -> Result<(String, Vec<String>), ToolError> {
    // No twaco.toml means no solution; a broken one is an error.
    let solution = match Solution::discover(root) {
        Ok(solution) => Some(solution),
        Err(crate::core::config::ConfigError::NotFound { .. }) => None,
        Err(error) => return Err(ToolError::coded(error)),
    };
    let mut notes = Vec::new();
    let chosen = help::choose_version(version, named, solution.as_ref(), profile, &mut notes)
        // The only failures are a version that does not parse: the caller's own text when one was
        // given, otherwise the solution's `[help] version`.
        .map_err(|why| {
            let code = if version.is_some() {
                ErrorCode::InvalidArguments
            } else {
                ErrorCode::InvalidData
            };
            ToolError::with(code, why)
        })?;
    Ok((chosen, notes))
}

/// Search the help center. Needs no solution; downloads go to the user's cache.
/// The knowledge topics, built in and the solution's own; needs no solution.
pub(crate) fn guide_tool(root: &Path, arguments: tool::GuideRequest) -> Result<Value, ToolError> {
    let solution = match Solution::discover(root) {
        Ok(solution) => Some(solution),
        Err(crate::core::config::ConfigError::NotFound { .. }) => None,
        Err(error) => return Err(ToolError::coded(error)),
    };
    let (topics, problems) = guide::topics(solution.as_ref());
    let action = match arguments.action.as_ref() {
        Some(action) => *action,
        None if arguments.text.is_some() => tool::GuideAction::Search,
        None if arguments.topic.is_some() => tool::GuideAction::Read,
        None => tool::GuideAction::List,
    };
    let mut result = match action {
        tool::GuideAction::List => json!({ "ok": true, "topics": topics.iter().map(|t| json!({
            "topic": t.id,
            "title": t.title,
            "origin": if t.file.is_some() { "solution" } else { "built in" },
            "sections": guide::sections(&t.text).len(),
        })).collect::<Vec<_>>() }),
        tool::GuideAction::Search => {
            let query = required_text(&arguments.text, "text")?;
            let limit = arguments.limit as usize;
            let hits = guide::search(&topics, query, limit);
            json!({
                "ok": true,
                "results": hits.iter().map(|h| json!({ "topic": h.topic, "section": h.heading, "matched": h.matched, "of": h.of, "line": h.line })).collect::<Vec<_>>(),
                "next": "read a section with action read, topic and section",
            })
        }
        tool::GuideAction::Read => {
            let topic = guide::find(&topics, required_text(&arguments.topic, "topic")?)
                .map_err(ToolError::coded)?;
            match guide::read(topic, arguments.section.as_deref()).map_err(ToolError::coded)? {
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
    };
    if !problems.is_empty() {
        result["problems"] = json!(problems);
    }
    Ok(result)
}

pub(crate) fn help_search_tool(
    root: &Path,
    arguments: tool::HelpSearchRequest,
) -> Result<Value, ToolError> {
    let query = nonempty(&arguments.query, "query")?;
    let (version, notes) =
        help_version(root, arguments.version.as_deref(), &arguments.profile, None)?;
    let cache = help::cache_root().map_err(ToolError::coded)?;
    let bytes = help::cached(
        &help::Web::default(),
        &cache,
        &version,
        help::INDEX_FILE,
        arguments.refresh,
    )
    .map_err(ToolError::coded)?;
    let index = help::Index::parse(&String::from_utf8_lossy(&bytes)).map_err(ToolError::coded)?;
    let limit = arguments.limit as usize;
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
pub(crate) fn help_page_tool(
    root: &Path,
    arguments: tool::HelpPageRequest,
) -> Result<Value, ToolError> {
    let page = nonempty(&arguments.page, "page")?;
    let (named, path) = help::page_path(page).map_err(ToolError::coded)?;
    let (version, notes) = help_version(
        root,
        arguments.version.as_deref(),
        &arguments.profile,
        named,
    )?;
    let cache = help::cache_root().map_err(ToolError::coded)?;
    let html = help::cached(
        &help::Web::default(),
        &cache,
        &version,
        &path,
        arguments.refresh,
    )
    .map_err(ToolError::coded)?;
    let read = help::read(&html, &version, &path, arguments.section.as_deref())
        .map_err(ToolError::coded)?;
    let max = arguments.max_chars as usize;
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
pub(crate) fn javadoc_tool(arguments: tool::JavadocRequest) -> Result<Value, ToolError> {
    let action = arguments.action;
    let name = nonempty(&arguments.name, "name")?;
    let refresh = arguments.refresh;
    let cache = javadoc::cache_root().map_err(|why| ToolError::with(ErrorCode::IoError, why))?;
    let web = help::Web::new(javadoc::BASE);
    let fetch = |path: &str| javadoc::cached(&web, &cache, path, refresh);
    let types = fetch(javadoc::TYPE_INDEX)?;
    let result = match action {
        tool::JavadocAction::Search => {
            if arguments.member.is_some() {
                return Err(ToolError::invalid("`member` is only for action class"));
            }
            let members = fetch(javadoc::MEMBER_INDEX)?;
            let index = javadoc::Index::parse(
                &String::from_utf8_lossy(&types),
                &String::from_utf8_lossy(&members),
            )
            .map_err(|why| ToolError::with(ErrorCode::InvalidData, why))?;
            let limit = arguments.limit as usize;
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
        tool::JavadocAction::Class => {
            let index =
                javadoc::Index::parse(&String::from_utf8_lossy(&types), "memberSearchIndex = []")
                    .map_err(|why| ToolError::with(ErrorCode::InvalidData, why))?;
            // Unknown and ambiguous alike: the name the caller gave does not pick one class.
            let class = javadoc::find_class(&index, name)
                .map_err(|why| ToolError::with(ErrorCode::InvalidArguments, why))?;
            let path = javadoc::class_path(&class)
                .map_err(|why| ToolError::with(ErrorCode::InvalidData, why))?;
            let html = fetch(&path)?;
            let read = javadoc::read(&html, &class, arguments.member.as_deref())?;
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
    };
    Ok(result)
}
