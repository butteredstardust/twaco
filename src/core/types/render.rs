use super::super::datashape::Aspect;
use super::model::{Entity, Member, Model, Parameter, Service, TypedValue};
use super::platform::{merge_platform_meta, Platform};
use super::write::PLAIN_COLLECTIONS;
use serde_json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub(super) struct Generated {
    pub(super) datashapes: String,
    pub(super) entities: String,
    pub(super) collections: String,
}

pub(super) fn render_skipped_global(out: &mut String, name: &str) {
    out.push_str(&format!(
        "// Skipped {name}: script.js declares it at the start of a line.\n"
    ));
}

pub(super) fn generate(model: &Model, platform: Option<&Platform>) -> Generated {
    let data_shape_names: BTreeSet<&str> = model
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

    Generated {
        datashapes: render_datashapes(model, &data_shape_names, &data_shape_ids),
        entities: render_entities(
            model,
            platform,
            &data_shape_names,
            &data_shape_ids,
            &entity_ids,
        ),
        collections: render_collections(
            model,
            platform,
            &data_shape_names,
            &data_shape_ids,
            &entity_ids,
        ),
    }
}

pub(super) fn identifiers<'a>(
    names: impl IntoIterator<Item = &'a str>,
    prefix: &str,
) -> BTreeMap<String, String> {
    let mut names: Vec<&str> = names.into_iter().collect();
    names.sort();
    let mut counts = BTreeMap::<String, usize>::new();
    let mut taken = BTreeSet::<String>::new();
    let mut out = BTreeMap::new();
    for name in names {
        let source_name = name
            .split_once('\0')
            .map_or(name, |(entity_name, _)| entity_name);
        let stem: String = source_name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        let base = format!("{prefix}{stem}");
        // A suffixed identifier can itself be another name's plain one (`A.B` twice beside
        // `A_B_2`), so a candidate is taken only once nothing holds it.
        let count = counts.entry(base.clone()).or_default();
        let mut identifier = base.clone();
        while taken.contains(&identifier) {
            *count += 1;
            identifier = format!("{base}_{}", *count + 1);
        }
        taken.insert(identifier.clone());
        out.insert(name.to_string(), identifier);
    }
    out
}

fn render_datashapes(
    model: &Model,
    known_shapes: &BTreeSet<&str>,
    identifiers: &BTreeMap<String, String>,
) -> String {
    let mut out = String::from("declare namespace twx.ds {\n");
    for shape in &model.data_shapes {
        out.push_str(&format!(
            "    /** {} (DataShapes) */\n",
            jsdoc_text(&shape.name)
        ));
        out.push_str(&format!("    interface {} {{\n", identifiers[&shape.name]));
        let mut fields = shape.fields.iter().collect::<Vec<_>>();
        fields.sort_by(|a, b| a.name.cmp(&b.name));
        for field in fields {
            render_description(&mut out, 8, &field.description);
            let data_shape = match field.aspects.get("dataShape") {
                Some(Aspect::Text(value)) => Some(value.as_str()),
                _ => None,
            };
            let value = TypedValue {
                base_type: field.base_type.clone(),
                data_shape: data_shape.map(str::to_string),
            };
            out.push_str(&format!(
                "        {}?: {};\n",
                single_quoted(&field.name),
                type_name(&value, known_shapes, identifiers)
            ));
        }
        out.push_str("    }\n\n");
    }
    out.push_str("}\n");
    out
}

fn render_entities(
    model: &Model,
    platform: Option<&Platform>,
    known_shapes: &BTreeSet<&str>,
    data_shape_ids: &BTreeMap<String, String>,
    entity_ids: &BTreeMap<String, String>,
) -> String {
    let mut out = String::from("declare namespace twx {\n");
    for entity in &model.entities {
        let key = format!("{}\0{}", entity.name, entity.collection);
        out.push_str(&format!(
            "    /** {} ({}) */\n    interface {} {{\n",
            jsdoc_text(&entity.name),
            jsdoc_text(&entity.collection),
            entity_ids[&key]
        ));
        let (members, open) = flattened_members(entity, model, platform);
        for member in members.values() {
            match member {
                Member::Property(property) => {
                    render_description(&mut out, 8, &property.description);
                    out.push_str(&format!(
                        "        {}: {};\n",
                        member_name(&property.name),
                        type_name(&property.value, known_shapes, data_shape_ids)
                    ));
                }
                Member::Service(service) => {
                    render_service(&mut out, service, known_shapes, data_shape_ids)
                }
            }
        }
        if open {
            out.push_str("        [member: string]: any;\n");
        }
        out.push_str("    }\n\n");
    }
    out.push_str("}\n");
    out
}

fn flattened_members<'a>(
    entity: &'a Entity,
    model: &'a Model,
    platform: Option<&'a Platform>,
) -> (BTreeMap<String, Member>, bool) {
    let by_collection = |collection: &str, name: &str| {
        model
            .entities
            .iter()
            .find(|candidate| candidate.collection == collection && candidate.name == name)
    };
    let mut members = BTreeMap::new();
    let mut visited_templates = BTreeSet::new();
    let mut visited_shapes = BTreeSet::new();
    let mut current = Some(entity);
    let mut open = entity.collection == "ThingShapes";
    let mut platform_template = None;
    while let Some(item) = current {
        for member in &item.members {
            members
                .entry(member.name().to_string())
                .or_insert_with(|| member.clone());
        }
        for shape_name in &item.shapes {
            if visited_shapes.insert(shape_name.as_str()) {
                if let Some(shape) = by_collection("ThingShapes", shape_name) {
                    for member in &shape.members {
                        members
                            .entry(member.name().to_string())
                            .or_insert_with(|| member.clone());
                    }
                }
            }
        }
        let Some(template_name) = item.template.as_deref() else {
            break;
        };
        if !visited_templates.insert(template_name) {
            break;
        }
        match by_collection("ThingTemplates", template_name) {
            Some(template) => current = Some(template),
            None => {
                if let Some(meta) = platform.and_then(|cache| cache.templates.get(template_name)) {
                    platform_template = Some(meta);
                } else {
                    open = true;
                }
                break;
            }
        }
    }
    if let Some(platform) = platform {
        // External shapes are less specific than every repository member, but more specific than
        // the complete external template response merged below.
        let mut external = BTreeMap::new();
        let mut current = Some(entity);
        let mut visited = BTreeSet::new();
        while let Some(item) = current {
            for name in &item.shapes {
                if by_collection("ThingShapes", name).is_none() && visited.insert(name.as_str()) {
                    if let Some(meta) = platform.shapes.get(name) {
                        merge_platform_meta(&mut external, meta);
                    }
                }
            }
            current = item
                .template
                .as_deref()
                .and_then(|name| by_collection("ThingTemplates", name));
        }
        for (name, member) in external {
            members.entry(name).or_insert(member);
        }
        if let Some(meta) = platform_template {
            merge_platform_meta(&mut members, meta);
        }
        if entity.collection == "ThingShapes" {
            if let Some(generic) = platform.templates.get("GenericThing") {
                merge_platform_meta(&mut members, generic);
            }
        }
    }
    (members, open)
}

fn render_service(
    out: &mut String,
    service: &Service,
    known_shapes: &BTreeSet<&str>,
    data_shape_ids: &BTreeMap<String, String>,
) {
    let described_parameters: Vec<&Parameter> = service
        .parameters
        .iter()
        .filter(|parameter| !parameter.description.is_empty())
        .collect();
    if !service.description.is_empty() || !described_parameters.is_empty() {
        out.push_str("        /**\n");
        for line in jsdoc_lines(&service.description) {
            out.push_str(&format!("         * {line}\n"));
        }
        for parameter in described_parameters {
            out.push_str(&format!(
                "         * @param {} {}\n",
                jsdoc_text(&parameter.name),
                jsdoc_text(&parameter.description)
            ));
        }
        out.push_str("         */\n");
    }
    out.push_str("        ");
    out.push_str(&member_name(&service.name));
    if service.parameters.is_empty() {
        out.push_str("()");
    } else {
        let required = service
            .parameters
            .iter()
            .any(|parameter| parameter.required);
        out.push_str("(params");
        if !required {
            out.push('?');
        }
        out.push_str(": { ");
        for (index, parameter) in service.parameters.iter().enumerate() {
            if index > 0 {
                out.push_str("; ");
            }
            out.push_str(&member_name(&parameter.name));
            if !parameter.required {
                out.push('?');
            }
            out.push_str(": ");
            out.push_str(&type_name(&parameter.value, known_shapes, data_shape_ids));
        }
        out.push_str(" })");
    }
    out.push_str(": ");
    out.push_str(&type_name(&service.result, known_shapes, data_shape_ids));
    out.push_str(";\n");
}

pub(super) fn type_name(
    value: &TypedValue,
    known_shapes: &BTreeSet<&str>,
    data_shape_ids: &BTreeMap<String, String>,
) -> String {
    let upper = value.base_type.to_ascii_uppercase();
    match upper.as_str() {
        "NUMBER" | "INTEGER" | "LONG" => "number".to_string(),
        "BOOLEAN" => "boolean".to_string(),
        "DATETIME" => "Date".to_string(),
        "JSON" => "any".to_string(),
        "NOTHING" => "void".to_string(),
        "LOCATION" => "twx.LOCATION".to_string(),
        "INFOTABLE" => value
            .data_shape
            .as_deref()
            .filter(|name| known_shapes.contains(name))
            .and_then(|name| data_shape_ids.get(name))
            .map_or_else(
                || "twx.INFOTABLE<any>".to_string(),
                |identifier| format!("twx.INFOTABLE<twx.ds.{identifier}>"),
            ),
        "STRING" | "TEXT" | "HTML" | "HYPERLINK" | "IMAGELINK" | "PASSWORD" | "GUID" | "XML" => {
            "string".to_string()
        }
        // A query is a JSON object ({ filters, sorts }), passed as one, not its text.
        "QUERY" => "any".to_string(),
        _ if upper.ends_with("NAME") => "string".to_string(),
        _ => "any".to_string(),
    }
}

fn render_collections(
    model: &Model,
    platform: Option<&Platform>,
    known_shapes: &BTreeSet<&str>,
    data_shape_ids: &BTreeMap<String, String>,
    entity_ids: &BTreeMap<String, String>,
) -> String {
    let mut out = String::from("declare namespace twx {\n");
    for (collection, map) in [
        ("Things", "ThingsMap"),
        ("ThingTemplates", "ThingTemplatesMap"),
        ("ThingShapes", "ThingShapesMap"),
    ] {
        out.push_str(&format!("    interface {map} {{\n"));
        for entity in model
            .entities
            .iter()
            .filter(|entity| entity.collection == collection)
        {
            let key = format!("{}\0{}", entity.name, entity.collection);
            out.push_str(&format!(
                "        {}: twx.{};\n",
                double_quoted(&entity.name),
                entity_ids[&key]
            ));
        }
        out.push_str("    }\n\n");
    }
    if let Some(platform) = platform {
        let resource_ids = identifiers(platform.resources.keys().map(String::as_str), "R_");
        out.push_str("    interface ResourcesMap {\n");
        for name in platform.resources.keys() {
            out.push_str(&format!(
                "        {}: twx.{};\n",
                double_quoted(name),
                resource_ids[name]
            ));
        }
        out.push_str("    }\n\n");
        for (name, meta) in &platform.resources {
            out.push_str(&format!(
                "    /** {} (Resources) */\n    interface {} {{\n",
                jsdoc_text(name),
                resource_ids[name]
            ));
            let mut members = BTreeMap::new();
            merge_platform_meta(&mut members, meta);
            for member in members.values() {
                match member {
                    Member::Property(property) => {
                        render_description(&mut out, 8, &property.description);
                        out.push_str(&format!(
                            "        {}: {};\n",
                            member_name(&property.name),
                            type_name(&property.value, known_shapes, data_shape_ids)
                        ));
                    }
                    Member::Service(service) => {
                        render_service(&mut out, service, known_shapes, data_shape_ids);
                    }
                }
            }
            out.push_str("    }\n\n");
        }
    }
    out.push_str("}\n\n");
    for (collection, map) in [
        ("Things", "ThingsMap"),
        ("ThingTemplates", "ThingTemplatesMap"),
        ("ThingShapes", "ThingShapesMap"),
    ] {
        out.push_str(&format!(
            "declare const {collection}: twx.{map} & {{ [name: string]: any }};\n"
        ));
    }
    out.push_str("declare const DataShapes: { [name: string]: any };\n");
    if platform.is_some() {
        out.push_str("declare const Resources: twx.ResourcesMap & { [name: string]: any };\n");
    } else {
        out.push_str("declare const Resources: { [name: string]: any };\n");
    }
    for collection in PLAIN_COLLECTIONS {
        out.push_str(&format!(
            "declare const {collection}: {{ [name: string]: any }};\n"
        ));
    }
    out
}

fn render_description(out: &mut String, indent: usize, description: &str) {
    if description.is_empty() {
        return;
    }
    let padding = " ".repeat(indent);
    out.push_str(&format!("{padding}/**\n"));
    for line in jsdoc_lines(description) {
        out.push_str(&format!("{padding} * {line}\n"));
    }
    out.push_str(&format!("{padding} */\n"));
}

fn jsdoc_lines(value: &str) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        value
            .replace("*/", "*\\/")
            .lines()
            .map(str::to_string)
            .collect()
    }
}

pub(super) fn jsdoc_text(value: &str) -> String {
    value.replace("*/", "*\\/").replace(['\r', '\n'], " ")
}

fn member_name(value: &str) -> String {
    let mut characters = value.chars();
    let valid = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == '$')
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '$'
        });
    if valid {
        value.to_string()
    } else {
        single_quoted(value)
    }
}

fn single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn double_quoted(value: &str) -> String {
    serde_json::to_string(value).expect("a Rust string is always JSON encodable")
}

pub(super) fn ensure_lf_end(value: &str) -> String {
    let mut value = value.replace("\r\n", "\n");
    if !value.ends_with('\n') {
        value.push('\n');
    }
    value
}
