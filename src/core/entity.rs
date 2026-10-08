//! Reading what an entity document says about itself.
//!
//! An entity's `projectName` is authoritative for deciding which project it belongs to. A folder
//! layout is a convention: projects can share collection folders, and a file can be filed in the
//! wrong place. Reading the document is the only answer that cannot be wrong, and it catches the
//! misfiled entity that would otherwise import into the wrong project with nothing to show for
//! it.

use super::scan::{self, Kind, ScanError};
use std::path::{Path, PathBuf};

/// What one entity document declares about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityInfo {
    /// The ThingWorx collection, taken from the document rather than the folder name.
    pub collection: String,
    pub name: String,
    /// Empty when the document does not declare one.
    pub project: String,
}

#[derive(Debug, thiserror::Error)]
pub enum EntityError {
    #[error("cannot read {}: {why}", .path.display())]
    Unreadable { path: PathBuf, why: String },
    #[error("{}: {why}", .path.display())]
    Scan { path: PathBuf, why: ScanError },
    /// The file parsed but is not an entity export: no `<Entities>` wrapper with a collection.
    #[error("{} is not a ThingWorx entity export", .path.display())]
    NotAnEntityDocument { path: PathBuf },
}

/// The document element every ThingWorx export is wrapped in.
const WRAPPER: &str = "Entities";

/// Read what a document declares about itself.
///
/// Returns `NotAnEntityDocument` for anything that is not an `<Entities>` export — a service
/// `definition.xml` sidecar fragment, for instance, which is a well-formed XML file and not an
/// entity. The corpus is 425 XML files of which 141 are exactly that, so telling them apart
/// matters and cannot be done by file extension.
pub fn read(path: &Path) -> Result<EntityInfo, EntityError> {
    let src = std::fs::read(path).map_err(|e| EntityError::Unreadable {
        path: path.to_path_buf(),
        why: e.to_string(),
    })?;
    parse(&src).map_err(|e| match e {
        ParseFailureKind::Scan(why) => EntityError::Scan {
            path: path.to_path_buf(),
            why,
        },
        ParseFailureKind::NotAnEntity => EntityError::NotAnEntityDocument {
            path: path.to_path_buf(),
        },
    })
}

/// The parsing half, separated from the filesystem so it can be tested on bytes.
pub fn parse(src: &[u8]) -> Result<EntityInfo, ParseFailureKind> {
    let tokens = scan::tokenize(src).map_err(ParseFailureKind::Scan)?;
    let mut tags = tokens
        .iter()
        .filter(|t| matches!(t.kind, Kind::Start | Kind::Empty));

    // <Entities ...> then <Collection> then the entity element itself.
    let wrapper = tags.next().ok_or(ParseFailureKind::NotAnEntity)?;
    if wrapper.name.of(src) != WRAPPER.as_bytes() {
        return Err(ParseFailureKind::NotAnEntity);
    }
    let collection = tags.next().ok_or(ParseFailureKind::NotAnEntity)?;
    let entity = tags.next().ok_or(ParseFailureKind::NotAnEntity)?;

    let name = scan::attribute(src, entity, "name")
        .map_err(ParseFailureKind::Scan)?
        .map(|s| scan::decode_entities(&String::from_utf8_lossy(s.of(src))))
        .unwrap_or_default();
    if name.is_empty() {
        return Err(ParseFailureKind::NotAnEntity);
    }
    let project = scan::attribute(src, entity, "projectName")
        .map_err(ParseFailureKind::Scan)?
        .map(|s| scan::decode_entities(&String::from_utf8_lossy(s.of(src))))
        .unwrap_or_default();

    Ok(EntityInfo {
        collection: String::from_utf8_lossy(collection.name.of(src)).into_owned(),
        name,
        project,
    })
}

/// Why a document could not be read as an entity export.
#[derive(Debug)]
pub enum ParseFailureKind {
    Scan(ScanError),
    NotAnEntity,
}

/// How an entity relates to the project whose folder holds it.
#[derive(Debug, PartialEq, Eq)]
pub enum Attribution {
    /// The document's `projectName` matches the project it was found under.
    Matches,
    /// The document declares a different project: it is filed in the wrong place.
    Mismatched { declared: String },
    /// The document declares no project at all.
    Undeclared,
}

pub fn attribute_to(info: &EntityInfo, project: &str) -> Attribution {
    if info.project.is_empty() {
        Attribution::Undeclared
    } else if info.project == project {
        Attribution::Matches
    } else {
        Attribution::Mismatched {
            declared: info.project.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPORT: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<Entities majorVersion="10" minorVersion="1">
    <Things>
        <Thing name="My.Thing" projectName="My.Project" enabled="true">
            <PropertyDefinitions></PropertyDefinitions>
        </Thing>
    </Things>
</Entities>"#;

    #[test]
    fn an_entity_reports_its_collection_name_and_project() {
        let info = parse(EXPORT).unwrap();
        assert_eq!(info.collection, "Things");
        assert_eq!(info.name, "My.Thing");
        assert_eq!(info.project, "My.Project");
    }

    #[test]
    fn the_collection_comes_from_the_document_not_a_folder() {
        // A DataShape filed under Things/ still reports DataShapes.
        let src = br#"<Entities><DataShapes><DataShape name="D" projectName="P"></DataShape></DataShapes></Entities>"#;
        assert_eq!(parse(src).unwrap().collection, "DataShapes");
    }

    #[test]
    fn a_definition_fragment_is_not_an_entity_document() {
        // 141 of the 425 XML files in the corpus are exactly this shape.
        let src = br#"<ServiceDefinition name="GetThing" isAsync="false"></ServiceDefinition>"#;
        assert!(matches!(parse(src), Err(ParseFailureKind::NotAnEntity)));
    }

    #[test]
    fn an_entity_without_a_project_is_undeclared_not_an_error() {
        let src = br#"<Entities><Things><Thing name="T"></Thing></Things></Entities>"#;
        let info = parse(src).unwrap();
        assert_eq!(info.project, "");
        assert_eq!(attribute_to(&info, "Anything"), Attribution::Undeclared);
    }

    #[test]
    fn a_misfiled_entity_is_reported_rather_than_trusted() {
        let info = parse(EXPORT).unwrap();
        assert_eq!(attribute_to(&info, "My.Project"), Attribution::Matches);
        assert_eq!(
            attribute_to(&info, "Other.Project"),
            Attribution::Mismatched {
                declared: "My.Project".to_string()
            }
        );
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_guess() {
        assert!(matches!(
            parse(b"<Entities><Things><Thing name=\"oops"),
            Err(ParseFailureKind::Scan(_))
        ));
    }
}
