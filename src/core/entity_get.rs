//! `entity get`: one entity's XML as the server has it. Read-only.
//!
//! `Collection/Name` reaches any entity on the server, in the repository or not (an entity
//! `search` found, a platform one). A bare name is looked up in the repository, the way the
//! other entity commands take one.

use super::config::Solution;
use super::entity_key::EntityKey;
use super::server::{Client, ServerError};
use super::workspace::{self, WorkspaceError};
use std::fmt;

/// What `entity get` asks of the server, as a trait so it is tested offline.
pub trait Remote {
    fn fetch(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError>;
}

impl Remote for Client {
    fn fetch(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError> {
        self.fetch_entity(key)
    }
}

#[derive(Debug)]
pub enum GetError {
    Invalid(String),
    Resolve(WorkspaceError),
    /// The server answered that it has no such entity.
    NotOnServer(EntityKey),
    Remote(ServerError),
}

impl fmt::Display for GetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GetError::Invalid(why) => f.write_str(why),
            GetError::Resolve(error) => write!(f, "{error}"),
            GetError::NotOnServer(key) => write!(
                f,
                "the server has no {key}; `twaco search` finds an entity by part of its name"
            ),
            GetError::Remote(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for GetError {}

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
pub fn get(
    remote: &dyn Remote,
    solution: &Solution,
    text: &str,
) -> Result<(EntityKey, Vec<u8>), GetError> {
    let key = target(solution, text)?;
    let bytes = fetch(remote, &key)?;
    Ok((key, bytes))
}

/// The server's XML for `key`; a 404 is said as the entity not being there.
pub fn fetch(remote: &dyn Remote, key: &EntityKey) -> Result<Vec<u8>, GetError> {
    match remote.fetch(key) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.is_not_found() => Err(GetError::NotOnServer(key.clone())),
        Err(error) => Err(GetError::Remote(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;

    impl Remote for Fake {
        fn fetch(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError> {
            if key.name() == "Acme.Missing" {
                return Err(ServerError::Http {
                    method: crate::core::server::Method::Get,
                    url: format!("http://server/Thingworx/{}", key.url_path()),
                    status: 404,
                    body: String::new(),
                });
            }
            Ok(format!("<Entities>{key}</Entities>").into_bytes())
        }
    }

    fn solution() -> (std::path::PathBuf, Solution) {
        let root = std::env::temp_dir().join(format!(
            "twaco-entity-get-{}-{}",
            std::process::id(),
            crate::test_nonce()
        ));
        std::fs::create_dir_all(root.join("Things")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join("Things/Acme.Orders.Thing.xml"),
            "<Entities><Things><Thing name=\"Acme.Orders.Thing\" projectName=\"P\"></Thing></Things></Entities>",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root, solution)
    }

    #[test]
    fn collection_and_name_reach_any_entity_and_a_bare_name_the_repositorys() {
        let (root, solution) = solution();
        let (key, bytes) = get(&Fake, &solution, "Resources/EntityServices").unwrap();
        assert_eq!(key.to_string(), "Resources/EntityServices");
        assert_eq!(bytes, b"<Entities>Resources/EntityServices</Entities>");
        // The last dotted segment, case-insensitively, as the other entity commands take it.
        let (key, _) = get(&Fake, &solution, "thing").unwrap();
        assert_eq!(key.to_string(), "Things/Acme.Orders.Thing");
        assert!(matches!(
            get(&Fake, &solution, "Acme.Elsewhere"),
            Err(GetError::Resolve(_))
        ));
        assert!(matches!(
            get(&Fake, &solution, "Things/a/b"),
            Err(GetError::Invalid(_))
        ));
        let missing = get(&Fake, &solution, "Things/Acme.Missing").unwrap_err();
        assert!(matches!(missing, GetError::NotOnServer(_)));
        assert!(missing.to_string().contains("twaco search"), "{missing}");
        let _ = std::fs::remove_dir_all(root);
    }
}
