use std::path::PathBuf;
use std::time::Duration;

/// One command's arguments, after validation.
#[derive(Debug, Default)]
pub(crate) struct Args {
    pub(crate) names: Vec<String>,
    pub(crate) flags: Vec<String>,
    /// `--project <name>`, narrowing to one project of the solution.
    pub(crate) project: Option<String>,
    /// Server profile name. It is only resolved by server commands.
    pub(crate) profile: Option<String>,
    /// Raw entity GET destination.
    pub(crate) out: Option<PathBuf>,
    /// Repeated deploy entity selectors.
    pub(crate) only: Vec<String>,
    /// Deploy project selectors parsed from comma-separated values.
    pub(crate) only_projects: Vec<String>,
    /// Per-request timeout for an opaque service call.
    pub(crate) timeout: Option<Duration>,
    /// Where `config-table --backup` writes, and what `--restore` reads.
    pub(crate) backup: Option<PathBuf>,
    pub(crate) restore: Option<PathBuf>,
    /// `adopt --entity`: name fragments, repeatable.
    pub(crate) entity_filters: Vec<String>,
    /// `mashup new --visibility`: principals, repeatable.
    pub(crate) visibility: Vec<String>,
    /// `mashup add --prop`: properties, repeatable.
    pub(crate) props: Vec<String>,
    /// `mashup new --param`, repeatable.
    pub(crate) params: Vec<String>,
    /// `mashup new --tab`, repeatable.
    pub(crate) tabs: Vec<String>,
    /// `mashup add --input`, repeatable.
    pub(crate) inputs: Vec<String>,
    /// `mashup recipe --pass`, repeatable.
    pub(crate) pass: Vec<String>,
    /// `mashup recipe --column`, repeatable.
    pub(crate) columns: Vec<String>,
    /// `new --shape`, repeatable.
    pub(crate) shapes: Vec<String>,
    /// `new data-shape --field`, repeatable.
    pub(crate) fields: Vec<String>,
    pub(crate) indexes: Vec<String>,
    pub(crate) states: Vec<String>,
    pub(crate) items: Vec<String>,
    /// Flags that take one value and need no parsing here, such as `logs --since`.
    pub(crate) values: std::collections::BTreeMap<String, String>,
}

impl Args {
    pub(crate) fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}
