use super::{
    schema::{Node, Page},
    Error,
};
use crate::core::{config::Solution, mashup::authoring::AuthoringError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use serde_saphyr::{Location, Spanned};
use std::collections::BTreeMap;

#[derive(Debug)]
pub(crate) struct Invocation {
    pub(crate) id: Spanned<String>,
    name: Spanned<String>,
    props: BTreeMap<String, Spanned<Value>>,
}

impl<'de> Deserialize<'de> for Invocation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Invocation;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("component use, id and props")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut id = None;
                let mut name = None;
                let mut props = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "id" => {
                            if id.is_some() {
                                return Err(serde::de::Error::duplicate_field("id"));
                            }
                            id = Some(map.next_value()?);
                        }
                        "use" => {
                            if name.is_some() {
                                return Err(serde::de::Error::duplicate_field("use"));
                            }
                            name = Some(map.next_value()?);
                        }
                        _ => {
                            if props.insert(key, map.next_value()?).is_some() {
                                return Err(serde::de::Error::custom("duplicate component prop"));
                            }
                        }
                    }
                }
                Ok(Invocation {
                    id: id.ok_or_else(|| serde::de::Error::missing_field("id"))?,
                    name: name.ok_or_else(|| serde::de::Error::missing_field("use"))?,
                    props,
                })
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    name: String,
    source: String,
    #[serde(default)]
    props: BTreeMap<String, Prop>,
    tree: Value,
    contract: Option<Contract>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Prop {
    #[serde(rename = "type")]
    kind: String,
    default: Option<Value>,
    #[serde(default)]
    required: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Contract {
    data: String,
    fields: BTreeMap<String, String>,
}

#[derive(Debug)]
pub(crate) struct Requirement {
    pub(crate) location: Location,
    pub(crate) data: String,
    pub(crate) fields: BTreeMap<String, String>,
}

pub(crate) struct Registry(BTreeMap<String, Definition>);

fn error(message: impl ToString) -> Error {
    Error::at(Location::UNKNOWN, message)
}

impl Registry {
    pub(crate) fn builtin() -> Result<Self, Error> {
        let mut registry = Self(BTreeMap::new());
        for text in [
            include_str!("components/kpi_tile.yaml"),
            include_str!("components/trend_tile.yaml"),
            include_str!("components/status_card.yaml"),
            include_str!("components/filter_panel.yaml"),
            include_str!("components/toolbar_band.yaml"),
            include_str!("components/page_header.yaml"),
            include_str!("components/section_card.yaml"),
            include_str!("components/side_panel.yaml"),
        ] {
            registry.insert(text, "built-in")?;
        }
        Ok(registry)
    }

    fn insert(&mut self, text: &str, path: &str) -> Result<(), Error> {
        let def: Definition =
            serde_saphyr::from_str(text).map_err(|e| error(format!("{path}: {e}")))?;
        if def.name.is_empty()
            || def.source.is_empty()
            || def.props.contains_key("id")
            || def.props.contains_key("use")
        {
            return Err(error(format!(
                "{path}: component needs name/source; id and use are reserved"
            )));
        }
        for (name, prop) in &def.props {
            if !["string", "number", "boolean", "array", "object", "value"]
                .contains(&prop.kind.as_str())
            {
                return Err(error(format!(
                    "{path}: prop {name} has unknown type {}",
                    prop.kind
                )));
            }
            if prop
                .default
                .as_ref()
                .is_some_and(|v| !accepts(&prop.kind, v))
            {
                return Err(error(format!(
                    "{path}: prop {name} default must be {}",
                    prop.kind
                )));
            }
        }
        self.0.insert(def.name.clone(), def);
        Ok(())
    }

    pub(crate) fn load(solution: &Solution) -> Result<Self, Error> {
        let mut registry = Self::builtin()?;
        let dir = solution.root.join("twaco/components");
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(registry),
            Err(e) => return Err(error(format!("{}: {e}", dir.display()))),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let path = entry.map_err(error)?.path();
            if path.extension().is_some_and(|ext| ext == "yaml") {
                paths.push(path);
            }
        }
        paths.sort();
        let mut names = std::collections::BTreeSet::new();
        for path in paths {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| error(format!("{}: {e}", path.display())))?;
            let def: Definition = serde_saphyr::from_str(&text).map_err(error)?;
            if !names.insert(def.name) {
                return Err(error(format!(
                    "{}: duplicate solution component",
                    path.display()
                )));
            }
            registry.insert(&text, &path.display().to_string())?;
        }
        Ok(registry)
    }

    pub(crate) fn expand_page(&self, page: &mut Page) -> Result<(), Error> {
        for nodes in page.slots.values_mut() {
            for node in &mut nodes.value {
                self.expand(node, &mut Vec::new(), &mut page.contracts)?;
            }
        }
        Ok(())
    }

    fn expand(
        &self,
        node: &mut Node,
        stack: &mut Vec<String>,
        contracts: &mut Vec<Requirement>,
    ) -> Result<(), Error> {
        if let Node::Component(invocation) = node {
            let loc = invocation.name.referenced;
            let name = invocation.name.value.clone();
            if stack.len() >= 32 {
                return Err(Error::at(loc, format!("component cycle/depth at {name}")));
            }
            let def = self.0.get(&name).ok_or_else(|| {
                Error::at(
                    loc,
                    format!("unknown component {name}; use components list"),
                )
            })?;
            let mut values = BTreeMap::from([("id".into(), json!(invocation.id.value))]);
            for (key, value) in &invocation.props {
                let prop = def.props.get(key).ok_or_else(|| {
                    Error::at(
                        value.referenced,
                        format!("component {name}: unknown prop {key}"),
                    )
                })?;
                if !accepts(&prop.kind, &value.value) {
                    return Err(Error::at(
                        value.referenced,
                        format!("component {name}: prop {key} must be {}", prop.kind),
                    ));
                }
                values.insert(key.clone(), value.value.clone());
            }
            for (key, prop) in &def.props {
                if !values.contains_key(key) {
                    if prop.required && prop.default.is_none() {
                        return Err(Error::at(
                            loc,
                            format!("component {name}: missing required prop {key}"),
                        ));
                    }
                    values.insert(key.clone(), prop.default.clone().unwrap_or(Value::Null));
                }
            }
            if let Some(contract) = &def.contract {
                let data = values
                    .get(&contract.data)
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        Error::at(
                            loc,
                            format!(
                                "component {name}: contract needs data prop {}",
                                contract.data
                            ),
                        )
                    })?;
                if contract.fields.is_empty() || data.is_empty() {
                    return Err(Error::at(loc, "component contract needs data and fields"));
                }
                contracts.push(Requirement {
                    location: loc,
                    data: data.into(),
                    fields: contract.fields.clone(),
                });
            }
            let tree = substitute(&def.tree, &values).map_err(|e| Error::at(loc, e))?;
            *node = serde_saphyr::from_str(&tree.to_string())
                .map_err(|e| Error::at(loc, format!("component {name}: {e}")))?;
            relocate(node, loc);
            stack.push(name.clone());
            self.expand(node, stack, contracts)?;
            stack.pop();
            return Ok(());
        }
        let body = node.body_mut();
        for child in body
            .children
            .iter_mut()
            .chain(&mut body.fields)
            .chain(&mut body.toolbar)
        {
            self.expand(child, stack, contracts)?;
        }
        for tab in &mut body.tabs {
            if let Some(content) = &mut tab.content {
                for child in content {
                    self.expand(child, stack, contracts)?;
                }
            }
        }
        Ok(())
    }
}

fn accepts(kind: &str, v: &Value) -> bool {
    match kind {
        "value" => true,
        "string" => v.is_string(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        _ => false,
    }
}

fn substitute(tree: &Value, values: &BTreeMap<String, Value>) -> Result<Value, String> {
    Ok(match tree {
        Value::String(text) => {
            if let Some(key) = text.strip_prefix('$').filter(|s| !s.contains(['{', '}'])) {
                return values
                    .get(key)
                    .cloned()
                    .ok_or_else(|| format!("unknown component prop {key}"));
            }
            let mut text = text.clone();
            for (key, value) in values {
                if let Some(v) = value.as_str() {
                    text = text.replace(&format!("${{{key}}}"), v);
                }
            }
            Value::String(text)
        }
        Value::Array(items) => {
            let mut expanded = Vec::new();
            for item in items {
                let value = substitute(item, values)?;
                match (item.as_str().is_some_and(|s| s.starts_with('$')), value) {
                    (true, Value::Array(children)) => expanded.extend(children),
                    (_, value) => expanded.push(value),
                }
            }
            Value::Array(expanded)
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), substitute(v, values)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

/// List the embedded and solution component definitions, or inspect one definition.
pub fn inspect(solution: &Solution, name: Option<&str>) -> Result<Value, AuthoringError> {
    let registry = Registry::load(solution).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
    match name {
        Some(name) => registry.0.get(name).map(|d| json!(d)).ok_or_else(|| {
            AuthoringError::Unknown(format!("unknown component {name}; use components list"))
        }),
        None => Ok(
            json!({"count":registry.0.len(),"components": registry.0.values().map(|d|json!({"name":d.name,"source":d.source,"props":d.props})).collect::<Vec<_>>()}),
        ),
    }
}

pub(crate) fn inputs(
    solution: &Solution,
) -> Result<Vec<(std::path::PathBuf, Vec<u8>)>, AuthoringError> {
    let dir = solution.root.join("twaco/components");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(AuthoringError::Unreadable(format!(
                "{}: {e}",
                dir.display()
            )))
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| AuthoringError::Unreadable(e.to_string()))?
            .path();
        if path.extension().is_some_and(|e| e == "yaml") {
            let bytes = std::fs::read(&path)
                .map_err(|e| AuthoringError::Unreadable(format!("{}: {e}", path.display())))?;
            files.push((path, bytes));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

fn relocate(node: &mut Node, loc: Location) {
    fn span<T>(value: &mut Spanned<T>, loc: Location) {
        value.referenced = loc;
        value.defined = loc;
    }
    if let Node::Component(invocation) = node {
        span(&mut invocation.id, loc);
        span(&mut invocation.name, loc);
        for value in invocation.props.values_mut() {
            span(value, loc);
        }
        return;
    }
    let b = node.body_mut();
    span(&mut b.id, loc);
    for value in b.props.values_mut() {
        span(value, loc);
    }
    for value in b
        .value
        .iter_mut()
        .chain(b.status.iter_mut())
        .chain(b.delta.iter_mut())
    {
        span(value, loc);
    }
    for value in b
        .kind
        .iter_mut()
        .chain(b.data.iter_mut())
        .chain(b.options.iter_mut())
        .chain(b.variant.iter_mut())
        .chain(b.icon.iter_mut())
        .chain(b.enabled_when.iter_mut())
        .chain(b.trend.iter_mut())
        .chain(b.state.iter_mut())
    {
        span(value, loc);
    }
    for value in b.sparkline.iter_mut() {
        span(value, loc);
    }
    for value in b
        .min
        .iter_mut()
        .chain(b.max.iter_mut())
        .chain(b.weight.iter_mut())
    {
        span(value, loc);
    }
    for value in b.size.iter_mut() {
        span(value, loc);
    }
    for value in &mut b.series {
        span(value, loc);
    }
    for value in &mut b.style {
        span(value, loc);
    }
    for column in &mut b.columns {
        span(column, loc);
    }
    for child in b
        .children
        .iter_mut()
        .chain(&mut b.fields)
        .chain(&mut b.toolbar)
    {
        relocate(child, loc);
    }
    for tab in &mut b.tabs {
        if let Some(content) = &mut tab.content {
            for child in content {
                relocate(child, loc);
            }
        }
    }
}
