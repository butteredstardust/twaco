use super::Error;
use serde::Deserialize;
use serde_json::Value;
use serde_saphyr::Spanned;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Spec {
    pub(crate) version: Spanned<u32>,
    pub(crate) rules: Spanned<String>,
    pub(crate) page: Spanned<Page>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Page {
    pub(crate) name: Spanned<String>,
    pub(crate) template: Spanned<String>,
    pub(crate) title: Option<Spanned<String>>,
    pub(crate) theme: Option<Spanned<String>>,
    #[serde(default)]
    pub(crate) data: BTreeMap<String, Spanned<Source>>,
    #[serde(default)]
    pub(crate) slots: BTreeMap<String, Spanned<Vec<Node>>>,
    #[serde(default)]
    pub(crate) events: Vec<Spanned<Event>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    pub(crate) service: Spanned<String>,
    pub(crate) on: Option<Spanned<String>>,
    #[serde(default)]
    pub(crate) params: BTreeMap<String, Spanned<Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Node {
    Grid(Body),
    Label(Body),
    Heading(Body),
    Button(Body),
    Widget(Body),
}

impl Node {
    pub(crate) fn body(&self) -> &Body {
        match self {
            Self::Grid(b)
            | Self::Label(b)
            | Self::Heading(b)
            | Self::Button(b)
            | Self::Widget(b) => b,
        }
    }
    pub(crate) fn widget(&self) -> Result<&str, Error> {
        Ok(match self {
            Self::Grid(_) => "ptcsgrid",
            Self::Label(_) | Self::Heading(_) => "ptcslabel",
            Self::Button(_) => "ptcsbutton",
            Self::Widget(b) => b
                .kind
                .as_ref()
                .ok_or_else(|| Error::at(b.id.referenced, "widget needs type"))?
                .value
                .as_str(),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Body {
    pub(crate) id: Spanned<String>,
    #[serde(rename = "type")]
    pub(crate) kind: Option<Spanned<String>>,
    #[serde(alias = "label")]
    pub(crate) text: Option<String>,
    pub(crate) data: Option<Spanned<String>>,
    #[serde(default)]
    pub(crate) columns: Vec<Spanned<Column>>,
    #[serde(default)]
    pub(crate) props: BTreeMap<String, Spanned<Value>>,
    #[serde(default)]
    pub(crate) style: Vec<Spanned<String>>,
    pub(crate) surface: Option<bool>,
    pub(crate) size: Option<Spanned<Size>>,
    #[serde(default)]
    pub(crate) children: Vec<Node>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Column {
    pub(crate) field: String,
    pub(crate) title: Option<String>,
    pub(crate) width: Option<u32>,
    pub(crate) format: Option<String>,
    pub(crate) status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Size {
    Fixed(u32),
    Fit,
    Fill,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Event {
    pub(crate) on: Spanned<String>,
    pub(crate) invoke: Spanned<String>,
}

pub(crate) fn parse(text: &str) -> Result<Spec, Error> {
    let spec: Spec = serde_saphyr::from_str(text).map_err(|error| {
        Error::at(
            error.location().unwrap_or(serde_saphyr::Location::UNKNOWN),
            error,
        )
    })?;
    if spec.version.value != 1 {
        return Err(Error::at(spec.version.referenced, "version must be 1"));
    }
    if spec.rules.value != "v1" {
        return Err(Error::at(spec.rules.referenced, "rules must be v1"));
    }
    let mut ids = BTreeMap::new();
    fn check(
        nodes: &[Node],
        ids: &mut BTreeMap<String, serde_saphyr::Location>,
        page: &Page,
    ) -> Result<(), Error> {
        for node in nodes {
            let b = node.body();
            if b.id.value.is_empty()
                || !b
                    .id
                    .value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(Error::at(
                    b.id.referenced,
                    "id must contain letters, digits, _ or -",
                ));
            }
            if let Some(previous) = ids.insert(b.id.value.clone(), b.id.referenced) {
                let later = if (previous.line(), previous.column())
                    > (b.id.referenced.line(), b.id.referenced.column())
                {
                    previous
                } else {
                    b.id.referenced
                };
                return Err(Error::at(later, format!("duplicate id {}", b.id.value)));
            }
            if page.data.contains_key(&b.id.value) {
                return Err(Error::at(
                    b.id.referenced,
                    "node id also names a data declaration; choose distinct names",
                ));
            }
            node.widget()?;
            if let Some(data) = &b.data {
                if !page.data.contains_key(&data.value) {
                    return Err(Error::at(
                        data.referenced,
                        format!("unknown data {}", data.value),
                    ));
                }
            }
            if !matches!(node, Node::Grid(_)) && (b.data.is_some() || !b.columns.is_empty()) {
                return Err(Error::at(
                    b.id.referenced,
                    "data and columns belong to grid",
                ));
            }
            if !matches!(node, Node::Widget(_)) && b.kind.is_some() {
                return Err(Error::at(b.id.referenced, "type belongs to widget"));
            }
            check(&b.children, ids, page)?;
        }
        Ok(())
    }
    for nodes in spec.page.value.slots.values() {
        check(&nodes.value, &mut ids, &spec.page.value)?;
    }
    Ok(spec)
}
