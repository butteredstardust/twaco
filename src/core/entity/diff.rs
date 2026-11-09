//! `entity diff`: where the repository's copy of one entity and the server's differ, compared as
//! `entity status` and a deploy's read-back compare them. Read-only.
//!
//! The comparison is the hash's own: what the server derives (inherited members, timestamps,
//! owner, `effective*`) and the profile's server-owned parts are left out, and its patches are
//! applied to the repository side first. Each difference is a path of element names and `name`
//! attributes, never a value or text, so a password or other credential is never shown; a row
//! that holds one shows as that cell's text changing.

use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::entity::get;
use crate::core::entity::key::EntityKey;
use crate::core::normalise;
use crate::core::server::ServerError;
use crate::core::workspace::{self, WorkspaceError};

const SIDES: normalise::Sides = normalise::Sides {
    left_only: "only in the repository",
    right_only: "only on the server",
    left: "in the repository",
    right: "on the server",
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffOutcome {
    /// `Collection/Name`.
    pub entity: String,
    /// Whether both sides hash the same, which is what status and deploy go by.
    pub identical: bool,
    /// The paths where they differ; empty when identical. Among unnamed siblings of different
    /// kinds, the first mismatch is named and the rest of that parent is not walked.
    pub differences: Vec<String>,
    /// The server-owned parts left out of the comparison, by label.
    pub server_owned: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    #[error("{0}")]
    Resolve(WorkspaceError),
    #[error("{0}")]
    Unreadable(String),
    #[error("{0}")]
    Get(get::GetError),
}

impl From<ServerError> for DiffError {
    fn from(error: ServerError) -> Self {
        Self::Get(get::GetError::Remote(error))
    }
}

impl Coded for DiffError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Resolve(error) => error.code(),
            Self::Unreadable(_) => ErrorCode::InvalidArguments,
            Self::Get(error) => error.code(),
        }
    }
}

/// Compare the repository entity `text` names (full name, last dotted segment, or
/// `Collection/Name`) with the server's copy under `profile`'s rules.
pub fn diff(
    remote: &dyn get::Remote,
    solution: &Solution,
    text: &str,
    profile: &str,
) -> Result<DiffOutcome, DiffError> {
    let found = workspace::discover(solution);
    let entity = workspace::resolve(&found.entities, text.trim()).map_err(DiffError::Resolve)?;
    let (collection, name) = (&entity.info.collection, &entity.info.name);
    let unreadable =
        |why: String| DiffError::Unreadable(format!("{}: {why}", entity.path.display()));
    let working = std::fs::read(&entity.path)
        .map_err(|error| unreadable(error.to_string()))
        .and_then(|bytes| {
            crate::core::patch::apply(&bytes, collection, name, solution.patches_for(profile))
                .map_err(unreadable)
        })?;
    let key = EntityKey::new(collection.as_str(), name.as_str())
        .map_err(|why| DiffError::Unreadable(why.to_string()))?;
    let server = get::fetch(remote, &key).map_err(DiffError::Get)?;
    let parts =
        crate::core::server_owned::parts(&solution.server_owned_for(profile), collection, name);
    let hash = |bytes: &[u8], side: &str| {
        normalise::hash_with_parts(bytes, &parts)
            .map_err(|error| DiffError::Unreadable(format!("{side} copy of {key}: {error}")))
    };
    let identical = hash(&working, "the repository's")? == hash(&server, "the server's")?;
    let differences = if identical {
        Vec::new()
    } else {
        normalise::all_differences(&working, &server, &parts, &SIDES)
    };
    Ok(DiffOutcome {
        entity: key.to_string(),
        identical,
        differences,
        server_owned: crate::core::server_owned::labels(&parts),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Option<&'static str>);

    impl get::Remote for Fake {
        fn fetch(&self, _: &EntityKey) -> Result<Vec<u8>, ServerError> {
            match self.0 {
                Some(xml) => Ok(xml.as_bytes().to_vec()),
                None => Err(ServerError::Http {
                    method: crate::core::server::Method::Get,
                    status: 404,
                    url: "http://server".to_string(),
                    body: "Entity Not Found".to_string(),
                }),
            }
        }
    }

    const WORKING: &str = r#"<Entities><Things><Thing name="P.T" projectName="P" description="d"><ConfigurationTables><ConfigurationTable name="Login" isMultiRow="false"><DataShape><FieldDefinitions><FieldDefinition baseType="PASSWORD" name="password"/></FieldDefinitions></DataShape><Rows><Row><password><![CDATA[]]></password></Row></Rows></ConfigurationTable></ConfigurationTables></Thing></Things></Entities>"#;

    fn solution() -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("twaco.toml"), "[[project]]\nname = 'P'\n").unwrap();
        std::fs::create_dir_all(dir.path().join("Things")).unwrap();
        std::fs::write(dir.path().join("Things/P.T.xml"), WORKING).unwrap();
        let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
        (dir, solution)
    }

    #[test]
    fn the_same_entity_is_identical_with_no_differences() {
        let (_dir, solution) = solution();
        let outcome = diff(&Fake(Some(WORKING)), &solution, "T", "default").unwrap();
        assert_eq!(outcome.entity, "Things/P.T");
        assert!(outcome.identical);
        assert!(outcome.differences.is_empty());
    }

    #[test]
    fn a_difference_names_its_path_and_never_a_credential() {
        let (_dir, solution) = solution();
        let server = r#"<Thing name="P.T" projectName="P" description="d" lastModifiedDate="now"><Owner name="Administrator" type="User"/><ConfigurationTables><ConfigurationTable name="Login" isMultiRow="false"><DataShape><FieldDefinitions><FieldDefinition baseType="PASSWORD" name="password"/></FieldDefinitions></DataShape><Rows><Row><password><![CDATA[s3cret]]></password></Row></Rows></ConfigurationTable></ConfigurationTables><PropertyDefinitions><PropertyDefinition name="extra" baseType="STRING"/></PropertyDefinitions></Thing>"#;
        let outcome = diff(&Fake(Some(server)), &solution, "Things/P.T", "default").unwrap();
        assert!(!outcome.identical);
        assert!(
            outcome
                .differences
                .iter()
                .any(|line| line.ends_with("PropertyDefinitions: only on the server")),
            "{:?}",
            outcome.differences
        );
        assert!(
            outcome
                .differences
                .iter()
                .any(|line| line.contains("ConfigurationTable[Login]")
                    && line.ends_with("password: text changed")),
            "{:?}",
            outcome.differences
        );
        assert!(!outcome
            .differences
            .iter()
            .any(|line| line.contains("s3cret")));
    }

    #[test]
    fn an_entity_the_server_lacks_is_said_so() {
        let (_dir, solution) = solution();
        let error = diff(&Fake(None), &solution, "T", "default").unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnknownEntity);
    }
}
