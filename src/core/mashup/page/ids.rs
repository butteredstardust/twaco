use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(crate) const VERSION: &str = "twaco-page-ids-v2";

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

/// Widget and wiring ids that override the hashed ones, kept in the baseline: an adopted mashup
/// keeps the ids of the widgets it already has.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Pins {
    /// `(node, role, id)` as the allocator named them.
    pub(crate) widgets: Vec<(String, String, String)>,
    /// A generated binding or event id to the id the mashup already has.
    pub(crate) wiring: std::collections::BTreeMap<String, String>,
}

pub(crate) struct Ids {
    preserved: std::collections::BTreeMap<String, String>,
    used: BTreeSet<u64>,
    names: BTreeSet<String>,
    probe: bool,
    allocated: Vec<(String, String, String)>,
}

impl Ids {
    pub(crate) fn new(reserved: impl Iterator<Item = String>) -> Self {
        let reserved: Vec<_> = reserved.collect();
        Self {
            preserved: std::collections::BTreeMap::new(),
            used: reserved
                .iter()
                .cloned()
                .filter_map(|id| id.rsplit('-').next()?.parse().ok())
                .collect(),
            names: reserved.into_iter().collect(),
            probe: false,
            allocated: Vec::new(),
        }
    }
    /// What `widget` handed out, as `(node, role, id)`.
    pub(crate) fn allocated(&self) -> &[(String, String, String)] {
        &self.allocated
    }
    pub(crate) fn preserve(&mut self, node: &str, id: &str) {
        self.probe = true;
        if let Some(number) = id.rsplit('-').next().and_then(|n| n.parse().ok()) {
            self.used.insert(number);
        }
        self.preserved.insert(node.into(), id.into());
    }
    pub(crate) fn preserved(&self, node: &str) -> Option<String> {
        self.preserved.get(node).cloned()
    }
    pub(crate) fn explicit_as(
        &mut self,
        node: &str,
        role: &str,
        id: &str,
    ) -> Result<String, String> {
        let id = self.explicit(id, role)?;
        self.allocated.last_mut().expect("allocated").0 = node.into();
        Ok(id)
    }
    pub(crate) fn widget(&mut self, kind: &str, node: &str, role: &str) -> Result<String, String> {
        let digest = hash(node, role);
        // Authored pages keep their existing refusal policy. Source ids pin derived pages;
        // only new generated roles may probe without moving any existing widget.
        let mut number = 1000
            + u64::from(u32::from_be_bytes(digest[..4].try_into().expect("4 bytes"))) % 999_000;
        if self.probe {
            let first = number;
            while self.used.contains(&number) {
                number = 1000 + (number - 1000 + 1) % 999_000;
                if number == first {
                    return Err("no free stable widget ids".into());
                }
            }
        }
        self.reserve(number)?;
        let id = format!("{kind}-{number}");
        self.names.insert(id.clone());
        self.allocated.push((node.into(), role.into(), id.clone()));
        Ok(id)
    }
    /// Preserve an explicit Composer-style id from a derived spec. Fresh pages cannot collide
    /// with another document, but a duplicate within this page is still refused.
    pub(crate) fn explicit(&mut self, node: &str, role: &str) -> Result<String, String> {
        let number = node
            .rsplit_once('-')
            .and_then(|(_, suffix)| suffix.parse::<u64>().ok());
        if !self.names.insert(node.into()) {
            return Err(format!("duplicate widget id {node}"));
        }
        self.probe = true;
        if let Some(number) = number {
            self.used.insert(number);
        }
        self.allocated.push((node.into(), role.into(), node.into()));
        Ok(node.into())
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

pub(super) fn references(message: &str, id: &str) -> bool {
    message
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-')))
        .any(|word| word == id)
}

/// Diagnostics use authored node names, including data aliases, rather than temporary ids.
pub(super) fn diagnostic(message: &str, allocated: &[(String, String, String)]) -> String {
    let names: std::collections::BTreeMap<_, _> = allocated
        .iter()
        .map(|(node, _, id)| (id.as_str(), node.strip_prefix("$data:").unwrap_or(node)))
        .collect();
    let mut result = String::new();
    let mut start = 0;
    for (end, character) in message.char_indices() {
        if character.is_alphanumeric() || matches!(character, '_' | '-') {
            continue;
        }
        let word = &message[start..end];
        result.push_str(names.get(word).copied().unwrap_or(word));
        result.push(character);
        start = end + character.len_utf8();
    }
    let word = &message[start..];
    result.push_str(names.get(word).copied().unwrap_or(word));
    result
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
    #[test]
    fn pinned_pages_probe_hash_collisions_and_keep_distinct_explicit_ids() {
        let mut ids = Ids::new(std::iter::empty());
        ids.explicit("flexcontainer-48", "widget").unwrap();
        ids.explicit("ptcscheckbox-48", "widget").unwrap();
        assert!(ids.explicit("ptcscheckbox-48", "widget").is_err());
        assert_eq!(
            ids.widget("ptcslabel", "node-697", "widget").unwrap(),
            "ptcslabel-576705"
        );
        assert_eq!(
            ids.widget("ptcsbutton", "node-903", "widget").unwrap(),
            "ptcsbutton-576706"
        );
    }
    #[test]
    fn readable_ids_are_stable_and_real_hash_collisions_are_refused() {
        let mut first = Ids::new(std::iter::empty());
        let id = first.widget("ptcsbutton", "refresh", "widget").unwrap();
        assert_eq!(
            id,
            Ids::new(std::iter::empty())
                .widget("ptcsbutton", "refresh", "widget")
                .unwrap()
        );
        let suffix = id.rsplit('-').next().unwrap();
        assert!((4..=6).contains(&suffix.len()));
        first.widget("ptcslabel", "node-697", "widget").unwrap();
        assert!(first
            .widget("ptcsbutton", "node-903", "widget")
            .unwrap_err()
            .contains("576705"));
        let mut reserved = Ids::new([id].into_iter());
        assert!(reserved.widget("ptcsbutton", "refresh", "widget").is_err());
    }
}
