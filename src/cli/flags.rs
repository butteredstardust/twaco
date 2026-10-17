//! Flags several typed commands take, each declared once and flattened into the commands
//! (`#[command(flatten)]`). A group declares only what a flag is called and the name of its
//! value, as `FLAGS` in [`super::spec`] does; how it behaves on the command line is
//! [`super::spec::decorate`]'s. `#[group(skip)]` keeps clap from adding an argument group per
//! struct, which the grammar dump would show and nothing reads.

/// `--profile <name>`: the server profile. Its default is applied here, not by clap, so a parse
/// can tell it was not given.
#[derive(Debug, Default, clap::Args)]
#[group(skip)]
pub(crate) struct Profile {
    #[arg(long = "profile", value_name = "name")]
    pub(crate) profile: Option<String>,
}

impl Profile {
    /// The profile named, or `default`.
    pub(crate) fn name(&self) -> &str {
        self.profile.as_deref().unwrap_or("default")
    }
}

/// `--apply`: do it, rather than say what would be done.
#[derive(Debug, Default, clap::Args)]
#[group(skip)]
pub(crate) struct Apply {
    #[arg(long = "apply")]
    pub(crate) apply: bool,
}
