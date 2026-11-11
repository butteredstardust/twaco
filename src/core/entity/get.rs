//! `entity get`: one entity's XML as the server has it. Read-only.
//!
//! `Collection/Name` reaches any entity on the server, in the repository or not (an entity
//! `search` found, a platform one). A bare name is looked up in the repository, the way the
//! other entity commands take one.

use crate::core::backup;
use crate::core::codes::{Coded, ErrorCode};
use crate::core::config::Solution;
use crate::core::entity::key::EntityKey;
use crate::core::server::{Client, ServerError};
use crate::core::workspace::{self, WorkspaceError};

/// What `entity get` asks of the server, as a trait so it is tested offline.
pub trait Remote {
    /// The importable Exporter XML, or `None` when the server has no such entity.
    fn export(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError>;
    /// The ordinary REST XML representation, including runtime data where the server exposes it.
    fn fetch_rest(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError>;
}

impl Remote for Client {
    fn export(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
        backup::Remote::export(self, key)
    }

    fn fetch_rest(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError> {
        self.fetch_entity(key)
    }
}

/// Which server representation `entity get` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Exporter XML, importable as-is.
    Export,
    /// The ordinary REST XML read, which may hold runtime data and is not importable.
    Rest,
}

#[derive(Debug, thiserror::Error)]
pub enum GetError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Resolve(WorkspaceError),
    /// The server answered that it has no such entity.
    #[error("the server has no {0}; `twaco search` finds an entity by part of its name")]
    NotOnServer(EntityKey),
    /// The REST read finds the entity, but the Exporter returned no export of it.
    #[error("the server has {0}, but its Exporter returned no export of it; `--rest` reads the REST form, which the Importer does not take")]
    NoExport(EntityKey),
    #[error("{0}")]
    Remote(ServerError),
}

/// The entity `text` names: `Collection/Name` as given, or a bare name found in the repository.
pub fn target(solution: &Solution, text: &str) -> Result<EntityKey, GetError> {
    let text = text.trim();
    if text.contains('/') {
        return EntityKey::parse(text).map_err(|why| GetError::Invalid(why.to_string()));
    }
    let found = workspace::discover(solution);
    let entity = workspace::resolve(&found.entities, text).map_err(GetError::Resolve)?;
    EntityKey::new(entity.info.collection.as_str(), entity.info.name.as_str())
        .map_err(|why| GetError::Invalid(why.to_string()))
}

/// The server's XML for the entity `text` names.
#[cfg(test)]
pub(crate) fn get(
    remote: &dyn Remote,
    solution: &Solution,
    text: &str,
    view: View,
) -> Result<(EntityKey, Vec<u8>), GetError> {
    let key = target(solution, text)?;
    let bytes = fetch(remote, &key, view)?;
    Ok((key, bytes))
}

/// The server's selected XML representation for `key`.
pub fn fetch(remote: &dyn Remote, key: &EntityKey, view: View) -> Result<Vec<u8>, GetError> {
    match view {
        View::Export => match remote.export(key).map_err(GetError::Remote)? {
            Some(bytes) => Ok(bytes),
            // An empty or unrecognised export is not proof of absence: only a REST 404 is.
            None => match remote.fetch_rest(key) {
                Ok(_) => Err(GetError::NoExport(key.clone())),
                Err(error) if error.is_not_found() => Err(GetError::NotOnServer(key.clone())),
                Err(error) => Err(GetError::Remote(error)),
            },
        },
        View::Rest => match remote.fetch_rest(key) {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.is_not_found() => Err(GetError::NotOnServer(key.clone())),
            Err(error) => Err(GetError::Remote(error)),
        },
    }
}

impl Coded for GetError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Resolve(error) => error.code(),
            Self::NotOnServer(_) => ErrorCode::UnknownEntity,
            Self::NoExport(_) => ErrorCode::ServerError,
            Self::Remote(error) => error.code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;

    impl Remote for Fake {
        fn export(&self, key: &EntityKey) -> Result<Option<Vec<u8>>, ServerError> {
            if matches!(key.name(), "Acme.Missing" | "Acme.NoExport") {
                return Ok(None);
            }
            Ok(Some(
                format!("<Entities><Export>{key}</Export></Entities>").into_bytes(),
            ))
        }

        fn fetch_rest(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError> {
            if key.name() == "Acme.Missing" {
                return Err(ServerError::Http {
                    method: crate::core::server::Method::Get,
                    status: 404,
                    url: key.to_string(),
                    body: String::new(),
                });
            }
            Ok(format!("<Rest>{key}</Rest>").into_bytes())
        }
    }

    fn solution() -> (tempfile::TempDir, std::path::PathBuf, Solution) {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-entity-get-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join("Things/Acme.Orders.Thing.xml"),
            "<Entities><Things><Thing name=\"Acme.Orders.Thing\" projectName=\"P\"></Thing></Things></Entities>",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root_guard, root, solution)
    }

    #[test]
    fn collection_and_name_reach_any_entity_and_a_bare_name_the_repositorys() {
        let (_dir, _, solution) = solution();
        let (key, bytes) = get(&Fake, &solution, "Resources/EntityServices", View::Export).unwrap();
        assert_eq!(key.to_string(), "Resources/EntityServices");
        assert_eq!(
            bytes,
            b"<Entities><Export>Resources/EntityServices</Export></Entities>"
        );
        // The last dotted segment, case-insensitively, as the other entity commands take it.
        let (key, _) = get(&Fake, &solution, "thing", View::Export).unwrap();
        assert_eq!(key.to_string(), "Things/Acme.Orders.Thing");
        assert!(matches!(
            get(&Fake, &solution, "Acme.Elsewhere", View::Export),
            Err(GetError::Resolve(_))
        ));
        assert!(matches!(
            get(&Fake, &solution, "Things/a/b", View::Export),
            Err(GetError::Invalid(_))
        ));
        let missing = get(&Fake, &solution, "Things/Acme.Missing", View::Export).unwrap_err();
        assert!(matches!(missing, GetError::NotOnServer(_)));
        assert!(missing.to_string().contains("twaco search"), "{missing}");
        // The REST read has it: the export, not the entity, is missing.
        let no_export = get(
            &Fake,
            &solution,
            "MediaEntities/Acme.NoExport",
            View::Export,
        )
        .unwrap_err();
        assert!(matches!(no_export, GetError::NoExport(_)), "{no_export}");
        assert!(no_export.to_string().contains("--rest"), "{no_export}");
    }

    #[test]
    fn rest_bytes_are_returned_only_when_requested() {
        let (_dir, _, solution) = solution();
        let key = target(&solution, "Things/Acme.Orders.Thing").unwrap();
        assert_eq!(
            fetch(&Fake, &key, View::Export).unwrap(),
            b"<Entities><Export>Things/Acme.Orders.Thing</Export></Entities>"
        );
        assert_eq!(
            fetch(&Fake, &key, View::Rest).unwrap(),
            b"<Rest>Things/Acme.Orders.Thing</Rest>"
        );
    }
}
