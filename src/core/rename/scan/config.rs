use super::findings::{add_review, add_service_edit, Place, XmlPass};
use crate::core::scan;
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Deserialize, Default)]
struct RenameConfig {
    #[serde(default)]
    project: Vec<RenameProject>,
    #[serde(default)]
    validate: RenameValidate,
}

#[derive(Deserialize, Default)]
struct RenameProject {
    #[serde(default)]
    deploy: RenameDeploy,
}

#[derive(Deserialize, Default)]
struct RenameDeploy {
    entry_point_thing: Option<toml::Spanned<String>>,
    deploy_service: Option<toml::Spanned<String>>,
    deploy_parameters: Option<toml::Spanned<toml::Table>>,
    #[serde(default)]
    post_import: Vec<RenamePostImport>,
}

#[derive(Deserialize)]
struct RenamePostImport {
    thing: toml::Spanned<String>,
    service: toml::Spanned<String>,
    target: Option<toml::Spanned<String>>,
    parameters: Option<toml::Spanned<toml::Table>>,
}

#[derive(Deserialize, Default)]
struct RenameValidate {
    #[serde(default)]
    inherited_overrides: Vec<toml::Spanned<String>>,
}

/// Renames service selectors in deploy configuration and qualified inherited overrides.
pub(crate) fn scan_service_config(
    src: &[u8],
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
) -> Result<XmlPass, String> {
    let text = std::str::from_utf8(src).map_err(|error| error.to_string())?;
    let config: RenameConfig = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut pass = XmlPass::new(src);
    for project in &config.project {
        let deploy = &project.deploy;
        if let (Some(entity), Some(service)) = (&deploy.entry_point_thing, &deploy.deploy_service) {
            if callers.contains(entity.get_ref()) && service.get_ref() == old {
                add_toml_edit(
                    src,
                    service,
                    new,
                    "project.deploy.deploy_service",
                    &mut pass,
                )?;
            }
        }
        for item in &deploy.post_import {
            let entity = item
                .target
                .as_ref()
                .map(|target| {
                    target
                        .get_ref()
                        .split_once('/')
                        .map_or(target.get_ref().as_str(), |(_, name)| name)
                })
                .unwrap_or_else(|| item.thing.get_ref());
            if callers.contains(entity) && item.service.get_ref() == old {
                add_toml_edit(
                    src,
                    &item.service,
                    new,
                    "project.deploy.post_import.service",
                    &mut pass,
                )?;
            }
        }
    }
    for item in &config.validate.inherited_overrides {
        if item.get_ref() == old {
            let span = toml_string_span(src, item.span(), old)?;
            add_review(
                src,
                span,
                Place::Config {
                    key: "validate.inherited_overrides".to_string(),
                },
                &mut pass,
            );
        } else if let Some(entity) = item.get_ref().strip_suffix(&format!(".{old}")) {
            if callers.contains(entity) {
                let whole = toml_string_span(src, item.span(), item.get_ref())?;
                let span = scan::Span::new(whole.end - old.len(), whole.end);
                add_service_edit(
                    src,
                    span,
                    new,
                    Place::Config {
                        key: "validate.inherited_overrides".to_string(),
                    },
                    &mut pass,
                );
            }
        }
    }
    Ok(pass)
}

/// Result of scanning configured calls for one renamed parameter.
#[derive(Debug)]
pub(crate) struct ParamConfigPass {
    pub(crate) pass: XmlPass,
    pub(crate) conflicts: Vec<String>,
}

/// Renames direct parameter keys for configured calls to the selected service.
pub(crate) fn scan_param_config(
    src: &[u8],
    service: &str,
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
) -> Result<ParamConfigPass, String> {
    let text = std::str::from_utf8(src).map_err(|error| error.to_string())?;
    let config: RenameConfig = toml::from_str(text).map_err(|error| error.to_string())?;
    // Spans of keys, which `toml::Spanned` does not give: a table written under its own
    // `[header]` spans only the header, and a comment can hold `key =` too.
    let document = toml_edit::Document::parse(text).map_err(|error| error.to_string())?;
    let mut pass = XmlPass::new(src);
    let mut conflicts = Vec::new();
    for (at, project) in config.project.iter().enumerate() {
        let written = document
            .get("project")
            .and_then(|projects| projects.get(at));
        let deploy_written = written.and_then(|project| project.get("deploy"));
        let deploy = &project.deploy;
        if let (Some(entity), Some(selected), Some(_)) = (
            &deploy.entry_point_thing,
            &deploy.deploy_service,
            &deploy.deploy_parameters,
        ) {
            if callers.contains(entity.get_ref()) && selected.get_ref() == service {
                rename_toml_parameter(
                    src,
                    deploy_written.and_then(|deploy| deploy.get("deploy_parameters")),
                    old,
                    new,
                    "project.deploy.deploy_parameters",
                    &mut pass,
                    &mut conflicts,
                )?;
            }
        }
        for (call, item) in deploy.post_import.iter().enumerate() {
            let entity = item
                .target
                .as_ref()
                .map(|target| {
                    target
                        .get_ref()
                        .split_once('/')
                        .map_or(target.get_ref().as_str(), |(_, name)| name)
                })
                .unwrap_or_else(|| item.thing.get_ref());
            if callers.contains(entity)
                && item.service.get_ref() == service
                && item.parameters.is_some()
            {
                rename_toml_parameter(
                    src,
                    deploy_written
                        .and_then(|deploy| deploy.get("post_import"))
                        .and_then(|calls| calls.get(call))
                        .and_then(|call| call.get("parameters")),
                    old,
                    new,
                    "project.deploy.post_import.parameters",
                    &mut pass,
                    &mut conflicts,
                )?;
            }
        }
    }
    Ok(ParamConfigPass { pass, conflicts })
}

fn rename_toml_parameter(
    src: &[u8],
    written: Option<&toml_edit::Item>,
    old: &str,
    new: &str,
    context: &str,
    pass: &mut XmlPass,
    conflicts: &mut Vec<String>,
) -> Result<(), String> {
    let table = written
        .and_then(toml_edit::Item::as_table_like)
        .ok_or_else(|| format!("cannot read {context} as a table in twaco.toml"))?;
    if table.contains_key(new) {
        conflicts.push(format!("parameter {new} in {context}"));
    }
    if table.contains_key(old) {
        let span = toml_key_span(src, table, old)?;
        add_service_edit(
            src,
            span,
            new,
            Place::Config {
                key: context.to_string(),
            },
            pass,
        );
    }
    Ok(())
}

/// Where `key` is written in `table`, inside its quotes if it has them: under its own
/// `[header]`, inline or dotted alike. A quoted key spelled with an escape is refused, because its
/// bytes are not its name.
fn toml_key_span(
    src: &[u8],
    table: &dyn toml_edit::TableLike,
    key: &str,
) -> Result<scan::Span, String> {
    let missing = || format!("cannot locate parameter key {key:?} in its TOML table");
    let range = table
        .get_key_value(key)
        .and_then(|(written_key, _)| written_key.span())
        .ok_or_else(missing)?;
    let raw = src.get(range.clone()).ok_or_else(missing)?;
    let (start, end) = match raw.first() {
        Some(b'"' | b'\'') => (range.start + 1, range.end - 1),
        _ => (range.start, range.end),
    };
    if src.get(start..end) != Some(key.as_bytes()) {
        return Err(format!(
            "parameter key {key:?} is written with an escape in its TOML table; rename it by hand"
        ));
    }
    Ok(scan::Span::new(start, end))
}

fn add_toml_edit(
    src: &[u8],
    value: &toml::Spanned<String>,
    new: &str,
    key: &str,
    pass: &mut XmlPass,
) -> Result<(), String> {
    let span = toml_string_span(src, value.span(), value.get_ref())?;
    add_service_edit(
        src,
        span,
        new,
        Place::Config {
            key: key.to_string(),
        },
        pass,
    );
    Ok(())
}

fn toml_string_span(
    src: &[u8],
    range: std::ops::Range<usize>,
    value: &str,
) -> Result<scan::Span, String> {
    let raw = src
        .get(range.clone())
        .ok_or_else(|| "TOML returned an out-of-range span".to_string())?;
    let at = raw
        .windows(value.len())
        .position(|part| part == value.as_bytes())
        .ok_or_else(|| format!("cannot locate {value:?} in its TOML value"))?;
    Ok(scan::Span::new(
        range.start + at,
        range.start + at + value.len(),
    ))
}
