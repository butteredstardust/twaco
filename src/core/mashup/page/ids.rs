use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const VERSION: &str = "twaco-page-ids-v1";

fn hash(node: &str, role: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in [VERSION, node, role] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    hash.finalize().into()
}

pub(crate) fn uuid(role: &str) -> String {
    let mut bytes: [u8; 16] = hash("uuid", role)[..16].try_into().expect("16 bytes");
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

pub(crate) struct Ids {
    used: BTreeSet<u64>,
}

impl Ids {
    pub(crate) fn new(reserved: impl Iterator<Item = String>) -> Self {
        Self {
            used: reserved
                .filter_map(|id| id.rsplit('-').next()?.parse().ok())
                .collect(),
        }
    }
    pub(crate) fn widget(&mut self, kind: &str, node: &str, role: &str) -> Result<String, String> {
        let digest = hash(node, role);
        // Stay within Composer/JavaScript's signed 32-bit numeric range.
        let number =
            u64::from(u32::from_be_bytes(digest[..4].try_into().expect("4 bytes")) & 0x7fff_ffff);
        self.reserve(number)?;
        Ok(format!("{kind}-{number}"))
    }
    fn reserve(&mut self, number: u64) -> Result<(), String> {
        if number == 0 || !self.used.insert(number) {
            return Err(format!(
                "stable widget id collision at {number}; change the node id"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collisions_are_refused_and_template_numbers_reserved() {
        let mut ids = Ids::new(["flexcontainer-2".into()].into_iter());
        assert!(ids.reserve(2).is_err());
        assert!(ids.reserve(0).is_err());
        ids.widget("ptcslabel", "title", "widget").unwrap();
        assert!(ids.widget("ptcslabel", "title", "widget").is_err());
        assert_eq!(uuid("Acme.Data.GetRows"), uuid("Acme.Data.GetRows"));
    }
}
