//! Flags several commands take, each declared once and flattened into the commands
//! (`#[command(flatten)]`). A group declares only what a flag is called and the name of its
//! value; how it behaves on the command line is [`super::spec::decorate`]'s. `#[group(skip)]`
//! keeps clap from adding an argument group per struct, which the grammar dump would show and
//! nothing reads.

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
    /// `--all` with names says two things; refused before the command runs.
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

/// `--timeout <seconds>`: how long one request may take. clap refuses a value that is not a
/// positive whole number of seconds, before the command runs.
#[derive(Debug, Default, clap::Args)]
#[group(skip)]
pub(crate) struct Timeout {
    #[arg(long = "timeout", value_name = "seconds", value_parser = seconds)]
    pub(crate) timeout: Option<std::time::Duration>,
}

impl Timeout {
    /// The timeout given, or `default`.
    pub(crate) fn or(&self, default: std::time::Duration) -> std::time::Duration {
        self.timeout.unwrap_or(default)
    }
}

/// A positive whole number of seconds, as `--timeout` takes it.
fn seconds(value: &str) -> Result<std::time::Duration, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)
        .map(std::time::Duration::from_secs)
        .ok_or_else(|| "needs a positive whole number of seconds".to_string())
}

/// The parser for a value that names a file (`value_parser = super::flags::path()`): any
/// string, read as a path. clap's own path parser refuses an empty value and accepts one that is
/// not UTF-8, which twaco has always done the other way round.
pub(crate) fn path() -> impl clap::builder::TypedValueParser<Value = std::path::PathBuf> {
    use clap::builder::TypedValueParser as _;
    clap::builder::StringValueParser::new().map(std::path::PathBuf::from)
}

/// `--only-projects <a,b>`: the projects to work on, separated by commas. clap refuses a list
/// that names none, before the command runs.
#[derive(Debug, Default, clap::Args)]
#[group(skip)]
pub(crate) struct OnlyProjects {
    #[arg(long = "only-projects", value_name = "a,b", value_parser = project_list)]
    pub(crate) only_projects: Option<ProjectList>,
}

/// The projects one `--only-projects` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectList(pub(crate) Vec<String>);

impl OnlyProjects {
    /// The projects named; none when the flag was not given.
    pub(crate) fn projects(&self) -> Vec<String> {
        self.only_projects
            .as_ref()
            .map(|list| list.0.clone())
            .unwrap_or_default()
    }
}

/// The names between commas, trimmed, empty names dropped; refused when none is left.
fn project_list(value: &str) -> Result<ProjectList, String> {
    let selected: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    if selected.is_empty() {
        return Err("needs at least one project".to_string());
    }
    Ok(ProjectList(selected))
}
