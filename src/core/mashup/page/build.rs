use super::{baseline::Baseline, compile, merge, schema};
use crate::core::{
    config::Solution,
    entity,
    lock::WorkspaceLock,
    mashup,
    mashup::authoring::AuthoringError,
    transaction::{Transaction, TransactionError},
    workspace,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

type SpecLines = BTreeMap<String, (u64, BTreeMap<String, (u64, String)>)>;

#[derive(Debug)]
struct File {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct Plan {
    pub report: Value,
    files: Vec<File>,
    inputs: Vec<(PathBuf, Option<Vec<u8>>)>,
    components: Vec<(PathBuf, Vec<u8>)>,
}

fn invalid(message: impl ToString) -> AuthoringError {
    AuthoringError::Invalid(message.to_string())
}
fn read(path: &Path) -> Result<Option<Vec<u8>>, AuthoringError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AuthoringError::Unreadable(format!(
            "{}: {e}",
            path.display()
        ))),
    }
}
fn text(bytes: &[u8]) -> Result<&str, AuthoringError> {
    std::str::from_utf8(bytes)
        .map(|s| s.trim_start_matches('\u{feff}'))
        .map_err(invalid)
}

/// A spec handed to a build: a file the user named, or a page the compiler generated for another.
struct SpecInput {
    path: PathBuf,
    bytes: Vec<u8>,
    generated: bool,
}

fn legacy_cell_matches(
    solution: &Solution,
    cell: &compile::GeneratedCell,
    inputs: &mut Vec<(PathBuf, Option<Vec<u8>>)>,
) -> Result<bool, AuthoringError> {
    let dir = solution.src_root().join(&cell.name);
    let paths = [
        dir.join(".page-baseline.json"),
        dir.join("page.yaml"),
        dir.join("mashup/content.json"),
        dir.join("mashup/custom.css"),
    ];
    let mut bytes = Vec::new();
    for path in paths {
        let before = read(&path)?;
        inputs.push((path, before.clone()));
        bytes.push(before);
    }
    let Some(baseline) = bytes[0].as_deref().and_then(|b| Baseline::read(b).ok()) else {
        return Ok(false);
    };
    let assets = mashup::Assets {
        content: mashup::to_sidecar(&baseline.content),
        css: baseline.css,
    };
    if bytes[1].as_deref() != Some(cell.spec.as_bytes())
        || bytes[2].as_deref() != Some(assets.content.as_bytes())
        || bytes[3].as_deref() != Some(assets.css.as_bytes())
    {
        return Ok(false);
    }
    // Recover the old generated XML through payload splicing, preserving the entity envelope.
    let expected = mashup::sync(&cell.artifacts.entity, &assets)
        .map_err(AuthoringError::Mashup)?
        .0;
    let found = workspace::entities(solution);
    let Some(entity) = found
        .iter()
        .find(|e| e.info.collection == "Mashups" && e.info.name == cell.name)
    else {
        return Ok(false);
    };
    let before = read(&entity.path)?;
    let matches = before.as_deref() == Some(expected.as_slice());
    inputs.push((entity.path.clone(), before));
    Ok(matches)
}

fn shown(value: &Option<Value>) -> String {
    let text = value
        .as_ref()
        .map_or("(none)".to_string(), Value::to_string);
    if text.chars().count() > 80 {
        format!("{}...", text.chars().take(77).collect::<String>())
    } else {
        text
    }
}

/// A conflict as JSON: the path, both values where known, and the spec line responsible: the
/// property's own line when the node states it, else the node's `id:` line.
fn conflict(
    name: &str,
    spec_name: &str,
    path: &str,
    report: &merge::Report,
    lines: &SpecLines,
    spec: &schema::Spec,
) -> Value {
    let segments: Vec<&str> = path.split('/').collect();
    let found = segments
        .iter()
        .rposition(|segment| lines.contains_key(*segment));
    let (line, key) = match found {
        Some(at) => {
            let (id_line, props) = &lines[segments[at]];
            let key = segments
                .iter()
                .skip(at + 1)
                .skip_while(|segment| **segment != "Properties")
                .nth(1)
                .copied();
            let property = key.and_then(|key| props.get(key));
            (
                property.map_or(*id_line, |p| p.0),
                property.map(|p| p.1.as_str()),
            )
        }
        None => (spec.page.referenced.line().max(1), None),
    };
    let sides = report.sides.get(path);
    let mut out = json!({"path": path, "file": spec_name, "line": line});
    let mut message = format!("{name}: {path}");
    if let Some((current, generated)) = sides {
        out["current"] = current.clone().unwrap_or(Value::Null);
        out["generated"] = generated.clone().unwrap_or(Value::Null);
        message.push_str(&format!(
            ": the mashup has {}, the spec generates {}",
            shown(current),
            shown(generated)
        ));
    }
    message.push_str(&format!(
        "; change {spec_name}:{line}{} or resolve the hand edit",
        key.map_or(String::new(), |key| format!(" ({key})"))
    ));
    out["message"] = json!(message);
    out
}

pub(crate) fn plan(
    solution: &Solution,
    name: &str,
    project: Option<&str>,
    spec_file: Option<&Path>,
) -> Result<Plan, AuthoringError> {
    let spec = spec_file
        .map(|path| {
            read(path)?
                .map(|bytes| SpecInput {
                    path: path.to_path_buf(),
                    bytes,
                    generated: false,
                })
                .ok_or_else(|| invalid(format!("{}: missing page spec", path.display())))
        })
        .transpose()?;
    plan_from(solution, name, project, spec)
}

fn plan_from(
    solution: &Solution,
    name: &str,
    project: Option<&str>,
    spec_input: Option<SpecInput>,
) -> Result<Plan, AuthoringError> {
    let components = super::components::inputs(solution)?;
    let spec_file = spec_input.as_ref().map(|s| s.path.as_path());
    entity::key::EntityKey::new("Mashups", name).map_err(invalid)?;
    let found = workspace::entities(solution);
    let existing = found
        .iter()
        .find(|e| e.info.collection == "Mashups" && e.info.name == name);
    if spec_input.as_ref().is_some_and(|s| !s.generated)
        && found
            .iter()
            .any(|e| e.info.collection == "Mashups" && e.info.name.eq_ignore_ascii_case(name))
    {
        return Err(AuthoringError::Exists(format!(
            "mashup {name} already exists; use mashup build"
        )));
    }
    if existing.is_none() && spec_file.is_none() {
        return Err(AuthoringError::Unknown(format!(
            "no mashup {name}; use mashup new {name} --spec <file>"
        )));
    }
    let project = existing
        .map(|e| e.info.project.as_str())
        .or(project)
        .or_else(|| (solution.projects.len() == 1).then(|| solution.projects[0].name.as_str()))
        .or_else(|| {
            // A name that starts with exactly one project's name belongs to that project.
            let mut named = solution.projects.iter().filter(|p| {
                name.strip_prefix(p.name.as_str())
                    .is_some_and(|rest| rest.starts_with('.'))
            });
            let first = named.next()?;
            named.next().is_none().then_some(first.name.as_str())
        })
        .ok_or_else(|| invalid("mashup new --spec needs --project in a multi-project solution"))?;
    let configured = solution
        .project(project)
        .ok_or_else(|| invalid(format!("unknown project {project}")))?;
    let dir = solution.src_root().join(name);
    let page_path = dir.join("page.yaml");
    let generated_spec = spec_input.as_ref().is_some_and(|s| s.generated);
    let spec_path = spec_file.unwrap_or(&page_path).to_path_buf();
    let spec_path = spec_path.as_path();
    let spec_bytes = match spec_input.as_ref() {
        Some(input) => input.bytes.clone(),
        None => read(spec_path)?
            .ok_or_else(|| invalid(format!("{}: missing page spec", spec_path.display())))?,
    };
    let yaml = text(&spec_bytes)?;
    let spec_name = solution.relative(spec_path);
    let mut spec = schema::parse_at(yaml, solution, spec_path.parent().unwrap_or(&solution.root))
        .map_err(|e| invalid(e.in_file(&spec_name)))?;
    let overridden =
        spec.page.value.name.value != name && existing.is_none() && spec_file.is_some();
    let renamed = if overridden {
        let span = spec.page.value.name.referenced.span();
        let offset = usize::try_from(
            span.byte_offset()
                .ok_or_else(|| invalid("page.name: cannot locate scalar"))?,
        )
        .map_err(invalid)?;
        let len = usize::try_from(
            span.byte_len()
                .ok_or_else(|| invalid("page.name: cannot locate scalar"))?,
        )
        .map_err(invalid)?;
        let mut renamed = yaml.to_string();
        renamed.replace_range(
            offset..offset + len,
            &serde_json::to_string(name).map_err(invalid)?,
        );
        Some(renamed)
    } else {
        None
    };
    let yaml = renamed.as_deref().unwrap_or(yaml);
    if overridden {
        spec = schema::parse_at(yaml, solution, spec_path.parent().unwrap_or(&solution.root))
            .map_err(|e| invalid(e.in_file(&spec_name)))?;
    } else if spec.page.value.name.value != name {
        return Err(invalid(format!(
            "{}:{}: page.name must equal {name}",
            spec_path.display(),
            spec.page.value.name.referenced.line()
        )));
    }
    let baseline_path = dir.join(".page-baseline.json");
    let before_baseline = read(&baseline_path)?;
    let old = before_baseline
        .as_deref()
        .map(Baseline::read)
        .transpose()
        .map_err(invalid)?;
    let spec_base = spec_path.parent().unwrap_or(&solution.root);
    let mut generated = compile::compile_pinned(
        solution,
        project,
        yaml,
        spec_base,
        &old.as_ref().map(Baseline::pins).unwrap_or_default(),
    )
    .map_err(|e| invalid(e.in_file(&spec_name)))?;
    let mut baseline = Baseline::generated(&generated, name, project);
    let entity_path = existing.map_or_else(
        || {
            solution
                .project_root(configured)
                .join("Mashups")
                .join(format!("{name}.xml"))
        },
        |e| e.path.clone(),
    );
    let entity_before = read(&entity_path)?;
    let content_path = dir.join("mashup/content.json");
    let css_path = dir.join("mashup/custom.css");
    let content_before = read(&content_path)?;
    let css_before = read(&css_path)?;
    if existing.is_none()
        && [
            before_baseline.as_ref(),
            content_before.as_ref(),
            css_before.as_ref(),
        ]
        .iter()
        .any(|b| b.is_some())
    {
        return Err(invalid(format!(
            "{name}: destination sidecars already exist; choose a new name"
        )));
    }
    let page_before = read(&page_path)?;
    if spec_file.is_some()
        && !generated_spec
        && page_before.as_ref().is_some_and(|b| b != &spec_bytes)
    {
        return Err(invalid(format!(
            "{} already exists with a different spec; reconcile it first",
            page_path.display()
        )));
    }
    let mut report = merge::Report::default();
    let (content, css) = if let Some(bytes) = &entity_before {
        let extracted = mashup::extract(bytes).map_err(AuthoringError::Mashup)?;
        let current_assets = match (&content_before, &css_before) {
            (Some(c), Some(s)) => mashup::Assets {
                content: text(c)?.into(),
                css: text(s)?.into(),
            },
            (None, None) => extracted,
            _ => {
                return Err(invalid(format!(
                    "{name}: incomplete mashup sidecars; run twaco extract first"
                )))
            }
        };
        if !mashup::sync(bytes, &current_assets)
            .map_err(AuthoringError::Mashup)?
            .1
            .is_empty()
        {
            return Err(AuthoringError::Refused(format!("{name}: content.json/custom.css and entity disagree; run twaco sync or twaco extract first")));
        }
        mashup::edit::parse(&current_assets.content).map_err(AuthoringError::Edit)?;
        let current: Value = serde_json::from_str(&current_assets.content).map_err(invalid)?;
        if let Some(old) = &old {
            let info = entity::parse(bytes).map_err(|e| invalid(format!("{name}: {e:?}")))?;
            let current_xml = json!({"name": info.name, "projectName": info.project});
            merge::merge(
                Some(&json!(old.managed_xml)),
                Some(&current_xml),
                Some(&json!(baseline.managed_xml)),
                "xml",
                &mut report,
            );
            let merged = merge::content(&old.content, &current, &baseline.content, &mut report);
            let edits = old.content_edits(&current);
            report.kept.retain(|path| {
                !path.starts_with("content")
                    || edits.iter().any(|edit| {
                        edit == path
                            || edit.starts_with(&format!("{path}/"))
                            || path.starts_with(&format!("{edit}/"))
                    })
            });
            if let Some(expected) = &old.comparison {
                baseline.comparison = Some(merge::content(
                    &old.content,
                    expected,
                    &baseline.content,
                    &mut merge::Report::default(),
                ));
            }
            // Page specs intentionally do not own custom.css. Keep it byte-for-byte (aside
            // from its existing terminator convention) instead of treating it as generated
            // content or a hand edit on every subsequent build.
            // A spec that names `page.css` owns the sidecar until someone edits the sidecar.
            let css = if spec.page.value.css.is_some()
                && mashup::same_css(&current_assets.css, &old.css)
            {
                baseline.css.clone()
            } else {
                current_assets.css
            };
            (merged, css)
        } else {
            // The spec was derived from this mashup: claim its widgets and wiring by id.
            if let Some(aligned) =
                super::align::adopt(solution, project, yaml, spec_base, &current, &generated)
            {
                generated = aligned;
                baseline = Baseline::generated(&generated, name, project);
            }
            for difference in super::align::remaining(&current, &baseline.content) {
                report.sides.insert(
                    difference.path.clone(),
                    (difference.current, Some(difference.generated)),
                );
                report.conflicts.push(difference.path);
            }
            let wanted_css = mashup::without_terminator(&baseline.css);
            if !wanted_css.is_empty()
                && mashup::without_terminator(&current_assets.css) != wanted_css
            {
                report.sides.insert(
                    "custom.css".into(),
                    (
                        Some(json!(mashup::without_terminator(&current_assets.css))),
                        Some(json!(wanted_css)),
                    ),
                );
                report.conflicts.push("custom.css".into());
            }
            baseline.comparison = Some(current.clone());
            (current, current_assets.css)
        }
    } else {
        (baseline.content.clone(), baseline.css.clone())
    };
    // `custom.css` is the source of truth for CustomMashupCss. In particular, adoption must
    // not turn a stylesheet sidecar into a content.json edit: that makes the following build
    // look like both a content and stylesheet hand edit.
    let mut lines = SpecLines::new();
    fn node_lines(nodes: &[schema::Node], ids: &BTreeMap<String, String>, lines: &mut SpecLines) {
        for n in nodes {
            let b = n.body();
            if let Some(id) = ids.get(&b.id.value) {
                let mut props: BTreeMap<_, _> = b
                    .props
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            (v.referenced.line().max(1), format!("props.{k}")),
                        )
                    })
                    .collect();
                if let Some(text) = &b.text {
                    let property = if matches!(n, schema::Node::Button(_)) {
                        "Label"
                    } else {
                        "LabelText"
                    };
                    props
                        .entry(property.into())
                        .or_insert((text.referenced.line().max(1), "text".into()));
                }
                lines.insert(id.clone(), (b.id.referenced.line().max(1), props));
            }
            for child in super::compile::descendants(n) {
                node_lines(std::slice::from_ref(child), ids, lines);
            }
        }
    }
    for slot in spec.page.value.slots.values() {
        node_lines(&slot.value, &baseline.ids, &mut lines);
    }
    let conflicts: Vec<_> = report
        .conflicts
        .iter()
        .map(|path| conflict(name, &spec_name, path, &report, &lines, &spec))
        .collect();
    let mut problems = Vec::new();
    mashup::validate::check_content(&mashup::to_sidecar(&content), &mut problems);
    if conflicts.is_empty()
        && problems
            .iter()
            .any(|p| p.severity == mashup::validate::Severity::Error)
    {
        return Err(invalid(format!(
            "{name}: merged content is invalid: {}",
            problems
                .iter()
                .filter(|p| p.severity == mashup::validate::Severity::Error)
                .map(|p| p.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    let assets = mashup::Assets {
        content: mashup::to_sidecar(&content),
        css,
    };
    // The entity's mashup type is not rewritten by a build, so page <-> master is refused.
    if let Some(before) = &entity_before {
        let kind = |entity: &[u8]| {
            let text = String::from_utf8_lossy(entity);
            text.split_once("aspect.mashupType=\"")
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(kind, _)| kind.to_string())
        };
        let (was, now) = (kind(before), kind(&generated.entity));
        if was != now {
            return Err(AuthoringError::Invalid(format!(
                "{name} is a {} mashup and page.kind asks for {}; changing the kind of an existing \
                 mashup is not supported: create a new mashup with this spec instead",
                was.as_deref().unwrap_or("plain"),
                now.as_deref().unwrap_or("plain"),
            )));
        }
    }
    let mut entity_after = mashup::sync(
        entity_before.as_deref().unwrap_or(&generated.entity),
        &assets,
    )
    .map_err(AuthoringError::Mashup)?
    .0;
    if let Some(before) = entity_before
        .as_ref()
        .filter(|before| **before != entity_after)
    {
        let (raw, span) = mashup::payload(before).map_err(AuthoringError::Mashup)?;
        let (target, _) = mashup::payload(&entity_after).map_err(AuthoringError::Mashup)?;
        let wanted: Value = serde_json::from_str(target.trim()).map_err(invalid)?;
        let rendered = super::preserve::render(raw.trim(), &wanted);
        let existing = String::from_utf8_lossy(span.of(before));
        let payload = crate::core::sync::render_payload(
            mashup::inner_of(&existing),
            &rendered,
            crate::core::sync::newline_of(&existing),
            true,
        );
        entity_after = crate::core::splice::splice(
            before,
            &[crate::core::splice::Edit::new(
                span,
                crate::core::scan::render_cdata(payload.as_bytes()),
            )],
        )
        .map_err(invalid)?;
    }
    let content_after = if content_before.as_deref().is_some_and(|b| {
        text(b)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .as_ref()
            == Some(&content)
    }) {
        content_before.clone().unwrap()
    } else if let Some(before) = &content_before {
        {
            let mut out = if before.starts_with(b"\xEF\xBB\xBF") {
                b"\xEF\xBB\xBF".to_vec()
            } else {
                Vec::new()
            };
            out.extend(super::preserve::render(text(before)?, &content).into_bytes());
            out
        }
    } else {
        assets.content.into_bytes()
    };
    let css_after = if css_before
        .as_deref()
        .is_some_and(|b| text(b).is_ok_and(|s| mashup::same_css(s, &assets.css)))
    {
        css_before.clone().unwrap()
    } else {
        assets.css.into_bytes()
    };
    let mut nodes: Vec<_> = baseline
        .ids
        .iter()
        .map(|(node, id)| {
            fn widget<'a>(v: &'a Value, id: &str) -> Option<&'a Value> {
                match v {
                    Value::Object(m) => {
                        if v.pointer("/Properties/Id").and_then(Value::as_str) == Some(id) {
                            Some(v)
                        } else {
                            m.values().find_map(|v| widget(v, id))
                        }
                    }
                    Value::Array(a) => a.iter().find_map(|v| widget(v, id)),
                    _ => None,
                }
            }
            let previous = old.as_ref().and_then(|b| widget(&b.content, id));
            let status = if old.is_none() && existing.is_some() {
                "unchanged"
            } else if previous.is_none() {
                "created"
            } else if previous == widget(&baseline.content, id) {
                "unchanged"
            } else {
                "changed"
            };
            json!({"node":node,"widget":id,"status":status})
        })
        .collect();
    if let Some(old) = &old {
        for (node, id) in &old.ids {
            if !baseline.ids.contains_key(node) {
                nodes.push(json!({"node":node,"widget":id,"status":"changed","removed":true}));
            }
        }
    }
    // Digest the content as it lands in content.json: the next build reads it back from there.
    // A build that only keeps hand edits writes nothing, so it carries the old digest forward.
    let written = match &old {
        Some(old) if content_before.as_deref() == Some(content_after.as_slice()) => {
            old.written.clone()
        }
        _ => text(&content_after)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .map_or_else(|| Baseline::digest(&content), |v| Baseline::digest(&v)),
    };
    let mut files = vec![
        File {
            path: entity_path,
            before: entity_before,
            after: entity_after,
        },
        File {
            path: content_path,
            before: content_before,
            after: content_after,
        },
        File {
            path: css_path,
            before: css_before,
            after: css_after,
        },
        File {
            path: baseline_path,
            before: before_baseline,
            after: baseline.bytes(),
        },
    ];
    if spec_file.is_some() {
        files.push(File {
            path: page_path.clone(),
            before: page_before.clone(),
            after: stored_spec(yaml, &spec, spec_path, &page_path, solution)?,
        });
    }
    // A stylesheet the spec names sits beside its stored page.yaml, so a rebuild finds it.
    let mut css_input = None;
    if let Some(css) = &spec.page.value.css {
        let source = spec_base.join(&css.value);
        let bytes = read(&source)?
            .ok_or_else(|| invalid(format!("{}: page.css {} is missing", spec_name, css.value)))?;
        let target = dir.join(&css.value);
        if spec_file.is_some() && target != source {
            files.push(File {
                before: read(&target)?,
                path: target,
                after: bytes.clone(),
            });
        }
        css_input = Some((source, Some(bytes)));
    }
    for f in &files {
        crate::core::transaction::check_inside(&solution.root, &f.path).map_err(invalid)?;
    }
    let mut inputs = vec![
        (
            spec_path.to_path_buf(),
            if generated_spec {
                page_before
            } else {
                Some(spec_bytes.clone())
            },
        ),
        (
            solution.root.join("twaco.toml"),
            read(&solution.root.join("twaco.toml"))?,
        ),
    ];
    inputs.extend(css_input);
    inputs.extend(
        spec.resolved_tokens
            .inputs
            .iter()
            .map(|(path, bytes)| (path.clone(), Some(bytes.clone()))),
    );
    let mut created_cells = Vec::new();
    let mut cell_notices = generated.warnings.clone();
    if overridden {
        cell_notices.push(format!(
            "page.name was overridden with {name}; the copied page.yaml uses the command's name"
        ));
    }
    if let Some(tokens) = &spec.tokens {
        if spec
            .page
            .value
            .theme
            .as_ref()
            .is_none_or(|t| t.value == "PTC Convergence Theme")
        {
            let mashup = &spec.page.value.name.value;
            let theme = format!("{}_TH", mashup.strip_suffix("_MU").unwrap_or(mashup));
            // `new theme --tokens` resolves a file from the solution root, not from the spec.
            let source = super::compile::rebase_tokens(
                &tokens.value,
                spec_path.parent().unwrap_or(&solution.root),
                &solution.root,
            );
            cell_notices.push(format!("the default theme was not generated from these tokens; run twaco new theme {theme} --tokens {source} --apply, then set page.theme: {theme}"));
        }
    }
    for cell in &generated.cells {
        let page = solution.src_root().join(&cell.name).join("page.yaml");
        let present = found
            .iter()
            .any(|e| e.info.collection == "Mashups" && e.info.name == cell.name)
            || read(&page)?.is_some();
        let previous = old.as_ref().and_then(|b| b.generated_cells.get(&cell.name));
        if present {
            let mut untouched = if previous.is_some() {
                true
            } else {
                legacy_cell_matches(solution, cell, &mut inputs)?
            };
            if let Some(hashes) = previous {
                untouched &= !hashes.is_empty();
                for (relative, hash) in hashes {
                    let path = solution.root.join(relative);
                    crate::core::transaction::check_inside(&solution.root, &path)
                        .map_err(invalid)?;
                    let bytes = read(&path)?;
                    untouched &= bytes
                        .as_ref()
                        .is_some_and(|b| crate::core::repo::sha256_hex(b) == *hash);
                    inputs.push((path, bytes));
                }
            }
            if !untouched {
                cell_notices.push(format!("{}: cell edited, not regenerated", cell.name));
                if let Some(hashes) = previous {
                    baseline
                        .generated_cells
                        .insert(cell.name.clone(), hashes.clone());
                }
                continue;
            }
        }
        let sub = plan_from(
            solution,
            &cell.name,
            Some(project),
            Some(SpecInput {
                path: page,
                bytes: cell.spec.clone().into_bytes(),
                generated: true,
            }),
        )?;
        if sub.conflicts() {
            return Err(invalid(format!(
                "{}: generated cell has conflicts; reconcile its files before building",
                cell.name
            )));
        }
        baseline.generated_cells.insert(
            cell.name.clone(),
            sub.files
                .iter()
                .map(|f| {
                    (
                        solution.relative(&f.path),
                        crate::core::repo::sha256_hex(&f.after),
                    )
                })
                .collect(),
        );
        files.extend(sub.files);
        inputs.extend(sub.inputs);
        created_cells.push(cell.name.clone());
    }
    baseline.written = written;
    let baseline_bytes = baseline.bytes();
    // Flattening and expanding an adopted tree may reorder object keys. Baseline metadata is
    // semantic JSON too; do not turn that ordering into a write on a clean rebuild.
    files[3].after = match &files[3].before {
        Some(before)
            if serde_json::from_slice::<Value>(before).ok()
                == serde_json::from_slice::<Value>(&baseline_bytes).ok() =>
        {
            before.clone()
        }
        _ => baseline_bytes,
    };
    let mut plan = Plan {
        report: json!({"mashup":name,"project":project,"template":spec.page.value.template.value,"slots":spec.page.value.slots.keys().collect::<Vec<_>>(),"applied":false,"dry_run":true,"nodes":nodes,"hand_edits_kept":report.kept.iter().map(|p| merge::display_path(p)).collect::<Vec<_>>(),"conflicts":conflicts,"cells":created_cells,"cell_notices":cell_notices,"files_state":"would write","files":files.iter().filter(|f| f.before.as_deref() != Some(&f.after)).map(|f| solution.relative(&f.path)).collect::<Vec<_>>()}),
        files,
        inputs,
        components,
    };
    if plan.conflicts() {
        plan.report["files_state"] = json!("refused");
        plan.report["files"] = json!([]);
    } else if plan.report["files"].as_array().is_some_and(Vec::is_empty) {
        plan.report["files_state"] = json!("nothing to write");
    }
    Ok(plan)
}

pub(crate) fn generated_cell(
    solution: &Solution,
    project: &str,
    cell: &compile::GeneratedCell,
) -> Result<Plan, AuthoringError> {
    let path = solution.src_root().join(&cell.name).join("page.yaml");
    let bytes = if let Some(existing) = read(&path)? {
        let built = compile::compile(solution, project, text(&existing)?).map_err(invalid)?;
        if built.content != cell.artifacts.content || built.css != cell.artifacts.css {
            return Err(invalid(format!("{}: existing cell spec differs from this component; reconcile it or use an existing cell mashup",path.display())));
        }
        existing
    } else {
        cell.spec.as_bytes().to_vec()
    };
    let plan = plan_from(
        solution,
        &cell.name,
        Some(project),
        Some(SpecInput {
            path,
            bytes,
            generated: true,
        }),
    )?;
    if plan.conflicts() {
        return Err(invalid(format!(
            "{}: generated cell has conflicts; reconcile its files before applying the component",
            cell.name
        )));
    }
    Ok(plan)
}

impl Plan {
    pub fn conflicts(&self) -> bool {
        self.report["conflicts"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    }

    pub(crate) fn transaction<'a>(
        &self,
        solution: &'a Solution,
    ) -> Result<Transaction<'a>, AuthoringError> {
        let mut tx = Transaction::new(&solution.root, "mashup build");
        self.append_to(solution, &mut tx)?;
        Ok(tx)
    }

    pub(crate) fn append_to(
        &self,
        solution: &Solution,
        tx: &mut Transaction<'_>,
    ) -> Result<(), AuthoringError> {
        if super::components::inputs(solution)? != self.components {
            return Err(AuthoringError::Write(TransactionError::Stale(
                "twaco/components changed since planning; run mashup build again".into(),
            )));
        }
        if self.conflicts() {
            return Err(AuthoringError::Refused(
                "page build has conflicts; resolve them before applying".into(),
            ));
        }
        for (path, expected) in self
            .inputs
            .iter()
            .map(|pair| (&pair.0, &pair.1))
            .chain(self.files.iter().map(|f| (&f.path, &f.before)))
        {
            if &read(path)? != expected {
                return Err(AuthoringError::Write(TransactionError::Stale(format!(
                    "{} changed since planning; run mashup build again",
                    path.display()
                ))));
            }
        }
        for file in &self.files {
            match &file.before {
                Some(before) => tx.replace_file(&file.path, before, file.after.clone()),
                None => tx.create_file(&file.path, file.after.clone()),
            }
            .map_err(AuthoringError::Write)?;
        }
        Ok(())
    }

    pub fn apply(
        &mut self,
        solution: &Solution,
        lock: &WorkspaceLock,
    ) -> Result<(), AuthoringError> {
        self.transaction(solution)?
            .apply(lock)
            .map_err(AuthoringError::Write)?;
        self.report["applied"] = json!(true);
        if self.report["files"].as_array().is_some_and(Vec::is_empty) {
            self.report["files_state"] = json!("nothing to write");
        } else {
            self.report["files_state"] = json!("written");
        }
        self.report["dry_run"] = json!(false);
        Ok(())
    }
}

fn stored_spec(
    yaml: &str,
    spec: &schema::Spec,
    source: &Path,
    destination: &Path,
    solution: &Solution,
) -> Result<Vec<u8>, AuthoringError> {
    let Some(tokens) = &spec.tokens else {
        return Ok(yaml.as_bytes().to_vec());
    };
    if mashup::tokens::PRESETS.contains(&tokens.value.as_str()) || source == destination {
        return Ok(yaml.as_bytes().to_vec());
    }
    let target = std::fs::canonicalize(
        source
            .parent()
            .unwrap_or(&solution.root)
            .join(&tokens.value),
    )
    .map_err(invalid)?;
    let root = std::fs::canonicalize(&solution.root).map_err(invalid)?;
    let base = root.join(
        destination
            .parent()
            .unwrap()
            .strip_prefix(&solution.root)
            .map_err(invalid)?,
    );
    let base: Vec<_> = base.components().collect();
    let target_parts: Vec<_> = target.components().collect();
    let common = base
        .iter()
        .zip(&target_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let relative = if common == 0 {
        target
    } else {
        let mut path = PathBuf::new();
        for _ in common..base.len() {
            path.push("..");
        }
        for part in &target_parts[common..] {
            path.push(part.as_os_str());
        }
        path
    };
    let value = relative.to_string_lossy().replace('\\', "/");
    let span = tokens.referenced.span();
    let offset = usize::try_from(
        span.byte_offset()
            .ok_or_else(|| invalid("tokens: cannot locate scalar"))?,
    )
    .map_err(invalid)?;
    let len = usize::try_from(
        span.byte_len()
            .ok_or_else(|| invalid("tokens: cannot locate scalar"))?,
    )
    .map_err(invalid)?;
    let mut stored = yaml.to_string();
    stored.replace_range(
        offset..offset + len,
        &serde_json::to_string(&value).map_err(invalid)?,
    );
    Ok(stored.into_bytes())
}
