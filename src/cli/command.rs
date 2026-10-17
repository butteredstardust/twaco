//! The commands clap derives, one variant each, while the rest are still built from
//! [`GRAMMAR`](super::spec::GRAMMAR) rows. Both kinds sit in one tree: [`super::spec::tree`] takes
//! these from [`Typed::augment_subcommands`](clap::Subcommand::augment_subcommands) in
//! [`USAGE`](super::spec::USAGE)'s order, and [`super::spec::decorate`] gives every command the
//! same grammar policy. A family moves here as a whole, as one variant holding its own enum.
//!
//! A command's struct sits beside its handler and declares only names: every value is an
//! `Option` (a required field would replace the handler's own message for a missing value),
//! operands are a `Vec<String>` with the id `operands`, each flag has an explicit `long` and the
//! `value_name` FLAGS gives it (clap's errors quote it), and `#[group(skip)]` is on every struct.
//! The grammar dump test holds the result to the grammar the builder had.

/// The commands parsed into structs rather than [`Args`](super::args::Args).
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Typed {
    Init(crate::Init),
    Doctor(crate::Doctor),
    Projects,
    Mcp(crate::Mcp),
    Update(super::info::Update),
    Guide(super::info::Guide),
    Help(super::info::Help),
    Javadoc(super::info::Javadoc),
    Package(super::content::Package),
}

impl Typed {
    /// The command's [`USAGE`](super::spec::USAGE) path, for the log span and messages.
    pub(crate) fn path(&self) -> &'static str {
        match self {
            Typed::Init(_) => "init",
            Typed::Doctor(_) => "doctor",
            Typed::Projects => "projects",
            Typed::Mcp(_) => "mcp",
            Typed::Update(_) => "update",
            Typed::Guide(_) => "guide",
            Typed::Help(_) => "help",
            Typed::Javadoc(_) => "javadoc",
            Typed::Package(_) => "package",
        }
    }

    /// What clap cannot say about the arguments, refused before the command runs, as the
    /// builder's commands refuse it in [`args_of`](super::spec::args_of) and in its words.
    pub(crate) fn validate(&self) -> Result<(), String> {
        match self {
            Typed::Init(_)
            | Typed::Doctor(_)
            | Typed::Projects
            | Typed::Mcp(_)
            | Typed::Update(_)
            | Typed::Guide(_)
            | Typed::Help(_)
            | Typed::Javadoc(_)
            | Typed::Package(_) => Ok(()),
        }
    }
}
