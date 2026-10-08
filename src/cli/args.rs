use super::super::*;

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
    /// Flags that take one value and need no parsing here, such as `logs --since`.
    pub(crate) values: std::collections::BTreeMap<String, String>,
}

impl Args {
    /// Parse `args` (no command words) for a command that takes the flags `known` and operands,
    /// as the command line does. For tests; the command line parses whole commands with
    /// [`super::spec::tree`].
    #[cfg(test)]
    pub(crate) fn parse(args: &[String], known: &[&str]) -> Result<Args, String> {
        let command = clap::Command::new("twaco")
            .no_binary_name(true)
            .disable_help_flag(true)
            .disable_version_flag(true)
            .args(super::spec::arguments(known, true));
        let matches = command
            .try_get_matches_from(args)
            .map_err(|e| e.to_string())?;
        super::spec::args_from(known, true, &matches)
    }

    pub(crate) fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}
