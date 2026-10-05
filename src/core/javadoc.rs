//! The ThingWorx Platform Java API documentation, searched and read as Markdown.
//!
//! The public site has one release, 10.1.0. Its generated Javadoc indexes name every class
//! and member; only paths derived from those entries are fetched. Files share the help
//! center's redirect-bounded client and atomic cache writer.

use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::help::{self, Fetch};

pub const BASE: &str = "https://support.ptc.com/help/thingworx_hc/javadoc";
pub const VERSION: &str = "10.1.0";
pub const TYPE_INDEX: &str = "type-search-index.js";
pub const MEMBER_INDEX: &str = "member-search-index.js";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
    pub package: String,
    pub name: String,
}

impl Class {
    pub fn qualified(&self) -> String {
        if self.package.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.package, self.name)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub package: String,
    pub class: String,
    pub label: String,
    pub anchor: Option<String>,
}

impl Member {
    pub fn name(&self) -> &str {
        self.label.split('(').next().unwrap_or(&self.label)
    }
}

#[derive(Debug, Default, Clone)]
pub struct Index {
    pub classes: Vec<Class>,
    pub members: Vec<Member>,
}

impl Index {
    pub fn parse(types: &str, members: &str) -> Result<Self, String> {
        let types = array(types, TYPE_INDEX)?;
        let members = array(members, MEMBER_INDEX)?;
        // An entry without a package is a navigation link ("All Classes and Interfaces"), not a
        // class; the real 10.1.0 index has one.
        let classes = types
            .iter()
            .filter(|entry| entry.get("p").is_some())
            .map(|entry| {
                Ok(Class {
                    package: field(entry, "p", TYPE_INDEX)?,
                    name: field(entry, "l", TYPE_INDEX)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let members = members
            .iter()
            .map(|entry| {
                Ok(Member {
                    package: field(entry, "p", MEMBER_INDEX)?,
                    class: field(entry, "c", MEMBER_INDEX)?,
                    label: field(entry, "l", MEMBER_INDEX)?,
                    anchor: entry.get("u").and_then(Value::as_str).map(str::to_string),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self { classes, members })
    }
}

fn array(text: &str, name: &str) -> Result<Vec<Value>, String> {
    let start = text
        .find('[')
        .ok_or_else(|| format!("{name} has no JSON array"))?;
    let end = text
        .rfind(']')
        .ok_or_else(|| format!("{name} has no JSON array"))?;
    serde_json::from_str(&text[start..=end]).map_err(|e| format!("{name}: {e}"))
}

fn field(entry: &Value, field: &str, name: &str) -> Result<String, String> {
    entry
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{name} has an entry without {field:?}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Class,
    Member,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub kind: Kind,
    pub package: String,
    pub class: String,
    pub label: String,
    pub path: String,
    pub url: String,
    pub rank: u8,
}

impl Hit {
    pub fn display(&self) -> String {
        match self.kind {
            Kind::Class => {
                if self.package.is_empty() {
                    self.class.clone()
                } else {
                    format!("{}.{}", self.package, self.class)
                }
            }
            Kind::Member => {
                let member = format!("{}.{}", self.class, self.label);
                if self.package.is_empty() {
                    member
                } else {
                    format!("{member}  ({})", self.package)
                }
            }
        }
    }
}

fn match_rank(candidate: &str, query: &str) -> Option<u8> {
    let candidate = candidate.to_lowercase();
    let query = query.to_lowercase();
    if candidate == query {
        Some(0)
    } else if candidate.starts_with(&query) {
        Some(1)
    } else if candidate.contains(&query) {
        Some(2)
    } else {
        None
    }
}

/// Classes and members by simple name: exact, prefix, contains; classes first within a rank.
/// `Class.member` searches that class and members of it in one query.
pub fn search(index: &Index, query: &str, limit: usize) -> Vec<Hit> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let dotted = query
        .rsplit_once('.')
        .map(|(class, member)| (class.rsplit('.').next().unwrap_or(class), member));
    let class_query = dotted.map_or(query, |(class, _)| class);
    let mut hits = Vec::new();
    for class in &index.classes {
        if let Some(rank) = match_rank(&class.name, class_query) {
            if let Ok(path) = class_path(class) {
                hits.push(Hit {
                    kind: Kind::Class,
                    package: class.package.clone(),
                    class: class.name.clone(),
                    label: class.name.clone(),
                    url: document_url(&path),
                    path,
                    rank,
                });
            }
        }
    }
    for member in &index.members {
        let rank = match dotted {
            Some((class, wanted)) if member.class.eq_ignore_ascii_case(class) => {
                match_rank(member.name(), wanted)
            }
            Some(_) => None,
            None => match_rank(member.name(), query),
        };
        if let Some(rank) = rank {
            let class = Class {
                package: member.package.clone(),
                name: member.class.clone(),
            };
            if let Ok(path) = class_path(&class) {
                let anchor = member.anchor.as_deref().unwrap_or(&member.label);
                hits.push(Hit {
                    kind: Kind::Member,
                    package: member.package.clone(),
                    class: member.class.clone(),
                    label: member.label.clone(),
                    url: format!("{}#{anchor}", document_url(&path)),
                    path,
                    rank,
                });
            }
        }
    }
    hits.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.package.to_lowercase().cmp(&b.package.to_lowercase()))
            .then_with(|| a.class.to_lowercase().cmp(&b.class.to_lowercase()))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
    hits.truncate(limit);
    hits
}

/// Resolve a class from the type index. No page name supplied by a caller becomes a path.
pub fn find_class(index: &Index, wanted: &str) -> Result<Class, String> {
    let wanted = wanted.trim();
    let matches: Vec<&Class> = index
        .classes
        .iter()
        .filter(|class| {
            class.qualified().eq_ignore_ascii_case(wanted)
                || class.name.eq_ignore_ascii_case(wanted)
        })
        .collect();
    match matches.as_slice() {
        [class] => Ok((*class).clone()),
        many if many.len() > 1 => Err(format!(
            "class {wanted:?} is ambiguous; use one of: {}",
            many.iter()
                .map(|class| class.qualified())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => {
            let mut close: Vec<(&Class, u8)> = index
                .classes
                .iter()
                .filter_map(|class| match_rank(&class.name, wanted).map(|rank| (class, rank)))
                .collect();
            if close.is_empty() {
                close = index
                    .classes
                    .iter()
                    .map(|class| {
                        (
                            class,
                            edit_distance(&class.name.to_lowercase(), &wanted.to_lowercase()) as u8,
                        )
                    })
                    .collect();
            }
            close.sort_by_key(|(class, rank)| (*rank, class.qualified().to_lowercase()));
            close.truncate(5);
            let names = close
                .iter()
                .map(|(class, _)| class.qualified())
                .collect::<Vec<_>>();
            Err(if names.is_empty() {
                format!("unknown class {wanted:?}")
            } else {
                format!(
                    "unknown class {wanted:?}; close matches: {}",
                    names.join(", ")
                )
            })
        }
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let mut row: Vec<usize> = (0..=b.chars().count()).collect();
    for (i, left) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, right) in b.chars().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if left == right {
                previous
            } else {
                1 + previous.min(above).min(row[j])
            };
            previous = above;
        }
    }
    row[b.chars().count()]
}

/// The class page path encoded by a type or member index entry.
pub fn class_path(class: &Class) -> Result<String, String> {
    let plain = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$'))
    };
    if !class.package.is_empty() && !class.package.split('.').all(plain) {
        return Err(format!(
            "the Javadoc index has an unsafe package name {:?}",
            class.package
        ));
    }
    if class.name.is_empty()
        || class.name.contains(['/', '\\', ':', '%'])
        || !class.name.split('.').all(plain)
    {
        return Err(format!(
            "the Javadoc index has an unsafe class name {:?}",
            class.name
        ));
    }
    let file = format!("{}.html", class.name);
    Ok(if class.package.is_empty() {
        file
    } else {
        format!("{}/{file}", class.package.replace('.', "/"))
    })
}

pub fn document_url(path: &str) -> String {
    format!("{BASE}/{path}")
}

pub fn cache_root() -> Result<PathBuf, String> {
    dirs::cache_dir()
        .map(|dir| dir.join("twaco").join("javadoc"))
        .ok_or_else(|| "this machine has no user cache folder to keep the Javadoc in".to_string())
}

/// Fetch an index-derived Javadoc file through the same atomic cache as the help center.
pub fn cached(
    fetch: &dyn Fetch,
    cache: &Path,
    path: &str,
    refresh: bool,
) -> Result<Vec<u8>, String> {
    let local = cache
        .join(VERSION)
        .join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
    help::cached_file(fetch, &local, &document_url(path), refresh).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Detail {
    name: String,
    signature: String,
    parameters: String,
    return_type: String,
    description: String,
    category: Option<String>,
    service_description: Option<String>,
    notes: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub title: String,
    pub path: String,
    pub url: String,
    pub markdown: String,
    pub methods: usize,
}

/// Render a class overview, or every overload of one member, from generated Javadoc HTML.
pub fn read(html: &[u8], class: &Class, member: Option<&str>) -> Result<Reading, String> {
    let path = class_path(class)?;
    let source = document_url(&path);
    let document = Html::parse_document(&String::from_utf8_lossy(html));
    let description_selector =
        Selector::parse("section#class-description .block").expect("a valid selector");
    let description = document
        .select(&description_selector)
        .next()
        .map(|element| markdown(element, &source))
        .unwrap_or_default();
    // Every kind of member the search index lists has its details here: constructors, fields,
    // enum constants and annotation elements as well as methods.
    let detail_selector = Selector::parse(
        "section.method-details section.detail, section.constructor-details section.detail,          section.field-details section.detail, section.constant-details section.detail,          section.member-details section.detail",
    )
    .expect("a valid selector");
    let details: Vec<Detail> = document
        .select(&detail_selector)
        .filter_map(parse_detail)
        .collect();
    if details.is_empty() && description.is_empty() {
        return Err(format!("{path} does not look like a Javadoc class page"));
    }
    let title = class.qualified();
    let mut methods = details.len();
    let markdown = match member {
        None => overview(&title, &description, &details),
        Some(wanted) => {
            let selected: Vec<&Detail> = details
                .iter()
                .filter(|detail| detail.name.eq_ignore_ascii_case(wanted))
                .collect();
            if selected.is_empty() {
                let names: BTreeSet<String> =
                    details.iter().map(|detail| detail.name.clone()).collect();
                let mut close: Vec<(String, u8)> = names
                    .iter()
                    .filter_map(|name| match_rank(name, wanted).map(|rank| (name.clone(), rank)))
                    .collect();
                if close.is_empty() {
                    close = names
                        .iter()
                        .map(|name| {
                            (
                                name.clone(),
                                edit_distance(&name.to_lowercase(), &wanted.to_lowercase()) as u8,
                            )
                        })
                        .collect();
                }
                close.sort_by_key(|(name, rank)| (*rank, name.to_lowercase()));
                close.truncate(5);
                return Err(format!(
                    "unknown member {wanted:?} of {title}; close matches: {}",
                    close
                        .into_iter()
                        .map(|(name, _)| name)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            methods = selected.len();
            member_markdown(&title, &selected)
        }
    };
    Ok(Reading {
        title,
        path,
        url: source,
        markdown,
        methods,
    })
}

fn parse_detail(element: ElementRef) -> Option<Detail> {
    let select = |css: &str| Selector::parse(css).expect("a valid selector");
    let signature_element = element.select(&select(".member-signature")).next()?;
    let signature = text(signature_element);
    let name = element
        .select(&select(".element-name"))
        .next()
        .map(text)
        .or_else(|| {
            element
                .value()
                .attr("id")
                .map(|id| id.split('(').next().unwrap_or(id).to_string())
        })?;
    let parameters = element
        .select(&select(".parameters"))
        .next()
        .map(text)
        .unwrap_or_else(|| "()".to_string());
    let return_type = element
        .select(&select(".return-type"))
        .next()
        .map(text)
        .unwrap_or_else(|| "void".to_string());
    let description = element
        .select(&select(":scope > .block"))
        .next()
        .map(|block| text(block))
        .unwrap_or_default();
    let category = labelled(&description, "Service Category:", "Service Description:");
    let service_description = labelled(&description, "Service Description:", "Service Category:");
    let term = select("dl dt, dl dd");
    let mut heading = String::new();
    let mut notes = Vec::new();
    for item in element.select(&term) {
        if item.value().name() == "dt" {
            heading = text(item).trim_end_matches(':').to_string();
        } else if !heading.is_empty() {
            notes.push((heading.clone(), text(item)));
        }
    }
    Some(Detail {
        name,
        signature,
        parameters,
        return_type,
        description,
        category,
        service_description,
        notes,
    })
}

fn labelled(text: &str, label: &str, other: &str) -> Option<String> {
    let rest = text.split_once(label)?.1.trim();
    let value = rest
        .split_once(other)
        .map_or(rest, |(value, _)| value)
        .trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn overview(title: &str, description: &str, details: &[Detail]) -> String {
    let mut out = format!("# {title}\n\n");
    if !description.is_empty() {
        out.push_str(description.trim());
        out.push_str("\n\n");
    }
    out.push_str("## Methods\n\n");
    for detail in details {
        let summary = match (&detail.category, &detail.service_description) {
            (Some(category), Some(description)) => format!("{category}: {description}"),
            _ => first_sentence(&detail.description),
        };
        out.push_str(&format!(
            "- `{}`{} → `{}`",
            detail.name, detail.parameters, detail.return_type
        ));
        if !summary.is_empty() {
            out.push_str(&format!(" — {summary}"));
        }
        out.push('\n');
    }
    out
}

fn member_markdown(title: &str, details: &[&Detail]) -> String {
    let mut out = format!("# {title}.{}\n\n", details[0].name);
    for detail in details {
        out.push_str(&format!("## `{}`\n\n", detail.signature));
        if !detail.description.is_empty() {
            out.push_str(&detail.description);
            out.push_str("\n\n");
        }
        for heading in ["Parameters", "Returns", "Throws"] {
            let notes: Vec<&str> = detail
                .notes
                .iter()
                .filter(|(kind, _)| kind == heading)
                .map(|(_, text)| text.as_str())
                .collect();
            if notes.is_empty() {
                continue;
            }
            out.push_str(&format!("### {heading}\n\n"));
            for note in notes {
                out.push_str(&format!("- {note}\n"));
            }
            out.push('\n');
        }
    }
    out
}

fn first_sentence(text: &str) -> String {
    for (at, character) in text.char_indices() {
        if matches!(character, '.' | '!' | '?')
            && text[at + character.len_utf8()..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
        {
            return text[..at + character.len_utf8()].to_string();
        }
    }
    text.to_string()
}

fn text(element: ElementRef) -> String {
    element
        .text()
        .flat_map(|part| part.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(" )", ")")
        .replace(" ,", ",")
}

fn markdown(element: ElementRef, source: &str) -> String {
    let converted = htmd::convert(&element.inner_html()).unwrap_or_else(|_| text(element));
    help::absolute_links(&unwrap_code_links(converted.trim()), source)
}

/// Javadoc puts a linked type in code inside a link inside code, which converts to
/// ``` ``[`Name`](url "class in pkg")`` ```. Read as Markdown that is noise, so it becomes
/// `` `Name` ``; the class is one `javadoc class` away.
fn unwrap_code_links(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(start) = rest.find("``[`") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        let unwrapped = after.find("`](").and_then(|name_end| {
            let tail = &after[name_end + 3..];
            tail.find(")``")
                .map(|close| (&after[..name_end], &tail[close + 3..]))
        });
        match unwrapped {
            Some((name, remainder)) if !name.contains('`') => {
                out.push('`');
                out.push_str(name);
                out.push('`');
                rest = remainder;
            }
            _ => {
                out.push_str("``[`");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_skips_its_navigation_entry() {
        // The real 10.1.0 type index starts with a link to the class list, which has no package.
        let types = r#"typeSearchIndex = [{"l":"All Classes and Interfaces","u":"allclasses-index.html"},{"p":"com.thingworx.types","l":"InfoTable"}];updateSearchResults();"#;
        let index = Index::parse(types, "memberSearchIndex = []").unwrap();
        assert_eq!(index.classes.len(), 1);
        assert_eq!(index.classes[0].name, "InfoTable");
    }

    #[test]
    fn a_linked_type_in_code_reads_as_plain_code() {
        let text = "has a ``[`DataShapeDefinition`](https://x/D.html \"class in a\")`` and ``[`ValueCollection`](https://x/V.html \"class\")``s; ``plain``";
        assert_eq!(
            unwrap_code_links(text),
            "has a `DataShapeDefinition` and `ValueCollection`s; ``plain``"
        );
    }

    const TYPES: &str = r#"typeSearchIndex = [{"p":"com.thingworx.types","l":"InfoTable"},{"p":"com.thingworx.resources.queries","l":"InfoTableFunctions"},{"p":"other","l":"InfoTable"},{"p":"example","l":"Sorter"}];updateSearchResults();"#;
    const MEMBERS: &str = r#"memberSearchIndex = [{"p":"com.thingworx.resources.queries","c":"InfoTableFunctions","l":"Sort(InfoTable, String, Boolean)","u":"Sort(com.thingworx.types.InfoTable,java.lang.String,java.lang.Boolean)"},{"p":"com.thingworx.types","c":"InfoTable","l":"getRowCount()"},{"p":"example","c":"Sorter","l":"sortAll()"}];"#;

    fn index() -> Index {
        Index::parse(TYPES, MEMBERS).unwrap()
    }

    #[test]
    fn indexes_are_read_through_their_javascript_wrappers() {
        let index = index();
        assert_eq!(
            index.classes[0].qualified(),
            "com.thingworx.types.InfoTable"
        );
        assert_eq!(index.members[0].name(), "Sort");
        assert_eq!(
            index.members[0].anchor.as_deref(),
            Some("Sort(com.thingworx.types.InfoTable,java.lang.String,java.lang.Boolean)")
        );
    }

    #[test]
    fn search_ranks_exact_prefix_contains_and_classes_before_members() {
        let hits = search(&index(), "sort", 10);
        assert_eq!(hits.iter().map(Hit::display).collect::<Vec<_>>(), [
            "InfoTableFunctions.Sort(InfoTable, String, Boolean)  (com.thingworx.resources.queries)",
            "example.Sorter",
            "Sorter.sortAll()  (example)",
        ]);
        assert_eq!(
            hits.iter().map(|hit| hit.rank).collect::<Vec<_>>(),
            [0, 1, 1]
        );
    }

    #[test]
    fn dotted_search_finds_the_class_and_only_its_members() {
        let hits = search(&index(), "InfoTableFunctions.Sort", 10);
        assert_eq!(hits.iter().map(Hit::display).collect::<Vec<_>>(), [
            "com.thingworx.resources.queries.InfoTableFunctions",
            "InfoTableFunctions.Sort(InfoTable, String, Boolean)  (com.thingworx.resources.queries)",
        ]);
    }

    #[test]
    fn ambiguous_simple_classes_name_every_qualified_choice() {
        let error = find_class(&index(), "InfoTable").unwrap_err();
        assert!(error.contains("com.thingworx.types.InfoTable"), "{error}");
        assert!(error.contains("other.InfoTable"), "{error}");
        assert_eq!(
            find_class(&index(), "com.thingworx.types.InfoTable")
                .unwrap()
                .package,
            "com.thingworx.types"
        );
    }

    #[test]
    fn a_real_javadoc_page_becomes_an_overview_and_full_member_details() {
        let html = include_bytes!("../../tests/fixtures/InfoTableFunctions.html");
        let class = Class {
            package: "com.thingworx.resources.queries".to_string(),
            name: "InfoTableFunctions".to_string(),
        };
        let overview = read(html, &class, None).unwrap();
        assert!(overview
            .markdown
            .starts_with("# com.thingworx.resources.queries.InfoTableFunctions"));
        assert!(
            overview
                .markdown
                .contains("Functions for working with info tables."),
            "{}",
            overview.markdown
        );
        assert!(
            overview.markdown.contains(
                "`Sort`(InfoTable t, String sortColumn, Boolean ascending) → `InfoTable`"
            ),
            "{}",
            overview.markdown
        );
        assert!(
            overview
                .markdown
                .contains("InfoTable: Sorts an InfoTable by the specified column."),
            "{}",
            overview.markdown
        );

        let member = read(html, &class, Some("sort")).unwrap();
        assert_eq!(member.methods, 2);
        assert_eq!(
            member
                .markdown
                .matches("## `public static InfoTable Sort")
                .count(),
            2,
            "{}",
            member.markdown
        );
        assert!(member.markdown.contains("### Parameters"));
        assert!(member
            .markdown
            .contains("t - the table to sort - INFOTABLE"));
        assert!(member.markdown.contains("### Returns"));
        assert!(member.markdown.contains("### Throws"));
    }
}
