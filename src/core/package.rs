//! Packaging the repository for release, offline: a bundle of the solution or a
//! project (all, backend or frontend), a source-control zip, or extension zips.
//!
//! The extension format is the ThingWorx extension package layout: a zip with `metadata.xml` and one file
//! per entity at `Entities/<Collection>/<Name>.xml`, editability being only the entity
//! attribute `aspect.isEditableExtensionObject`. A solution is an outer zip of its projects'
//! extension zips. Entity files go in byte for byte, but for that one attribute.

use super::config::Solution;
use super::workspace::EntityFile;
use std::io::Write;
use std::path::PathBuf;

const EDITABLE: &str = "aspect.isEditableExtensionObject";

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("{0}")]
    Invalid(String),
    #[error("{}: {why}", .path.display())]
    Io { path: PathBuf, why: String },
}

/// The project an entity file belongs to: its own `projectName`, else where it was filed.
fn owner(entity: &EntityFile) -> &str {
    if entity.info.project.is_empty() {
        &entity.found_under
    } else {
        &entity.info.project
    }
}

/// The entity files of the solution, or of one project, in the bundle's source order.
fn entities(solution: &Solution, project: Option<&str>) -> Result<Vec<EntityFile>, PackageError> {
    if let Some(project) = project {
        if solution.project(project).is_none() {
            return Err(PackageError::Invalid(format!(
                "this solution has no project named {project}"
            )));
        }
    }
    let found = super::workspace::discover(solution);
    if !found.unreadable.is_empty() {
        return Err(PackageError::Invalid(format!(
            "unreadable entity files: {}",
            found.unreadable.join("; ")
        )));
    }
    let order = super::bundle::source_files(solution);
    let mut chosen: Vec<EntityFile> = found
        .entities
        .into_iter()
        .filter(|e| project.is_none_or(|p| owner(e) == p))
        .collect();
    chosen.sort_by_key(|e| {
        order
            .iter()
            .position(|p| *p == e.path)
            .unwrap_or(usize::MAX)
    });
    if chosen.is_empty() {
        return Err(PackageError::Invalid(
            "nothing to package: no entity files".to_string(),
        ));
    }
    Ok(chosen)
}

fn read(path: &std::path::Path) -> Result<Vec<u8>, PackageError> {
    std::fs::read(path).map_err(|e| PackageError::Io {
        path: path.to_path_buf(),
        why: e.to_string(),
    })
}

/// An entity file's bytes, refused when it holds more than the one entity it is filed as: a
/// zip entry is one entity, and a second one inside would travel under the first's name.
fn read_one(entity: &EntityFile) -> Result<Vec<u8>, PackageError> {
    let bytes = read(&entity.path)?;
    let held = super::imports::entities_in_xml(&bytes)
        .map(|found| found.len())
        .unwrap_or(0);
    if held > 1 {
        return Err(PackageError::Invalid(format!(
            "{} holds {held} entities; package one entity per file",
            entity.path.display()
        )));
    }
    Ok(bytes)
}

/// A name as one zip path component: never a separator, a dot folder, a drive or a control
/// character, so that no archive twaco writes can unpack outside its folder.
fn component(name: &str) -> Result<&str, PackageError> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control());
    if bad {
        return Err(PackageError::Invalid(format!(
            "{name:?} cannot be a file name in a package"
        )));
    }
    Ok(name)
}

/// Each entry once: two entity files that land on one name are refused, naming both.
fn add(
    entries: &mut Vec<(String, Vec<u8>)>,
    from: &mut std::collections::BTreeMap<String, PathBuf>,
    name: String,
    entity: &EntityFile,
    bytes: Vec<u8>,
) -> Result<(), PackageError> {
    if let Some(first) = from.insert(name.clone(), entity.path.clone()) {
        return Err(PackageError::Invalid(format!(
            "{} and {} are both {name}",
            first.display(),
            entity.path.display()
        )));
    }
    entries.push((name, bytes));
    Ok(())
}

/// Which collections a bundle holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    All,
    /// Everything but `[bundle] ui_collections`.
    Backend,
    /// Only `[bundle] ui_collections`.
    Frontend,
}

/// One importable document of the solution or a project, as `bundle` builds it.
pub fn bundle(
    solution: &Solution,
    project: Option<&str>,
    part: Part,
) -> Result<super::bundle::Bundle, PackageError> {
    let files: Vec<PathBuf> = entities(solution, project)?
        .into_iter()
        .map(|e| e.path)
        .collect();
    let selection = match part {
        Part::All => super::bundle::Selection::everything(),
        Part::Backend => super::bundle::Selection::backend(solution),
        Part::Frontend => {
            if solution.bundle.ui_collections.is_empty() {
                return Err(PackageError::Invalid(
                    "a frontend bundle needs [bundle] ui_collections in twaco.toml, the collections a designer owns".to_string(),
                ));
            }
            super::bundle::Selection {
                collections: Some(
                    solution
                        .bundle
                        .ui_collections
                        .iter()
                        .map(String::as_str)
                        .collect(),
                ),
            }
        }
    };
    // `build` itself refuses an entity declared twice.
    super::bundle::build(&files, &selection).map_err(|e| PackageError::Invalid(e.to_string()))
}

fn zip_bytes(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, PackageError> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            zip.start_file(name.as_str(), options)
                .map_err(|e| PackageError::Invalid(e.to_string()))?;
            zip.write_all(bytes)
                .map_err(|e| PackageError::Invalid(e.to_string()))?;
        }
        zip.finish()
            .map_err(|e| PackageError::Invalid(e.to_string()))?;
    }
    Ok(buffer.into_inner())
}

/// The source-control layout, `<Project>/<Collection>/<Name>.xml`, as a zip.
pub fn source_control(
    solution: &Solution,
    project: Option<&str>,
) -> Result<(Vec<u8>, usize), PackageError> {
    let chosen = entities(solution, project)?;
    let mut entries = Vec::new();
    let mut from = std::collections::BTreeMap::new();
    for entity in &chosen {
        let name = format!(
            "{}/{}/{}.xml",
            component(owner(entity))?,
            component(&entity.info.collection)?,
            component(&entity.info.name)?
        );
        add(&mut entries, &mut from, name, entity, read_one(entity)?)?;
    }
    Ok((zip_bytes(&entries)?, entries.len()))
}

/// An entity document with its root entity's editability set: the attribute's value replaced
/// where present, the attribute added after the element name where not. Nothing else moves.
pub fn set_editable(src: &[u8], editable: bool) -> Result<Vec<u8>, PackageError> {
    let bad = |why: String| PackageError::Invalid(format!("an entity document {why}"));
    let tokens = super::scan::tokenize(src).map_err(|e| bad(format!("will not scan: {e}")))?;
    let at = super::sidecar::entity_element(&tokens, src)
        .ok_or_else(|| bad("has no entity element".to_string()))?;
    let token = &tokens[at];
    let value = if editable { "true" } else { "false" };
    let mut out = Vec::with_capacity(src.len() + 48);
    match super::scan::attribute(src, token, EDITABLE).map_err(|e| bad(e.to_string()))? {
        Some(span) => {
            out.extend_from_slice(&src[..span.start]);
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(&src[span.end..]);
        }
        None => {
            out.extend_from_slice(&src[..token.name.end]);
            out.extend_from_slice(format!(" {EDITABLE}=\"{value}\"").as_bytes());
            out.extend_from_slice(&src[token.name.end..]);
        }
    }
    Ok(out)
}

/// What a project's `ExtensionPackage` says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub version: String,
    pub group: String,
    pub vendor: String,
    pub minimum_thingworx: String,
}

impl Metadata {
    pub fn from_solution(solution: &Solution) -> Metadata {
        let config = &solution.package;
        Metadata {
            version: config
                .version
                .clone()
                .unwrap_or_else(|| "1.0.0".to_string()),
            group: config
                .group
                .clone()
                .unwrap_or_else(|| solution.solution.name.clone()),
            vendor: config.vendor.clone().unwrap_or_default(),
            minimum_thingworx: config
                .minimum_thingworx
                .clone()
                .unwrap_or_else(|| "9.0.0".to_string()),
        }
    }
}

/// An attribute value, escaped; a character XML 1.0 cannot hold at all is refused.
fn escape(text: &str) -> Result<String, PackageError> {
    let allowed = |c: char| matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..);
    if let Some(c) = text.chars().find(|c| !allowed(*c)) {
        return Err(PackageError::Invalid(format!(
            "{text:?} holds {c:?}, which XML cannot hold"
        )));
    }
    Ok(text
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;"))
}

/// `metadata.xml` for one project, `dependsOn` from its `depends_on`.
pub fn metadata_xml(
    project: &str,
    depends_on: &[String],
    meta: &Metadata,
) -> Result<String, PackageError> {
    let depends: Vec<String> = depends_on.iter().map(|p| format!("{p}:0.0.0")).collect();
    let build = meta.version.rsplit('.').next().unwrap_or("0").to_string();
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Entities>\n    <ExtensionPackages>\n        <ExtensionPackage \
         artifactId=\"{}\" buildNumber=\"{}\" dependsOn=\"{}\" description=\"\" groupId=\"{}\" haCompatible=\"true\" \
         minimumThingWorxVersion=\"{}\" name=\"{}\" packageVersion=\"{}\" vendor=\"{}\"/>\n    </ExtensionPackages>\n</Entities>\n",
        escape(&project.to_lowercase())?,
        escape(&build)?,
        escape(&depends.join(","))?,
        escape(&meta.group)?,
        escape(&meta.minimum_thingworx)?,
        escape(project)?,
        escape(&meta.version)?,
        escape(&meta.vendor)?,
    ))
}

/// One project as an extension zip. Returns the bytes and how many entities it holds.
pub fn extension(
    solution: &Solution,
    project: &str,
    editable: bool,
    meta: &Metadata,
) -> Result<(Vec<u8>, usize), PackageError> {
    let declared = solution.project(project).ok_or_else(|| {
        PackageError::Invalid(format!("this solution has no project named {project}"))
    })?;
    let chosen = entities(solution, Some(project))?;
    let mut entries = vec![(
        "metadata.xml".to_string(),
        metadata_xml(project, &declared.depends_on, meta)?.into_bytes(),
    )];
    let mut from = std::collections::BTreeMap::new();
    for entity in &chosen {
        let bytes = set_editable(&read_one(entity)?, editable)
            .map_err(|e| PackageError::Invalid(format!("{}: {e}", entity.path.display())))?;
        let name = format!(
            "Entities/{}/{}.xml",
            component(&entity.info.collection)?,
            component(&entity.info.name)?
        );
        add(&mut entries, &mut from, name, entity, bytes)?;
    }
    Ok((zip_bytes(&entries)?, chosen.len()))
}

/// How many entities each project's package holds, in deploy order.
pub type Counts = Vec<(String, usize)>;

/// The solution as an outer zip of its projects' extension zips, in deploy order.
pub fn solution_extensions(
    solution: &Solution,
    editable: bool,
    meta: &Metadata,
) -> Result<(Vec<u8>, Counts), PackageError> {
    let order = solution
        .deploy_order()
        .map_err(|e| PackageError::Invalid(e.to_string()))?;
    let mut entries = Vec::new();
    let mut counts = Vec::new();
    for project in order {
        let (bytes, count) = extension(solution, &project.name, editable, meta)?;
        let kind = if editable {
            "devextension"
        } else {
            "extension"
        };
        entries.push((
            format!(
                "{}-{}-{kind}.zip",
                component(&project.name)?,
                component(&meta.version)?
            ),
            bytes,
        ));
        counts.push((project.name.clone(), count));
    }
    Ok((zip_bytes(&entries)?, counts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editability_is_set_on_the_entity_element_and_nothing_else_moves() {
        let src = b"<?xml version=\"1.0\"?>\n<Entities>\n    <Things>\n        <Thing\n         name=\"T\"\n         projectName=\"P\">\n            <x/>\n        </Thing>\n    </Things>\n</Entities>\n";
        let out = set_editable(src, true).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(
            text.contains("<Thing aspect.isEditableExtensionObject=\"true\"\n         name=\"T\""),
            "{text}"
        );
        assert_eq!(
            out.len(),
            src.len() + " aspect.isEditableExtensionObject=\"true\"".len()
        );
        // An existing value is replaced in place.
        let again = set_editable(&out, false).unwrap();
        assert!(String::from_utf8(again.clone())
            .unwrap()
            .contains("isEditableExtensionObject=\"false\""));
        assert_eq!(again.len(), out.len() + 1, "only the value changed");
        assert!(set_editable(b"<NotAnExport/>", true).is_err());
    }

    #[test]
    fn metadata_names_the_project_and_its_dependencies() {
        let meta = Metadata {
            version: "2.1.7".into(),
            group: "io.example".into(),
            vendor: "Me & Co".into(),
            minimum_thingworx: "9.6.0".into(),
        };
        let xml = metadata_xml("P.Two", &["P.One".to_string()], &meta).unwrap();
        for expected in [
            "name=\"P.Two\"",
            "packageVersion=\"2.1.7\"",
            "buildNumber=\"7\"",
            "artifactId=\"p.two\"",
            "dependsOn=\"P.One:0.0.0\"",
            "groupId=\"io.example\"",
            "vendor=\"Me &amp; Co\"",
            "minimumThingWorxVersion=\"9.6.0\"",
        ] {
            assert!(xml.contains(expected), "{expected} in {xml}");
        }
        assert!(
            crate::core::extensions::inspect(
                &zip_bytes(&[("metadata.xml".into(), xml.into_bytes())]).unwrap()
            )
            .is_ok(),
            "twaco's own extension check reads it"
        );
        let odd = Metadata {
            vendor: "bad \u{FFFE}".into(),
            ..meta
        };
        assert!(
            metadata_xml("P", &[], &odd).is_err(),
            "a character XML cannot hold"
        );
    }

    fn entity(collection: &str, tag: &str, name: &str, project: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Entities>\n    <{collection}>\n        <{tag}\n         \
             name=\"{name}\"\n         projectName=\"{project}\">\n            <x/>\n        </{tag}>\n    </{collection}>\n</Entities>\n"
        )
    }

    /// Two projects, P.Two depending on P.One, each in its own folder.
    fn two_projects(label: &str) -> (PathBuf, Solution) {
        let nonce = crate::test_nonce();
        let root = std::env::temp_dir().join(format!(
            "twaco-package-{label}-{}-{nonce}",
            std::process::id()
        ));
        for folder in ["one/Things", "two/Things", "two/Mashups"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        std::fs::write(
            root.join("twaco.toml"),
            "[solution]\nname = \"S\"\n\n[[project]]\nname = \"P.One\"\nroot = \"one\"\n\n\
             [[project]]\nname = \"P.Two\"\nroot = \"two\"\ndepends_on = [\"P.One\"]\n\n[package]\nversion = \"2.0.5\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("one/Things/P.One.T.xml"),
            entity("Things", "Thing", "P.One.T", "P.One"),
        )
        .unwrap();
        std::fs::write(
            root.join("two/Things/P.Two.T.xml"),
            entity("Things", "Thing", "P.Two.T", "P.Two"),
        )
        .unwrap();
        std::fs::write(
            root.join("two/Mashups/P.Two.M.xml"),
            entity("Mashups", "Mashup", "P.Two.M", "P.Two"),
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root, solution)
    }

    fn unzip(bytes: &[u8]) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        (0..archive.len())
            .map(|i| {
                let mut entry = archive.by_index(i).unwrap();
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
                (entry.name().to_string(), bytes)
            })
            .collect()
    }

    #[test]
    fn a_project_extension_holds_its_metadata_and_only_its_own_entities() {
        let (root, solution) = two_projects("ext");
        let meta = Metadata::from_solution(&solution);
        assert_eq!(
            (
                meta.version.as_str(),
                meta.group.as_str(),
                meta.minimum_thingworx.as_str()
            ),
            ("2.0.5", "S", "9.0.0")
        );
        let (bytes, count) = extension(&solution, "P.Two", true, &meta).unwrap();
        assert_eq!(count, 2);
        let files = unzip(&bytes);
        assert_eq!(
            files.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "Entities/Mashups/P.Two.M.xml",
                "Entities/Things/P.Two.T.xml",
                "metadata.xml"
            ]
        );
        let metadata = String::from_utf8(files["metadata.xml"].clone()).unwrap();
        assert!(
            metadata.contains("dependsOn=\"P.One:0.0.0\"") && metadata.contains("name=\"P.Two\""),
            "{metadata}"
        );
        let original = std::fs::read(root.join("two/Things/P.Two.T.xml")).unwrap();
        let packed = String::from_utf8(files["Entities/Things/P.Two.T.xml"].clone()).unwrap();
        assert_eq!(
            packed
                .replacen(" aspect.isEditableExtensionObject=\"true\"", "", 1)
                .as_bytes(),
            original,
            "only the aspect differs"
        );
        // The solution: one zip per project, dependencies first.
        let (outer, counts) = solution_extensions(&solution, false, &meta).unwrap();
        assert_eq!(counts, [("P.One".to_string(), 1), ("P.Two".to_string(), 2)]);
        let inner = unzip(&outer);
        assert_eq!(
            inner.keys().map(String::as_str).collect::<Vec<_>>(),
            ["P.One-2.0.5-extension.zip", "P.Two-2.0.5-extension.zip"]
        );
        let one = unzip(&inner["P.One-2.0.5-extension.zip"]);
        assert!(String::from_utf8_lossy(&one["Entities/Things/P.One.T.xml"])
            .contains("aspect.isEditableExtensionObject=\"false\""));
        assert!(
            extension(&solution, "P.Nope", true, &meta).is_err(),
            "an unknown project"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn source_control_and_bundles_follow_the_project_filter() {
        let (root, solution) = two_projects("sc");
        let (bytes, count) = source_control(&solution, None).unwrap();
        assert_eq!(count, 3);
        assert_eq!(
            unzip(&bytes).keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "P.One/Things/P.One.T.xml",
                "P.Two/Mashups/P.Two.M.xml",
                "P.Two/Things/P.Two.T.xml"
            ]
        );
        assert_eq!(source_control(&solution, Some("P.One")).unwrap().1, 1);
        assert!(source_control(&solution, Some("P.Nope")).is_err());
        assert_eq!(
            bundle(&solution, Some("P.Two"), Part::All)
                .unwrap()
                .entities
                .len(),
            2
        );
        assert!(
            bundle(&solution, None, Part::Frontend).is_err(),
            "no ui_collections declared"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn what_cannot_be_packaged_faithfully_is_refused() {
        // The same entity in two files.
        let (root, solution) = two_projects("twice");
        std::fs::create_dir_all(root.join("one/Things/old")).unwrap();
        std::fs::write(
            root.join("one/Things/old/copy.xml"),
            entity("Things", "Thing", "P.One.T", "P.One"),
        )
        .unwrap();
        let error = extension(
            &solution,
            "P.One",
            false,
            &Metadata::from_solution(&solution),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("copy.xml") && error.contains("P.One.T.xml"),
            "{error}"
        );
        assert!(source_control(&solution, Some("P.One")).is_err());
        let why = bundle(&solution, Some("P.One"), Part::All)
            .err()
            .unwrap()
            .to_string();
        assert!(why.contains("declared 2 times"), "{why}");
        let _ = std::fs::remove_dir_all(root);

        // A name that would leave the archive's folder.
        let (root, solution) = two_projects("name");
        std::fs::write(
            root.join("one/Things/evil.xml"),
            entity("Things", "Thing", "../../evil", "P.One"),
        )
        .unwrap();
        assert!(source_control(&solution, None)
            .unwrap_err()
            .to_string()
            .contains("cannot be a file name"));
        let _ = std::fs::remove_dir_all(root);

        // A file holding two entities.
        let (root, solution) = two_projects("two");
        let both = "<Entities><Things><Thing name=\"P.One.A\" projectName=\"P.One\"/><Thing name=\"P.One.B\" projectName=\"P.One\"/></Things></Entities>";
        std::fs::write(root.join("one/Things/both.xml"), both).unwrap();
        assert!(source_control(&solution, None)
            .unwrap_err()
            .to_string()
            .contains("holds 2 entities"));
        let _ = std::fs::remove_dir_all(root);

        // An export whose first collection is empty still holds an entity: never skipped.
        let (root, solution) = two_projects("empty");
        let hidden = "<Entities><Things/><Mashups><Mashup name=\"P.One.Hidden\" projectName=\"P.One\"/></Mashups></Entities>";
        std::fs::write(root.join("one/Things/hidden.xml"), hidden).unwrap();
        assert!(source_control(&solution, None)
            .unwrap_err()
            .to_string()
            .contains("first collection holds no entity"));
        let _ = std::fs::remove_dir_all(root);
    }
}
