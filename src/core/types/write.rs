use super::super::config::Solution;
use super::super::workspace;
use super::model::{load_model, script_declares, Member, Model, Service};
use super::platform::Platform;
use super::render::{
    ensure_lf_end, generate, identifiers, jsdoc_text, render_skipped_global, type_name,
};
use super::{Outcome, TypesError};
use serde_json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const BASE: &str = include_str!("../types_base.d.ts");
pub(super) const ENTITY_COLLECTIONS: &[&str] = &["Things", "ThingTemplates", "ThingShapes"];
pub(super) const PLAIN_COLLECTIONS: &[&str] = &[
    "Mashups",
    "Users",
    "Groups",
    "Projects",
    "Networks",
    "Organizations",
    "Subsystems",
    "MediaEntities",
    "StyleDefinitions",
    "StateDefinitions",
    "LocalizationTables",
    "Dashboards",
    "Logs",
    "ModelTags",
    "Notifications",
    "Authenticators",
    "Applications",
];

/// Generate and atomically update the four shared declaration files.
pub fn write(solution: &Solution) -> Result<Outcome, TypesError> {
    let (model, mut skipped) = load_model(solution);
    let platform_path = solution.root.join(".twaco/platform.json");
    let platform = match std::fs::read(&platform_path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(platform) => Some(platform),
            Err(error) => {
                skipped.push(format!(
                    "{} is malformed and was ignored: {error}",
                    platform_path.display()
                ));
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            skipped.push(format!(
                "{} could not be read and was ignored: {error}",
                platform_path.display()
            ));
            None
        }
    };
    write_model(solution, &model, platform.as_ref(), skipped)
}

pub(super) fn write_model(
    solution: &Solution,
    model: &Model,
    platform: Option<&Platform>,
    mut skipped: Vec<String>,
) -> Result<Outcome, TypesError> {
    let generated = generate(model, platform);
    let directory = solution.root.join(".twaco").join("types");
    let files = [
        ("twx.d.ts", ensure_lf_end(BASE)),
        ("datashapes.d.ts", generated.datashapes),
        ("entities.d.ts", generated.entities),
        ("collections.d.ts", generated.collections),
    ];
    let mut files_written = 0;
    for (name, content) in files {
        files_written += usize::from(workspace::write_lf_if_changed(
            &directory.join(name),
            &content,
        )?);
    }

    let (services, service_files_written) = write_service_projects(solution, model, &mut skipped)?;
    files_written += service_files_written;
    skipped.sort();

    Ok(Outcome {
        entities: model.entities.len(),
        data_shapes: model.data_shapes.len(),
        services,
        files_written,
        skipped,
        gitignore_covers_types: gitignore_covers_types(&solution.root),
    })
}

fn write_service_projects(
    solution: &Solution,
    model: &Model,
    skipped: &mut Vec<String>,
) -> Result<(usize, usize), TypesError> {
    let known_shapes: BTreeSet<&str> = model
        .data_shapes
        .iter()
        .map(|shape| shape.name.as_str())
        .collect();
    let data_shape_ids = identifiers(
        model.data_shapes.iter().map(|shape| shape.name.as_str()),
        "D_",
    );
    let entity_keys: Vec<String> = model
        .entities
        .iter()
        .map(|entity| format!("{}\0{}", entity.name, entity.collection))
        .collect();
    let entity_ids = identifiers(entity_keys.iter().map(String::as_str), "E_");

    let mut services = 0;
    let mut files_written = 0;
    for entity in &model.entities {
        let entity_key = format!("{}\0{}", entity.name, entity.collection);
        let services_dir = solution.src_root().join(&entity.name).join("services");
        for member in &entity.members {
            let Member::Service(service) = member else {
                continue;
            };
            let directory = services_dir.join(&service.name);
            let script_path = directory.join("script.js");
            if !script_path.is_file() {
                continue;
            }
            let script = match std::fs::read_to_string(&script_path) {
                Ok(script) => script,
                Err(error) => {
                    skipped.push(format!("{}: {error}", script_path.display()));
                    continue;
                }
            };
            let jsconfig = render_jsconfig(&solution.root, &directory);
            let globals = render_globals(
                service,
                &entity_ids[&entity_key],
                &script,
                &known_shapes,
                &data_shape_ids,
            );
            files_written += usize::from(workspace::write_lf_if_changed(
                &directory.join("jsconfig.json"),
                &jsconfig,
            )?);
            files_written += usize::from(workspace::write_lf_if_changed(
                &directory.join("twaco-globals.d.ts"),
                &globals,
            )?);
            services += 1;
        }
    }
    Ok((services, files_written))
}

pub(super) fn render_jsconfig(root: &Path, service_dir: &Path) -> String {
    let depth = service_dir
        .strip_prefix(root)
        .expect("a service sidecar is inside the solution root")
        .components()
        .count();
    let relative_root = std::iter::repeat_n("..", depth)
        .collect::<Vec<_>>()
        .join("/");
    let value = serde_json::json!({
        "compilerOptions": {
            "allowJs": true,
            "checkJs": false,
            "noEmit": true,
            "target": "ES2015",
            "lib": ["ES2015"],
            "types": []
        },
        "include": [
            "script.js",
            "twaco-globals.d.ts",
            format!("{relative_root}/.twaco/types/*.d.ts")
        ]
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("the jsconfig value is JSON encodable")
    )
}

pub(super) fn render_globals(
    service: &Service,
    entity_id: &str,
    script: &str,
    known_shapes: &BTreeSet<&str>,
    data_shape_ids: &BTreeMap<String, String>,
) -> String {
    let mut out = format!("declare const me: twx.{entity_id};\n");
    for parameter in &service.parameters {
        if script_declares(script, &parameter.name) {
            render_skipped_global(&mut out, &parameter.name);
            continue;
        }
        if !parameter.description.is_empty() {
            out.push_str(&format!("/** {} */\n", jsdoc_text(&parameter.description)));
        }
        out.push_str(&format!(
            "declare let {}: {};\n",
            parameter.name,
            type_name(&parameter.value, known_shapes, data_shape_ids)
        ));
    }
    if service.result.base_type.eq_ignore_ascii_case("NOTHING") {
        return out;
    }
    if script_declares(script, "result") {
        render_skipped_global(&mut out, "result");
    } else {
        out.push_str(&format!(
            "declare let result: {};\n",
            type_name(&service.result, known_shapes, data_shape_ids)
        ));
    }
    out
}

pub(super) fn gitignore_covers_types(root: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join(".gitignore")) else {
        return false;
    };
    let entries: BTreeSet<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.trim_start_matches('/').trim_end_matches('/'))
        .collect();
    let shared = entries.iter().any(|line| {
        let line = line.trim_start_matches('/').trim_end_matches('/');
        matches!(
            line,
            ".twaco" | ".twaco/**" | ".twaco/types" | ".twaco/types/**"
        )
    });
    shared
        && entries.contains("**/services/*/jsconfig.json")
        && entries.contains("**/services/*/twaco-globals.d.ts")
}
