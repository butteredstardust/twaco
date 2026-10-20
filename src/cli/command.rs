//! Every command, one variant each. [`super::spec::tree`] takes them from
//! [`Command::augment_subcommands`](clap::Subcommand::augment_subcommands) in
//! [`USAGE`](super::spec::USAGE)'s order, and [`super::spec::decorate`] gives every command the
//! same grammar policy. A family (`entity`, `rename`...) is one variant holding its own enum.
//!
//! A command's struct sits beside its handler and declares only names: every value is an
//! `Option` (a required field would replace the handler's own message for a missing value),
//! operands are a `Vec<String>` with the id `operands`, each flag has an explicit `long` and a
//! `value_name` (clap's errors quote it, and a flag several commands take names its value the
//! same in each), and `#[group(skip)]` is on every struct. Flags several commands take are
//! flattened from [`super::flags`]. The grammar dump test (`tests/fixtures/cli_grammar.txt`)
//! holds the result.

/// What the command line can say: one command and what it was given.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Command {
    Init(crate::Init),
    Doctor(crate::Doctor),
    Projects,
    Mcp(crate::Mcp),
    Update(super::info::Update),
    Completions(crate::Completions),
    Guide(super::info::Guide),
    Help(super::info::Help),
    Javadoc(super::info::Javadoc),
    Package(super::content::Package),
    Catalog(super::info::Catalog),
    #[command(subcommand)]
    Contract(super::contract::Contract),
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
    #[command(name = "appkey")]
    AppKey(super::appkey::AppKey),
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
    Network(super::network::Network),
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
    #[command(subcommand)]
    Requirements(super::requirements::Requirements),
    Bundle(super::source::Bundle),
    Deploy(super::source::Deploy),
    ConfigTable(super::data::ConfigTable),
    #[command(subcommand)]
    Datatable(super::data::Datatable),
    #[command(subcommand)]
    Db(super::data::Db),
}

impl Command {
    /// The command's [`USAGE`](super::spec::USAGE) path, for the log span and messages.
    pub(crate) fn path(&self) -> &'static str {
        match self {
            Command::Init(_) => "init",
            Command::Doctor(_) => "doctor",
            Command::Projects => "projects",
            Command::Mcp(_) => "mcp",
            Command::Update(_) => "update",
            Command::Completions(_) => "completions",
            Command::Guide(_) => "guide",
            Command::Help(_) => "help",
            Command::Javadoc(_) => "javadoc",
            Command::Package(_) => "package",
            Command::Catalog(_) => "catalog",
            Command::Contract(super::contract::Contract::Diff(_)) => "contract diff",
            Command::Impact(_) => "impact",
            Command::Trace(_) => "trace",
            Command::Unused(_) => "unused",
            Command::Docs(_) => "docs",
            Command::Widgets(_) => "widgets",
            Command::Extract(_) => "extract",
            Command::Sync(_) => "sync",
            Command::Fmt(_) => "fmt",
            Command::Check(_) => "check",
            Command::Types(_) => "types",
            Command::Users(_) => "users",
            Command::AppKey(_) => "appkey",
            Command::Test(_) => "test",
            Command::Localization(_) => "localization",
            Command::Repo(_) => "repo",
            Command::Ext(_) => "ext",
            Command::Search(_) => "search",
            Command::Export(_) => "export",
            Command::Import(_) => "import",
            Command::Call(_) => "call",
            Command::Logs(_) => "logs",
            Command::Settings(_) => "settings",
            Command::Thing(thing) => match thing {
                super::entity_edit::Thing::Property(_) => "thing property",
                super::entity_edit::Thing::Service(_) => "thing service",
                super::entity_edit::Thing::Subscription(_) => "thing subscription",
                super::entity_edit::Thing::Shape(_) => "thing shape",
            },
            Command::Timer(_) => "timer set",
            Command::Scheduler(_) => "scheduler set",
            Command::Style(_) => "style set",
            Command::Media(_) => "media set",
            Command::Menu(_) => "menu item",
            Command::State(_) => "state set",
            Command::Network(network) => match network {
                super::network::Network::List(_) => "network list",
                super::network::Network::Show(_) => "network show",
                super::network::Network::Diff(_) => "network diff",
                super::network::Network::Pull(_) => "network pull",
                super::network::Network::Add(_) => "network add",
                super::network::Network::Move(_) => "network move",
                super::network::Network::Remove(_) => "network remove",
                super::network::Network::Import(_) => "network import",
            },
            Command::New(super::entity_new::New::BuildingBlock(_)) => "new building-block",
            Command::New(super::entity_new::New::Entity(new)) => new.path(),
            Command::Mashup(mashup) => mashup.path(),
            Command::Adopt(_) => "adopt",
            Command::Handoff(_) => "handoff",
            Command::Rename(rename) => rename.path(),
            Command::Move(relocate) => relocate.path(),
            Command::Copy(relocate) => relocate.path(),
            Command::Retemplate(_) => "retemplate",
            Command::Reproject(_) => "reproject",
            Command::Entity(entity) => entity.path(),
            Command::Permissions(permissions) => permissions.path(),
            Command::Requirements(requirements) => match requirements {
                super::requirements::Requirements::Init(_) => "requirements init",
                super::requirements::Requirements::Check(_) => "requirements check",
            },
            Command::Bundle(_) => "bundle",
            Command::Deploy(_) => "deploy",
            Command::ConfigTable(_) => "config-table",
            Command::Datatable(_) => "datatable copy",
            Command::Db(db) => db.path(),
        }
    }

    /// What clap cannot say about the arguments, refused before the command runs (and before
    /// the solution is looked for), exit 2.
    pub(crate) fn validate(&self) -> Result<(), String> {
        match self {
            Command::Extract(extract) => extract.selection.validate(),
            Command::Sync(sync) => sync.selection.validate(),
            Command::Mashup(super::mashup::Mashup::Validate(validate)) => {
                validate.selection.validate()
            }
            Command::Entity(super::entity::Entity::Status(status)) => status.selection.validate(),
            Command::Permissions(super::entity::Permissions::Diff(diff)) => {
                diff.selection.validate()
            }
            Command::Permissions(super::entity::Permissions::Push(push)) => {
                push.diff.selection.validate()
            }
            Command::Init(_)
            | Command::Doctor(_)
            | Command::Projects
            | Command::Mcp(_)
            | Command::Update(_)
            | Command::Completions(_)
            | Command::Guide(_)
            | Command::Help(_)
            | Command::Javadoc(_)
            | Command::Package(_)
            | Command::Catalog(_)
            | Command::Contract(_)
            | Command::Impact(_)
            | Command::Trace(_)
            | Command::Unused(_)
            | Command::Docs(_)
            | Command::Widgets(_)
            | Command::Fmt(_)
            | Command::Check(_)
            | Command::Types(_)
            | Command::Users(_)
            | Command::AppKey(_)
            | Command::Test(_)
            | Command::Localization(_)
            | Command::Repo(_)
            | Command::Ext(_)
            | Command::Search(_)
            | Command::Export(_)
            | Command::Import(_)
            | Command::Logs(_)
            | Command::Settings(_)
            | Command::Thing(_)
            | Command::Timer(_)
            | Command::Scheduler(_)
            | Command::Style(_)
            | Command::Media(_)
            | Command::Menu(_)
            | Command::State(_)
            | Command::Network(_)
            | Command::New(_)
            | Command::Mashup(_)
            | Command::Adopt(_)
            | Command::Handoff(_)
            | Command::Rename(_)
            | Command::Move(_)
            | Command::Copy(_)
            | Command::Retemplate(_)
            | Command::Reproject(_)
            | Command::Entity(_)
            | Command::Permissions(_)
            | Command::Requirements(_)
            | Command::Bundle(_)
            | Command::ConfigTable(_)
            | Command::Datatable(_)
            | Command::Call(_)
            | Command::Deploy(_)
            | Command::Db(_) => Ok(()),
        }
    }
}
