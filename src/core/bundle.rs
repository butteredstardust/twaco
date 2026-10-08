//! Assembling one importable document from the split entity files.
//!
//! The repository keeps one entity per file so sidecars and diffs stay readable, but importing
//! a hundred files one at a time is slow. ThingWorx's own exporter emits a single `<Entities>`
//! document, and this produces the same thing from the split files.
//!
//! **Entity XML is copied through as bytes.** The body of each collection is sliced out
//! textually and concatenated; nothing is re-serialised, so CDATA script bodies, `<json>`
//! wrappers and formatting survive exactly. The result is re-scanned and its entity set compared
//! against the sources before anyone is told it worked.

use super::config::Solution;
use super::entity;
use super::scan::{self, Kind};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// The document element every ThingWorx export and every bundle is wrapped in.
///
/// The line breaks inside the tag are how ThingWorx itself writes it, and are kept so a bundle
/// diffs cleanly against a Composer export. A bundle whose header disagrees with what the
/// platform expects can import as an empty document and still report success.
const ENTITIES_OPEN: &str =
    "<Entities\n majorVersion=\"10\"\n minorVersion=\"1\"\n universal=\"password\">";
const ENTITIES_CLOSE: &str = "</Entities>";
const XML_DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";

/// Every collection ThingWorx's exporter emits, ordered so a definition precedes what fills it.
///
/// Composer's own export puts Things *before* ThingTemplates. That is fine for content, because
/// the platform resolves references internally, but not for a configuration table whose columns
/// changed: the Thing's rows arrive while its template still declares the old columns, and the
/// values land empty. Importing the template before the Thing preserves those values.
pub const COLLECTION_ORDER: &[&str] = &[
    "StyleDefinitions",
    "Networks",
    "PersistenceProviderPackages",
    "QueueProviders",
    "ScriptFunctionLibraries",
    "Projects",
    "Users",
    "DirectoryServices",
    "Widgets",
    "MediaEntities",
    "Groups",
    "PersistenceProviders",
    "ModelTags",
    "LocalizationTables",
    "Dashboards",
    "StyleThemes",
    "DataShapes",
    "ThingShapes",
    "ThingTemplates",
    "Things",
    // ThingWorx 10.2 AI collections. Both come after Things: an MCPNamespace tool names a Thing or
    // ThingTemplate provider, and an AIAgent names the namespaces it draws tools from.
    "MCPNamespaces",
    "AIAgents",
    "Mashups",
    "Logs",
    "Authenticators",
    "QueueProviderPackages",
    "ThingPackages",
    "NotificationDefinitions",
    "ApplicationKeys",
    "StateDefinitions",
    "ExtensionPackages",
    "Organizations",
    "Menus",
    "ThingGroups",
    "Resources",
    "DataTags",
    "Subsystems",
    "NotificationContents",
];

/// Collections a server before 10.2 does not know, written only when a bundle has some.
const SINCE_10_2: &[&str] = &["MCPNamespaces", "AIAgents"];

#[derive(Debug)]
pub enum BundleError {
    Unreadable {
        path: PathBuf,
        why: String,
    },
    NotWellFormed {
        path: PathBuf,
        why: String,
    },
    NoCollection {
        path: PathBuf,
    },
    UnknownCollection {
        path: PathBuf,
        tag: String,
    },
    Empty,
    /// The assembled document does not contain what the sources did.
    Verification {
        missing: Vec<String>,
        unexpected: Vec<String>,
    },
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BundleError::Unreadable { path, why } => write!(f, "cannot read {}: {why}", path.display()),
            BundleError::NotWellFormed { path, why } => {
                write!(f, "{}: not well-formed XML: {why}", path.display())
            }
            BundleError::NoCollection { path } => {
                write!(f, "{}: no top-level entity collection", path.display())
            }
            BundleError::UnknownCollection { path, tag } => write!(
                f,
                "{}: <{tag}> is not a collection in the known ThingWorx order",
                path.display()
            ),
            BundleError::Empty => write!(f, "no entity XML to bundle"),
            BundleError::Verification { missing, unexpected } => write!(
                f,
                "the bundle does not match its sources: missing {missing:?}, unexpected {unexpected:?}"
            ),
        }
    }
}

impl std::error::Error for BundleError {}

/// One assembled document and what went into it.
pub struct Bundle {
    pub bytes: Vec<u8>,
    pub files: usize,
    /// How many times each `(collection, name)` appeared. Counted rather than collected:
    /// a set cannot tell one copy of an entity from two, and two is a bundle that imports the
    /// same thing twice with whichever body happens to come last winning.
    pub entities: BTreeMap<(String, String), usize>,
}

/// What a bundle should contain.
pub struct Selection<'a> {
    /// Only these collections. Empty means all of them.
    pub collections: Option<BTreeSet<&'a str>>,
}

impl Selection<'_> {
    pub fn everything() -> Self {
        Selection { collections: None }
    }

    /// A backend distribution: everything except the collections a designer owns in Composer.
    ///
    /// The split is by collection, never by inspecting an entity. Easy to state, impossible to
    /// get subtly wrong, and it is what lets backend work deploy without rolling back mashups
    /// that have not been exported back into the repository yet.
    pub fn backend(solution: &Solution) -> Selection<'_> {
        let ui: BTreeSet<&str> = solution
            .bundle
            .ui_collections
            .iter()
            .map(String::as_str)
            .collect();
        let kept = COLLECTION_ORDER
            .iter()
            .copied()
            .filter(|tag| !ui.contains(tag))
            .collect();
        Selection {
            collections: Some(kept),
        }
    }

    pub fn wants(&self, tag: &str) -> bool {
        self.collections
            .as_ref()
            .is_none_or(|allowed| allowed.contains(tag))
    }
}

/// Assemble the entity files under `root` into one document.
pub fn build(files: &[PathBuf], selection: &Selection) -> Result<Bundle, BundleError> {
    let mut bodies: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut entities: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut used = 0usize;

    for path in files {
        let text = std::fs::read_to_string(path).map_err(|e| BundleError::Unreadable {
            path: path.clone(),
            why: e.to_string(),
        })?;
        let bytes = text.as_bytes();
        let tokens = scan::tokenize(bytes).map_err(|e| BundleError::NotWellFormed {
            path: path.clone(),
            why: e.to_string(),
        })?;

        let sliced = slice_collections(&tokens, bytes, path)?;
        let mut took_any = false;
        for (tag, body) in sliced {
            if !COLLECTION_ORDER.contains(&tag.as_str()) {
                return Err(BundleError::UnknownCollection {
                    path: path.clone(),
                    tag,
                });
            }
            if !selection.wants(&tag) {
                continue;
            }
            if body.trim().is_empty() {
                continue;
            }
            for name in names_in(&tokens, bytes, &tag) {
                *entities.entry((tag.clone(), name)).or_insert(0) += 1;
            }
            bodies.entry(tag).or_default().push(body);
            took_any = true;
        }
        if took_any {
            used += 1;
        }
    }

    if bodies.is_empty() {
        return Err(BundleError::Empty);
    }

    // Every collection appears, empty ones included: this is what ThingWorx's own exporter
    // emits, and a bundle that diffs cleanly against a Composer export is easier to trust. The
    // 10.2 collections are the exception: a 10.1 server has never heard of them, so they appear
    // only when they hold something.
    let mut parts: Vec<String> = vec![XML_DECLARATION.to_string(), ENTITIES_OPEN.to_string()];
    for tag in COLLECTION_ORDER {
        if SINCE_10_2.contains(tag) && !bodies.contains_key(*tag) {
            continue;
        }
        match bodies.get(*tag) {
            Some(collected) => {
                parts.push(format!("    <{tag}>"));
                parts.extend(collected.iter().cloned());
                parts.push(format!("    </{tag}>"));
            }
            None => parts.push(format!("    <{tag}></{tag}>")),
        }
    }
    parts.push(ENTITIES_CLOSE.to_string());

    let bytes = (parts.join("\n") + "\n").into_bytes();
    verify(&bytes, &entities)?;
    Ok(Bundle {
        bytes,
        files: used,
        entities,
    })
}

/// The raw inner text of each top-level collection in one document.
///
/// Sliced by byte range rather than re-serialised, which is the whole point: a collection's body
/// reaches the bundle exactly as it sits in the source file.
fn slice_collections(
    tokens: &[scan::Token],
    src: &[u8],
    path: &Path,
) -> Result<Vec<(String, String)>, BundleError> {
    let wrapper = tokens
        .iter()
        .position(|t| t.kind == Kind::Start && t.name.of(src) == b"Entities")
        .ok_or_else(|| BundleError::NoCollection {
            path: path.to_path_buf(),
        })?;
    // Name-checked: depth alone accepts `<Things><Thing name="A"></Things></Thing>`, and the
    // body sliced from that is not the collection.
    let wrapper_end =
        scan::element_end_in(tokens, src, wrapper).ok_or_else(|| BundleError::NotWellFormed {
            path: path.to_path_buf(),
            why: "<Entities> is not closed by a matching tag".to_string(),
        })?;

    let mut out = Vec::new();
    let mut index = wrapper + 1;
    while index < wrapper_end {
        match tokens[index].kind {
            Kind::Start => {
                let tag = String::from_utf8_lossy(tokens[index].name.of(src)).into_owned();
                let end = scan::element_end_in(tokens, src, index).ok_or_else(|| {
                    BundleError::NotWellFormed {
                        path: path.to_path_buf(),
                        why: format!("<{tag}> is not closed by a matching tag"),
                    }
                })?;
                // The body is everything between the collection's own tags, trimmed of the
                // newline each one sits on so the joined result lines up.
                let body = &src[tokens[index].span.end..tokens[end].span.start];
                out.push((tag, trim_edge_newlines(&String::from_utf8_lossy(body))));
                index = end + 1;
            }
            Kind::Empty => {
                let tag = String::from_utf8_lossy(tokens[index].name.of(src)).into_owned();
                out.push((tag, String::new()));
                index += 1;
            }
            _ => index += 1,
        }
    }
    if out.is_empty() {
        return Err(BundleError::NoCollection {
            path: path.to_path_buf(),
        });
    }
    Ok(out)
}

/// Drop the newline immediately inside each collection tag, keeping the indentation between.
fn trim_edge_newlines(body: &str) -> String {
    let without_leading = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'))
        .unwrap_or(body);
    // Only whitespace that forms the closing tag's own indentation is dropped, and only when a
    // newline precedes it. A body that ends in a tab with no newline is content, not layout.
    let trimmed = without_leading.trim_end_matches([' ', '\t']);
    match trimmed
        .strip_suffix('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s))
    {
        Some(shorter) => shorter.to_string(),
        None => without_leading.to_string(),
    }
}

/// The names of entities inside every collection of one tag in a document.
///
/// Every, not the first: a source file may legitimately hold two `<Things>` sections, and
/// reading only the first counted one body's entities while bundling both.
fn names_in(tokens: &[scan::Token], src: &[u8], collection: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut at = 0usize;
    while at < tokens.len() {
        if tokens[at].kind != Kind::Start || tokens[at].name.of(src) != collection.as_bytes() {
            at += 1;
            continue;
        }
        let Some(end) = scan::element_end_in(tokens, src, at) else {
            at += 1;
            continue;
        };
        let mut index = at + 1;
        while index < end {
            if matches!(tokens[index].kind, Kind::Start | Kind::Empty) {
                if let Ok(Some(span)) = scan::attribute(src, &tokens[index], "name") {
                    names.push(scan::decode_entities(&String::from_utf8_lossy(
                        span.of(src),
                    )));
                }
                index = scan::element_end_in(tokens, src, index).map_or(end, |e| e + 1);
            } else {
                index += 1;
            }
        }
        at = end + 1;
    }
    names
}

/// Re-scan the assembled document and confirm it holds exactly what the sources declared.
///
/// A bundle is imported wholesale. One entity silently missing is a rollback nobody notices
/// until the feature it belonged to stops working.
fn verify(bytes: &[u8], expected: &BTreeMap<(String, String), usize>) -> Result<(), BundleError> {
    let tokens = match scan::tokenize(bytes) {
        Ok(t) => t,
        Err(e) => {
            return Err(BundleError::Verification {
                missing: vec![format!("the bundle will not parse: {e}")],
                unexpected: Vec::new(),
            })
        }
    };
    let mut found: BTreeMap<(String, String), usize> = BTreeMap::new();
    for tag in COLLECTION_ORDER {
        for name in names_in(&tokens, bytes, tag) {
            *found.entry(((*tag).to_string(), name)).or_insert(0) += 1;
        }
    }

    let mut missing = Vec::new();
    let mut unexpected = Vec::new();
    for (key, wanted) in expected {
        let got = found.get(key).copied().unwrap_or(0);
        if got < *wanted {
            missing.push(format!("{}/{} ({got} of {wanted})", key.0, key.1));
        } else if got > *wanted {
            unexpected.push(format!("{}/{} ({got}, expected {wanted})", key.0, key.1));
        }
        // A duplicate in the sources is a defect of its own: two files declaring the same
        // entity means one body silently wins on import.
        if *wanted > 1 {
            unexpected.push(format!(
                "{}/{} is declared {wanted} times in the sources",
                key.0, key.1
            ));
        }
    }
    for (key, got) in &found {
        if !expected.contains_key(key) {
            unexpected.push(format!("{}/{} ({got}, not in the sources)", key.0, key.1));
        }
    }
    if missing.is_empty() && unexpected.is_empty() {
        Ok(())
    } else {
        Err(BundleError::Verification {
            missing,
            unexpected,
        })
    }
}

/// Entity files for a solution, in the order the bundle wants them.
///
/// Sorted by collection folder and then by filename, which is what makes two runs produce the
/// same bytes.
pub fn source_files(solution: &Solution) -> Vec<PathBuf> {
    // Projects in deploy order, so a dependency's entities precede the entities that bind to
    // them within each collection. Falls back to declaration order only when the order cannot
    // be computed, which validation already refuses to load.
    let ordered: Vec<&super::config::Project> = solution
        .deploy_order()
        .unwrap_or_else(|_| solution.projects.iter().collect());

    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for tag in COLLECTION_ORDER {
        for project in &ordered {
            let dir = solution.project_root(project).join(tag);
            let mut here = Vec::new();
            collect_xml(&dir, &mut here);
            // Case-insensitively, with the exact name as a tie-break. Filesystem path comparison
            // differs by platform, so `Acme.First_CT` and `Acme.Second_DS` can otherwise change
            // order. Pinning the rule makes the bytes the same on every machine.
            here.sort_by(|a, b| {
                let key = |p: &PathBuf| p.to_string_lossy().to_lowercase();
                key(a).cmp(&key(b)).then_with(|| a.cmp(b))
            });
            // Two projects may share a root. Bundling the same file twice would emit the entity
            // twice, which verification now refuses rather than accepts.
            for path in here {
                if seen.insert(path.clone()) {
                    out.push(path);
                }
            }
        }
    }
    // Every entity the solution holds, wherever it is filed. One in a folder not named for its
    // collection (a DataTable under DataTables/) would otherwise never be read, and so never
    // bundled or deployed, with nothing to say so; one whose collection is unknown is refused
    // by name in `build`.
    for entity in super::workspace::discover(solution).entities {
        if seen.insert(entity.path.clone()) {
            out.push(entity.path);
        }
    }
    out
}

/// Every `.xml` beneath a collection directory, including subfolders.
///
/// Recursive, to agree with how entities are discovered elsewhere. A flat read made
/// `Things/nested/A.xml` invisible to the bundle *and* to its verification, so an entity could
/// go missing with nothing reporting it.
fn collect_xml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_xml(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("xml"))
        {
            out.push(path);
        }
    }
}

/// Entities that reference something the bundle does not contain.
///
/// Dependencies matter inside a collection too. The collection order above is
/// what the platform's own importer expects and the stable bundle order used here, so this
/// reports rather than reorders: a ThingTemplate deriving from one that is not here is a real
/// problem, and one that merely sorts later is not.
/// [`dangling_references`] for the bundle `twaco bundle` builds: everything, or with
/// `backend_only` the collections a designer does not own.
pub fn dangling_in_selection(solution: &Solution, backend_only: bool) -> Vec<String> {
    let selection = if backend_only {
        Selection::backend(solution)
    } else {
        Selection::everything()
    };
    let all = source_files(solution);
    let selected: Vec<PathBuf> = all
        .iter()
        .filter(|path| {
            std::fs::read(path)
                .ok()
                .and_then(|bytes| entity::parse(&bytes).ok())
                .is_some_and(|info| selection.wants(&info.collection))
        })
        .cloned()
        .collect();
    dangling_references(&selected, &all)
}

pub fn dangling_references(selected: &[PathBuf], all: &[PathBuf]) -> Vec<String> {
    let mut present = BTreeSet::new();
    let mut wanted: Vec<(String, String, String)> = Vec::new();

    // What the whole solution defines, so a reference to a platform entity such as
    // GenericThing is not mistaken for a missing one. Only a name this repository owns and
    // this bundle leaves out is worth reporting.
    let mut owned = BTreeSet::new();
    for path in all {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(info) = entity::parse(text.as_bytes()) {
                owned.insert(info.name);
            }
        }
    }

    for path in selected {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let bytes = text.as_bytes();
        let Ok(info) = entity::parse(bytes) else {
            continue;
        };
        present.insert(info.name.clone());
        let Ok(tokens) = scan::tokenize(bytes) else {
            continue;
        };
        for token in tokens
            .iter()
            .filter(|t| matches!(t.kind, Kind::Start | Kind::Empty))
        {
            for attribute in ["baseThingTemplate", "thingTemplate", "baseDataShape"] {
                if let Ok(Some(span)) = scan::attribute(bytes, token, attribute) {
                    let value = scan::decode_entities(&String::from_utf8_lossy(span.of(bytes)));
                    if !value.is_empty() {
                        wanted.push((info.name.clone(), attribute.to_string(), value));
                    }
                }
            }
        }
    }

    let mut out: Vec<String> = wanted
        .into_iter()
        .filter(|(_, _, target)| !present.contains(target) && owned.contains(target))
        .map(|(from, attribute, target)| format!("{from} names {target} as its {attribute}"))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backend_bundle_names_what_it_leaves_out_and_a_whole_one_does_not() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-bundle-notes-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::create_dir_all(root.join("ThingTemplates")).unwrap();
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]
name = \"P\"

[bundle]
ui_collections = [\"ThingTemplates\"]
",
        )
        .unwrap();
        std::fs::write(
            root.join("ThingTemplates/P.TT.xml"),
            "<Entities><ThingTemplates><ThingTemplate name=\"P.TT\" projectName=\"P\"></ThingTemplate></ThingTemplates></Entities>",
        )
        .unwrap();
        std::fs::write(
            root.join("Things/P.T.xml"),
            "<Entities><Things><Thing name=\"P.T\" projectName=\"P\" thingTemplate=\"P.TT\"></Thing></Things></Entities>",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        assert!(dangling_in_selection(&solution, false).is_empty());
        assert_eq!(
            dangling_in_selection(&solution, true),
            ["P.T names P.TT as its thingTemplate"]
        );
    }

    #[test]
    fn the_body_of_a_collection_is_sliced_without_its_own_newlines() {
        assert_eq!(
            trim_edge_newlines("\n        <Thing/>\n    "),
            "        <Thing/>"
        );
        assert_eq!(trim_edge_newlines("\r\n  <A/>\r\n  "), "  <A/>");
        assert_eq!(trim_edge_newlines(""), "");
    }

    #[test]
    fn a_backend_selection_drops_the_designers_collections() {
        let mut solution: Solution = toml::from_str("[[project]]\nname = \"P\"\n").unwrap();
        solution.bundle.ui_collections = vec!["Mashups".to_string(), "MediaEntities".to_string()];
        let selection = Selection::backend(&solution);
        assert!(selection.wants("Things"));
        assert!(
            !selection.wants("Mashups"),
            "a designer's collection is left alone"
        );
        assert!(!selection.wants("MediaEntities"));
    }

    #[test]
    fn everything_wants_every_collection() {
        let selection = Selection::everything();
        assert!(selection.wants("Mashups"));
        assert!(selection.wants("Things"));
    }

    #[test]
    fn verification_names_what_went_missing() {
        let expected: BTreeMap<(String, String), usize> =
            [(("Things".to_string(), "A".to_string()), 1)]
                .into_iter()
                .collect();
        let empty = format!("{XML_DECLARATION}\n{ENTITIES_OPEN}\n{ENTITIES_CLOSE}\n");
        match verify(empty.as_bytes(), &expected) {
            Err(BundleError::Verification { missing, .. }) => {
                assert_eq!(missing, vec!["Things/A (0 of 1)"])
            }
            other => panic!("expected a verification failure, got {other:?}"),
        }
    }

    #[test]
    fn verification_catches_an_entity_bundled_twice() {
        // A set could not tell one "Things/A" from two, so a bundle importing the same entity
        // twice -- last body wins -- passed verification.
        let expected: BTreeMap<(String, String), usize> =
            [(("Things".to_string(), "A".to_string()), 1)]
                .into_iter()
                .collect();
        let doubled = format!(
            "{XML_DECLARATION}\n{ENTITIES_OPEN}\n    <Things><Thing name=\"A\"></Thing><Thing name=\"A\"></Thing></Things>\n{ENTITIES_CLOSE}\n"
        );
        match verify(doubled.as_bytes(), &expected) {
            Err(BundleError::Verification { unexpected, .. }) => {
                assert!(
                    unexpected.iter().any(|u| u.contains("Things/A")),
                    "got {unexpected:?}"
                );
            }
            other => panic!("expected a verification failure, got {other:?}"),
        }
    }

    #[test]
    fn a_body_ending_in_a_tab_keeps_it() {
        // Trailing whitespace is layout only when a newline precedes it; otherwise it is content.
        assert_eq!(trim_edge_newlines("<Thing/>\t"), "<Thing/>\t");
    }

    #[test]
    fn the_collection_order_puts_definitions_before_what_fills_them() {
        let position = |tag: &str| COLLECTION_ORDER.iter().position(|t| *t == tag).unwrap();
        assert!(position("Things") < position("MCPNamespaces"));
        assert!(position("MCPNamespaces") < position("AIAgents"));
        assert!(position("DataShapes") < position("ThingShapes"));
        assert!(position("ThingShapes") < position("ThingTemplates"));
        // The one that mattered: a Thing's rows must not arrive before its template's columns.
        assert!(position("ThingTemplates") < position("Things"));
        assert!(position("Things") < position("Mashups"));
    }
}
