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

/// `--json`: results as JSON on stdout instead of text.
#[derive(Debug, Default, clap::Args)]
#[group(skip)]
pub(crate) struct Json {
    #[arg(long = "json")]
    pub(crate) json: bool,
}

/// `--detail`: the whole result, not only its summary.
#[derive(Debug, Default, clap::Args)]
#[group(skip)]
pub(crate) struct Detail {
    #[arg(long = "detail")]
    pub(crate) detail: bool,
}

/// `--project <name>`: narrow to one project of the solution.
#[derive(Debug, Default, clap::Args)]
#[group(skip)]
pub(crate) struct Project {
    #[arg(long = "project", value_name = "name")]
    pub(crate) project: Option<String>,
}

/// `--all` or the entities named: what a command works on.
#[derive(Debug, Default, clap::Args)]
#[group(skip)]
pub(crate) struct Selection {
    #[arg(long = "all")]
    pub(crate) all: bool,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

impl Selection {
    /// `--all` with names says two things; refused before the command runs, in the builder's
    /// words.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.all && !self.operands.is_empty() {
            return Err(format!(
                "--all and {:?} say different things; pass one or the other",
                self.operands
            ));
        }
        Ok(())
    }
}

/// The parser for a value that names a file (`value_parser = super::flags::path()`): any
/// string, as the builder took it, read as a path. clap's own path parser refuses an empty value
/// and accepts one that is not UTF-8, which the builder did the other way round.
pub(crate) fn path() -> impl clap::builder::TypedValueParser<Value = std::path::PathBuf> {
    use clap::builder::TypedValueParser as _;
    clap::builder::StringValueParser::new().map(std::path::PathBuf::from)
}
