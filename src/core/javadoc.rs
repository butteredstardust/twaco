//! The ThingWorx Platform Java API documentation, searched and read as Markdown.
//!
//! The public site has one release, 10.1.0. Its generated Javadoc indexes name every class
//! and member; only paths derived from those entries are fetched. Files share the help
//! center's redirect-bounded client and atomic cache writer.

use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use super::help::{self, Fetch};

pub const BASE: &str = "https://support.ptc.com/help/thingworx_hc/javadoc";
pub const VERSION: &str = "10.1.0";
pub const TYPE_INDEX: &str = "type-search-index.js";
pub const MEMBER_INDEX: &str = "member-search-index.js";

/// Where Javadoc pages are read from. SDK files are read in place: their contents never enter
/// the cache or the repository.
pub enum Source {
    Web {
        web: help::Web,
        cache: PathBuf,
        refresh: bool,
    },
    Sdk(Sdk),
}

impl Source {
    /// Pick the first configured SDK path, or the public cached Javadoc when none is set.
    pub fn configured(
        flag: Option<&Path>,
        environment: Option<&Path>,
        profile: Option<&Path>,
        profile_name: &str,
        refresh: bool,
    ) -> Result<Self, String> {
        if let Some(path) = flag {
            return Self::sdk(path, "--sdk");
        }
        if let Some(path) = environment {
            return Self::sdk(path, "TWACO_SDK");
        }
        if let Some(path) = profile {
            return Self::sdk(path, &format!("profile {profile_name}"));
        }
        Ok(Self::web(cache_root()?, refresh))
    }

    pub fn web(cache: PathBuf, refresh: bool) -> Self {
        Self::Web {
            web: help::Web::new(BASE),
            cache,
            refresh,
        }
    }

    pub fn sdk(path: &Path, configured_by: &str) -> Result<Self, String> {
        Sdk::open(path, configured_by).map(Self::Sdk)
    }

    pub fn fetch(&self, path: &str) -> Result<Vec<u8>, help::HelpError> {
        match self {
            Self::Web {
                web,
                cache,
                refresh,
            } => cached(web, cache, path, *refresh),
            Self::Sdk(sdk) => sdk.read(path).map_err(help::HelpError::Invalid),
        }
    }

    pub fn version(&self) -> &str {
        match self {
            Self::Web { .. } => VERSION,
            Self::Sdk(sdk) => &sdk.version,
        }
    }

    pub fn url(&self, path: &str) -> String {
        match self {
            Self::Web { .. } => document_url(path),
            Self::Sdk(sdk) => sdk.url(path),
        }
    }

    pub fn display(&self, path: &str) -> String {
        match self {
            Self::Web { .. } => self.url(path),
            Self::Sdk(sdk) => format!("{}!{path}", sdk.display),
        }
    }
}

/// A Javadoc jar, either on disk or held only while reading an SDK archive.
pub struct Sdk {
    jar: Jar,
    display: String,
    version: String,
}

enum Jar {
    File(PathBuf),
    Memory(Vec<u8>),
}

impl Sdk {
    fn open(path: &Path, configured_by: &str) -> Result<Self, String> {
        if !path.exists() {
            return Err(format!(
                "SDK path {} from {configured_by} does not exist",
                path.display()
            ));
        }
        if path.is_dir() {
            let jar = find_jar(path).ok_or_else(|| no_jar(path, configured_by))?;
            return Self::from_file(jar, configured_by);
        }
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
        {
            return Self::from_file(path.to_path_buf(), configured_by);
        }
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
        {
            let file = File::open(path).map_err(|error| {
                format!(
                    "cannot read SDK path {} from {configured_by}: {error}",
                    path.display()
                )
            })?;
            let mut outer = zip::ZipArchive::new(file).map_err(|error| {
                format!(
                    "{} from {configured_by} is not an SDK zip: {error}",
                    path.display()
                )
            })?;
            let name = (0..outer.len())
                .filter_map(|index| {
                    outer
                        .by_index(index)
                        .ok()
                        .and_then(|entry| entry.name().ok().map(|name| name.to_string()))
                })
                .find(|name| is_jar_name(name) && !name.contains('/'))
                .ok_or_else(|| no_jar(path, configured_by))?;
            let mut entry = outer.by_name(&name).map_err(|error| {
                format!(
                    "cannot read {name} in SDK path {} from {configured_by}: {error}",
                    path.display()
                )
            })?;
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|error| {
                format!(
                    "cannot read {name} in SDK path {} from {configured_by}: {error}",
                    path.display()
                )
            })?;
            return Self::from_jar(
                Jar::Memory(bytes),
                format!("{}!{name}", path.display()),
                &name,
                path,
                configured_by,
            );
        }
        Err(no_jar(path, configured_by))
    }

    fn from_file(path: PathBuf, configured_by: &str) -> Result<Self, String> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        Self::from_jar(
            Jar::File(path.clone()),
            path.display().to_string(),
            name,
            &path,
            configured_by,
        )
    }

    fn from_jar(
        jar: Jar,
        display: String,
        name: &str,
        configured_path: &Path,
        configured_by: &str,
    ) -> Result<Self, String> {
        let result = Self {
            jar,
            display,
            version: jar_version(name).unwrap_or_else(|| "SDK".to_string()),
        };
        result.open_archive().map_err(|error| {
            format!(
                "SDK path {} from {configured_by} has an unreadable Javadoc jar: {error}",
                configured_path.display()
            )
        })?;
        Ok(result)
    }

    fn open_archive(&self) -> Result<zip::ZipArchive<Box<dyn ReadSeek + '_>>, String> {
        let reader: Box<dyn ReadSeek + '_> = match &self.jar {
            Jar::File(path) => Box::new(File::open(path).map_err(|error| error.to_string())?),
            Jar::Memory(bytes) => Box::new(Cursor::new(bytes.as_slice())),
        };
        zip::ZipArchive::new(reader).map_err(|error| error.to_string())
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        let mut archive = self.open_archive()?;
        let mut entry = archive
            .by_name(path)
            .map_err(|_| format!("{} has no {path}", self.display))?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read {path} in {}: {error}", self.display))?;
        Ok(bytes)
    }

    fn url(&self, path: &str) -> String {
        let path = path.replace('\\', "/");
        let display = self.display.replace('\\', "/");
        format!("jar:file:///{}!/{path}", display.trim_start_matches('/'))
    }
}

trait ReadSeek: Read + std::io::Seek {}
impl<T: Read + std::io::Seek> ReadSeek for T {}

fn is_jar_name(name: &str) -> bool {
    name.strip_prefix("thingworx-javadocs-")
        .is_some_and(|rest| rest.ends_with(".jar"))
}

fn jar_version(name: &str) -> Option<String> {
    is_jar_name(name).then(|| {
        name.strip_prefix("thingworx-javadocs-")?
            .split("-b")
            .next()
            .filter(|version| !version.is_empty())
            .map(str::to_string)
    })?
}

fn no_jar(path: &Path, configured_by: &str) -> String {
    format!(
        "SDK path {} from {configured_by} has no thingworx-javadocs-*.jar",
        path.display()
    )
}

fn find_jar(folder: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(folder).ok()?.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_jar_name)
        {
            candidates.push(path);
        } else if path.is_dir() {
            for nested in std::fs::read_dir(path).ok()?.flatten() {
                let nested = nested.path();
                if nested.is_file()
                    && nested
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(is_jar_name)
                {
                    candidates.push(nested);
                }
            }
        }
    }
    candidates.sort();
    candidates.into_iter().next()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
    pub(crate) package: String,
    pub(crate) name: String,
}

impl Class {
    pub(crate) fn qualified(&self) -> String {
        if self.package.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.package, self.name)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Member {
    pub(crate) package: String,
    pub(crate) class: String,
    pub(crate) label: String,
    pub(crate) anchor: Option<String>,
}

impl Member {
    pub(crate) fn name(&self) -> &str {
        self.label.split('(').next().unwrap_or(&self.label)
    }
}

#[derive(Debug, Default, Clone)]
pub struct Index {
    pub(crate) classes: Vec<Class>,
    pub(crate) members: Vec<Member>,
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
                            u8::try_from(strsim::levenshtein(
                                &class.name.to_lowercase(),
                                &wanted.to_lowercase(),
                            ))
                            .unwrap_or(u8::MAX),
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

pub(crate) fn document_url(path: &str) -> String {
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
) -> Result<Vec<u8>, help::HelpError> {
    let local = cache
        .join(VERSION)
        .join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
    help::cached_file(fetch, &local, &document_url(path), refresh)
}

/// Why a class page could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// The page, or the class's path, is not what the Javadoc generator writes.
    #[error("{0}")]
    NotAPage(String),
    /// The class has no member of the name asked for.
    #[error("{0}")]
    UnknownMember(String),
}

impl super::codes::Coded for ReadError {
    fn code(&self) -> super::codes::ErrorCode {
        match self {
            Self::NotAPage(_) => super::codes::ErrorCode::InvalidData,
            Self::UnknownMember(_) => super::codes::ErrorCode::InvalidArguments,
        }
    }
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

/// A service as a script, a log or a REST path names it, reduced to where its documentation
/// starts: the classes to look in first, and the member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRef {
    /// What was asked for, normalised: `Resources/InfoTableFunctions.Sort`, `me.GetPropertyValues`.
    pub shown: String,
    /// Simple or qualified class names to start from, in order; the first in the index wins.
    pub(crate) classes: Vec<String>,
    pub member: String,
    /// Whether the target is an entity a solution may define services on, so a miss may be the
    /// solution's own service rather than the platform's.
    pub(crate) entity: bool,
}

/// Read a service reference: `Resources["X"].Y(...)`, `Things['T'].Y`, `me.Y`,
/// `Collection/Name.Y`, `Collection/Name/Services/Y`, or `Class.member`.
pub fn parse_service_ref(text: &str) -> Result<ServiceRef, String> {
    let given = text.trim().trim_end_matches(';').trim();
    // A REST path as a browser or a log shows it.
    let given = given
        .strip_prefix("/Thingworx/")
        .or_else(|| given.strip_prefix("Thingworx/"))
        .unwrap_or(given);
    // A call's arguments say nothing about which service it is. They start after the entity's
    // name, which may itself hold a parenthesis.
    // The name ends at the quote and bracket that close it, not at the last bracket: the
    // arguments may hold an array.
    // Only a bracket that opens before the call does: one inside the arguments is not the name's.
    let call = given.find('(').unwrap_or(given.len());
    let name_end = ["[\"", "['"]
        .iter()
        .filter_map(|open| given.find(open).map(|at| (at, &open[1..])))
        .filter(|(at, _)| *at < call)
        .min_by_key(|(at, _)| *at)
        .and_then(|(at, quote)| {
            let from = at + 2;
            given[from..]
                .find(&format!("{quote}]"))
                .map(|end| from + end + 2)
        })
        .unwrap_or(0);
    let given = match given[name_end..].find('(') {
        Some(open) if given.ends_with(')') => given[..name_end + open].trim_end(),
        _ => given,
    };
    let invalid = || {
        format!(
            "{text:?} is not a service reference; use Resources[\"X\"].Service, Things/Name.Service, \
             me.Service or Class.member"
        )
    };
    let plain = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$'))
    };
    let entity_ref = |collection: &str, name: &str, member: &str| -> Result<ServiceRef, String> {
        let name = name.trim();
        if name.is_empty() || !plain(member) {
            return Err(invalid());
        }
        let (collection, singular) = super::search::TYPES
            .iter()
            .find(|(plural, _)| plural.eq_ignore_ascii_case(collection))
            .copied()
            .ok_or_else(|| {
                format!("{collection:?} is not an entity collection; one of: Things, ThingTemplates, ThingShapes, Resources, Subsystems, ...")
            })?;
        // A Resource or Subsystem is its own class; any other entity is its type's class.
        let mut classes = Vec::new();
        if matches!(collection, "Resources" | "Subsystems") {
            classes.push(name.to_string());
        }
        classes.push(singular.to_string());
        Ok(ServiceRef {
            shown: format!("{collection}/{name}.{member}"),
            classes,
            member: member.to_string(),
            entity: !matches!(collection, "Resources" | "Subsystems"),
        })
    };
    if let Some(member) = given.strip_prefix("me.") {
        if !plain(member) {
            return Err(invalid());
        }
        return Ok(ServiceRef {
            shown: format!("me.{member}"),
            classes: vec!["Thing".to_string()],
            member: member.to_string(),
            entity: true,
        });
    }
    if let Some((collection, rest)) = given.split_once('[') {
        let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''));
        let (name, after) = quote
            .and_then(|quote| rest[1..].split_once(quote))
            .ok_or_else(invalid)?;
        let member = after.strip_prefix("].").ok_or_else(invalid)?;
        return entity_ref(collection.trim(), name, member);
    }
    if let Some((collection, rest)) = given.split_once('/') {
        let services = rest.to_ascii_lowercase().find("/services/");
        let (name, member) = match services {
            Some(at) => (&rest[..at], &rest[at + "/services/".len()..]),
            None => rest.rsplit_once('.').ok_or_else(invalid)?,
        };
        return entity_ref(collection, name, member);
    }
    let (class, member) = given.rsplit_once('.').ok_or_else(invalid)?;
    if !class.split('.').all(plain) || !plain(member) {
        return Err(invalid());
    }
    Ok(ServiceRef {
        shown: format!("{class}.{member}"),
        classes: vec![class.to_string()],
        member: member.to_string(),
        entity: false,
    })
}

/// A class by simple or qualified name, as `find_class`, except that a simple name several
/// packages share is taken from the one `com.thingworx` package that has it.
pub(crate) fn find_platform_class(index: &Index, wanted: &str) -> Result<Class, String> {
    find_class(index, wanted).or_else(|error| {
        let platform: Vec<&Class> = index
            .classes
            .iter()
            .filter(|class| {
                class.name.eq_ignore_ascii_case(wanted.trim())
                    && class.package.starts_with("com.thingworx.")
            })
            .collect();
        match platform.as_slice() {
            [class] => Ok((*class).clone()),
            _ => Err(error),
        }
    })
}

/// The classes a class page says it extends, nearest first, as far as the index knows them
/// (`java.lang.Object` and other outside classes are not in it).
pub fn superclasses(html: &[u8], index: &Index) -> Vec<Class> {
    let document = Html::parse_document(&String::from_utf8_lossy(html));
    let links =
        Selector::parse("div.inheritance > a, ul.inheritance > li > a").expect("a valid selector");
    let mut found: Vec<Class> = document
        .select(&links)
        .filter_map(|link| {
            let qualified = text(link);
            index
                .classes
                .iter()
                .find(|class| class.qualified() == qualified)
                .cloned()
        })
        .collect();
    // The tree is drawn from the root down.
    found.reverse();
    found.dedup();
    found
}

/// Which class declares `member`: the first of `chain` the member index lists it under.
pub fn declaring_class<'a>(index: &Index, chain: &'a [Class], member: &str) -> Option<&'a Class> {
    chain.iter().find(|class| {
        index.members.iter().any(|entry| {
            entry.package == class.package
                && entry.class == class.name
                && entry.name().eq_ignore_ascii_case(member)
        })
    })
}

/// Why a service reference found no documentation: what was searched, and where else a member
/// of that name is declared.
pub fn service_not_found(index: &Index, wanted: &ServiceRef, searched: &[Class]) -> String {
    let mut elsewhere: Vec<String> = index
        .members
        .iter()
        .filter(|entry| entry.name().eq_ignore_ascii_case(&wanted.member))
        .map(|entry| {
            Class {
                package: entry.package.clone(),
                name: entry.class.clone(),
            }
            .qualified()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    elsewhere.truncate(5);
    let mut message = if searched.is_empty() {
        format!(
            "{}: the Javadoc has no class {}",
            wanted.shown,
            wanted.classes.join(" or ")
        )
    } else {
        format!(
            "{}: no member {} in {}",
            wanted.shown,
            wanted.member,
            searched
                .iter()
                .map(Class::qualified)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if !elsewhere.is_empty() {
        message.push_str(&format!(
            "; {} is declared by {} (javadoc class <Name> --member {})",
            wanted.member,
            elsewhere.join(", "),
            wanted.member
        ));
    }
    if wanted.entity {
        message.push_str("; a service the solution defines is in `twaco catalog`");
    }
    message
}

/// Why a service reference could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The reference does not name a service, or no class the Javadoc has declares it.
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Fetch(help::HelpError),
    /// An index or page is not what the Javadoc generator writes.
    #[error("{0}")]
    Data(String),
    #[error("{0}")]
    Read(ReadError),
}

impl super::codes::Coded for ServiceError {
    fn code(&self) -> super::codes::ErrorCode {
        match self {
            Self::Invalid(_) => super::codes::ErrorCode::InvalidArguments,
            Self::Fetch(error) => error.code(),
            Self::Data(_) => super::codes::ErrorCode::InvalidData,
            Self::Read(error) => error.code(),
        }
    }
}

/// One service's documentation, found from what a script names.
#[derive(Debug)]
pub struct ServiceReading {
    /// The reference, normalised.
    pub asked: String,
    /// The class whose page documents the member.
    pub declared_in: String,
    /// Whether that class is a superclass of the one the reference starts from.
    pub inherited: bool,
    /// Said when the class the reference names is not in the Javadoc and a more general one
    /// answered instead: what is shown is that class's member, not the named entity's.
    pub note: Option<String>,
    pub reading: Reading,
}

/// Find and read the service a reference names: the starting class, then each class it extends,
/// until one declares the member. `fetch` reads an index-derived file, from the cache or the site.
pub fn read_service(
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, help::HelpError>,
    reference: &str,
) -> Result<ServiceReading, ServiceError> {
    read_service_at(fetch, reference, &document_url)
}

/// Like [`read_service`], with links resolved against the source that supplied each page.
pub fn read_service_at(
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, help::HelpError>,
    reference: &str,
    source: &dyn Fn(&str) -> String,
) -> Result<ServiceReading, ServiceError> {
    let wanted = parse_service_ref(reference).map_err(ServiceError::Invalid)?;
    let types = fetch(TYPE_INDEX).map_err(ServiceError::Fetch)?;
    let members = fetch(MEMBER_INDEX).map_err(ServiceError::Fetch)?;
    let index = Index::parse(
        &String::from_utf8_lossy(&types),
        &String::from_utf8_lossy(&members),
    )
    .map_err(ServiceError::Data)?;
    let mut searched: Vec<Class> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    for start in &wanted.classes {
        let Ok(class) = find_platform_class(&index, start) else {
            missing.push(start);
            continue;
        };
        let page =
            fetch(&class_path(&class).map_err(ServiceError::Data)?).map_err(ServiceError::Fetch)?;
        let mut chain = vec![class.clone()];
        chain.extend(superclasses(&page, &index));
        if let Some(declaring) = declaring_class(&index, &chain, &wanted.member) {
            let html = if *declaring == class {
                page
            } else {
                fetch(&class_path(declaring).map_err(ServiceError::Data)?)
                    .map_err(ServiceError::Fetch)?
            };
            let path = class_path(declaring).map_err(ServiceError::Data)?;
            let reading = read_at(&html, declaring, Some(&wanted.member), source(&path))
                .map_err(ServiceError::Read)?;
            let note = (!missing.is_empty()).then(|| {
                format!(
                    "the Javadoc has no class {}; this is the member every {} has",
                    missing.join(" or "),
                    class.name
                )
            });
            return Ok(ServiceReading {
                asked: wanted.shown.clone(),
                declared_in: declaring.qualified(),
                inherited: *declaring != class,
                note,
                reading,
            });
        }
        for class in chain {
            if !searched.contains(&class) {
                searched.push(class);
            }
        }
    }
    Err(ServiceError::Invalid(service_not_found(
        &index, &wanted, &searched,
    )))
}

/// Render a class overview, or every overload of one member, from generated Javadoc HTML.
pub fn read(html: &[u8], class: &Class, member: Option<&str>) -> Result<Reading, ReadError> {
    let path = class_path(class).map_err(ReadError::NotAPage)?;
    read_at(html, class, member, document_url(&path))
}

/// Render a generated class page whose links should resolve against `source`.
pub fn read_at(
    html: &[u8],
    class: &Class,
    member: Option<&str>,
    source: String,
) -> Result<Reading, ReadError> {
    let path = class_path(class).map_err(ReadError::NotAPage)?;
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
        return Err(ReadError::NotAPage(format!(
            "{path} does not look like a Javadoc class page"
        )));
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
                                u8::try_from(strsim::levenshtein(
                                    &name.to_lowercase(),
                                    &wanted.to_lowercase(),
                                ))
                                .unwrap_or(u8::MAX),
                            )
                        })
                        .collect();
                }
                close.sort_by_key(|(name, rank)| (*rank, name.to_lowercase()));
                close.truncate(5);
                return Err(ReadError::UnknownMember(format!(
                    "unknown member {wanted:?} of {title}; close matches: {}",
                    close
                        .into_iter()
                        .map(|(name, _)| name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
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
    use std::io::Write;

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

    fn sdk_jar() -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in [
            (TYPE_INDEX, TYPES.as_bytes()),
            (MEMBER_INDEX, MEMBERS.as_bytes()),
            (
                "com/thingworx/resources/queries/InfoTableFunctions.html",
                include_bytes!("../../tests/fixtures/InfoTableFunctions.html") as &[u8],
            ),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn sdk_jar_folder_and_sdk_zip_read_search_class_and_service_offline() {
        let temporary = tempfile::tempdir().unwrap();
        let jar_name = "thingworx-javadocs-10.2.0-b64.jar";
        let jar = temporary.path().join(jar_name);
        std::fs::write(&jar, sdk_jar()).unwrap();
        let folder = temporary.path().join("sdk");
        std::fs::create_dir(&folder).unwrap();
        std::fs::copy(&jar, folder.join(jar_name)).unwrap();
        let outer = temporary.path().join("sdk.zip");
        let mut archive = zip::ZipWriter::new(File::create(&outer).unwrap());
        archive
            .start_file(jar_name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&sdk_jar()).unwrap();
        archive.finish().unwrap();

        for path in [&jar, &folder, &outer] {
            let source = Source::sdk(path, "--sdk").unwrap();
            assert_eq!(source.version(), "10.2.0");
            let fetch = |path: &str| source.fetch(path);
            let types = fetch(TYPE_INDEX).unwrap();
            let members = fetch(MEMBER_INDEX).unwrap();
            let index = Index::parse(
                &String::from_utf8_lossy(&types),
                &String::from_utf8_lossy(&members),
            )
            .unwrap();
            assert!(!search(&index, "Sort", 10).is_empty());
            let class = find_class(&index, "InfoTableFunctions").unwrap();
            let path = class_path(&class).unwrap();
            let class_read =
                read_at(&fetch(&path).unwrap(), &class, None, source.url(&path)).unwrap();
            assert!(class_read.methods > 0);
            assert!(class_read.url.starts_with("jar:file:///"));
            let service =
                read_service_at(&fetch, r#"Resources["InfoTableFunctions"].Sort"#, &|path| {
                    source.url(path)
                })
                .unwrap();
            assert_eq!(
                service.declared_in,
                "com.thingworx.resources.queries.InfoTableFunctions"
            );
        }
    }

    #[test]
    fn an_invalid_sdk_path_names_its_configuration_source() {
        let path = Path::new("not-an-sdk");
        let Err(error) = Source::sdk(path, "TWACO_SDK") else {
            panic!("a missing SDK path resolved");
        };
        assert!(error.contains("not-an-sdk"), "{error}");
        assert!(error.contains("TWACO_SDK"), "{error}");
    }

    #[test]
    fn configured_sdk_paths_take_flag_then_environment_then_profile() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = [
            temporary.path().join("thingworx-javadocs-10.2.0-b1.jar"),
            temporary.path().join("thingworx-javadocs-10.3.0-b1.jar"),
            temporary.path().join("thingworx-javadocs-10.4.0-b1.jar"),
        ];
        for path in &paths {
            std::fs::write(path, sdk_jar()).unwrap();
        }
        let source = Source::configured(
            Some(&paths[0]),
            Some(&paths[1]),
            Some(&paths[2]),
            "default",
            false,
        )
        .unwrap();
        assert_eq!(source.version(), "10.2.0");
        let source =
            Source::configured(None, Some(&paths[1]), Some(&paths[2]), "default", false).unwrap();
        assert_eq!(source.version(), "10.3.0");
        let source = Source::configured(None, None, Some(&paths[2]), "default", false).unwrap();
        assert_eq!(source.version(), "10.4.0");
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

    #[test]
    fn a_service_reference_is_read_from_script_rest_and_class_spellings() {
        let resource = |text: &str| parse_service_ref(text).unwrap();
        for given in [
            r#"Resources["InfoTableFunctions"].Sort"#,
            "Resources['InfoTableFunctions'].Sort({ t: rows, sortColumn: 'a' });",
            r#"Resources["InfoTableFunctions"].Sort({ rows: [], keys: ["a"] })"#,
            "Resources/InfoTableFunctions.Sort",
            "resources/InfoTableFunctions/Services/Sort",
        ] {
            let parsed = resource(given);
            assert_eq!(parsed.shown, "Resources/InfoTableFunctions.Sort", "{given}");
            assert_eq!(
                parsed.classes,
                ["InfoTableFunctions", "Resource"],
                "{given}"
            );
            assert_eq!(parsed.member, "Sort");
            assert!(!parsed.entity);
        }
        let thing = resource(r#"Things["Acme.Orders.Pump"].GetPropertyValues()"#);
        assert_eq!(thing.shown, "Things/Acme.Orders.Pump.GetPropertyValues");
        assert_eq!(thing.classes, ["Thing"]);
        assert!(thing.entity);
        assert_eq!(resource("Things/Acme.Orders.Pump.GetPropertyValues"), thing);
        assert_eq!(
            resource("/Thingworx/Things/Acme.Orders.Pump/services/GetPropertyValues"),
            thing
        );
        let odd = resource(r#"Things["Pump (1)"].GetPropertyValues()"#);
        assert_eq!(odd.shown, "Things/Pump (1).GetPropertyValues");
        let bracketed = resource(r#"Things["Pump [2]"].GetPropertyValues({ a: [1] })"#);
        assert_eq!(bracketed.shown, "Things/Pump [2].GetPropertyValues");
        assert_eq!(
            resource("me.GetPropertyValues([x])").member,
            "GetPropertyValues"
        );
        assert_eq!(
            resource(r#"me.GetPropertyValues({ names: ["a"] })"#).member,
            "GetPropertyValues"
        );
        assert_eq!(
            resource("Things/Acme.Orders.Pump.GetPropertyValues({ a: ['x'] })"),
            thing
        );
        let me = resource("me.GetPropertyValues");
        assert_eq!((me.classes, me.entity), (vec!["Thing".to_string()], true));
        let class = resource("com.thingworx.types.InfoTable.getRowCount");
        assert_eq!(class.classes, ["com.thingworx.types.InfoTable"]);
        assert_eq!(class.member, "getRowCount");
        for bad in [
            "Sort",
            "Widgets/X.Y",
            r#"Things["A".Y"#,
            "Things/A",
            "me.",
            "a b.c",
            "Resources/X.Y.Z/Services/",
        ] {
            assert!(parse_service_ref(bad).is_err(), "{bad}");
        }
    }

    const THING_TYPES: &str = r#"typeSearchIndex = [{"p":"com.thingworx.entities","l":"RootEntity"},{"p":"com.thingworx.things","l":"Thing"},{"p":"other.things","l":"Thing"},{"p":"com.thingworx.resources.queries","l":"InfoTableFunctions"},{"p":"com.thingworx.subsystems","l":"Subsystem"}];"#;
    const THING_MEMBERS: &str = r#"memberSearchIndex = [{"p":"com.thingworx.entities","c":"RootEntity","l":"GetDescription()"},{"p":"com.thingworx.things","c":"Thing","l":"GetPropertyValues()"},{"p":"com.thingworx.resources.queries","c":"InfoTableFunctions","l":"Sort(InfoTable, String)"}];"#;

    fn page(name: &str, tree: &str, member: &str) -> String {
        format!(
            r#"<html><body><div class="inheritance" title="Inheritance Tree">java.lang.Object{tree}</div>
<section class="class-description" id="class-description"><div class="block">The {name} class.</div></section>
<section class="method-details"><section class="detail" id="{member}()"><h3>{member}</h3>
<div class="member-signature"><span class="return-type">String</span>&nbsp;<span class="element-name">{member}</span><span class="parameters">()</span></div>
<div class="block">Service Category: Entity<br>Service Description: Does {member}.</div></section></section></body></html>"#
        )
    }

    fn fake_site(path: &str) -> Result<Vec<u8>, help::HelpError> {
        let root = r#"<a href="../entities/RootEntity.html" title="class in com.thingworx.entities">com.thingworx.entities.RootEntity</a>"#;
        let body = match path {
            TYPE_INDEX => THING_TYPES.to_string(),
            MEMBER_INDEX => THING_MEMBERS.to_string(),
            "com/thingworx/things/Thing.html" => page(
                "Thing",
                &format!(
                    r#"<div class="inheritance">{root}<div class="inheritance">com.thingworx.things.Thing</div></div>"#
                ),
                "GetPropertyValues",
            ),
            "com/thingworx/entities/RootEntity.html" => page("RootEntity", "", "GetDescription"),
            "com/thingworx/subsystems/Subsystem.html" => page(
                "Subsystem",
                &format!(
                    r#"<div class="inheritance">{root}<div class="inheritance">com.thingworx.subsystems.Subsystem</div></div>"#
                ),
                "Start",
            ),
            "com/thingworx/resources/queries/InfoTableFunctions.html" => String::from_utf8_lossy(
                include_bytes!("../../tests/fixtures/InfoTableFunctions.html"),
            )
            .into_owned(),
            other => panic!("fetched {other}, which no reference should reach"),
        };
        Ok(body.into_bytes())
    }

    #[test]
    fn a_service_is_found_on_its_class_or_the_class_it_extends() {
        let index = Index::parse(THING_TYPES, THING_MEMBERS).unwrap();
        let thing = find_platform_class(&index, "Thing").unwrap();
        assert_eq!(
            thing.package, "com.thingworx.things",
            "the platform's Thing wins"
        );
        let html = fake_site("com/thingworx/things/Thing.html").unwrap();
        assert_eq!(
            superclasses(&html, &index)
                .iter()
                .map(Class::qualified)
                .collect::<Vec<_>>(),
            ["com.thingworx.entities.RootEntity"]
        );

        let own = read_service(&fake_site, "me.GetPropertyValues").unwrap();
        assert_eq!(own.declared_in, "com.thingworx.things.Thing");
        assert!(!own.inherited);
        assert!(
            own.reading.markdown.contains("Does GetPropertyValues."),
            "{}",
            own.reading.markdown
        );

        assert_eq!(own.note, None);
        let inherited = read_service(&fake_site, r#"Things["Acme.Pump"].GetDescription"#).unwrap();
        assert_eq!(inherited.asked, "Things/Acme.Pump.GetDescription");
        assert_eq!(inherited.declared_in, "com.thingworx.entities.RootEntity");
        assert!(inherited.inherited);

        let resource = read_service(&fake_site, r#"Resources["InfoTableFunctions"].Sort"#).unwrap();
        assert_eq!(
            resource.declared_in,
            "com.thingworx.resources.queries.InfoTableFunctions"
        );
        assert_eq!(resource.reading.methods, 2);
        assert_eq!(resource.note, None);

        // A Subsystem the Javadoc lacks falls back to its type's class, and says so.
        let general =
            read_service(&fake_site, "Subsystems/NoSuchSubsystem.GetDescription").unwrap();
        assert_eq!(general.declared_in, "com.thingworx.entities.RootEntity");
        assert_eq!(
            general.note.as_deref(),
            Some(
                "the Javadoc has no class NoSuchSubsystem; this is the member every Subsystem has"
            )
        );
    }

    #[test]
    fn a_service_nobody_declares_says_where_it_looked_and_where_the_name_is() {
        let error = read_service(&fake_site, "Things/Acme.Pump.Sort").unwrap_err();
        let message = error.to_string();
        assert!(
            message.starts_with("Things/Acme.Pump.Sort: no member Sort in com.thingworx.things.Thing, com.thingworx.entities.RootEntity"),
            "{message}"
        );
        assert!(
            message
                .contains("Sort is declared by com.thingworx.resources.queries.InfoTableFunctions"),
            "{message}"
        );
        assert!(message.contains("twaco catalog"), "{message}");
        assert!(matches!(error, ServiceError::Invalid(_)));
        let error = read_service(&fake_site, "Resources/Nowhere.Sort")
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with(
                "Resources/Nowhere.Sort: the Javadoc has no class Nowhere or Resource"
            ),
            "{error}"
        );
        assert!(
            !error.contains("catalog"),
            "a Resource is the platform's: {error}"
        );
    }
}
