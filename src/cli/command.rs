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
    Catalog(super::info::Catalog),
    Impact(super::info::Impact),
    Trace(super::info::Trace),
    Unused(super::info::Unused),
    Docs(super::info::Docs),
    Widgets(super::widgets::Widgets),
    Extract(super::source::Extract),
    Sync(super::source::Sync),
    Fmt(super::source::Fmt),
    Check(super::source::Check),
    Types(super::source::Types),
    Users(super::users::Users),
    Test(super::service_tests::Test),
    Localization(super::localization::Localization),
    Repo(super::content::Repo),
    Ext(super::content::Ext),
    Search(super::content::Search),
    Export(super::content::Export),
    Import(super::content::Import),
    Call(super::data::Call),
    Logs(super::data::Logs),
    Settings(super::info::Settings),
    #[command(subcommand)]
    Thing(super::entity_edit::Thing),
    #[command(subcommand)]
    Timer(super::entity_edit::Timer),
    #[command(subcommand)]
    Scheduler(super::entity_edit::Scheduler),
    #[command(subcommand)]
    Style(super::entity_edit::Style),
    #[command(subcommand)]
    Media(super::entity_edit::Media),
    #[command(subcommand)]
    Menu(super::entity_edit::Menu),
    #[command(subcommand)]
    State(super::entity_edit::State),
    #[command(subcommand)]
    New(super::entity_new::New),
    #[command(subcommand)]
    Mashup(super::mashup::Mashup),
    Adopt(super::refactor::Adopt),
    Handoff(super::refactor::Handoff),
    #[command(subcommand)]
    Rename(super::refactor::Rename),
    #[command(subcommand)]
    Move(super::refactor::MoveMember),
    #[command(subcommand)]
    Copy(super::refactor::CopyMember),
    Retemplate(super::refactor::Retemplate),
    Reproject(super::refactor::Reproject),
    #[command(subcommand)]
    Entity(super::entity::Entity),
    #[command(subcommand)]
    Permissions(super::entity::Permissions),
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
            Typed::Catalog(_) => "catalog",
            Typed::Impact(_) => "impact",
            Typed::Trace(_) => "trace",
            Typed::Unused(_) => "unused",
            Typed::Docs(_) => "docs",
            Typed::Widgets(_) => "widgets",
            Typed::Extract(_) => "extract",
            Typed::Sync(_) => "sync",
            Typed::Fmt(_) => "fmt",
            Typed::Check(_) => "check",
            Typed::Types(_) => "types",
            Typed::Users(_) => "users",
            Typed::Test(_) => "test",
            Typed::Localization(_) => "localization",
            Typed::Repo(_) => "repo",
            Typed::Ext(_) => "ext",
            Typed::Search(_) => "search",
            Typed::Export(_) => "export",
            Typed::Import(_) => "import",
            Typed::Call(_) => "call",
            Typed::Logs(_) => "logs",
            Typed::Settings(_) => "settings",
            Typed::Thing(thing) => match thing {
                super::entity_edit::Thing::Property(_) => "thing property",
                super::entity_edit::Thing::Subscription(_) => "thing subscription",
                super::entity_edit::Thing::Shape(_) => "thing shape",
            },
            Typed::Timer(_) => "timer set",
            Typed::Scheduler(_) => "scheduler set",
            Typed::Style(_) => "style set",
            Typed::Media(_) => "media set",
            Typed::Menu(_) => "menu item",
            Typed::State(_) => "state set",
            Typed::New(super::entity_new::New::BuildingBlock(_)) => "new building-block",
            Typed::New(super::entity_new::New::Entity(new)) => new.path(),
            Typed::Mashup(mashup) => mashup.path(),
            Typed::Adopt(_) => "adopt",
            Typed::Handoff(_) => "handoff",
            Typed::Rename(rename) => rename.path(),
            Typed::Move(relocate) => relocate.path(),
            Typed::Copy(relocate) => relocate.path(),
            Typed::Retemplate(_) => "retemplate",
            Typed::Reproject(_) => "reproject",
            Typed::Entity(entity) => entity.path(),
            Typed::Permissions(permissions) => permissions.path(),
        }
    }

    /// What clap cannot say about the arguments, refused before the command runs, as the
    /// builder's commands refuse it in [`args_of`](super::spec::args_of) and in its words.
    pub(crate) fn validate(&self) -> Result<(), String> {
        match self {
            Typed::Extract(extract) => extract.selection.validate(),
            Typed::Sync(sync) => sync.selection.validate(),
            Typed::Call(call) => call.timeout.duration().map(|_| ()),
            Typed::Mashup(super::mashup::Mashup::Validate(validate)) => {
                validate.selection.validate()
            }
            Typed::Entity(super::entity::Entity::Status(status)) => status.selection.validate(),
            Typed::Permissions(super::entity::Permissions::Diff(diff)) => diff.selection.validate(),
            Typed::Permissions(super::entity::Permissions::Push(push)) => {
                push.diff.selection.validate()
            }
            Typed::Init(_)
            | Typed::Doctor(_)
            | Typed::Projects
            | Typed::Mcp(_)
            | Typed::Update(_)
            | Typed::Guide(_)
            | Typed::Help(_)
            | Typed::Javadoc(_)
            | Typed::Package(_)
            | Typed::Catalog(_)
            | Typed::Impact(_)
            | Typed::Trace(_)
            | Typed::Unused(_)
            | Typed::Docs(_)
            | Typed::Widgets(_)
            | Typed::Fmt(_)
            | Typed::Check(_)
            | Typed::Types(_)
            | Typed::Users(_)
            | Typed::Test(_)
            | Typed::Localization(_)
            | Typed::Repo(_)
            | Typed::Ext(_)
            | Typed::Search(_)
            | Typed::Export(_)
            | Typed::Import(_)
            | Typed::Logs(_)
            | Typed::Settings(_)
            | Typed::Thing(_)
            | Typed::Timer(_)
            | Typed::Scheduler(_)
            | Typed::Style(_)
            | Typed::Media(_)
            | Typed::Menu(_)
            | Typed::State(_)
            | Typed::New(_)
            | Typed::Mashup(_)
            | Typed::Adopt(_)
            | Typed::Handoff(_)
            | Typed::Rename(_)
            | Typed::Move(_)
            | Typed::Copy(_)
            | Typed::Retemplate(_)
            | Typed::Reproject(_)
            | Typed::Entity(_)
            | Typed::Permissions(_) => Ok(()),
        }
    }
}
