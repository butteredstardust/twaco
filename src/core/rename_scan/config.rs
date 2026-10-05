use super::super::scan;
use super::findings::{add_review, add_service_edit, Place, XmlPass};
use super::service::skip_ws;
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
pub fn scan_service_config(
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
pub struct ParamConfigPass {
    pub pass: XmlPass,
    pub conflicts: Vec<String>,
}

/// Renames direct parameter keys for configured calls to the selected service.
pub fn scan_param_config(
    src: &[u8],
    service: &str,
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
) -> Result<ParamConfigPass, String> {
    let text = std::str::from_utf8(src).map_err(|error| error.to_string())?;
    let config: RenameConfig = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut pass = XmlPass::new(src);
    let mut conflicts = Vec::new();
    for project in &config.project {
        let deploy = &project.deploy;
        if let (Some(entity), Some(selected), Some(parameters)) = (
            &deploy.entry_point_thing,
            &deploy.deploy_service,
            &deploy.deploy_parameters,
        ) {
            if callers.contains(entity.get_ref()) && selected.get_ref() == service {
                rename_toml_parameter(
                    src,
                    parameters,
                    old,
                    new,
                    "project.deploy.deploy_parameters",
                    &mut pass,
                    &mut conflicts,
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
            if callers.contains(entity) && item.service.get_ref() == service {
                if let Some(parameters) = &item.parameters {
                    rename_toml_parameter(
                        src,
                        parameters,
                        old,
                        new,
                        "project.deploy.post_import.parameters",
                        &mut pass,
                        &mut conflicts,
                    )?;
                }
            }
        }
    }
    Ok(ParamConfigPass { pass, conflicts })
}

fn rename_toml_parameter(
    src: &[u8],
    table: &toml::Spanned<toml::Table>,
    old: &str,
    new: &str,
    context: &str,
    pass: &mut XmlPass,
    conflicts: &mut Vec<String>,
) -> Result<(), String> {
    if table.get_ref().contains_key(new) {
        conflicts.push(format!("parameter {new} in {context}"));
    }
    if table.get_ref().contains_key(old) {
        let span = toml_table_key_span(src, table.span(), old)?;
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

fn toml_table_key_span(
    src: &[u8],
    range: std::ops::Range<usize>,
    key: &str,
) -> Result<scan::Span, String> {
    let raw = src
        .get(range.clone())
        .ok_or_else(|| "TOML returned an out-of-range table span".to_string())?;
    let mut at = 0;
    let mut depth = 0isize;
    while at < raw.len() {
        match raw[at] {
            b'{' => {
                depth += 1;
                at += 1;
            }
            b'}' => {
                depth -= 1;
                at += 1;
            }
            b'\'' | b'"' => {
                let quote = raw[at];
                let start = at + 1;
                at += 1;
                while at < raw.len() && raw[at] != quote {
                    at += if raw[at] == b'\\' { 2 } else { 1 };
                }
                let end = at.min(raw.len());
                at = (at + 1).min(raw.len());
                let equals = skip_ws(raw, at);
                if depth <= 1
                    && raw.get(equals) == Some(&b'=')
                    && raw.get(start..end) == Some(key.as_bytes())
                {
                    return Ok(scan::Span::new(range.start + start, range.start + end));
                }
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = at;
                at += 1;
                while raw.get(at).is_some_and(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-')
                }) {
                    at += 1;
                }
                let equals = skip_ws(raw, at);
                if depth <= 1
                    && raw.get(equals) == Some(&b'=')
                    && raw.get(start..at) == Some(key.as_bytes())
                {
                    return Ok(scan::Span::new(range.start + start, range.start + at));
                }
            }
            _ => at += 1,
        }
    }
    Err(format!(
        "cannot locate parameter key {key:?} in its TOML table"
    ))
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
