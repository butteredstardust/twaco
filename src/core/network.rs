//! Network entities: their lexical connections and the forest they declare.
//!
//! A Network is deliberately read from the same token stream used for every entity edit.  In
//! particular, connection changes replace only a Connection span or the Connections element;
//! they never serialise the enclosing export again.

use super::config::Solution;
use super::entity;
use super::scan::{self, Kind, Span, Token};
use super::splice::{self, Edit};
use super::workspace;
use super::{
    entity::key::ServiceTarget,
    server::{Client, ServerError},
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Duration;

const SERVER_DEPTH: u32 = 10_000;
const SERVER_TIMEOUT: Duration = Duration::from_secs(60);

/// A connection returned by a Network service. Templates are present only in the richer
/// `GetNetworkConnectionsWithTemplate` response, whose rows carry `thingTemplate`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConnection {
    pub connection_type: String,
    pub from: String,
    pub to: String,
    pub template: Option<String>,
}

/// The deliberately narrow, read-only Network server surface.
pub trait Remote: Sync {
    fn connections(
        &self,
        network: &str,
        templates: bool,
    ) -> Result<Vec<ServerConnection>, ServerError>;
}

impl Remote for Client {
    fn connections(
        &self,
        network: &str,
        templates: bool,
    ) -> Result<Vec<ServerConnection>, ServerError> {
        let service = if templates {
            "GetNetworkConnectionsWithTemplate"
        } else {
            "GetNetworkConnections"
        };
        let target = ServiceTarget::entity("Networks", network)?;
        let reply = self.call_service(
            &target,
            service,
            &json!({"maxDepth": SERVER_DEPTH}),
            SERVER_TIMEOUT,
        )?;
        let url = format!("Networks/{network}/Services/{service}");
        let rows = reply
            .and_then(|value| value.get("rows").and_then(Value::as_array).cloned())
            .ok_or_else(|| ServerError::InvalidResponse {
                url: url.clone(),
                why: "returned no rows".into(),
            })?;
        rows.into_iter()
            .map(|row| {
                let text = |field: &str| {
                    row.get(field)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| ServerError::InvalidResponse {
                            url: url.clone(),
                            why: format!("a row has no {field}"),
                        })
                };
                Ok(ServerConnection {
                    connection_type: text("connectionType")?,
                    from: text("from")?,
                    to: text("to")?,
                    template: if templates {
                        row.get("thingTemplate")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    } else {
                        None
                    },
                })
            })
            .collect()
    }
}

/// One asset row from a RFC 4180 CSV import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRow {
    pub line: usize,
    pub name: String,
    pub parent: String,
    pub connection_type: String,
    /// Whether the row gave its type; without one it is "Contains" for a new connection.
    pub typed: bool,
    pub template: Option<String>,
    pub description: String,
    pub project: Option<String>,
}

/// Read the deliberately small asset-import dialect.  This is kept here rather than accepting
/// an ad-hoc split on commas: descriptions and names exported by asset tools regularly contain
/// commas and newlines.
pub fn assets_csv(src: &[u8]) -> Result<Vec<AssetRow>, NetworkError> {
    let text = std::str::from_utf8(src)
        .map_err(|error| NetworkError::Invalid(format!("CSV is not UTF-8: {error}")))?
        .strip_prefix('\u{feff}')
        .unwrap_or_else(|| std::str::from_utf8(src).expect("checked UTF-8"));
    let mut records = Vec::<(usize, Vec<String>)>::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut at_start = true;
    let mut line = 1usize;
    let mut row_line = 1usize;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if at_start => {
                quoted = true;
                at_start = false;
            }
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            }
            ',' if !quoted => {
                row.push(std::mem::take(&mut field));
                at_start = true;
            }
            '\r' if !quoted => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                records.push((row_line, std::mem::take(&mut row)));
                line += 1;
                row_line = line;
                at_start = true;
            }
            '\n' if !quoted => {
                row.push(std::mem::take(&mut field));
                records.push((row_line, std::mem::take(&mut row)));
                line += 1;
                row_line = line;
                at_start = true;
            }
            '\r' => {
                field.push('\r');
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    field.push('\n');
                    line += 1;
                }
            }
            '\n' => {
                field.push('\n');
                line += 1;
            }
            _ => {
                field.push(ch);
                at_start = false;
            }
        }
    }
    if quoted {
        return Err(NetworkError::Invalid(format!(
            "CSV line {row_line} has an unclosed quoted field"
        )));
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        records.push((row_line, row));
    }
    let Some((header_line, header)) = records.first() else {
        return Err(NetworkError::Invalid("CSV has no header row".into()));
    };
    let header = header.clone();
    let header_line = *header_line;
    let mut columns = BTreeMap::new();
    for (index, name) in header.iter().enumerate() {
        if !matches!(
            name.as_str(),
            "name" | "parent" | "type" | "template" | "description" | "project"
        ) {
            return Err(NetworkError::Invalid(format!(
                "CSV line {header_line} has unknown column {name:?}"
            )));
        }
        if columns.insert(name.as_str(), index).is_some() {
            return Err(NetworkError::Invalid(format!(
                "CSV line {header_line} has column {name:?} twice"
            )));
        }
    }
    for required in ["name", "parent"] {
        if !columns.contains_key(required) {
            return Err(NetworkError::Invalid(format!(
                "CSV header needs {required}"
            )));
        }
    }
    let get = |record: &[String], name: &str| -> String {
        columns
            .get(name)
            .and_then(|index| record.get(*index))
            .cloned()
            .unwrap_or_default()
    };
    let mut names = HashSet::new();
    let mut rows = Vec::new();
    for (line, record) in records.into_iter().skip(1) {
        // A blank line, as a spreadsheet leaves between groups, is no row.
        if record.iter().all(|field| field.trim().is_empty()) {
            continue;
        }
        if record.len() > header.len() {
            return Err(NetworkError::Invalid(format!(
                "CSV line {line} has more fields than its header"
            )));
        }
        let name = get(&record, "name");
        if name.is_empty() {
            return Err(NetworkError::Invalid(format!(
                "CSV line {line} has an empty name"
            )));
        }
        if !names.insert(name.clone()) {
            return Err(NetworkError::Invalid(format!(
                "CSV line {line} names {name} more than once"
            )));
        }
        let optional = |key: &str| {
            let value = get(&record, key);
            (!value.is_empty()).then_some(value)
        };
        rows.push(AssetRow {
            line,
            name,
            parent: get(&record, "parent"),
            connection_type: optional("type").unwrap_or_else(|| "Contains".into()),
            typed: optional("type").is_some(),
            template: optional("template"),
            description: get(&record, "description"),
            project: optional("project"),
        });
    }
    Ok(rows)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub connection_type: String,
    pub from: String,
    pub to: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Network {
    pub name: String,
    pub project: String,
    pub connections: Vec<Connection>,
    entity_span: Span,
    connections_span: Option<Span>,
    connections_open: Option<Span>,
    server_templates: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    TwoParents {
        child: String,
        first: String,
        second: String,
    },
    Cycle {
        node: String,
    },
    DanglingParent {
        parent: String,
        child: String,
    },
    Duplicate {
        from: String,
        to: String,
    },
    EmptyType {
        from: String,
        to: String,
    },
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let root = |name: &str| {
            if name.is_empty() {
                "the root".to_string()
            } else {
                name.to_string()
            }
        };
        match self {
            Self::TwoParents {
                child,
                first,
                second,
            } => write!(
                f,
                "{child} has two parents, {} and {}; a Thing is in a network once",
                root(first),
                root(second)
            ),
            Self::Cycle { node } => write!(f, "{node} is its own ancestor"),
            Self::DanglingParent { parent, child } => {
                write!(f, "{child}'s parent {parent} is not itself in the network")
            }
            Self::Duplicate { from, to } => {
                write!(f, "the connection {} -> {to} is there twice", root(from))
            }
            Self::EmptyType { from, to } => {
                write!(
                    f,
                    "the connection {} -> {to} has no connection type",
                    root(from)
                )
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub connection_type: String,
    pub depth: usize,
    pub template: Option<String>,
    pub project: Option<String>,
    pub outside_repository: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forest {
    pub roots: Vec<String>,
    pub children: BTreeMap<String, Vec<String>>,
    pub depth: BTreeMap<String, usize>,
    pub problems: Vec<Problem>,
}

#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error("{0}")]
    Invalid(String),
}

fn value(src: &[u8], token: &Token, attribute: &str) -> Result<String, NetworkError> {
    scan::attribute(src, token, attribute)
        .map_err(|error| NetworkError::Invalid(error.to_string()))?
        .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(src))))
        .ok_or_else(|| NetworkError::Invalid(format!("Connection has no {attribute} attribute")))
}

/// Read a single Network export, retaining every Connection's exact element span.
pub fn parse(src: &[u8]) -> Result<Network, NetworkError> {
    let info = entity::parse(src)
        .map_err(|error| NetworkError::Invalid(format!("not a Network: {error:?}")))?;
    if info.collection != "Networks" {
        return Err(NetworkError::Invalid(format!(
            "{} is not a Network export",
            info.collection
        )));
    }
    let tokens = scan::tokenize(src).map_err(|error| NetworkError::Invalid(error.to_string()))?;
    let entity_at = tokens
        .iter()
        .position(|token| {
            matches!(token.kind, Kind::Start | Kind::Empty) && token.name.of(src) == b"Network"
        })
        .ok_or_else(|| NetworkError::Invalid("Network export has no Network element".into()))?;
    let entity_span = scan::element_span(&tokens, entity_at)
        .ok_or_else(|| NetworkError::Invalid("Network element is not closed".into()))?;
    let connections_at = scan::child_tags(&tokens, src, "Connections", entity_at)
        .into_iter()
        .next();
    let mut connections = Vec::new();
    let (connections_span, connections_open) = if let Some(at) = connections_at {
        let span = scan::element_span(&tokens, at)
            .ok_or_else(|| NetworkError::Invalid("Connections element is not closed".into()))?;
        for connection in scan::child_tags(&tokens, src, "Connection", at) {
            let span = scan::element_span(&tokens, connection)
                .ok_or_else(|| NetworkError::Invalid("Connection element is not closed".into()))?;
            connections.push(Connection {
                connection_type: value(src, &tokens[connection], "connectionType")?,
                from: value(src, &tokens[connection], "from")?,
                to: value(src, &tokens[connection], "to")?,
                span,
            });
        }
        (Some(span), Some(tokens[at].span))
    } else {
        (None, None)
    };
    Ok(Network {
        name: info.name,
        project: info.project,
        connections,
        entity_span,
        connections_span,
        connections_open,
        server_templates: HashMap::new(),
    })
}

/// Construct a Network-shaped model from a server response. Its zero-length spans are never
/// spliced: callers that write first parse the repository bytes again.
pub fn from_server(name: &str, project: &str, rows: &[ServerConnection]) -> Network {
    let mut server_templates = HashMap::new();
    let connections = rows
        .iter()
        .map(|row| {
            if let Some(template) = &row.template {
                server_templates.insert(row.to.clone(), template.clone());
            }
            Connection {
                connection_type: row.connection_type.clone(),
                from: row.from.clone(),
                to: row.to.clone(),
                span: Span::new(0, 0),
            }
        })
        .collect();
    Network {
        name: name.into(),
        project: project.into(),
        connections,
        entity_span: Span::new(0, 0),
        connections_span: None,
        connections_open: None,
        server_templates,
    }
}

/// Validate the shape the server itself accepts and construct the document-order forest.
pub fn forest(network: &Network) -> Forest {
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut parents: HashMap<&str, &str> = HashMap::new();
    let mut nodes: HashSet<&str> = HashSet::new();
    let mut pairs = HashSet::new();
    let mut problems = Vec::new();
    for connection in &network.connections {
        nodes.insert(&connection.to);
        if !connection.from.is_empty() {
            nodes.insert(&connection.from);
        }
        if !pairs.insert((&connection.from, &connection.to)) {
            problems.push(Problem::Duplicate {
                from: connection.from.clone(),
                to: connection.to.clone(),
            });
        }
        if connection.connection_type.is_empty() {
            problems.push(Problem::EmptyType {
                from: connection.from.clone(),
                to: connection.to.clone(),
            });
        }
        if let Some(first) = parents.insert(&connection.to, &connection.from) {
            if first != connection.from {
                problems.push(Problem::TwoParents {
                    child: connection.to.clone(),
                    first: first.into(),
                    second: connection.from.clone(),
                });
            }
        }
        children
            .entry(connection.from.clone())
            .or_default()
            .push(connection.to.clone());
    }
    for connection in &network.connections {
        if !connection.from.is_empty()
            && !network
                .connections
                .iter()
                .any(|other| other.to == connection.from)
        {
            problems.push(Problem::DanglingParent {
                parent: connection.from.clone(),
                child: connection.to.clone(),
            });
        }
    }
    let roots: Vec<String> = network
        .connections
        .iter()
        .filter(|connection| connection.from.is_empty())
        .map(|connection| connection.to.clone())
        .collect();
    let mut depth = BTreeMap::new();
    fn visit(
        node: &str,
        level: usize,
        children: &BTreeMap<String, Vec<String>>,
        depth: &mut BTreeMap<String, usize>,
        visiting: &mut HashSet<String>,
        problems: &mut Vec<Problem>,
    ) {
        if !visiting.insert(node.to_string()) {
            problems.push(Problem::Cycle { node: node.into() });
            return;
        }
        depth.entry(node.into()).or_insert(level);
        if let Some(next) = children.get(node) {
            for child in next {
                visit(child, level + 1, children, depth, visiting, problems);
            }
        }
        visiting.remove(node);
    }
    for root in &roots {
        visit(
            root.as_str(),
            0,
            &children,
            &mut depth,
            &mut HashSet::new(),
            &mut problems,
        );
    }
    // A cycle with no root still needs reporting.
    for node in nodes {
        if !depth.contains_key(node) {
            visit(
                node,
                0,
                &children,
                &mut depth,
                &mut HashSet::new(),
                &mut problems,
            );
        }
    }
    Forest {
        roots,
        children,
        depth,
        problems,
    }
}

/// Join declared nodes to repository Things without treating server-only Things as defects.
pub fn nodes(solution: &Solution, network: &Network) -> Result<Vec<Node>, NetworkError> {
    let things: HashMap<_, _> = workspace::entities(solution)
        .into_iter()
        .filter(|entry| entry.info.collection == "Things")
        .map(|entry| (entry.info.name.clone(), entry))
        .collect();
    let forest = forest(network);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for connection in &network.connections {
        if !seen.insert(connection.to.clone()) {
            continue;
        }
        let thing = things.get(&connection.to);
        let template = network
            .server_templates
            .get(&connection.to)
            .cloned()
            .or_else(|| {
                thing
                    .and_then(|thing| std::fs::read(&thing.path).ok())
                    .and_then(|src| {
                        let tokens = scan::tokenize(&src).ok()?;
                        let thing = tokens.iter().find(|token| {
                            matches!(token.kind, Kind::Start | Kind::Empty)
                                && token.name.of(&src) == b"Thing"
                        })?;
                        scan::attribute(&src, thing, "thingTemplate")
                            .ok()
                            .flatten()
                            .map(|span| {
                                scan::decode_entities(&String::from_utf8_lossy(span.of(&src)))
                            })
                    })
            });
        out.push(Node {
            name: connection.to.clone(),
            connection_type: connection.connection_type.clone(),
            depth: forest.depth.get(&connection.to).copied().unwrap_or(0),
            template,
            project: thing.map(|thing| thing.info.project.clone()),
            outside_repository: thing.is_none(),
        });
    }
    Ok(out)
}

/// What `network show` reports: the nodes in tree order (each root, then its descendants depth
/// first, children in document order), cut to a subtree and a depth when asked.
#[derive(Clone, Debug)]
pub struct Show {
    pub name: String,
    pub project: String,
    /// The node and its depth relative to the shown subtree's top.
    pub rows: Vec<(Node, usize)>,
    pub nodes: usize,
    pub roots: usize,
    pub depth: usize,
    /// Nodes per ThingTemplate (`None`: outside the repository), over the whole network.
    pub templates: BTreeMap<Option<String>, usize>,
    pub problems: Vec<Problem>,
}

pub fn show(
    solution: &Solution,
    network: &Network,
    root: Option<&str>,
    max_depth: Option<usize>,
) -> Result<Show, NetworkError> {
    let forest = forest(network);
    let nodes = nodes(solution, network)?;
    let by_name: HashMap<&str, &Node> = nodes
        .iter()
        .map(|node| (node.name.as_str(), node))
        .collect();
    let starts: Vec<String> = match root {
        Some(root) if !by_name.contains_key(root) => {
            return Err(NetworkError::Invalid(format!(
                "{root} is not in Network {}",
                network.name
            )))
        }
        Some(root) => vec![root.to_string()],
        None => forest.roots.clone(),
    };
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<(String, usize)> =
        starts.into_iter().rev().map(|start| (start, 0)).collect();
    while let Some((name, level)) = stack.pop() {
        // A cycle or a second parent would visit a node again: show it once.
        if !seen.insert(name.clone()) {
            continue;
        }
        if let Some(node) = by_name.get(name.as_str()) {
            rows.push(((*node).clone(), level));
        }
        if max_depth.is_some_and(|maximum| level >= maximum) {
            continue;
        }
        if let Some(children) = forest.children.get(&name) {
            for child in children.iter().rev() {
                stack.push((child.clone(), level + 1));
            }
        }
    }
    let mut templates = BTreeMap::new();
    for node in &nodes {
        *templates.entry(node.template.clone()).or_insert(0) += 1;
    }
    Ok(Show {
        name: network.name.clone(),
        project: network.project.clone(),
        rows,
        nodes: nodes.len(),
        roots: forest.roots.len(),
        depth: forest.depth.values().copied().max().unwrap_or(0),
        templates,
        problems: forest.problems,
    })
}

/// The stable JSON projection shared by the CLI and MCP adapters.
pub fn show_json(network: &Network, report: &Show) -> Value {
    json!({
        "name": report.name, "project": report.project, "nodes": report.nodes,
        "roots": report.roots, "depth": report.depth,
        "templates": report.templates.iter().map(|(template, count)| json!({"template": template, "count": count})).collect::<Vec<_>>(),
        "tree": report.rows.iter().map(|(node, level)| json!({"name": node.name, "level": level, "type": node.connection_type, "template": node.template, "project": node.project, "outside_repository": node.outside_repository})).collect::<Vec<_>>(),
        "connections": network.connections.iter().map(|connection| json!({"type": connection.connection_type, "from": connection.from, "to": connection.to})).collect::<Vec<_>>(),
        "problems": report.problems.iter().map(ToString::to_string).collect::<Vec<_>>(),
    })
}

/// One compact row per repository Network, the JSON form of `network list`.
pub fn list_json(solution: &Solution) -> Vec<Value> {
    workspace::entities(solution).into_iter().filter(|entry| entry.info.collection == "Networks").filter_map(|entry| {
        let bytes = std::fs::read(&entry.path).ok()?;
        let model = parse(&bytes).ok()?;
        let forest = forest(&model);
        Some(json!({"name": model.name, "project": model.project, "connections": model.connections.len(), "roots": forest.roots.len(), "depth": forest.depth.values().copied().max().unwrap_or(0)}))
    }).collect()
}

fn line_ending(src: &[u8]) -> &'static str {
    if src.windows(2).any(|pair| pair == b"\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}
fn indent_before(src: &[u8], at: usize) -> String {
    let start = src[..at]
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |line| line + 1);
    String::from_utf8_lossy(&src[start..at])
        .chars()
        .take_while(|character| character.is_whitespace())
        .collect()
}

/// Render a connection in the export's conventional, one-attribute-per-line layout.
pub fn connection_xml(
    connection_type: &str,
    from: &str,
    to: &str,
    indent: &str,
    ending: &str,
) -> Vec<u8> {
    format!("{indent}<Connection{ending}{indent} connectionType=\"{}\"{ending}{indent} from=\"{}\"{ending}{indent} to=\"{}\"></Connection>", scan::escape_attribute(connection_type), scan::escape_attribute(from), scan::escape_attribute(to)).into_bytes()
}

/// Add one connection in the export's own layout: on a line of its own after the last one,
/// indented like it; into an empty `<Connections></Connections>` (or `<Connections/>`) as its
/// only child; or, with no `<Connections>` at all, as a new element before `</Network>`.
pub fn add(
    src: &[u8],
    network: &Network,
    connection_type: &str,
    from: &str,
    to: &str,
) -> Result<Vec<u8>, NetworkError> {
    add_all(src, network, &[(connection_type, from, to)])
}

/// Add connections `(type, from, to)` in order, as [`add`] would one by one, in one splice.
pub fn add_all(
    src: &[u8],
    network: &Network,
    rows: &[(&str, &str, &str)],
) -> Result<Vec<u8>, NetworkError> {
    if rows.is_empty() {
        return Ok(src.to_vec());
    }
    let ending = line_ending(src);
    let elements = |indent: &str, separator: &str| {
        rows.iter()
            .map(|(connection_type, from, to)| {
                String::from_utf8(connection_xml(connection_type, from, to, indent, ending))
                    .expect("network XML is UTF-8")
            })
            .collect::<Vec<_>>()
            .join(separator)
    };
    let element = |indent: &str| elements(indent, ending);
    let edit = if let Some(last) = network.connections.last() {
        let indent = indent_before(src, last.span.start);
        Edit::new(
            Span::new(last.span.end, last.span.end),
            format!("{ending}{}", element(&indent)),
        )
    } else if let (Some(span), Some(open)) = (network.connections_span, network.connections_open) {
        let inner = &src[open.end.min(span.end)..span.end];
        let inner = String::from_utf8_lossy(inner);
        let inner = inner.strip_suffix("</Connections>").unwrap_or(&inner);
        if !inner.trim().is_empty() {
            return Err(NetworkError::Invalid(
                "the Connections element holds something other than connections".into(),
            ));
        }
        let indent = indent_before(src, open.start);
        Edit::new(
            span,
            format!(
                "<Connections>{ending}{}{ending}{indent}</Connections>",
                element(&format!("{indent}    "))
            ),
        )
    } else if src[..network.entity_span.end].ends_with(b"/>") {
        // A self-closing <Network/> opens up to hold the new element.
        let end = network.entity_span.end;
        Edit::new(
            Span::new(end - 2, end),
            format!("><Connections>{}</Connections></Network>", elements("", "")),
        )
    } else {
        let close = network.entity_span.end - "</Network>".len();
        let line_start = src[..close]
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |at| at + 1);
        let indent = format!("{}    ", indent_before(src, network.entity_span.start));
        if src[line_start..close].iter().all(u8::is_ascii_whitespace) {
            Edit::new(
                Span::new(line_start, line_start),
                format!(
                    "{indent}<Connections>{ending}{}{ending}{indent}</Connections>{ending}",
                    element(&format!("{indent}    "))
                ),
            )
        } else {
            Edit::new(
                Span::new(close, close),
                format!("<Connections>{}</Connections>", elements("", "")),
            )
        }
    };
    splice::splice(src, &[edit]).map_err(|error| NetworkError::Invalid(error.to_string()))
}

/// The edits removing `spans`: the whole lines (indentation and line endings) of any run of
/// lines that holds nothing else, so no blank line is left behind, even when several removed
/// elements share a line; otherwise just the elements.
fn removals(src: &[u8], spans: &[Span]) -> Vec<Edit> {
    let lines = |span: Span| {
        let start = src[..span.start]
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |at| at + 1);
        let end = src[span.end..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(src.len(), |at| span.end + at + 1);
        (start, end)
    };
    let mut spans = spans.to_vec();
    spans.sort_by_key(|span| span.start);
    let mut edits = Vec::new();
    let mut at = 0;
    while at < spans.len() {
        let (start, mut end) = lines(spans[at]);
        let mut group = vec![spans[at]];
        at += 1;
        while at < spans.len() && spans[at].start < end {
            end = end.max(lines(spans[at]).1);
            group.push(spans[at]);
            at += 1;
        }
        let mut rest = src[start..group[0].start].to_vec();
        for (span, next) in group.iter().zip(group.iter().skip(1)) {
            rest.extend_from_slice(&src[span.end..next.start]);
        }
        rest.extend_from_slice(&src[group[group.len() - 1].end..end]);
        if rest.iter().all(u8::is_ascii_whitespace) {
            edits.push(Edit::new(Span::new(start, end), Vec::new()));
        } else {
            edits.extend(group.into_iter().map(|span| Edit::new(span, Vec::new())));
        }
    }
    edits
}

/// Delete the connections whose child is one of `children`, each with its line. When none is
/// left, the element goes back to the empty `<Connections></Connections>` an export writes.
pub fn remove_children(
    src: &[u8],
    network: &Network,
    children: &BTreeSet<String>,
) -> Result<Vec<u8>, NetworkError> {
    let removed: Vec<&Connection> = network
        .connections
        .iter()
        .filter(|connection| children.contains(&connection.to))
        .collect();
    if removed.is_empty() {
        return Ok(src.to_vec());
    }
    let edits: Vec<Edit> = match network.connections_span {
        Some(span) if removed.len() == network.connections.len() => {
            vec![Edit::new(span, "<Connections></Connections>")]
        }
        _ => removals(
            src,
            &removed
                .iter()
                .map(|connection| connection.span)
                .collect::<Vec<_>>(),
        ),
    };
    splice::splice(src, &edits).map_err(|error| NetworkError::Invalid(error.to_string()))
}

/// Delete exactly the named connection, with its line. A missing connection is a no-op.
pub fn remove(
    src: &[u8],
    network: &Network,
    from: &str,
    to: &str,
) -> Result<Vec<u8>, NetworkError> {
    if !network
        .connections
        .iter()
        .any(|connection| connection.from == from && connection.to == to)
    {
        return Ok(src.to_vec());
    }
    remove_children(src, network, &BTreeSet::from([to.to_string()]))
}

#[cfg(test)]
mod tests {
    use super::*;
    const XML: &str = "<Entities>\r\n  <Networks>\r\n    <Network name=\"Acme.Plant\" projectName=\"Acme\">\r\n      <Connections>\r\n        <Connection\r\n         connectionType=\"Contains\"\r\n         from=\"\"\r\n         to=\"Acme.Site\"></Connection>\r\n        <Connection connectionType=\"Contains\" from=\"Acme.Site\" to=\"Acme.Line\"></Connection>\r\n      </Connections>\r\n    </Network>\r\n  </Networks>\r\n</Entities>\r\n";
    #[test]
    fn parses_export_layout_and_builds_a_forest() {
        let network = parse(XML.as_bytes()).unwrap();
        assert_eq!(network.connections.len(), 2);
        let forest = forest(&network);
        assert_eq!(forest.roots, ["Acme.Site"]);
        assert_eq!(forest.depth["Acme.Line"], 1);
        assert!(forest.problems.is_empty());
    }
    #[test]
    fn reports_server_refusals() {
        let network = parse(XML.replace("</Connections>", "<Connection connectionType=\"\" from=\"Acme.Line\" to=\"Acme.Site\"></Connection><Connection connectionType=\"Contains\" from=\"Other\" to=\"Acme.Line\"></Connection></Connections>").as_bytes()).unwrap();
        let problems = forest(&network).problems;
        assert!(problems
            .iter()
            .any(|problem| matches!(problem, Problem::Cycle { .. })));
        assert!(problems
            .iter()
            .any(|problem| matches!(problem, Problem::TwoParents { .. })));
        assert!(problems
            .iter()
            .any(|problem| matches!(problem, Problem::DanglingParent { .. })));
        assert!(problems
            .iter()
            .any(|problem| matches!(problem, Problem::EmptyType { .. })));
    }
    #[test]
    fn adds_without_reformatting_the_document() {
        let network = parse(XML.as_bytes()).unwrap();
        let changed = add(XML.as_bytes(), &network, "Child", "Acme.Site", "Acme.Area").unwrap();
        assert!(changed.windows(2).any(|pair| pair == b"\r\n"));
        assert!(String::from_utf8(changed)
            .unwrap()
            .contains("to=\"Acme.Area\""));
    }

    #[test]
    fn an_empty_connections_element_is_an_empty_network_and_can_receive_a_connection() {
        let xml = b"<Entities><Networks><Network name=\"Acme.Empty\" projectName=\"Acme\"><Connections/></Network></Networks></Entities>";
        let network = parse(xml).unwrap();
        assert!(network.connections.is_empty());
        let changed = add(xml, &network, "Contains", "", "Acme.Site").unwrap();
        assert_eq!(parse(&changed).unwrap().connections.len(), 1);
    }

    /// The exporter's layout: each attribute on a line of its own.
    const EXPORT: &str = "<Entities>
    <Networks>
        <Network
         name=\"Acme.Plant\"
         projectName=\"Acme\">
            <ConfigurationTables></ConfigurationTables>
            <Connections>
                <Connection
                 connectionType=\"Contains\"
                 from=\"\"
                 to=\"Acme.Site\"></Connection>
                <Connection
                 connectionType=\"Contains\"
                 from=\"Acme.Site\"
                 to=\"Acme.Line\"></Connection>
            </Connections>
        </Network>
    </Networks>
</Entities>
";

    #[test]
    fn an_added_connection_takes_the_export_layout_and_removing_it_restores_every_byte() {
        let network = parse(EXPORT.as_bytes()).unwrap();
        let added = add(
            EXPORT.as_bytes(),
            &network,
            "Child",
            "Acme.Line",
            "Acme.Machine",
        )
        .unwrap();
        let text = String::from_utf8(added.clone()).unwrap();
        assert!(
            text.contains(
                "to=\"Acme.Line\"></Connection>
                <Connection
                 connectionType=\"Child\"
                 from=\"Acme.Line\"
                 to=\"Acme.Machine\"></Connection>
            </Connections>"
            ),
            "{text}"
        );
        let reparsed = parse(&added).unwrap();
        assert_eq!(reparsed.connections.len(), 3);
        let removed = remove(&added, &reparsed, "Acme.Line", "Acme.Machine").unwrap();
        assert_eq!(String::from_utf8(removed).unwrap(), EXPORT);
    }

    #[test]
    fn the_empty_export_form_takes_a_first_connection_and_gets_it_back_when_emptied() {
        let empty = "<Entities>
    <Networks>
        <Network name=\"Acme.Fleet\" projectName=\"Acme\">
            <ConfigurationTables></ConfigurationTables>
            <Connections></Connections>
        </Network>
    </Networks>
</Entities>
";
        let network = parse(empty.as_bytes()).unwrap();
        let added = add(empty.as_bytes(), &network, "Contains", "", "Acme.Site").unwrap();
        let text = String::from_utf8(added.clone()).unwrap();
        assert!(
            text.contains(
                "            <Connections>
                <Connection
                 connectionType=\"Contains\"
                 from=\"\"
                 to=\"Acme.Site\"></Connection>
            </Connections>
        </Network>"
            ),
            "{text}"
        );
        let removed = remove(&added, &parse(&added).unwrap(), "", "Acme.Site").unwrap();
        assert_eq!(String::from_utf8(removed).unwrap(), empty);
    }

    #[test]
    fn removing_a_subtree_leaves_no_blank_line() {
        let network = parse(EXPORT.as_bytes()).unwrap();
        let children = BTreeSet::from(["Acme.Line".to_string()]);
        let removed =
            String::from_utf8(remove_children(EXPORT.as_bytes(), &network, &children).unwrap())
                .unwrap();
        assert!(!removed.contains("Acme.Line"), "{removed}");
        assert!(
            removed.contains(
                "to=\"Acme.Site\"></Connection>
            </Connections>"
            ),
            "{removed}"
        );
    }

    #[test]
    fn asset_csv_accepts_bom_crlf_quotes_and_embedded_newlines() {
        let rows = assets_csv(b"\xef\xbb\xbfname,parent,type,description\r\nAcme.Site,,Contains,\"site, primary\"\r\nAcme.Line,Acme.Site,,\"line one\r\nline two\"\r\n").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].description, "site, primary");
        assert_eq!(rows[1].connection_type, "Contains");
        assert_eq!(rows[1].description, "line one\r\nline two");
    }

    #[test]
    fn asset_csv_refuses_bad_names_and_columns_at_their_lines() {
        assert!(assets_csv(b"name,parent,unknown\nAcme.Site,,x\n")
            .unwrap_err()
            .to_string()
            .contains("unknown column"));
        assert!(assets_csv(b"name,parent\n,Acme.Site\n")
            .unwrap_err()
            .to_string()
            .contains("line 2"));
        // A spreadsheet saves a blank row as empty fields: no row, not an error.
        assert_eq!(
            assets_csv(b"name,parent\nAcme.Site,\n,\n\nAcme.Line,Acme.Site\n")
                .unwrap()
                .len(),
            2
        );
        assert!(assets_csv(b"name,parent\nAcme.Site,\nAcme.Site,\n")
            .unwrap_err()
            .to_string()
            .contains("line 3"));
    }

    #[test]
    fn adding_to_a_self_closing_network_opens_it() {
        let src = b"<Entities><Networks><Network name=\"Acme.N\" projectName=\"Acme\"/></Networks></Entities>";
        let network = parse(src).unwrap();
        let out = add(src, &network, "Contains", "", "Acme.Site").unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(
            text.contains("projectName=\"Acme\"><Connections>"),
            "{text}"
        );
        assert!(
            text.ends_with("</Connections></Network></Networks></Entities>"),
            "{text}"
        );
        assert_eq!(parse(&out).unwrap().connections[0].to, "Acme.Site");
    }

    #[test]
    fn removing_connections_that_share_a_line_leaves_no_blank_line() {
        let pair = "    <Connection connectionType=\"Contains\" from=\"\" to=\"Acme.A\"/><Connection connectionType=\"Contains\" from=\"Acme.A\" to=\"Acme.B\"/>\n";
        let src = format!("<Entities>\n<Networks>\n<Network name=\"Acme.N\" projectName=\"Acme\">\n  <Connections>\n{pair}    <Connection connectionType=\"Contains\" from=\"\" to=\"Acme.C\"/>\n  </Connections>\n</Network>\n</Networks>\n</Entities>\n");
        let network = parse(src.as_bytes()).unwrap();
        let both = BTreeSet::from(["Acme.A".to_string(), "Acme.B".to_string()]);
        let out = remove_children(src.as_bytes(), &network, &both).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), src.replace(pair, ""));
        // Removing one of the two keeps the other on its line.
        let one = remove_children(
            src.as_bytes(),
            &network,
            &BTreeSet::from(["Acme.B".to_string()]),
        )
        .unwrap();
        assert!(String::from_utf8(one)
            .unwrap()
            .contains("    <Connection connectionType=\"Contains\" from=\"\" to=\"Acme.A\"/>\n"));
    }
}
