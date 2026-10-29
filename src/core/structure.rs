//! A deliberately small XSD reader for validating exported entity XML.
//!
//! The platform owns its schema, so this module reads an operator-provided copy at run time and
//! never embeds it. It accepts only the subset the export schema uses and refuses to guess at
//! declarations it cannot model.

use super::config::Solution;
use super::normalise::LIVE_ONLY_ELEMENTS;
use super::scan::{self, Kind};
use super::workspace;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

const JAR_ENTRY: &str = "com/thingworx/documents/schema.xsd";

pub(crate) struct Report {
    pub(crate) examined: usize,
    pub(crate) findings: Vec<Finding>,
    pub(crate) notes: Vec<String>,
    pub(crate) broken: Option<String>,
    pub(crate) skipped: Option<String>,
}

pub(crate) struct Finding {
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) kind: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug)]
struct Node {
    name: String,
    attrs: BTreeMap<String, String>,
    children: Vec<Node>,
    text: String,
    line: usize,
}

#[derive(Clone, Debug, Default)]
struct Decl {
    name: String,
    attrs: BTreeMap<String, Attr>,
    particles: Vec<Particle>,
    text: Option<Simple>,
    mixed: bool,
    any: bool,
}

#[derive(Clone, Debug)]
struct Attr {
    required: bool,
    simple: Option<Simple>,
}

#[derive(Clone, Debug)]
struct Particle {
    decl: Decl,
    min: usize,
    max: Option<usize>,
    choice: bool,
}

#[derive(Clone, Copy, Debug)]
enum Simple {
    String,
    Boolean,
    DateTime,
    Decimal,
    Int,
    UnsignedInt,
    Byte,
    Float,
}

#[derive(Default)]
struct Schema {
    elements: BTreeMap<String, Node>,
    types: BTreeMap<String, Node>,
}

/// Run without a profile or server. A missing source is normal; a requested bad source is not.
pub(crate) fn run(solution: &Solution, discovered: &workspace::Discovery) -> Report {
    // An empty value, the documented `structure_xsd = ""` or an exported empty variable, means
    // unset rather than the solution folder.
    let source = std::env::var_os("TWACO_STRUCTURE_XSD")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            solution
                .gates
                .structure_xsd
                .as_ref()
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                solution.root.join(path)
            }
        });
    let Some(source) = source else {
        return Report {
            examined: 0,
            findings: vec![],
            notes: vec!["structure skipped: set gates.structure_xsd or TWACO_STRUCTURE_XSD".into()],
            broken: None,
            skipped: Some("no XSD source configured".into()),
        };
    };
    let bytes = match schema_bytes(&source) {
        Ok(bytes) => bytes,
        Err(why) => {
            return Report {
                examined: 0,
                findings: vec![],
                notes: vec![],
                broken: Some(why),
                skipped: None,
            }
        }
    };
    let schema = match parse_xml(&bytes, true).and_then(Schema::from_root) {
        Ok(schema) => schema,
        Err(why) => {
            return Report {
                examined: 0,
                findings: vec![],
                notes: vec![],
                broken: Some(format!("could not read the structure XSD: {why}")),
                skipped: None,
            }
        }
    };
    let root = match schema.element("Entities") {
        Ok(root) => root,
        Err(why) => {
            return Report {
                examined: 0,
                findings: vec![],
                notes: vec![],
                broken: Some(format!("could not model the structure XSD: {why}")),
                skipped: None,
            }
        }
    };
    let mut report = Report {
        examined: 0,
        findings: vec![],
        notes: vec![],
        broken: None,
        skipped: None,
    };
    let mut unknown_collections = BTreeSet::new();
    for file in &discovered.entities {
        let bytes = match std::fs::read(&file.path) {
            Ok(bytes) => bytes,
            Err(error) => {
                report.findings.push(Finding {
                    file: solution.relative(&file.path),
                    line: 0,
                    kind: "unreadable".into(),
                    message: format!("could not be read: {error}"),
                });
                continue;
            }
        };
        let node = match parse_xml(&bytes, false) {
            Ok(node) => node,
            Err(error) => {
                report.findings.push(Finding {
                    file: solution.relative(&file.path),
                    line: 0,
                    kind: "invalid XML".into(),
                    message: error,
                });
                continue;
            }
        };
        report.examined += 1;
        if node.name != root.name {
            report.findings.push(Finding {
                file: solution.relative(&file.path),
                line: node.line,
                kind: "unexpected child".into(),
                message: format!(
                    "/{}: the document element is not <{}>",
                    node.name, root.name
                ),
            });
            continue;
        }
        validate(
            &root,
            &node,
            &mut Vec::new(),
            &solution.relative(&file.path),
            &mut report.findings,
            &mut unknown_collections,
            true,
        );
    }
    if !unknown_collections.is_empty() {
        report.notes.push(format!(
            "structure skipped unknown Entities collections: {}",
            unknown_collections
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    report
}

fn schema_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let path = path.to_path_buf();
    if path.is_dir() {
        let found = find_schema(&path, 0)
            .ok_or_else(|| format!("no schema.xsd or plugin jar below {}", path.display()))?;
        return schema_bytes(&found);
    }
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("xsd") => {
            std::fs::read(&path).map_err(|e| format!("could not read {}: {e}", path.display()))
        }
        Some("jar") => {
            let file = std::fs::File::open(&path)
                .map_err(|e| format!("could not read {}: {e}", path.display()))?;
            let mut archive = zip::ZipArchive::new(file)
                .map_err(|e| format!("{} is not a readable jar: {e}", path.display()))?;
            let mut entry = archive
                .by_name(JAR_ENTRY)
                .map_err(|_| format!("{} has no {JAR_ENTRY}", path.display()))?;
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("could not read {JAR_ENTRY}: {e}"))?;
            Ok(bytes)
        }
        _ => Err(format!(
            "{} is not an .xsd file, .jar file, or directory",
            path.display()
        )),
    }
}

/// The first `schema.xsd`, or jar holding the schema entry, below `dir`: nearer ones first, then
/// by name. Jars without the entry (most of a plugins folder) are passed over.
fn find_schema(dir: &Path, depth: usize) -> Option<PathBuf> {
    if depth > 8 {
        return None;
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in &entries {
        if entry.path().is_file()
            && entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("schema.xsd")
        {
            return Some(entry.path());
        }
    }
    for entry in &entries {
        let p = entry.path();
        if p.is_file()
            && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jar"))
            && jar_has_schema(&p)
        {
            return Some(p);
        }
    }
    entries
        .into_iter()
        .filter(|e| e.path().is_dir())
        .find_map(|e| find_schema(&e.path(), depth + 1))
}

fn jar_has_schema(path: &Path) -> bool {
    std::fs::File::open(path)
        .ok()
        .and_then(|file| zip::ZipArchive::new(file).ok())
        .is_some_and(|mut archive| archive.by_name(JAR_ENTRY).is_ok())
}

impl Schema {
    fn from_root(root: Node) -> Result<Self, String> {
        if root.name != "schema" {
            return Err(format!("root is <{}>, not <schema>", root.name));
        }
        let mut schema = Schema::default();
        for node in root.children {
            match node.name.as_str() {
                "element" => {
                    let name = required(&node, "name")?;
                    if node.attrs.contains_key("substitutionGroup") {
                        return Err("substitutionGroup is unsupported".into());
                    }
                    schema.elements.insert(name.into(), node);
                }
                "complexType" => {
                    let name = required(&node, "name")?;
                    schema.types.insert(name.into(), node);
                }
                "annotation" => {}
                "import" | "include" | "group" | "attributeGroup" => {
                    return Err(format!("<{}> is unsupported", node.name))
                }
                _ => return Err(format!("unsupported schema declaration <{}>", node.name)),
            }
        }
        Ok(schema)
    }
    fn element(&self, name: &str) -> Result<Decl, String> {
        self.global(name, &mut Vec::new())
    }
    /// A global element, resolved in full. References and named types share one stack, so a
    /// declaration that contains itself is refused rather than expanded forever.
    fn global(&self, name: &str, stack: &mut Vec<String>) -> Result<Decl, String> {
        let node = self
            .elements
            .get(name)
            .ok_or_else(|| format!("global element {name:?} is not declared"))?;
        let key = format!("element {name}");
        if stack.contains(&key) {
            return Err(format!("declaration cycle through {name}"));
        }
        stack.push(key);
        let decl = self.decl(node, stack);
        stack.pop();
        decl
    }
    fn decl(&self, node: &Node, stack: &mut Vec<String>) -> Result<Decl, String> {
        if let Some(reference) = node.attrs.get("ref") {
            return self.global(local(reference), stack);
        }
        let name = required(node, "name")?.to_string();
        let mut decl = Decl {
            name,
            ..Decl::default()
        };
        if let Some(ty) = node.attrs.get("type") {
            self.merge_type(&mut decl, ty, stack)?;
        }
        for child in &node.children {
            match child.name.as_str() {
                "complexType" => self.merge_complex(&mut decl, child, stack)?,
                "simpleType" => decl.text = simple_type(child)?,
                "annotation" => {}
                other => return Err(format!("unsupported element child <{other}>")),
            }
        }
        Ok(decl)
    }
    fn merge_type(&self, decl: &mut Decl, ty: &str, stack: &mut Vec<String>) -> Result<(), String> {
        if let Some(simple) = simple(ty) {
            decl.text = Some(simple);
            return Ok(());
        }
        let name = local(ty).to_string();
        if stack.contains(&name) {
            return Err(format!("declaration cycle through {name}"));
        }
        let node = self
            .types
            .get(&name)
            .ok_or_else(|| format!("unsupported named type {ty}"))?;
        stack.push(name);
        self.merge_complex(decl, node, stack)?;
        stack.pop();
        Ok(())
    }
    fn merge_complex(
        &self,
        decl: &mut Decl,
        node: &Node,
        stack: &mut Vec<String>,
    ) -> Result<(), String> {
        if node.attrs.get("mixed").is_some_and(|v| v == "true") {
            decl.mixed = true;
        }
        for child in &node.children {
            match child.name.as_str() {
                "sequence" | "all" | "choice" => {
                    let particles = self.particles(child, false, &mut decl.any, stack)?;
                    decl.particles.extend(particles);
                }
                "attribute" => {
                    let name = required(child, "name")?.to_string();
                    decl.attrs.insert(
                        name,
                        Attr {
                            required: child.attrs.get("use").is_some_and(|v| v == "required"),
                            simple: child.attrs.get("type").and_then(|t| simple(t)),
                        },
                    );
                }
                "anyAttribute" => return Err("<anyAttribute> is unsupported".into()),
                "simpleContent" => self.simple_content(decl, child, stack)?,
                "complexContent" => self.complex_content(decl, child, stack)?,
                "annotation" => {}
                other => return Err(format!("unsupported complexType child <{other}>")),
            }
        }
        Ok(())
    }
    fn simple_content(
        &self,
        decl: &mut Decl,
        node: &Node,
        stack: &mut Vec<String>,
    ) -> Result<(), String> {
        let extension = node
            .children
            .iter()
            .find(|n| n.name == "extension")
            .ok_or("simpleContent without extension")?;
        self.merge_type(decl, required(extension, "base")?, stack)?;
        self.extension(decl, extension, stack)?;
        Ok(())
    }
    fn complex_content(
        &self,
        decl: &mut Decl,
        node: &Node,
        stack: &mut Vec<String>,
    ) -> Result<(), String> {
        let extension = node
            .children
            .iter()
            .find(|n| n.name == "extension")
            .ok_or("complexContent without extension")?;
        self.merge_type(decl, required(extension, "base")?, stack)?;
        self.extension(decl, extension, stack)
    }
    fn extension(
        &self,
        decl: &mut Decl,
        extension: &Node,
        stack: &mut Vec<String>,
    ) -> Result<(), String> {
        for child in &extension.children {
            match child.name.as_str() {
                "sequence" | "all" | "choice" => {
                    let particles = self.particles(child, false, &mut decl.any, stack)?;
                    decl.particles.extend(particles);
                }
                "attribute" => {
                    let name = required(child, "name")?.to_string();
                    decl.attrs.insert(
                        name,
                        Attr {
                            required: child.attrs.get("use").is_some_and(|v| v == "required"),
                            simple: child.attrs.get("type").and_then(|t| simple(t)),
                        },
                    );
                }
                "annotation" => {}
                other => return Err(format!("unsupported extension child <{other}>")),
            }
        }
        Ok(())
    }
    /// The elements a compositor allows, its own occurrences folded into theirs: a
    /// `<choice maxOccurs="unbounded">` of one element allows that element any number of times.
    /// Which alternative of a choice is taken is not checked, only how often each may appear.
    fn particles(
        &self,
        node: &Node,
        within_choice: bool,
        any: &mut bool,
        stack: &mut Vec<String>,
    ) -> Result<Vec<Particle>, String> {
        let choice = within_choice || node.name == "choice";
        let (group_min, group_max) = (occurs(node, "minOccurs", 1)?, max_occurs(node)?);
        let mut out = Vec::new();
        for child in &node.children {
            match child.name.as_str() {
                "element" => out.push(Particle {
                    decl: self.decl(child, stack)?,
                    min: occurs(child, "minOccurs", 1)?,
                    max: max_occurs(child)?,
                    choice,
                }),
                "any" => {
                    if child
                        .attrs
                        .get("processContents")
                        .is_some_and(|v| v != "lax")
                    {
                        return Err("xs:any without processContents=lax is unsupported".into());
                    }
                    *any = true;
                }
                "sequence" | "all" | "choice" => {
                    out.extend(self.particles(child, choice, any, stack)?)
                }
                "annotation" => {}
                other => return Err(format!("unsupported compositor child <{other}>")),
            }
        }
        for particle in &mut out {
            particle.min *= group_min;
            particle.max = match (particle.max, group_max) {
                (Some(own), Some(group)) => Some(own * group),
                _ => None,
            };
        }
        Ok(out)
    }
}

fn validate(
    expected: &Decl,
    actual: &Node,
    path: &mut Vec<String>,
    file: &str,
    findings: &mut Vec<Finding>,
    unknown_collections: &mut BTreeSet<String>,
    entities_root: bool,
) {
    path.push(actual.name.clone());
    let shown = format!("/{}", path.join("/"));
    for (name, attr) in &actual.attrs {
        if name.starts_with("aspect.") || expected.attrs.contains_key(name) {
            if let Some(simple) = expected.attrs.get(name).and_then(|a| a.simple) {
                if !valid(simple, attr) {
                    findings.push(finding(
                        file,
                        actual.line,
                        "bad value",
                        format!("{shown}/@{name}: expected {}", type_name(simple)),
                        name,
                        attr,
                    ));
                }
            }
            continue;
        }
        findings.push(finding(
            file,
            actual.line,
            "unknown attribute",
            format!("{shown}/@{name}"),
            name,
            "",
        ));
    }
    for (name, attr) in &expected.attrs {
        if attr.required && !actual.attrs.contains_key(name) {
            findings.push(Finding {
                file: file.into(),
                line: actual.line,
                kind: "missing required attribute".into(),
                message: format!("{shown}: missing @{name}"),
            });
        }
    }
    if !expected.mixed && expected.text.is_none() && !actual.text.trim().is_empty() {
        findings.push(Finding {
            file: file.into(),
            line: actual.line,
            kind: "text not allowed".into(),
            message: shown.clone(),
        });
    }
    if let Some(simple) = expected.text {
        let value = actual.text.trim();
        if !value.is_empty() && !valid(simple, value) {
            findings.push(finding(
                file,
                actual.line,
                "bad value",
                format!("{shown}: expected {}", type_name(simple)),
                "",
                value,
            ));
        }
    }
    let entity_child = path.len() == 3;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for child in &actual.children {
        if entities_root
            && child.name != "ExtensionPackages"
            && child.name != "Entities"
            && !expected.particles.iter().any(|p| p.decl.name == child.name)
        {
            unknown_collections.insert(child.name.clone());
            continue;
        }
        // A 10.2 export also writes an empty <Subscriptions/> straight under a Thing, beside the
        // ThingShape's own (observed on build b75); the 9.1 schema has no place for it.
        if entity_child
            && (LIVE_ONLY_ELEMENTS.contains(&child.name.as_bytes())
                || child.name.starts_with("effective")
                || (child.name == "Subscriptions"
                    && child.children.is_empty()
                    && child.text.trim().is_empty()))
        {
            continue;
        }
        let candidates: Vec<_> = expected
            .particles
            .iter()
            .filter(|p| p.decl.name == child.name)
            .collect();
        if candidates.is_empty() {
            // `xs:any processContents="lax"`: what the schema does not declare goes unchecked.
            if expected.any {
                continue;
            }
            findings.push(Finding {
                file: file.into(),
                line: child.line,
                kind: "unexpected child".into(),
                message: format!("{shown}/{}", child.name),
            });
            continue;
        }
        *counts.entry(&child.name).or_default() += 1;
        validate(
            &candidates[0].decl,
            child,
            path,
            file,
            findings,
            unknown_collections,
            false,
        );
    }
    if !((actual.name == "Entities" || actual.name == "ExtensionPackages")
        && actual.children.is_empty())
    {
        for particle in &expected.particles {
            let count = counts
                .get(particle.decl.name.as_str())
                .copied()
                .unwrap_or(0);
            let min = if particle.choice { 0 } else { particle.min };
            if count < min {
                findings.push(Finding {
                    file: file.into(),
                    line: actual.line,
                    kind: "missing required child".into(),
                    message: format!("{shown}: missing <{}>", particle.decl.name),
                });
            }
            if particle.max.is_some_and(|max| count > max) {
                findings.push(Finding {
                    file: file.into(),
                    line: actual.line,
                    kind: "too many".into(),
                    message: format!("{shown}: too many <{}>", particle.decl.name),
                });
            }
        }
    }
    path.pop();
}

fn finding(
    file: &str,
    line: usize,
    kind: &str,
    message: String,
    attr: &str,
    value: &str,
) -> Finding {
    let secret =
        attr.to_ascii_lowercase().contains("password") || attr.to_ascii_lowercase().contains("key");
    let shown: String = value.chars().take(40).collect();
    Finding {
        file: file.into(),
        line,
        kind: kind.into(),
        message: if value.is_empty() || secret {
            message
        } else {
            format!("{message}; got {shown:?}")
        },
    }
}
fn valid(simple: Simple, value: &str) -> bool {
    match simple {
        Simple::String | Simple::DateTime => true,
        Simple::Boolean => matches!(value, "true" | "false" | "0" | "1"),
        Simple::Decimal => value.parse::<f64>().is_ok(),
        Simple::Int => value.parse::<i32>().is_ok(),
        Simple::UnsignedInt => value.parse::<u32>().is_ok(),
        Simple::Byte => value.parse::<i8>().is_ok(),
        Simple::Float => value.parse::<f32>().is_ok(),
    }
}
fn type_name(simple: Simple) -> &'static str {
    match simple {
        Simple::String => "xs:string",
        Simple::Boolean => "xs:boolean",
        Simple::DateTime => "xs:dateTime",
        Simple::Decimal => "xs:decimal",
        Simple::Int => "xs:int",
        Simple::UnsignedInt => "xs:unsignedInt",
        Simple::Byte => "xs:byte",
        Simple::Float => "xs:float",
    }
}
fn simple(name: &str) -> Option<Simple> {
    match local(name) {
        "string" => Some(Simple::String),
        "boolean" => Some(Simple::Boolean),
        "dateTime" => Some(Simple::DateTime),
        "decimal" => Some(Simple::Decimal),
        "int" => Some(Simple::Int),
        "unsignedInt" => Some(Simple::UnsignedInt),
        "byte" => Some(Simple::Byte),
        "float" => Some(Simple::Float),
        _ => None,
    }
}
fn simple_type(node: &Node) -> Result<Option<Simple>, String> {
    let restriction = node
        .children
        .iter()
        .find(|n| n.name == "restriction")
        .ok_or("simpleType without restriction")?;
    Ok(simple(required(restriction, "base")?))
}
fn occurs(node: &Node, name: &str, default: usize) -> Result<usize, String> {
    node.attrs.get(name).map_or(Ok(default), |v| {
        v.parse().map_err(|_| format!("invalid {name}={v:?}"))
    })
}
fn max_occurs(node: &Node) -> Result<Option<usize>, String> {
    match node.attrs.get("maxOccurs").map(String::as_str) {
        None => Ok(Some(1)),
        Some("unbounded") => Ok(None),
        Some(value) => value
            .parse()
            .map(Some)
            .map_err(|_| format!("invalid maxOccurs={value:?}")),
    }
}
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}
fn required<'a>(node: &'a Node, name: &str) -> Result<&'a str, String> {
    node.attrs
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("<{}> needs @{name}", node.name))
}

/// The document as a tree. A schema's own `xs:` prefixes are dropped; an entity document keeps its
/// names as written, so a prefixed element is not taken for the plain one the schema declares.
fn parse_xml(bytes: &[u8], schema: bool) -> Result<Node, String> {
    let name_of = |raw: &str| if schema { local(raw) } else { raw }.to_string();
    let scanned = scan::scan(bytes).map_err(|e| e.to_string())?;
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let mut stack: Vec<Node> = vec![];
    let mut root = None;
    // Tokens come in document order, so the line count only ever moves forward.
    let (mut line, mut counted) = (1, 0);
    for token in &scanned.tokens {
        let name = name_of(&text[token.name.start..token.name.end]);
        line += bytes[counted..token.span.start]
            .iter()
            .filter(|b| **b == b'\n')
            .count();
        counted = token.span.start;
        match token.kind {
            Kind::Start => stack.push(Node {
                name,
                attrs: attributes(bytes, token, schema)?,
                children: vec![],
                text: String::new(),
                line,
            }),
            Kind::Empty => {
                let node = Node {
                    name,
                    attrs: attributes(bytes, token, schema)?,
                    children: vec![],
                    text: String::new(),
                    line,
                };
                push_node(node, &mut stack, &mut root)?;
            }
            Kind::End => {
                let node = stack
                    .pop()
                    .ok_or("closing element without opening element")?;
                if node.name != name {
                    return Err(format!("<{}> closed by </{name}>", node.name));
                }
                push_node(node, &mut stack, &mut root)?;
            }
            Kind::Text | Kind::Cdata => {
                if let Some(node) = stack.last_mut() {
                    let raw = if token.kind == Kind::Cdata {
                        &text[token.inner.start..token.inner.end]
                    } else {
                        &text[token.span.start..token.span.end]
                    };
                    node.text.push_str(&scan::decode_entities(raw));
                }
            }
            Kind::Pi | Kind::Comment | Kind::DocType => {}
        }
    }
    if !stack.is_empty() {
        return Err("element is never closed".into());
    }
    root.ok_or_else(|| "no root element".into())
}
fn push_node(node: Node, stack: &mut [Node], root: &mut Option<Node>) -> Result<(), String> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
        Ok(())
    } else if root.is_none() {
        *root = Some(node);
        Ok(())
    } else {
        Err("a second root element".into())
    }
}
fn attributes(
    bytes: &[u8],
    token: &scan::Token,
    schema: bool,
) -> Result<BTreeMap<String, String>, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let mut attrs = BTreeMap::new();
    for attr in scan::attributes(bytes, token).map_err(|e| e.to_string())? {
        let name = &text[attr.name.start..attr.name.end];
        if name == "xmlns" || name.starts_with("xmlns:") {
            continue;
        }
        attrs.insert(
            if schema { local(name) } else { name }.to_string(),
            scan::decode_entities(&text[attr.value.start..attr.value.end]),
        );
    }
    Ok(attrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XSD: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
      <xs:element name="Entities"><xs:complexType><xs:sequence><xs:element name="Things"><xs:complexType><xs:all><xs:element ref="Thing" minOccurs="0" maxOccurs="unbounded"/></xs:all></xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element>
      <xs:element name="Thing" type="ThingType"/>
      <xs:complexType name="ThingType"><xs:sequence><xs:element name="Needed"/><xs:element name="One" minOccurs="0"/></xs:sequence><xs:attribute name="enabled" type="xs:boolean" use="required"/></xs:complexType>
    </xs:schema>"#;

    #[test]
    fn scanner_reader_and_model_report_the_structural_kinds() {
        let schema = Schema::from_root(parse_xml(XSD.as_bytes(), true).unwrap()).unwrap();
        let expected = schema.element("Entities").unwrap();
        let actual = parse_xml(b"<Entities><Things><Thing enabled='not-a-bool' surprise='x'>text<One/><One/><Else/></Thing></Things></Entities>", false).unwrap();
        let mut findings = Vec::new();
        validate(
            &expected,
            &actual,
            &mut Vec::new(),
            "T.xml",
            &mut findings,
            &mut BTreeSet::new(),
            true,
        );
        let kinds: BTreeSet<_> = findings.iter().map(|f| f.kind.as_str()).collect();
        assert!(kinds.contains("unknown attribute"));
        assert!(kinds.contains("missing required child"));
        assert!(kinds.contains("too many"));
        assert!(kinds.contains("unexpected child"));
        assert!(kinds.contains("text not allowed"));
        assert!(kinds.contains("bad value"));
    }

    fn findings_for(xml: &str) -> Vec<String> {
        let schema = Schema::from_root(parse_xml(XSD.as_bytes(), true).unwrap()).unwrap();
        let mut findings = Vec::new();
        validate(
            &schema.element("Entities").unwrap(),
            &parse_xml(xml.as_bytes(), false).unwrap(),
            &mut Vec::new(),
            "T.xml",
            &mut findings,
            &mut BTreeSet::new(),
            true,
        );
        findings
            .into_iter()
            .map(|f| format!("{}:{} {}", f.line, f.kind, f.message))
            .collect()
    }

    #[test]
    fn a_missing_attribute_and_lines_are_reported_and_a_10_2_subscriptions_is_allowed() {
        assert_eq!(
            findings_for(
                "<Entities>\n<Things>\n<Thing>\n<Needed/>\n<Bad/>\n</Thing></Things></Entities>"
            ),
            [
                "3:missing required attribute /Entities/Things/Thing: missing @enabled",
                "5:unexpected child /Entities/Things/Thing/Bad",
            ]
        );
        // Empty, straight under the entity: what a 10.2 export writes. Anything more is not.
        assert!(findings_for(
            "<Entities><Things><Thing enabled='true'><Needed/><Subscriptions/></Thing></Things></Entities>"
        )
        .is_empty());
        assert_eq!(
            findings_for(
                "<Entities><Things><Thing enabled='true'><Needed/><Subscriptions><Subscription/></Subscriptions></Thing></Things></Entities>"
            ),
            ["1:unexpected child /Entities/Things/Thing/Subscriptions"]
        );
    }

    const OPEN: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
      <xs:element name="Entities"><xs:complexType><xs:all>
        <xs:element name="Mashup" minOccurs="0"><xs:complexType><xs:all>
          <xs:element name="Things" minOccurs="0"><xs:complexType>
            <xs:choice minOccurs="0" maxOccurs="unbounded"><xs:element name="Thing" type="xs:string"/></xs:choice>
          </xs:complexType></xs:element>
          <xs:element name="Open" minOccurs="0"><xs:complexType><xs:sequence>
            <xs:element name="Known" type="xs:boolean"/>
            <xs:any minOccurs="0" maxOccurs="unbounded" processContents="lax"/>
          </xs:sequence></xs:complexType></xs:element>
        </xs:all></xs:complexType></xs:element>
      </xs:all></xs:complexType></xs:element>
    </xs:schema>"#;

    fn open_findings(xml: &str) -> Vec<String> {
        let schema = Schema::from_root(parse_xml(OPEN.as_bytes(), true).unwrap()).unwrap();
        let mut findings = Vec::new();
        validate(
            &schema.element("Entities").unwrap(),
            &parse_xml(xml.as_bytes(), false).unwrap(),
            &mut Vec::new(),
            "T.xml",
            &mut findings,
            &mut BTreeSet::new(),
            false,
        );
        findings
            .into_iter()
            .map(|f| format!("{} {}", f.kind, f.message))
            .collect()
    }

    #[test]
    fn an_unbounded_choice_allows_many_and_a_wildcard_still_checks_what_it_declares() {
        assert!(open_findings(
            "<Entities><Mashup><Things><Thing>A</Thing><Thing>B</Thing><Thing>C</Thing></Things></Mashup></Entities>"
        )
        .is_empty());
        // Beside the wildcard, the declared child is still required and still typed.
        assert!(open_findings(
            "<Entities><Mashup><Open><Known>true</Known><Whatever/></Open></Mashup></Entities>"
        )
        .is_empty());
        assert_eq!(
            open_findings("<Entities><Mashup><Open><Whatever/></Open></Mashup></Entities>"),
            ["missing required child /Entities/Mashup/Open: missing <Known>"]
        );
        assert_eq!(
            open_findings(
                "<Entities><Mashup><Open><Known>maybe</Known></Open></Mashup></Entities>"
            ),
            ["bad value /Entities/Mashup/Open/Known: expected xs:boolean; got \"maybe\""]
        );
        // A prefixed element is not the plain one the schema declares.
        assert_eq!(
            open_findings("<Entities><x:Mashup xmlns:x='urn:x'/></Entities>"),
            ["unexpected child /Entities/x:Mashup"]
        );
    }

    #[test]
    fn a_directory_search_passes_over_jars_without_the_schema() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let jar = |path: &Path, entry: &str| {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
            zip.start_file(entry, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(OPEN.as_bytes()).unwrap();
            zip.finish().unwrap();
        };
        jar(&dir.path().join("a-unrelated.jar"), "META-INF/MANIFEST.MF");
        let deep = dir.path().join("p/l/u/g/i/n");
        std::fs::create_dir_all(&deep).unwrap();
        jar(&deep.join("z-plugin.jar"), JAR_ENTRY);
        assert_eq!(find_schema(dir.path(), 0), Some(deep.join("z-plugin.jar")));
        assert!(schema_bytes(dir.path()).unwrap().starts_with(b"<xs:schema"));
    }

    #[test]
    fn sensitive_bad_values_are_not_in_a_finding() {
        let finding = finding(
            "T.xml",
            1,
            "bad value",
            "expected xs:boolean".into(),
            "apiKey",
            "secret",
        );
        assert!(!finding.message.contains("secret"));
    }

    #[test]
    fn rejects_declaration_cycles_and_unsupported_constructs() {
        let cycle = br#"<schema><element name="A" type="T"/><complexType name="T"><complexContent><extension base="T"/></complexContent></complexType></schema>"#;
        let schema = Schema::from_root(parse_xml(cycle, true).unwrap()).unwrap();
        assert!(schema.element("A").unwrap_err().contains("cycle"));
        let refs = br#"<schema><element name="A"><complexType><sequence><element ref="A" minOccurs="0"/></sequence></complexType></element></schema>"#;
        let schema = Schema::from_root(parse_xml(refs, true).unwrap()).unwrap();
        assert!(schema.element("A").unwrap_err().contains("cycle"));
        let unsupported = br#"<schema><include schemaLocation="no"/></schema>"#;
        assert!(Schema::from_root(parse_xml(unsupported, true).unwrap()).is_err());
    }
}
