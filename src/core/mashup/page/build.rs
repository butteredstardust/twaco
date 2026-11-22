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
    let spec_file = spec_input.as_ref().map(|s| s.path.as_path());
    entity::key::EntityKey::new("Mashups", name).map_err(invalid)?;
    let found = workspace::entities(solution);
    let existing = found
        .iter()
        .find(|e| e.info.collection == "Mashups" && e.info.name == name);
    if spec_file.is_some()
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
    let spec = schema::parse(yaml).map_err(invalid)?;
    if spec.page.value.name.value != name {
        return Err(invalid(format!(
            "{}:{}: page.name must equal {name}",
            spec_path.display(),
            spec.page.value.name.referenced.line()
        )));
    }
    let generated = compile::compile(solution, project, yaml).map_err(invalid)?;
    let baseline = Baseline::generated(&generated, name, project);
    let baseline_path = dir.join(".page-baseline.json");
    let before_baseline = read(&baseline_path)?;
    let old = before_baseline
        .as_deref()
        .map(Baseline::read)
        .transpose()
        .map_err(invalid)?;
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
    if spec_file.is_some() && page_before.as_ref().is_some_and(|b| b != &spec_bytes) {
        return Err(invalid(format!(
            "{} already exists with a different spec; reconcile it first",
            page_path.display()
        )));
    }
    let mut report = merge::Report::default();
    let (mut content, css) = if let Some(bytes) = &entity_before {
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
            let css = merge::merge(
                Some(&json!(mashup::without_terminator(&old.css))),
                Some(&json!(mashup::without_terminator(&current_assets.css))),
                Some(&json!(mashup::without_terminator(&baseline.css))),
                "custom.css",
                &mut report,
            )
            .unwrap_or(json!(""));
            (merged, format!("{}\n", css.as_str().unwrap_or_default()))
        } else {
            if !merge::adopts(&current, &baseline.content)
                || mashup::without_terminator(&current_assets.css)
                    != mashup::without_terminator(&baseline.css)
            {
                report.conflicts.push("first adoption: generated values differ from current; reconcile the spec and mashup before building".into());
            }
            (current, current_assets.css)
        }
    } else {
        (baseline.content.clone(), baseline.css.clone())
    };
    let effective_css = mashup::without_terminator(&css);
    match content.get("CustomMashupCss") {
        None | Some(Value::Null) if effective_css.is_empty() => {}
        _ => {
            content["CustomMashupCss"] = json!(effective_css);
        }
    }
    let mut lines = BTreeMap::new();
    fn node_lines(
        nodes: &[schema::Node],
        ids: &BTreeMap<String, String>,
        lines: &mut BTreeMap<String, u64>,
    ) {
        for n in nodes {
            let b = n.body();
            if let Some(id) = ids.get(&b.id.value) {
                lines.insert(id.clone(), b.id.referenced.line().max(1));
            }
            for child in super::compile::descendants(n) {
                node_lines(std::slice::from_ref(child), ids, lines);
            }
        }
    }
    for slot in spec.page.value.slots.values() {
        node_lines(&slot.value, &baseline.ids, &mut lines);
    }
    let conflicts: Vec<_> = report.conflicts.iter().map(|path| {
        let line = lines.iter().find(|(id,_)| path.contains(id.as_str())).map_or(spec.page.referenced.line().max(1),|(_,line)| *line);
        json!({"path":path,"file":solution.relative(spec_path),"line":line,"message":format!("{name}: {path}; resolve the hand edit or change page.yaml:{line}")})
    }).collect();
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
            before: page_before,
            after: spec_bytes.clone(),
        });
    }
    for f in &files {
        crate::core::transaction::check_inside(&solution.root, &f.path).map_err(invalid)?;
    }
    let mut inputs = vec![
        (
            spec_path.to_path_buf(),
            (!generated_spec).then(|| spec_bytes.clone()),
        ),
        (
            solution.root.join("twaco.toml"),
            read(&solution.root.join("twaco.toml"))?,
        ),
    ];
    // Pages the spec generates for its collections are created beside it, once: a mashup that
    // is already there is the user's from then on and the build leaves it alone.
    let mut created_cells = Vec::new();
    for cell in &generated.cells {
        let page = solution.src_root().join(&cell.name).join("page.yaml");
        let present = found
            .iter()
            .any(|e| e.info.collection == "Mashups" && e.info.name == cell.name)
            || read(&page)?.is_some();
        if present {
            continue;
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
        files.extend(sub.files);
        inputs.extend(sub.inputs);
        created_cells.push(cell.name.clone());
    }
    Ok(Plan {
        report: json!({"mashup":name,"project":project,"template":spec.page.value.template.value,"slots":spec.page.value.slots.keys().collect::<Vec<_>>(),"applied":false,"dry_run":true,"nodes":nodes,"hand_edits_kept":report.kept,"conflicts":conflicts,"cells":created_cells,"files":files.iter().filter(|f| f.before.as_deref() != Some(&f.after)).map(|f| solution.relative(&f.path)).collect::<Vec<_>>()}),
        files,
        inputs,
    })
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
        let mut tx = Transaction::new(&solution.root, "mashup build");
        for file in &self.files {
            match &file.before {
                Some(before) => tx.replace_file(&file.path, before, file.after.clone()),
                None => tx.create_file(&file.path, file.after.clone()),
            }
            .map_err(AuthoringError::Write)?;
        }
        Ok(tx)
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
        self.report["dry_run"] = json!(false);
        Ok(())
    }
}
