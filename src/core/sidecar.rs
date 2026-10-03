//! Turning an entity's service blocks into editable files and back.
//!
//! A service lives in two places in an entity export: a `ServiceDefinition` describing its
//! signature and a `ServiceImplementation` holding its script inside CDATA. The sidecar layout
//! splits them into `definition.xml` and `script.js`, which is what lets an editor, a linter and
//! `git diff` see a service as code instead of as a buried string.
//!
//! **A service that cannot be made into a sidecar is reported, not fatal.** Real entities carry
//! SQL services, and services whose definition is inherited from a ThingShape and only
//! *implemented* here. An extractor that treated either as an error would refuse to process the
//! entity at all, which is how the first version of this module behaved on three real files.

use super::scan::{self, Kind, ScanError, Token};
use std::collections::BTreeMap;
use std::fmt;

/// One service, as two files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSidecar {
    pub name: String,
    /// The `ServiceDefinition` block, trimmed and newline-terminated.
    pub definition: String,
    /// The script body, dedented, with surrounding blank lines removed.
    pub script: String,
}

/// One locally implemented script service, including an implementation whose definition is
/// inherited. Deploy parses these all; inheritance changes where the signature lives, not
/// whether bad JavaScript can break the entity after import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptService {
    pub name: String,
    pub script: String,
}

/// What an entity's service sections contained.
#[derive(Debug, Default)]
pub struct Extraction {
    /// Script services with both halves present here. These become sidecars.
    pub services: Vec<ServiceSidecar>,
    /// Implementations with a handler other than `Script`: SQL, and anything else.
    pub non_script: Vec<String>,
    /// Defined here but implemented elsewhere, or not at all.
    pub without_script: Vec<String>,
    /// Implemented here with the definition inherited from a shape or template. This is a
    /// legitimate override, not a missing definition.
    pub inherited: Vec<String>,
}

#[derive(Debug)]
pub enum SidecarError {
    Scan(ScanError),
    /// The document is not an entity export, so it has no entity element to look inside.
    NotAnEntity,
    Unnamed { what: &'static str, at: usize },
    Duplicate { what: &'static str, name: String },
    /// A `Script` implementation whose script cannot be located.
    NoScript { name: String },
    /// Several CDATA nodes in one `<code>`: writing one of them would truncate the script.
    ManyPayloads { name: String, count: usize },
    /// A sync would add or remove a service without having been told it may.
    StructuralChange { added: Vec<String>, removed: Vec<String> },
    Splice(super::splice::SpliceError),
}

impl fmt::Display for SidecarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SidecarError::Scan(e) => write!(f, "{e}"),
            SidecarError::NotAnEntity => write!(f, "not a ThingWorx entity export"),
            SidecarError::Unnamed { what, at } => {
                write!(f, "{what} at byte {at} has no name attribute")
            }
            SidecarError::Duplicate { what, name } => write!(f, "two {what} blocks named {name}"),
            SidecarError::NoScript { name } => {
                write!(f, "script service {name} has no <code> element under its Script table")
            }
            SidecarError::ManyPayloads { name, count } => write!(
                f,
                "service {name} has {count} CDATA payloads in one <code>; writing one would truncate it"
            ),
            SidecarError::StructuralChange { added, removed } => write!(
                f,
                "this would add {added:?} and remove {removed:?}; say --allow-add-remove to mean it"
            ),
            SidecarError::Splice(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SidecarError {}

impl From<ScanError> for SidecarError {
    fn from(e: ScanError) -> Self {
        SidecarError::Scan(e)
    }
}

/// Extract every script service an entity defines and implements itself.
///
/// The two sections are paired by their shared host element rather than by taking the first of
/// each in the document, so a definition is never paired with an unrelated implementation.
pub fn extract(src: &[u8]) -> Result<Extraction, SidecarError> {
    let tokens = scan::tokenize(src)?;
    let entity = entity_element(&tokens, src).ok_or(SidecarError::NotAnEntity)?;
    let host = member_host(&tokens, src, entity);

    let definitions = named_children(&tokens, src, host, "ServiceDefinitions", "ServiceDefinition")?;
    let implementations =
        named_children(&tokens, src, host, "ServiceImplementations", "ServiceImplementation")?;

    let mut out = Extraction::default();
    for (name, &index) in &implementations {
        if handler_of(&tokens, src, index)? != "Script" {
            out.non_script.push(name.clone());
            continue;
        }
        let Some(&definition) = definitions.get(name) else {
            out.inherited.push(name.clone());
            continue;
        };
        let script = script_of(&tokens, src, index, name)?;
        let block = scan::element_span(&tokens, definition)
            .ok_or(SidecarError::Unnamed { what: "ServiceDefinition", at: tokens[definition].span.start })?;
        out.services.push(ServiceSidecar {
            name: name.clone(),
            definition: definition_text(block.of(src)),
            script,
        });
    }
    for name in definitions.keys() {
        if !implementations.contains_key(name) {
            out.without_script.push(name.clone());
        }
    }
    out.services.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Backwards-compatible shorthand for callers that only want the sidecars.
pub fn extract_services(src: &[u8]) -> Result<Vec<ServiceSidecar>, SidecarError> {
    Ok(extract(src)?.services)
}

/// Extract every local `Script` implementation from one entity document.
pub fn script_services(src: &[u8]) -> Result<Vec<ScriptService>, SidecarError> {
    let tokens = scan::tokenize(src)?;
    let entity = entity_element(&tokens, src).ok_or(SidecarError::NotAnEntity)?;
    let host = member_host(&tokens, src, entity);
    let implementations =
        named_children(&tokens, src, host, "ServiceImplementations", "ServiceImplementation")?;
    let mut scripts = Vec::new();
    for (name, index) in implementations {
        if handler_of(&tokens, src, index)? == "Script" {
            scripts.push(ScriptService {
                script: script_of(&tokens, src, index, &name)?,
                name,
            });
        }
    }
    Ok(scripts)
}

/// The entity element: the first child of the first collection that has one.
///
/// An export can open with an empty collection — `<Entities><Things/><DataShapes>…` — so taking
/// the third tag in the document is not enough.
pub(crate) fn entity_element(tokens: &[Token], src: &[u8]) -> Option<usize> {
    let wrapper = tokens
        .iter()
        .position(|t| matches!(t.kind, Kind::Start) && t.name.of(src) == b"Entities")?;
    for collection in direct_children(tokens, wrapper) {
        if let Some(&entity) = direct_children(tokens, collection).first() {
            return Some(entity);
        }
    }
    None
}

/// Where an entity's own members live.
///
/// A `ThingShape` entity holds its sections directly. A `Thing` or `ThingTemplate` holds its own
/// members inside a nested, unnamed `<ThingShape>` — that element is not another entity, it is
/// how the platform models the entity's local shape. Exported ThingShapes have their sections at
/// the top level, while Things and ThingTemplates have theirs one level down.
fn member_host(tokens: &[Token], src: &[u8], entity: usize) -> usize {
    if !scan::child_tags(tokens, src, "ServiceDefinitions", entity).is_empty()
        || !scan::child_tags(tokens, src, "ServiceImplementations", entity).is_empty()
    {
        return entity;
    }
    scan::child_tags(tokens, src, "ThingShape", entity)
        .first()
        .copied()
        .unwrap_or(entity)
}

/// Indices of every direct child element of the element opening at `parent`.
fn direct_children(tokens: &[Token], parent: usize) -> Vec<usize> {
    let Some(end) = scan::element_end(tokens, parent) else { return Vec::new() };
    let mut out = Vec::new();
    let mut index = parent + 1;
    while index < end {
        match tokens[index].kind {
            Kind::Start => {
                out.push(index);
                index = scan::element_end(tokens, index).map_or(end, |e| e + 1);
            }
            Kind::Empty => {
                out.push(index);
                index += 1;
            }
            _ => index += 1,
        }
    }
    out
}

/// The element holding an entity's members, found from the document.
pub fn member_host_of(tokens: &[Token], src: &[u8]) -> Option<usize> {
    let entity = entity_element(tokens, src)?;
    Some(member_host(tokens, src, entity))
}

/// A service implementation's handler, defaulting to `Script` when it declares none.
pub fn handler_of(tokens: &[Token], src: &[u8], implementation: usize) -> Result<String, SidecarError> {
    Ok(scan::attribute(src, &tokens[implementation], "handlerName")?
        .map(|s| scan::decode_entities(&String::from_utf8_lossy(s.of(src))))
        .unwrap_or_else(|| "Script".to_string()))
}

/// The `<code>` element under an implementation's Script configuration table.
pub fn code_element_of(tokens: &[Token], src: &[u8], implementation: usize) -> Option<usize> {
    let tables = scan::child_tags(tokens, src, "ConfigurationTables", implementation);
    let script_table = tables
        .iter()
        .flat_map(|&t| scan::child_tags(tokens, src, "ConfigurationTable", t))
        .find(|&t| matches!(scan::attribute(src, &tokens[t], "name"), Ok(Some(v)) if v.of(src) == b"Script"));
    let root = script_table.unwrap_or(implementation);
    scan::tags_within(tokens, src, "code", root).into_iter().next()
}

/// Named blocks inside one direct-child section of the entity. Shared with `sync`.
pub fn named_children_of(
    tokens: &[Token],
    src: &[u8],
    host: usize,
    section: &'static str,
    block: &'static str,
) -> Result<BTreeMap<String, usize>, SidecarError> {
    named_children(tokens, src, host, section, block)
}

fn named_children(
    tokens: &[Token],
    src: &[u8],
    entity: usize,
    section: &'static str,
    block: &'static str,
) -> Result<BTreeMap<String, usize>, SidecarError> {
    let mut found = BTreeMap::new();
    let Some(&section_at) = scan::child_tags(tokens, src, section, entity).first() else {
        return Ok(found);
    };
    for index in scan::child_tags(tokens, src, block, section_at) {
        let name = scan::attribute(src, &tokens[index], "name")?
            .map(|s| scan::decode_entities(&String::from_utf8_lossy(s.of(src))))
            .filter(|n| !n.is_empty())
            .ok_or(SidecarError::Unnamed { what: block, at: tokens[index].span.start })?;
        if found.insert(name.clone(), index).is_some() {
            return Err(SidecarError::Duplicate { what: block, name });
        }
    }
    Ok(found)
}

/// The script inside one implementation's Script configuration table.
///
/// Walks to the `ConfigurationTable` named `Script` rather than taking the first `<code>`
/// anywhere below, and joins every CDATA and text node inside it, because a parser would
/// coalesce them and taking only the first would truncate the script.
pub(crate) fn script_of(
    tokens: &[Token],
    src: &[u8],
    implementation: usize,
    name: &str,
) -> Result<String, SidecarError> {
    let code = code_element_of(tokens, src, implementation)
        .ok_or_else(|| SidecarError::NoScript { name: name.to_string() })?;
    if tokens[code].kind == Kind::Empty {
        // `<code/>`: a service with no body, which is empty rather than missing.
        return Ok(String::new());
    }
    let end = scan::element_end(tokens, code)
        .ok_or_else(|| SidecarError::NoScript { name: name.to_string() })?;

    let mut payload = String::new();
    for token in &tokens[code + 1..end] {
        match token.kind {
            Kind::Cdata => payload.push_str(&String::from_utf8_lossy(token.inner.of(src))),
            Kind::Text => payload.push_str(&scan::decode_entities(&String::from_utf8_lossy(token.span.of(src)))),
            _ => {}
        }
    }
    Ok(trim_blank_edges(&dedent(&payload)))
}

/// A definition sidecar: the block, trimmed, with one trailing newline.
fn definition_text(block: &[u8]) -> String {
    let text = String::from_utf8_lossy(block).replace("\r\n", "\n");
    let mut out = text.trim().to_string();
    out.push('\n');
    out
}

/// Remove the longest whitespace prefix common to every non-blank line.
///
/// Matches the established `textwrap.dedent` semantics used by committed sidecars. Two
/// details are load-bearing and were verified against the real thing: only spaces and tabs count
/// as blank, so a line of vertical tabs is content and holds the margin at zero; and a tab is
/// not worth any number of spaces, so a tab-indented line and a space-indented one share no
/// prefix and nothing is removed.
pub fn dedent(text: &str) -> String {
    // XML normalises every line ending to a single newline before a parser sees it.
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();

    let mut margin: Option<&str> = None;
    for line in &lines {
        if is_blank(line) {
            continue;
        }
        let indent = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
        margin = Some(match margin {
            None => indent,
            Some(current) => common_prefix(current, indent),
        });
    }
    let margin = margin.unwrap_or("");

    lines
        .iter()
        .map(|line| {
            if is_blank(line) {
                ""
            } else {
                line.strip_prefix(margin).unwrap_or(line)
            }
        })
        .collect::<Vec<&str>>()
        .join("\n")
}

/// Blank in `textwrap` semantics means only spaces and tabs, not every Unicode space.
fn is_blank(line: &str) -> bool {
    line.chars().all(|c| c == ' ' || c == '\t')
}

fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let take = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    &a[..take]
}

/// Drop leading and trailing newlines, matching `.strip("\r\n")` semantics.
fn trim_blank_edges(text: &str) -> String {
    text.trim_matches(|c| c == '\n' || c == '\r').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTITY: &[u8] = br#"<Entities>
    <ThingShapes>
        <ThingShape name="My_TS" projectName="P">
            <ServiceDefinitions>
                <ServiceDefinition name="Beta" description="b"></ServiceDefinition>
                <ServiceDefinition name="Alpha" description="a"></ServiceDefinition>
            </ServiceDefinitions>
            <ServiceImplementations>
                <ServiceImplementation name="Beta" handlerName="Script">
                    <ConfigurationTables>
                        <ConfigurationTable name="Script">
                            <Rows><Row><code><![CDATA[
                                var b = 2;
                                result = b;
                            ]]></code></Row></Rows>
                        </ConfigurationTable>
                    </ConfigurationTables>
                </ServiceImplementation>
                <ServiceImplementation name="Alpha" handlerName="Script">
                    <ConfigurationTables>
                        <ConfigurationTable name="Script">
                            <Rows><Row><code><![CDATA[var a = 1;]]></code></Row></Rows>
                        </ConfigurationTable>
                    </ConfigurationTables>
                </ServiceImplementation>
            </ServiceImplementations>
        </ThingShape>
    </ThingShapes>
</Entities>"#;

    fn wrap(body: &str) -> Vec<u8> {
        format!("<Entities><Things><Thing name=\"T\">{body}</Thing></Things></Entities>").into_bytes()
    }

    #[test]
    fn services_come_back_named_and_sorted() {
        let names: Vec<String> = extract(ENTITY).unwrap().services.iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, vec!["Alpha", "Beta"]);
    }

    #[test]
    fn a_script_is_dedented_and_stripped() {
        let e = extract(ENTITY).unwrap();
        let beta = e.services.iter().find(|s| s.name == "Beta").unwrap();
        assert_eq!(beta.script, "var b = 2;\nresult = b;");
    }

    #[test]
    fn deploy_scripts_include_an_inherited_implementation() {
        let inherited = wrap(
            "<ServiceImplementations><ServiceImplementation name=\"Inherited\" handlerName=\"Script\">\
             <ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>\
             <code><![CDATA[result = 7;]]></code></Row></Rows></ConfigurationTable></ConfigurationTables>\
             </ServiceImplementation></ServiceImplementations>",
        );
        assert_eq!(
            script_services(&inherited).unwrap(),
            vec![ScriptService { name: "Inherited".to_string(), script: "result = 7;".to_string() }]
        );
    }

    #[test]
    fn indented_and_flush_left_payloads_extract_to_the_same_script() {
        let definition =
            "<ServiceDefinitions><ServiceDefinition name=\"S\"></ServiceDefinition></ServiceDefinitions>";
        let implementation = |payload: &str| {
            format!(
                "<ServiceImplementations><ServiceImplementation name=\"S\" handlerName=\"Script\">\
                 <ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row>\
                 <code><![CDATA[{payload}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables>\
                 </ServiceImplementation></ServiceImplementations>"
            )
        };
        let indented = wrap(&format!(
            "{definition}{}",
            implementation("\n            var a = 1;\n                result = a;\n            ")
        ));
        let flush = wrap(&format!(
            "{definition}{}",
            implementation("\nvar a = 1;\n    result = a;\n")
        ));

        let from_indented = extract(&indented).unwrap().services.remove(0).script;
        let from_flush = extract(&flush).unwrap().services.remove(0).script;
        assert_eq!(from_indented, "var a = 1;\n    result = a;");
        assert_eq!(from_flush, from_indented);
    }

    #[test]
    fn a_sql_service_is_reported_and_does_not_stop_the_others() {
        // Regression: a Database Thing with a SQLCommand service, and treating it as
        // a broken script aborted extraction of the whole entity.
        let body = r#"<ServiceDefinitions>
            <ServiceDefinition name="Js"></ServiceDefinition>
            <ServiceDefinition name="Sql"></ServiceDefinition>
        </ServiceDefinitions>
        <ServiceImplementations>
            <ServiceImplementation name="Js" handlerName="Script"><ConfigurationTables>
                <ConfigurationTable name="Script"><Rows><Row><code><![CDATA[ok();]]></code></Row></Rows>
            </ConfigurationTable></ConfigurationTables></ServiceImplementation>
            <ServiceImplementation name="Sql" handlerName="SQLCommand"><ConfigurationTables>
                <ConfigurationTable name="SQL"><Rows><Row><sql>DROP TABLE x</sql></Row></Rows>
            </ConfigurationTable></ConfigurationTables></ServiceImplementation>
        </ServiceImplementations>"#;
        let e = extract(&wrap(body)).unwrap();
        assert_eq!(e.services.len(), 1);
        assert_eq!(e.services[0].name, "Js");
        assert_eq!(e.non_script, vec!["Sql"]);
    }

    #[test]
    fn an_inherited_override_is_reported_not_an_error() {
        // A local implementation may inherit its definition from a shape.
        let body = r#"<ServiceDefinitions></ServiceDefinitions>
        <ServiceImplementations>
            <ServiceImplementation name="GetDBInfo" handlerName="Script"><ConfigurationTables>
                <ConfigurationTable name="Script"><Rows><Row><code><![CDATA[x();]]></code></Row></Rows>
            </ConfigurationTable></ConfigurationTables></ServiceImplementation>
        </ServiceImplementations>"#;
        let e = extract(&wrap(body)).unwrap();
        assert!(e.services.is_empty());
        assert_eq!(e.inherited, vec!["GetDBInfo"]);
    }

    #[test]
    fn an_empty_code_element_is_an_empty_script() {
        for body in [
            r#"<ServiceDefinitions><ServiceDefinition name="E"></ServiceDefinition></ServiceDefinitions>
               <ServiceImplementations><ServiceImplementation name="E" handlerName="Script">
               <ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code></code></Row></Rows>
               </ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>"#,
            r#"<ServiceDefinitions><ServiceDefinition name="E"></ServiceDefinition></ServiceDefinitions>
               <ServiceImplementations><ServiceImplementation name="E" handlerName="Script">
               <ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code/></Row></Rows>
               </ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>"#,
        ] {
            let e = extract(&wrap(body)).unwrap();
            assert_eq!(e.services.len(), 1, "empty code should still be a service");
            assert_eq!(e.services[0].script, "");
        }
    }

    #[test]
    fn two_cdata_nodes_in_one_code_element_are_joined() {
        let body = r#"<ServiceDefinitions><ServiceDefinition name="S"></ServiceDefinition></ServiceDefinitions>
        <ServiceImplementations><ServiceImplementation name="S" handlerName="Script">
        <ConfigurationTables><ConfigurationTable name="Script"><Rows><Row>
        <code><![CDATA[var a = 1;
]]><![CDATA[var b = 2;]]></code>
        </Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>"#;
        let e = extract(&wrap(body)).unwrap();
        assert_eq!(e.services[0].script, "var a = 1;\nvar b = 2;");
    }

    #[test]
    fn a_templates_services_live_in_its_nested_shape() {
        // A Thing or ThingTemplate keeps its own members inside an unnamed <ThingShape>. That
        // element is the entity's local shape, not another entity, so its services belong to
        // the enclosing entity.
        let src = br#"<Entities><ThingTemplates><ThingTemplate name="T_TT">
            <ThingShape>
                <ServiceDefinitions><ServiceDefinition name="Inner"></ServiceDefinition></ServiceDefinitions>
                <ServiceImplementations><ServiceImplementation name="Inner" handlerName="Script">
                    <ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code><![CDATA[inner();]]></code></Row></Rows>
                    </ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>
            </ThingShape>
        </ThingTemplate></ThingTemplates></Entities>"#;
        let e = extract(src).unwrap();
        let names: Vec<&str> = e.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Inner"]);
    }

    #[test]
    fn a_shape_entitys_own_sections_win_over_a_nested_shape() {
        // Management_TS holds its sections at the top level; nothing one level down should
        // shadow them.
        let src = br#"<Entities><ThingShapes><ThingShape name="S_TS">
            <ServiceDefinitions><ServiceDefinition name="Outer"></ServiceDefinition></ServiceDefinitions>
            <ServiceImplementations><ServiceImplementation name="Outer" handlerName="Script">
                <ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code><![CDATA[outer();]]></code></Row></Rows>
                </ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>
        </ThingShape></ThingShapes></Entities>"#;
        let e = extract(src).unwrap();
        assert_eq!(e.services.len(), 1);
        assert_eq!(e.services[0].name, "Outer");
    }

    #[test]
    fn an_entity_after_an_empty_collection_is_still_found() {
        let src = br#"<Entities><Things></Things><DataShapes><DataShape name="D" projectName="P">
            <ServiceDefinitions></ServiceDefinitions></DataShape></DataShapes></Entities>"#;
        assert!(extract(src).is_ok());
    }

    #[test]
    fn an_escaped_name_is_decoded() {
        let body = r#"<ServiceDefinitions><ServiceDefinition name="A&amp;B"></ServiceDefinition></ServiceDefinitions>
        <ServiceImplementations><ServiceImplementation name="A&amp;B" handlerName="Script">
        <ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code><![CDATA[x();]]></code></Row></Rows>
        </ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>"#;
        let e = extract(&wrap(body)).unwrap();
        assert_eq!(e.services[0].name, "A&B");
    }

    #[test]
    fn a_definition_without_an_implementation_is_reported() {
        let body = r#"<ServiceDefinitions><ServiceDefinition name="Lonely"></ServiceDefinition></ServiceDefinitions>
        <ServiceImplementations></ServiceImplementations>"#;
        let e = extract(&wrap(body)).unwrap();
        assert_eq!(e.without_script, vec!["Lonely"]);
        assert!(e.services.is_empty());
    }

    // --- dedent compatibility ---

    #[test]
    fn a_whitespace_only_line_is_emptied_and_ignored_for_the_margin() {
        assert_eq!(dedent("    a\n      \n    b\n"), "a\n\nb\n");
    }

    #[test]
    fn only_spaces_and_tabs_count_as_blank() {
        // A vertical tab is content under textwrap semantics, so the margin is zero and nothing is
        // stripped. Using Rust's `trim()` here would have dedented it.
        assert_eq!(dedent("  a\n\u{000b}\n  b"), "  a\n\u{000b}\n  b");
    }

    #[test]
    fn tabs_are_not_equal_to_spaces() {
        assert_eq!(dedent("\ta\n\tb\n"), "a\nb\n");
        assert_eq!(dedent("\ta\n    b\n"), "\ta\n    b\n");
    }

    #[test]
    fn a_line_at_the_margin_prevents_any_dedent() {
        assert_eq!(dedent("a\n    b\n"), "a\n    b\n");
    }

    #[test]
    fn a_common_margin_is_removed_and_deeper_indentation_kept() {
        assert_eq!(dedent("        x\n            y\n"), "x\n    y\n");
    }

    #[test]
    fn a_bare_carriage_return_becomes_a_newline_as_xml_says() {
        assert_eq!(dedent("a\rb"), "a\nb");
    }
}
