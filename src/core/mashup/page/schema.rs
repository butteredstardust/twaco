use super::Error;
use serde::Deserialize;
use serde_json::Value;
use serde_saphyr::Spanned;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Spec {
    pub(crate) tokens: Option<Spanned<String>>,
    #[serde(skip)]
    pub(crate) resolved_tokens: crate::core::mashup::tokens::Tokens,
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
    /// Input parameters (name to base type) of a page that is shown inside another, such as a
    /// collection cell; bind to one with `{bind: param.<name>}`.
    #[serde(default)]
    pub(crate) parameters: BTreeMap<String, Spanned<String>>,
    #[serde(default)]
    pub(crate) data: BTreeMap<String, Spanned<Source>>,
    #[serde(default)]
    pub(crate) slots: BTreeMap<String, Spanned<Vec<Node>>>,
    #[serde(default)]
    pub(crate) events: Vec<Spanned<Event>>,
    #[serde(skip)]
    pub(crate) contracts: Vec<super::components::Requirement>,
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
    Component(super::components::Invocation),
    Grid(Body),
    Label(Body),
    Heading(Body),
    Button(Body),
    Widget(Body),
    Row(Body),
    Column(Body),
    PageHeader(Body),
    Toolbar(Body),
    KpiRow(Body),
    Card(Body),
    FilterPanel(Body),
    Tabs(Body),
    Chart(Body),
    Spacer(Body),
    DateRange(Body),
    Dropdown(Body),
    TextField(Body),
    Pagination(Body),
    TrendTile(Body),
    DetailPair(Body),
    Panel(Body),
    Slider(Body),
    KpiDial(Body),
    SidePanel(Body),
    List(Body),
    Collection(Body),
    NavMenu(Body),
    CardGrid(Body),
    Text(Body),
    Block(Body),
}

impl Node {
    pub(crate) fn body(&self) -> &Body {
        match self {
            Self::Component(_) => unreachable!("components expand before elements"),
            Self::Grid(b)
            | Self::Label(b)
            | Self::Heading(b)
            | Self::Button(b)
            | Self::Row(b)
            | Self::Column(b)
            | Self::Widget(b)
            | Self::PageHeader(b)
            | Self::Toolbar(b)
            | Self::KpiRow(b)
            | Self::Card(b)
            | Self::FilterPanel(b)
            | Self::Tabs(b)
            | Self::Chart(b)
            | Self::Spacer(b)
            | Self::DateRange(b)
            | Self::Dropdown(b)
            | Self::TextField(b)
            | Self::Pagination(b)
            | Self::TrendTile(b)
            | Self::DetailPair(b)
            | Self::Panel(b)
            | Self::Slider(b)
            | Self::KpiDial(b)
            | Self::SidePanel(b)
            | Self::List(b)
            | Self::Collection(b)
            | Self::CardGrid(b)
            | Self::Text(b)
            | Self::Block(b)
            | Self::NavMenu(b) => b,
        }
    }
    pub(crate) fn body_mut(&mut self) -> &mut Body {
        match self {
            Self::Component(_) => unreachable!("components expand before elements"),
            Self::Grid(b)
            | Self::Label(b)
            | Self::Heading(b)
            | Self::Button(b)
            | Self::Row(b)
            | Self::Column(b)
            | Self::Widget(b)
            | Self::PageHeader(b)
            | Self::Toolbar(b)
            | Self::KpiRow(b)
            | Self::Card(b)
            | Self::FilterPanel(b)
            | Self::Tabs(b)
            | Self::Chart(b)
            | Self::Spacer(b)
            | Self::DateRange(b)
            | Self::Dropdown(b)
            | Self::TextField(b)
            | Self::Pagination(b)
            | Self::TrendTile(b)
            | Self::DetailPair(b)
            | Self::Panel(b)
            | Self::Slider(b)
            | Self::KpiDial(b)
            | Self::SidePanel(b)
            | Self::List(b)
            | Self::Collection(b)
            | Self::CardGrid(b)
            | Self::Text(b)
            | Self::Block(b)
            | Self::NavMenu(b) => b,
        }
    }
    pub(crate) fn into_body(self) -> Body {
        match self {
            Self::Component(_) => unreachable!("components expand before elements"),
            Self::Grid(b)
            | Self::Label(b)
            | Self::Heading(b)
            | Self::Button(b)
            | Self::Row(b)
            | Self::Column(b)
            | Self::Widget(b)
            | Self::PageHeader(b)
            | Self::Toolbar(b)
            | Self::KpiRow(b)
            | Self::Card(b)
            | Self::FilterPanel(b)
            | Self::Tabs(b)
            | Self::Chart(b)
            | Self::Spacer(b)
            | Self::DateRange(b)
            | Self::Dropdown(b)
            | Self::TextField(b)
            | Self::Pagination(b)
            | Self::TrendTile(b)
            | Self::DetailPair(b)
            | Self::Panel(b)
            | Self::Slider(b)
            | Self::KpiDial(b)
            | Self::SidePanel(b)
            | Self::List(b)
            | Self::Collection(b)
            | Self::CardGrid(b)
            | Self::Text(b)
            | Self::Block(b)
            | Self::NavMenu(b) => b,
        }
    }
    pub(crate) fn widget(&self) -> Result<&str, Error> {
        Ok(match self {
            Self::Component(_) => unreachable!("components expand before elements"),
            Self::Row(_)
            | Self::Column(_)
            | Self::PageHeader(_)
            | Self::Toolbar(_)
            | Self::KpiRow(_)
            | Self::Card(_)
            | Self::FilterPanel(_)
            | Self::TrendTile(_)
            | Self::DetailPair(_)
            | Self::Panel(_)
            | Self::SidePanel(_)
            | Self::NavMenu(_)
            | Self::CardGrid(_)
            | Self::Block(_)
            | Self::Spacer(_) => "flexcontainer",
            Self::List(_) => "ptcslist",
            Self::Collection(_) => "ptcscollection",
            Self::Slider(_) => "ptcsslider",
            Self::KpiDial(_) => "ptcskpidial",
            Self::Tabs(_) => "ptcstabset",
            Self::Chart(b) => match b.chart_kind.as_deref() {
                Some("line") => "ptcschartline",
                Some("bar") => "ptcschartbar",
                Some("pie") => "ptcschartpie",
                Some("combo") => "ptcschartcombo",
                _ => {
                    return Err(Error::at(
                        b.id.referenced,
                        "chart kind must be line, bar, pie or combo",
                    ))
                }
            },
            Self::DateRange(_) => "ptcsdatepicker",
            Self::Dropdown(_) => "ptcsdropdown",
            Self::TextField(_) => "ptcstextfield",
            Self::Pagination(_) => "ptcspagination",
            Self::Grid(_) => "ptcsgrid",
            Self::Label(_) | Self::Heading(_) | Self::Text(_) => "ptcslabel",
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
    #[serde(skip)]
    pub(crate) value_size: Option<String>,
    pub(crate) id: Spanned<String>,
    #[serde(rename = "type")]
    pub(crate) kind: Option<Spanned<String>>,
    #[serde(rename = "kind")]
    pub(crate) chart_kind: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) help: Option<String>,
    pub(crate) hint: Option<String>,
    pub(crate) band: Option<bool>,
    pub(crate) value: Option<Spanned<Value>>,
    pub(crate) status: Option<Spanned<Value>>,
    pub(crate) unit: Option<String>,
    pub(crate) variant: Option<Spanned<String>>,
    pub(crate) icon: Option<Spanned<String>>,
    pub(crate) enabled_when: Option<Spanned<String>>,
    pub(crate) trend: Option<Spanned<String>>,
    pub(crate) delta: Option<Spanned<Value>>,
    pub(crate) sparkline: Option<Spanned<Sparkline>>,
    pub(crate) state: Option<Spanned<String>>,
    /// The bound field a `state` formatter reads; set while expanding detail_pair.
    #[serde(skip)]
    pub(crate) state_field: Option<String>,
    pub(crate) auto_select: Option<bool>,
    pub(crate) color: Option<String>,
    pub(crate) options: Option<Spanned<String>>,
    pub(crate) display: Option<String>,
    pub(crate) x: Option<String>,
    pub(crate) stacked: Option<bool>,
    pub(crate) min: Option<Spanned<f64>>,
    pub(crate) max: Option<Spanned<f64>>,
    pub(crate) labels: Option<bool>,
    #[serde(default)]
    pub(crate) series: Vec<Spanned<Series>>,
    #[serde(default)]
    pub(crate) toolbar: Vec<Node>,
    #[serde(rename = "for")]
    pub(crate) grid: Option<Spanned<String>>,
    #[serde(default)]
    pub(crate) actions: Vec<Body>,
    #[serde(default)]
    pub(crate) fields: Vec<Node>,
    #[serde(default)]
    pub(crate) footer: Vec<Spanned<String>>,
    #[serde(default)]
    pub(crate) tiles: Vec<Body>,
    #[serde(default)]
    pub(crate) tabs: Vec<Tab>,
    /// The `name:TYPE` inputs of an expression or validator widget, which its wiring binds to.
    #[serde(default)]
    pub(crate) inputs: Vec<String>,

    #[serde(alias = "label")]
    pub(crate) text: Option<String>,
    pub(crate) data: Option<Spanned<String>>,
    #[serde(default)]
    pub(crate) columns: Vec<Spanned<Column>>,
    #[serde(default)]
    pub(crate) props: BTreeMap<String, Spanned<Value>>,
    #[serde(default, deserialize_with = "style_list")]
    pub(crate) style: Vec<Spanned<String>>,
    pub(crate) surface: Option<bool>,
    pub(crate) size: Option<Spanned<Size>>,
    pub(crate) weight: Option<Spanned<f64>>,
    pub(crate) gap: Option<Spanned<u32>>,
    pub(crate) legend: Option<bool>,
    /// Tooltip of an information icon after a heading, page header or side panel title.
    pub(crate) info: Option<String>,
    /// The empty-state text of a list, collection or grid.
    pub(crate) empty: Option<String>,
    pub(crate) selection: Option<Spanned<String>>,
    pub(crate) per_row: Option<Spanned<u32>>,
    /// The width in pixels `per_row` is worked out for (1400 when absent).
    pub(crate) across: Option<Spanned<u32>>,
    pub(crate) preset: Option<Spanned<String>>,
    pub(crate) cell: Option<Spanned<Cell>>,
    /// An existing cell mashup of a collection, with `map` from row fields to its parameters.
    pub(crate) mashup: Option<Spanned<String>>,
    #[serde(default)]
    pub(crate) map: BTreeMap<String, Spanned<String>>,
    pub(crate) collapsible: Option<bool>,
    pub(crate) search: Option<String>,
    #[serde(default)]
    pub(crate) groups: Vec<Spanned<Group>>,
    /// The tab strip label of a side panel.
    pub(crate) tab: Option<String>,
    #[serde(default)]
    pub(crate) children: Vec<Node>,
    /// Alignment of a card's content: `center` puts it in the middle on both axes.
    pub(crate) align: Option<Spanned<String>>,
    /// The caption of a block.
    pub(crate) caption: Option<String>,
    /// A background colour behind a heading, label or text.
    pub(crate) highlight: Option<Spanned<String>>,
    /// Set while expanding card_grid: this node is one of its equal cells.
    #[serde(skip)]
    pub(crate) grid_cell: bool,
    #[serde(skip)]
    pub(crate) band_row: bool,
}

/// `style:` is a list of `--style` settings or a map of `background`, `color`, `border`, `font`, `shadow` and
/// `radius`; a map entry becomes `~key=value` here, and `elements::expand` turns it into settings
/// once it knows the element.
fn style_list<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Spanned<String>>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Vec<Spanned<String>>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a list of style settings or a map of background, color, border, font, shadow and radius")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut out = vec![];
            while let Some(item) = seq.next_element::<Spanned<String>>()? {
                out.push(item);
            }
            Ok(out)
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut out = vec![];
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                out.push(Spanned::new(
                    format!("~{key}={value}"),
                    serde_saphyr::Location::UNKNOWN,
                    serde_saphyr::Location::UNKNOWN,
                ));
            }
            Ok(out)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(vec![])
        }
    }
    d.deserialize_any(Visitor)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cell {
    pub(crate) title: Option<String>,
    pub(crate) kpi_dial: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Group {
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) items: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Series {
    pub(crate) field: String,
    pub(crate) kind: String,
    pub(crate) title: Option<String>,
    pub(crate) axis: Option<Spanned<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Sparkline {
    pub(crate) field: String,
    pub(crate) x: String,
    pub(crate) data: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Tab {
    pub(crate) label: Spanned<String>,
    pub(crate) mashup: Option<Spanned<String>>,
    pub(crate) content: Option<Vec<Node>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ColumnInput {
    Name(String),
    Full(FullColumn),
}

impl<'de> Deserialize<'de> for Column {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match ColumnInput::deserialize(d)? {
            ColumnInput::Name(field) => Self {
                field,
                title: None,
                width: None,
                format: None,
                status: None,
            },
            ColumnInput::Full(c) => Self {
                field: c.field,
                title: c.title,
                width: c.width,
                format: c.format,
                status: c.status,
            },
        })
    }
}

#[derive(Debug)]
pub(crate) struct Column {
    pub(crate) field: String,
    pub(crate) title: Option<String>,
    pub(crate) width: Option<u32>,
    pub(crate) format: Option<String>,
    pub(crate) status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FullColumn {
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
    /// Text sizes of a heading, label or text; other elements refuse them.
    Sm,
    Md,
    Lg,
    Display,
    Xl,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Event {
    pub(crate) on: Spanned<String>,
    pub(crate) invoke: Spanned<String>,
}

#[cfg(test)]
pub(crate) fn parse(text: &str) -> Result<Spec, Error> {
    super::deep(|| {
        parse_nested(
            text,
            &super::components::Registry::builtin()?,
            None,
            std::path::Path::new("."),
        )
    })
}

pub(crate) fn parse_solution(
    text: &str,
    solution: &crate::core::config::Solution,
) -> Result<Spec, Error> {
    parse_at(text, solution, &solution.root)
}

pub(crate) fn parse_at(
    text: &str,
    solution: &crate::core::config::Solution,
    base: &std::path::Path,
) -> Result<Spec, Error> {
    super::deep(|| {
        parse_nested(
            text,
            &super::components::Registry::load(solution)?,
            Some(solution),
            base,
        )
    })
}

fn parse_nested(
    text: &str,
    registry: &super::components::Registry,
    solution: Option<&crate::core::config::Solution>,
    base: &std::path::Path,
) -> Result<Spec, Error> {
    let mut spec: Spec = serde_saphyr::from_str(text).map_err(|error| {
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
    registry.expand_page(&mut spec.page.value)?;
    let tokens = if let Some(source) = &spec.tokens {
        crate::core::mashup::tokens::Tokens::load(&source.value, base)
            .map_err(|e| Error::at(source.referenced, e))?
    } else if let Some(solution) = solution {
        crate::core::mashup::tokens::Tokens::active(solution)
            .map_err(|e| Error::at(spec.version.referenced, e))?
    } else {
        crate::core::mashup::tokens::Tokens::convergence().clone()
    };
    super::elements::expand(&mut spec.page.value, &tokens)?;
    spec.resolved_tokens = tokens;
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
                if let Some((list, _)) = data.value.split_once(".selected") {
                    let picked = super::compile::find_list(page, list);
                    if !data.value.ends_with(".selected")
                        || picked.is_none()
                        || !matches!(node, Node::List(_))
                    {
                        return Err(Error::at(
                            data.referenced,
                            "<list>.selected feeds a list and names a list with data",
                        ));
                    }
                } else if !page.data.contains_key(&data.value) {
                    return Err(Error::at(
                        data.referenced,
                        format!("unknown data {}", data.value),
                    ));
                }
            }
            if (!matches!(
                node,
                Node::Grid(_) | Node::Chart(_) | Node::List(_) | Node::Collection(_)
            ) && b.data.is_some())
                || (!matches!(node, Node::Grid(_)) && !b.columns.is_empty())
            {
                return Err(Error::at(
                    b.id.referenced,
                    "data belongs to grid/chart/list/collection; columns belong to grid",
                ));
            }
            if !matches!(node, Node::Widget(_)) && b.kind.is_some() {
                return Err(Error::at(b.id.referenced, "type belongs to widget"));
            }
            super::elements::validate(node, page)?;
            check(&b.children, ids, page)?;
            check(&b.fields, ids, page)?;
            for tab in &b.tabs {
                if let Some(content) = &tab.content {
                    check(content, ids, page)?;
                }
            }
        }
        Ok(())
    }
    for nodes in spec.page.value.slots.values() {
        check(&nodes.value, &mut ids, &spec.page.value)?;
    }
    Ok(spec)
}
