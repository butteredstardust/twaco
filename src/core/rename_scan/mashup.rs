use super::super::scan;
use super::findings::{add_review, add_service_edit, span_text, Place, XmlPass};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
enum JsonNode {
    Object(Vec<(String, scan::Span, JsonNode)>),
    Array(Vec<JsonNode>),
    String(String, scan::Span),
    Other,
}

/// Renames only service-identity strings tied to an affected `Data.<DataName>` declaration.
///
/// The applied `(key, context)` pairs, verified against mashups containing
/// `ListItems`, `CreateItem` and `GetItemData`, are exactly:
/// `Name` and `Target` in `Data.<DataName>.Services[]`; `SourceId` and `SourceName` in a
/// `DataBindings[]` object whose `SourceSection` is that DataName; `TargetId` where
/// `TargetSection` is that DataName; `EventHandlerService` where `EventHandlerId` is that
/// DataName; and `EventTriggerId` where `EventTriggerSection` is that DataName. Every other
/// JSON string exactly equal to the old service name is left as a Review finding.
pub fn scan_service_mashup(
    src: &[u8],
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
    xml: bool,
) -> Result<XmlPass, String> {
    let mut pass = XmlPass::new(src);
    if xml {
        let tokens = scan::tokenize(src).map_err(|error| error.to_string())?;
        let mut elements = Vec::<String>::new();
        for token in &tokens {
            match token.kind {
                scan::Kind::Start => elements.push(span_text(src, token.name).to_string()),
                scan::Kind::End => {
                    elements.pop();
                }
                scan::Kind::Cdata
                    if elements.last().is_some_and(|name| name == "mashupContent") =>
                {
                    scan_mashup_payload(src, token.inner, old, new, callers, &mut pass)?;
                }
                _ => {}
            }
        }
    } else {
        scan_mashup_payload(
            src,
            scan::Span::new(0, src.len()),
            old,
            new,
            callers,
            &mut pass,
        )?;
    }
    Ok(pass)
}

/// Renames only service-input keys tied to a resolved service in mashup content.
///
/// The applied `(key, context)` pairs, verified in representative `CreateItem` and
/// `GetItemData` data entries, are exactly: an object key in
/// `Data.<DataName>.Services[].Parameters` when that service's `Name` or `Target` is the selected
/// service; and `TargetProperty` in `DataBindings[].PropertyMaps[]` when `TargetArea` is `Data`,
/// `TargetSection` is that DataName and `TargetId` is the selected service. Every other JSON key
/// or string exactly equal to the old parameter is Review.
pub fn scan_param_mashup(
    src: &[u8],
    service: &str,
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
    xml: bool,
) -> Result<XmlPass, String> {
    let mut pass = XmlPass::new(src);
    if xml {
        let tokens = scan::tokenize(src).map_err(|error| error.to_string())?;
        let mut elements = Vec::<String>::new();
        for token in &tokens {
            match token.kind {
                scan::Kind::Start => elements.push(span_text(src, token.name).to_string()),
                scan::Kind::End => {
                    elements.pop();
                }
                scan::Kind::Cdata
                    if elements.last().is_some_and(|name| name == "mashupContent") =>
                {
                    scan_param_mashup_payload(
                        src,
                        token.inner,
                        service,
                        old,
                        new,
                        callers,
                        &mut pass,
                    )?;
                }
                _ => {}
            }
        }
    } else {
        scan_param_mashup_payload(
            src,
            scan::Span::new(0, src.len()),
            service,
            old,
            new,
            callers,
            &mut pass,
        )?;
    }
    Ok(pass)
}

fn scan_param_mashup_payload(
    src: &[u8],
    span: scan::Span,
    service: &str,
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
    pass: &mut XmlPass,
) -> Result<(), String> {
    let mut parser = JsonParser {
        src: span.of(src),
        at: 0,
    };
    let root = parser.value()?;
    parser.white();
    if parser.at != parser.src.len() {
        return Err("trailing content in mashup JSON".to_string());
    }
    let mut applied = BTreeMap::<(usize, usize), (String, String)>::new();
    let mut data_names = BTreeSet::new();
    if let Some(JsonNode::Object(entries)) = object_get(&root, "Data") {
        for (data_name, _, entry) in entries {
            if !node_text(object_get(entry, "EntityName"))
                .is_some_and(|entity| callers.contains(entity))
            {
                continue;
            }
            if let Some(JsonNode::Array(services)) = object_get(entry, "Services") {
                for item in services {
                    let selected = node_text(object_get(item, "Name")) == Some(service)
                        || node_text(object_get(item, "Target")) == Some(service);
                    if !selected {
                        continue;
                    }
                    data_names.insert(data_name.clone());
                    if let Some(JsonNode::Object(parameters)) = object_get(item, "Parameters") {
                        for (key, key_span, _) in parameters {
                            if key == old {
                                applied.insert(
                                    (key_span.start, key_span.end),
                                    (
                                        "object key".to_string(),
                                        "Data.<DataName>.Services[].Parameters".to_string(),
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(JsonNode::Array(bindings)) = object_get(&root, "DataBindings") {
        for binding in bindings {
            if node_text(object_get(binding, "TargetArea")) != Some("Data")
                || node_text(object_get(binding, "TargetId")) != Some(service)
                || !node_text(object_get(binding, "TargetSection"))
                    .is_some_and(|name| data_names.contains(name))
            {
                continue;
            }
            if let Some(JsonNode::Array(properties)) = object_get(binding, "PropertyMaps") {
                for property in properties {
                    mark_json(
                        property,
                        "TargetProperty",
                        old,
                        "DataBindings[].PropertyMaps[] targeting the service",
                        &mut applied,
                    );
                }
            }
        }
    }
    for ((start, end), (key, context)) in &applied {
        add_service_edit(
            src,
            scan::Span::new(span.start + start, span.start + end),
            new,
            Place::Mashup {
                key: key.clone(),
                context: context.clone(),
            },
            pass,
        );
    }
    let mut strings = Vec::new();
    collect_json_keys_and_strings(&root, &mut strings);
    for (value, found) in strings {
        if value == old && !applied.contains_key(&(found.start, found.end)) {
            add_review(
                src,
                scan::Span::new(span.start + found.start, span.start + found.end),
                Place::Mashup {
                    key: String::new(),
                    context: "other JSON key or string".to_string(),
                },
                pass,
            );
        }
    }
    Ok(())
}

fn collect_json_keys_and_strings<'a>(node: &'a JsonNode, out: &mut Vec<(&'a str, scan::Span)>) {
    match node {
        JsonNode::Object(entries) => {
            for (key, span, value) in entries {
                out.push((key, *span));
                collect_json_keys_and_strings(value, out);
            }
        }
        JsonNode::Array(items) => {
            for item in items {
                collect_json_keys_and_strings(item, out);
            }
        }
        JsonNode::String(value, span) => out.push((value, *span)),
        JsonNode::Other => {}
    }
}

fn scan_mashup_payload(
    src: &[u8],
    span: scan::Span,
    old: &str,
    new: &str,
    callers: &BTreeSet<String>,
    pass: &mut XmlPass,
) -> Result<(), String> {
    let mut parser = JsonParser {
        src: span.of(src),
        at: 0,
    };
    let root = parser.value()?;
    parser.white();
    if parser.at != parser.src.len() {
        return Err("trailing content in mashup JSON".to_string());
    }
    let mut applied = BTreeMap::<(usize, usize), (String, String)>::new();
    let mut data_names = BTreeSet::new();
    if let Some(JsonNode::Object(entries)) = object_get(&root, "Data") {
        for (data_name, _, entry) in entries {
            if node_text(object_get(entry, "EntityName"))
                .is_some_and(|entity| callers.contains(entity))
            {
                if let Some(JsonNode::Array(services)) = object_get(entry, "Services") {
                    let matching = services
                        .iter()
                        .any(|service| node_text(object_get(service, "Name")) == Some(old));
                    if matching {
                        data_names.insert(data_name.clone());
                        for service in services {
                            for key in ["Name", "Target"] {
                                mark_json(
                                    service,
                                    key,
                                    old,
                                    "Data.<DataName>.Services[]",
                                    &mut applied,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(JsonNode::Array(bindings)) = object_get(&root, "DataBindings") {
        for binding in bindings {
            if node_text(object_get(binding, "SourceSection"))
                .is_some_and(|name| data_names.contains(name))
            {
                mark_json(
                    binding,
                    "SourceId",
                    old,
                    "DataBindings[] with SourceSection",
                    &mut applied,
                );
                mark_json(
                    binding,
                    "SourceName",
                    old,
                    "DataBindings[] with SourceSection",
                    &mut applied,
                );
            }
            if node_text(object_get(binding, "TargetSection"))
                .is_some_and(|name| data_names.contains(name))
            {
                mark_json(
                    binding,
                    "TargetId",
                    old,
                    "DataBindings[] with TargetSection",
                    &mut applied,
                );
            }
        }
    }
    if let Some(JsonNode::Array(events)) = object_get(&root, "Events") {
        for event in events {
            if node_text(object_get(event, "EventHandlerId"))
                .is_some_and(|name| data_names.contains(name))
            {
                mark_json(
                    event,
                    "EventHandlerService",
                    old,
                    "Events[] with EventHandlerId",
                    &mut applied,
                );
            }
            if node_text(object_get(event, "EventTriggerSection"))
                .is_some_and(|name| data_names.contains(name))
            {
                mark_json(
                    event,
                    "EventTriggerId",
                    old,
                    "Events[] with EventTriggerSection",
                    &mut applied,
                );
            }
        }
    }
    for ((start, end), (key, context)) in &applied {
        add_service_edit(
            src,
            scan::Span::new(span.start + start, span.start + end),
            new,
            Place::Mashup {
                key: key.clone(),
                context: context.clone(),
            },
            pass,
        );
    }
    let mut strings = Vec::new();
    collect_json_strings(&root, &mut strings);
    for (value, found) in strings {
        if value == old && !applied.contains_key(&(found.start, found.end)) {
            add_review(
                src,
                scan::Span::new(span.start + found.start, span.start + found.end),
                Place::Mashup {
                    key: String::new(),
                    context: "other JSON string".to_string(),
                },
                pass,
            );
        }
    }
    Ok(())
}

fn object_get<'a>(node: &'a JsonNode, key: &str) -> Option<&'a JsonNode> {
    let JsonNode::Object(entries) = node else {
        return None;
    };
    entries
        .iter()
        .find(|(name, _, _)| name == key)
        .map(|(_, _, value)| value)
}

fn node_text(node: Option<&JsonNode>) -> Option<&str> {
    match node? {
        JsonNode::String(value, _) => Some(value),
        _ => None,
    }
}

fn mark_json(
    object: &JsonNode,
    key: &str,
    old: &str,
    context: &str,
    out: &mut BTreeMap<(usize, usize), (String, String)>,
) {
    if let Some(JsonNode::String(value, span)) = object_get(object, key) {
        if value == old {
            out.insert(
                (span.start, span.end),
                (key.to_string(), context.to_string()),
            );
        }
    }
}

fn collect_json_strings<'a>(node: &'a JsonNode, out: &mut Vec<(&'a str, scan::Span)>) {
    match node {
        JsonNode::Object(entries) => {
            for (_, _, value) in entries {
                collect_json_strings(value, out);
            }
        }
        JsonNode::Array(items) => {
            for item in items {
                collect_json_strings(item, out);
            }
        }
        JsonNode::String(value, span) => out.push((value, *span)),
        JsonNode::Other => {}
    }
}

struct JsonParser<'a> {
    src: &'a [u8],
    at: usize,
}

impl JsonParser<'_> {
    fn white(&mut self) {
        while self.src.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn value(&mut self) -> Result<JsonNode, String> {
        self.white();
        match self.src.get(self.at) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => {
                let (value, span) = self.string()?;
                Ok(JsonNode::String(value, span))
            }
            Some(_) => {
                while self.src.get(self.at).is_some_and(|byte| {
                    !byte.is_ascii_whitespace() && !matches!(*byte, b',' | b']' | b'}')
                }) {
                    self.at += 1;
                }
                Ok(JsonNode::Other)
            }
            None => Err("unexpected end of mashup JSON".to_string()),
        }
    }

    fn object(&mut self) -> Result<JsonNode, String> {
        self.at += 1;
        let mut entries = Vec::new();
        loop {
            self.white();
            if self.src.get(self.at) == Some(&b'}') {
                self.at += 1;
                break;
            }
            let (key, key_span) = self.string()?;
            self.white();
            if self.src.get(self.at) != Some(&b':') {
                return Err("object key without ':' in mashup JSON".to_string());
            }
            self.at += 1;
            entries.push((key, key_span, self.value()?));
            self.white();
            match self.src.get(self.at) {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    break;
                }
                _ => return Err("object without ',' or '}' in mashup JSON".to_string()),
            }
        }
        Ok(JsonNode::Object(entries))
    }

    fn array(&mut self) -> Result<JsonNode, String> {
        self.at += 1;
        let mut items = Vec::new();
        loop {
            self.white();
            if self.src.get(self.at) == Some(&b']') {
                self.at += 1;
                break;
            }
            items.push(self.value()?);
            self.white();
            match self.src.get(self.at) {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    break;
                }
                _ => return Err("array without ',' or ']' in mashup JSON".to_string()),
            }
        }
        Ok(JsonNode::Array(items))
    }

    fn string(&mut self) -> Result<(String, scan::Span), String> {
        if self.src.get(self.at) != Some(&b'"') {
            return Err("expected string in mashup JSON".to_string());
        }
        let quote = self.at;
        self.at += 1;
        let start = self.at;
        let mut escaped = false;
        while let Some(&byte) = self.src.get(self.at) {
            if byte == b'"' && !escaped {
                let end = self.at;
                self.at += 1;
                let value: String = serde_json::from_slice(&self.src[quote..self.at])
                    .map_err(|error| error.to_string())?;
                return Ok((value, scan::Span::new(start, end)));
            }
            escaped = byte == b'\\' && !escaped;
            self.at += 1;
        }
        Err("unterminated string in mashup JSON".to_string())
    }
}
