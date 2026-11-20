use super::{
    datashape, entity_exists, escape, parse_fields, plain_entity, scan, workspace, Aspect,
    BTreeMap, BTreeSet, Field, Kind, NewError, NewRequest, Solution, Value,
};

fn invalid(message: &str) -> NewError {
    NewError::Invalid(message.into())
}

fn reference(collection: &str, name: &str) -> Result<(), NewError> {
    crate::core::entity::key::EntityKey::new(collection, name)
        .map(|_| ())
        .map_err(|error| NewError::Invalid(error.to_string()))
}

fn guid(value: &str) -> bool {
    value.len() == 36 && uuid::Uuid::parse_str(value).is_ok()
}

fn event(value: &str) -> Result<String, NewError> {
    let parts: Vec<_> = value.split(':').collect();
    let [kind, source, name] = parts.as_slice() else {
        return Err(invalid("--event must be Type:Entity:Event"));
    };
    if parts.iter().any(|part| part.is_empty()) {
        return Err(invalid(
            "--event must be Type:Entity:Event with nonempty parts",
        ));
    }
    Ok(format!(
        "eventName=\"{}\" source=\"{}\" sourceType=\"{}\"",
        escape(name)?,
        escape(source)?,
        escape(kind)?
    ))
}

pub(super) fn warnings(solution: &Solution, request: &NewRequest) -> Vec<String> {
    if request.kind != Kind::Notification {
        return Vec::new();
    }
    request.contents.iter().filter(|name| !entity_exists(solution, "NotificationContents", name))
        .map(|name| format!("NotificationContent {name} is absent from the repository; it may exist on the server"))
        .collect()
}

pub(super) fn xml(
    solution: &Solution,
    request: &NewRequest,
    project: &str,
) -> Result<String, NewError> {
    match request.kind {
        Kind::ThingGroup => {
            let mut members = String::new();
            for member in &request.members {
                reference("Things", member)?;
                members.push_str(&format!(
                    "<Member name=\"{}\" type=\"Thing\"/>",
                    escape(member)?
                ));
            }
            Ok(plain_entity(
                "ThingGroups",
                "ThingGroup",
                request,
                project,
                &format!("            <Members>{members}</Members>"),
            )?)
        }
        Kind::Notification => definition(request, project),
        Kind::NotificationContent => content(solution, request, project),
        _ => unreachable!(),
    }
}

fn definition(request: &NewRequest, project: &str) -> Result<String, NewError> {
    let mut contents = String::new();
    for name in &request.contents {
        reference("NotificationContents", name)?;
        contents.push_str(&format!("<Member name=\"{}\"/>", escape(name)?));
    }
    let mut events = String::new();
    for value in &request.events {
        events.push_str(&format!("<Event {}/>", event(value)?));
    }
    let mut recipients: BTreeMap<String, BTreeMap<String, Vec<(String, String)>>> = BTreeMap::new();
    for value in &request.recipients {
        let (spec, recipient) = value
            .split_once('=')
            .ok_or_else(|| invalid("--recipient must be Type:Entity:Event=GUID:User|Group:name"))?;
        if !request.events.iter().any(|value| value == spec) {
            return Err(invalid(
                "a --recipient event must also be listed with --event",
            ));
        }
        let parts: Vec<_> = recipient.splitn(3, ':').collect();
        let [handler, kind, name] = parts.as_slice() else {
            return Err(invalid(
                "--recipient must be Type:Entity:Event=GUID:User|Group:name",
            ));
        };
        if !guid(handler) || !matches!(*kind, "User" | "Group") || name.is_empty() {
            return Err(invalid(
                "--recipient needs a handler GUID, User or Group, and a nonempty name",
            ));
        }
        recipients
            .entry(event(spec)?)
            .or_default()
            .entry(handler.to_ascii_lowercase())
            .or_default()
            .push((kind.to_string(), name.to_string()));
    }
    let mut rows = String::new();
    for (event, handlers) in recipients {
        rows.push_str(&format!("<Event {event}>"));
        for (handler, members) in handlers {
            rows.push_str(&format!("<Handler id=\"{handler}\">"));
            for (kind, name) in members {
                rows.push_str(&format!(
                    "<Member name=\"{}\" type=\"{kind}\"/>",
                    escape(&name)?
                ));
            }
            rows.push_str("</Handler>");
        }
        rows.push_str("</Event>");
    }
    Ok(plain_entity("NotificationDefinitions", "NotificationDefinition", request, project,
        &format!("            <Contents>{contents}</Contents>\n            <Recipients>{rows}</Recipients>\n            <Events>{events}</Events>"))?)
}

fn documents(solution: &Solution) -> BTreeMap<(String, String), Vec<u8>> {
    let mut documents = BTreeMap::new();
    let sources = workspace::entities(solution)
        .into_iter()
        .filter_map(|file| std::fs::read(file.path).ok())
        .chain(crate::core::extension_source::documents(solution));
    for bytes in sources {
        let Ok(tokens) = scan::tokenize(&bytes) else {
            continue;
        };
        for (at, token) in tokens.iter().enumerate() {
            let collection = match token.name.of(&bytes) {
                b"Thing" => "Things",
                b"ThingTemplate" => "ThingTemplates",
                b"DataShape" => "DataShapes",
                _ => continue,
            };
            if !matches!(token.kind, scan::Kind::Start | scan::Kind::Empty) {
                continue;
            }
            let Some(name) = attr(&bytes, token, "name") else {
                continue;
            };
            let Some(end) = scan::element_end_in(&tokens, &bytes, at) else {
                continue;
            };
            let mut document = format!("<Entities><{collection}>").into_bytes();
            document.extend_from_slice(&bytes[token.span.start..tokens[end].span.end]);
            document.extend_from_slice(format!("</{collection}></Entities>").as_bytes());
            documents
                .entry((collection.into(), name))
                .or_insert(document);
        }
    }
    documents
}

fn attr(bytes: &[u8], token: &scan::Token, name: &str) -> Option<String> {
    scan::attribute(bytes, token, name)
        .ok()
        .flatten()
        .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(bytes))))
}

fn registered_fields(solution: &Solution, handler: &str, id: &str) -> Option<Vec<Field>> {
    let documents = documents(solution);
    let mut key = ("Things".to_string(), handler.to_string());
    let mut seen = BTreeSet::new();
    while seen.insert(key.clone()) {
        let bytes = documents.get(&key)?;
        if let Ok(rows) = crate::core::config_table::repository_rows(bytes, "Handlers") {
            if let Some(row) = rows.iter().find(|row| {
                row.get("handlerID")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case(id))
            }) {
                let shape = row.get("configuration")?.as_str()?;
                return datashape::extract(documents.get(&("DataShapes".into(), shape.into()))?)
                    .ok();
            }
        }
        let tokens = scan::tokenize(bytes).ok()?;
        let at = crate::core::sidecar::entity_element(&tokens, bytes)?;
        let parent = attr(
            bytes,
            &tokens[at],
            if key.0 == "Things" {
                "thingTemplate"
            } else {
                "baseThingTemplate"
            },
        )?;
        key = ("ThingTemplates".into(), parent);
    }
    None
}

fn content(solution: &Solution, request: &NewRequest, project: &str) -> Result<String, NewError> {
    let handler = request
        .handler
        .as_deref()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| invalid("notification-content needs --handler Thing"))?;
    reference("Things", handler)?;
    let id = request.handler_id.as_deref().filter(|id| guid(id)).ok_or_else(|| invalid("notification-content needs --handler-id as a GUID (xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx)"))?;
    let fields = match registered_fields(solution, handler, id) {
        Some(fields) => {
            let signature = |fields: &[Field]| fields.iter().map(|field| (field.name.clone(), field.base_type.clone())).collect::<BTreeMap<_, _>>();
            if !request.fields.is_empty() && signature(&parse_fields(&request.fields)?) != signature(&fields) {
                return Err(invalid("--field conflicts with the registered handler DataShape; omit --field when it resolves offline"));
            }
            fields
        }
        None if !request.fields.is_empty() => parse_fields(&request.fields)?,
        None => return Err(invalid("cannot resolve the handler configuration DataShape offline; provide --field name:TYPE for each field, or add the handler Thing, its template chain and extension metadata. An extension such as Mail registers a hidden Handlers table row; a Thing on NotificationHandler alone is not a handler")),
    };
    let mut values = BTreeMap::new();
    for cell in &request.sets {
        let (name, value) = cell
            .split_once('=')
            .ok_or_else(|| invalid("--set must be field=value"))?;
        let field = fields.iter().find(|field| field.name == name).ok_or_else(|| invalid("--set names an undeclared field; use the handler DataShape or --field name:TYPE"))?;
        let lower = name.to_ascii_lowercase();
        if field.base_type == "PASSWORD"
            || [
                "password",
                "secret",
                "token",
                "credential",
                "apikey",
                "api_key",
            ]
            .iter()
            .any(|word| lower.contains(word))
        {
            return Err(invalid(
                "--set cannot store a secret; configure credentials on the server",
            ));
        }
        let valid = match field.base_type.as_str() {
            "BOOLEAN" => matches!(value, "true" | "false"),
            "INTEGER" => value.parse::<i32>().is_ok(),
            "LONG" => value.parse::<i64>().is_ok(),
            "NUMBER" => value.parse::<f64>().is_ok_and(f64::is_finite),
            "GUID" => guid(value),
            "JSON" => serde_json::from_str::<Value>(value).is_ok(),
            "INFOTABLE" | "QUERY" | "TAGS" | "VARIANT" | "LOCATION" | "VEC2" | "VEC3" | "VEC4" => return Err(invalid("--set supports scalar or JSON content fields; structured fields require an exported content XML")),
            _ => true,
        };
        if !valid {
            return Err(NewError::Invalid(format!(
                "--set field {name} needs {}; its value was not stored",
                field.base_type
            )));
        }
        if values.insert(name, value).is_some() {
            return Err(invalid("--set lists a field more than once"));
        }
    }
    let mut definitions = String::new();
    let mut row = String::new();
    for field in fields {
        if field.name.is_empty()
            || !field.name.chars().enumerate().all(|(i, c)| {
                c.is_ascii_alphabetic()
                    || c == '_'
                    || (i > 0 && (c.is_ascii_digit() || matches!(c, '-' | '.')))
            })
        {
            return Err(invalid(
                "handler DataShape field names must be valid XML cell names",
            ));
        }
        definitions.push_str(&format!(
            "<FieldDefinition name=\"{}\" baseType=\"{}\" description=\"{}\" ordinal=\"{}\"",
            escape(&field.name)?,
            escape(&field.base_type)?,
            escape(&field.description)?,
            escape(&field.ordinal)?
        ));
        for (name, aspect) in field.aspects {
            if name == "defaultValue" {
                continue;
            }
            let value = match aspect {
                Aspect::Bool(value) => value.to_string(),
                Aspect::Text(value) => value,
            };
            definitions.push_str(&format!(
                " aspect.{}=\"{}\"",
                escape(&name)?,
                escape(&value)?
            ));
        }
        definitions.push_str("/>");
        if let Some(value) = values.get(field.name.as_str()) {
            // JSON cells have an extra wrapper in ThingWorx XML.
            let text = escape(value)?;
            let text = if field.base_type == "JSON" {
                format!("<json>{text}</json>")
            } else {
                text
            };
            row.push_str(&format!("<{}>{text}</{}>", field.name, field.name));
        }
    }
    let table = format!("            <ConfigurationTables><ConfigurationTable dataShapeName=\"\" description=\"\" isMultiRow=\"false\" name=\"Content\" ordinal=\"1\"><DataShape><FieldDefinitions>{definitions}</FieldDefinitions></DataShape><Rows><Row>{row}</Row></Rows></ConfigurationTable></ConfigurationTables>");
    let xml = plain_entity(
        "NotificationContents",
        "NotificationContent",
        request,
        project,
        &format!(
            "            <HandlerEntity name=\"{}\" type=\"Thing\"/>",
            escape(handler)?
        ),
    )?;
    Ok(xml
        .replace(
            "<NotificationContent\n",
            &format!("<NotificationContent\n         handlerID=\"{id}\"\n"),
        )
        .replace(
            "            <ConfigurationTables></ConfigurationTables>",
            &table,
        ))
}
