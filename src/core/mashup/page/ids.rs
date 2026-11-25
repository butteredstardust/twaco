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
    explicit_numbers: BTreeSet<u64>,
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
            explicit_numbers: BTreeSet::new(),
            allocated: Vec::new(),
        }
    }
    /// What `widget` handed out, as `(node, role, id)`.
    pub(crate) fn allocated(&self) -> &[(String, String, String)] {
        &self.allocated
    }
    pub(crate) fn preserve(&mut self, node: &str, id: &str) {
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
        // Leave template numbers below 1000; refuse collisions instead of probing.
        let number = 1000
            + u64::from(u32::from_be_bytes(digest[..4].try_into().expect("4 bytes"))) % 999_000;
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
        // Composer numbers a mashup's ids uniquely; two spec ids sharing a number would also
        // share generated display names, so refuse rather than guess. (A template's own ids
        // may share a number with a source id: they differ by type.)
        if let Some(number) = number {
            if !self.explicit_numbers.insert(number) {
                return Err(format!(
                    "widget id {node} reuses number {number}, already taken by another widget id"
                ));
            }
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
