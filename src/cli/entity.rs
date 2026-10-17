use super::flags::{Apply, Detail, Json, Profile, Project, Selection};
use super::refactor::same_path;
use super::style;
use crate::{print_notices, DRIFT, FAILED, OK};
use std::io::Write;
use std::path::PathBuf;
use twaco::core::commands::{self, Mode};
use twaco::core::config::Solution;
use twaco::core::{backup, entity_carry, entity_delete, profile, push, server, status, workspace};

/// `twaco entity <command>`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Entity {
    Status(EntityStatus),
    Get(EntityGet),
    Push(EntityPush),
    Delete(EntityDelete),
    Carry(EntityCarry),
    Restore(EntityRestore),
}

impl Entity {
    /// The command's [`USAGE`](super::spec::USAGE) path.
    pub(crate) fn path(&self) -> &'static str {
        match self {
            Entity::Status(_) => "entity status",
            Entity::Get(_) => "entity get",
            Entity::Push(_) => "entity push",
            Entity::Delete(_) => "entity delete",
            Entity::Carry(_) => "entity carry",
            Entity::Restore(_) => "entity restore",
        }
    }
}

/// `twaco entity get <Collection/Name> [--out <path>]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct EntityGet {
    #[arg(long = "out", value_name = "path", value_parser = super::flags::path())]
    pub(crate) out: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// Fetch one raw export without ever replacing a file that belongs to the solution.
pub(crate) fn entity_get(solution: &Solution, args: &EntityGet) -> u8 {
    if args.operands.len() != 1 {
        eprintln!(
            "{} entity get needs exactly one entity name",
            style::prefix()
        );
        return FAILED;
    }
    use twaco::core::entity_get;
    let key = match entity_get::target(solution, &args.operands[0]) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    if let Some(out) = &args.out {
        let protected = workspace::entities(solution);
        if protected
            .iter()
            .any(|candidate| same_path(out, &candidate.path))
        {
            eprintln!(
                "{} --out {} is a project entity file; entity get never overwrites project source",
                style::prefix(),
                out.display()
            );
            return FAILED;
        }
    }

    let profile_name = args.profile.name();
    let profile = match profile::load(&solution.root, profile_name) {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let live = match entity_get::fetch(&server::Client::new(profile), &key) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };

    if let Some(out) = &args.out {
        if let Err(error) = workspace::write_entity(out, &live) {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
        println!("wrote {} raw bytes to {}", live.len(), out.display());
    } else if let Err(error) = std::io::stdout().write_all(&live) {
        eprintln!("{} stdout: {error}", style::prefix());
        return FAILED;
    }
    OK
}

/// `twaco entity status <name>|--all`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct EntityStatus {
    #[command(flatten)]
    pub(crate) selection: Selection,
    #[command(flatten)]
    pub(crate) project: Project,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[arg(long = "record")]
    pub(crate) record: bool,
}

/// Compare working, server and tracked ancestor, optionally recording matching hashes once.
pub(crate) fn entity_status(solution: &Solution, args: &EntityStatus) -> u8 {
    if args.selection.operands.len() > 1 {
        eprintln!(
            "{} entity status accepts one entity name, or --all",
            style::prefix()
        );
        return FAILED;
    }
    let request = commands::status::StatusRequest {
        target: if args.selection.all {
            commands::status::StatusTarget::All
        } else {
            commands::status::StatusTarget::Names(args.selection.operands.clone())
        },
        project: args.project.project.clone(),
        record: args.record,
        profile: args.profile.name().to_string(),
        lock_label: "entity status",
        refuse_unreadable: true,
        refuse_record_failures: false,
    };
    let mut notices = commands::Notices::default();
    let progress = super::progress::reporter();
    let outcome = match commands::status::execute(
        solution,
        &request,
        server::Client::new,
        &mut notices,
        &progress,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            if let commands::status::StatusCommandError::Unreadable(items) = &error {
                for problem in items {
                    eprintln!("{} {problem}", style::prefix());
                }
            } else {
                eprintln!("{} {error}", style::prefix());
            }
            return FAILED;
        }
    };
    print_notices(&notices);
    if !outcome.failures.is_empty() {
        for failure in &outcome.failures {
            eprintln!("{} {failure}", style::prefix());
        }
        eprintln!(
            "{} {} entity status request(s) failed",
            style::prefix(),
            outcome.failures.len()
        );
        return FAILED;
    }

    println!(
        "{}",
        style::heading(&format!("{} entity status(es)", outcome.statuses.len()))
    );
    for (verdict, count) in status::counts(&outcome.statuses) {
        let label = format!("{:<19}", verdict.label());
        let label = if count == 0 {
            style::dim(&label)
        } else if verdict.is_drift() {
            style::warn(&label)
        } else {
            style::ok(&label)
        };
        println!("  {label} {count}");
    }
    if args.detail.detail {
        println!();
        for status in &outcome.statuses {
            println!(
                "{}/{}  {}",
                status.collection,
                status.name,
                status.verdict.label()
            );
            println!("  working  {}", status.working);
            println!("  server   {}", status.server.as_deref().unwrap_or("-"));
            println!(
                "  baseline local  {}",
                status.local_baseline.as_deref().unwrap_or("-")
            );
            println!(
                "  baseline server {}",
                status.server_baseline.as_deref().unwrap_or("-")
            );
        }
    }
    if outcome
        .statuses
        .iter()
        .all(|status| !status.verdict.is_drift())
    {
        OK
    } else {
        DRIFT
    }
}

/// `twaco entity push <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct EntityPush {
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[arg(long = "force")]
    pub(crate) force: bool,
    #[arg(long = "no-backup")]
    pub(crate) no_backup: bool,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// Push one entity. A dry run unless `--apply`: the blast radius is a server.
pub(crate) fn entity_push(solution: &Solution, args: &EntityPush) -> u8 {
    if args.operands.len() != 1 {
        eprintln!(
            "{} entity push takes exactly one entity name",
            style::prefix()
        );
        return FAILED;
    }
    let request = commands::push::PushRequest {
        entity: args.operands[0].clone(),
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        force: args.force,
        backup: !args.no_backup,
        profile: args.profile.name().to_string(),
    };
    let mut notices = commands::Notices::default();
    let progress = super::progress::reporter();
    let result = commands::push::execute_with_progress(
        solution,
        &request,
        server::Client::new,
        &mut notices,
        &progress,
    );
    print_notices(&notices);
    match result {
        Ok(commands::push::PushOutcome::Plan {
            entity, decision, ..
        }) => {
            let label = entity.to_string();
            match decision {
                push::Decision::AlreadyThere => {
                    println!("{label}: nothing to push")
                }
                push::Decision::Create => println!("{label}: would create it on the server"),
                push::Decision::Update => {
                    println!(
                        "{label}: would update it; the server is unchanged since the last sync"
                    )
                }
                push::Decision::Refuse(refusal) => {
                    println!("{label}: {}: {refusal}", style::warn("would refuse"));
                    if !request.force {
                        return DRIFT;
                    }
                    println!("  --force would push anyway");
                }
            }
            println!("dry run: nothing was sent; pass --apply to push");
            OK
        }
        Ok(commands::push::PushOutcome::Applied {
            entity,
            result,
            backup: saved,
            ..
        }) => {
            let label = entity.to_string();
            if let Some(dir) = saved {
                println!("{label}: the server's copy was saved to {dir} before it is overwritten");
            }
            match result {
                push::Outcome::AlreadyThere => {
                    println!("{label}: nothing to push; baseline is current");
                    OK
                }
                push::Outcome::Pushed { created } => {
                    let verb = if created { "created" } else { "updated" };
                    println!("{label}: {verb}, read back and matching; baseline recorded");
                    OK
                }
                push::Outcome::Refused(refusal) => {
                    eprintln!("{} {label}: refused: {refusal}", style::prefix());
                    eprintln!(
                        "{} nothing was sent; --force pushes anyway",
                        style::prefix()
                    );
                    DRIFT
                }
                push::Outcome::WouldDo(_) => unreachable!("an applied outcome cannot be a plan"),
            }
        }
        Err(error) => {
            if let commands::push::PushCommandError::Unreadable(unreadable) = &error {
                for problem in unreadable {
                    eprintln!("{} {problem}", style::prefix());
                }
            } else if matches!(error, commands::push::PushCommandError::Backup { .. }) {
                eprintln!(
                    "{} {error} (--no-backup pushes without one)",
                    style::prefix()
                );
            } else {
                if let (Some(label), Some(dir)) = (error.label(), error.backup()) {
                    println!(
                        "{label}: the server's copy was saved to {dir} before it is overwritten"
                    );
                }
                eprintln!("{} {error}", style::prefix());
            }
            FAILED
        }
    }
}

/// `twaco entity delete <name>...|--renamed`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct EntityDelete {
    #[arg(long = "renamed")]
    pub(crate) renamed: bool,
    /// Deprecated: the two acknowledgements below, with a warning.
    #[arg(long = "force")]
    pub(crate) force: bool,
    #[arg(long = "allow-repository-defined")]
    pub(crate) allow_repository_defined: bool,
    #[arg(long = "allow-outside-dependents")]
    pub(crate) allow_outside_dependents: bool,
    #[arg(long = "allow-file-repository-data-loss")]
    pub(crate) allow_file_repository_data_loss: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[arg(long = "no-backup")]
    pub(crate) no_backup: bool,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// Delete server entities in dependency-safe order. Planning is read-only and always succeeds
/// even when it reports guarded refusals; an apply reports any refusal or failed confirmation as
/// exit 2 after attempting the rest.
pub(crate) fn entity_delete_cmd(solution: &Solution, args: &EntityDelete) -> u8 {
    let (acknowledged, force_used) = entity_delete::Acknowledged {
        repository_defined: args.allow_repository_defined,
        outside_dependents: args.allow_outside_dependents,
        file_repository_data_loss: args.allow_file_repository_data_loss,
    }
    .with_force(args.force);
    if force_used {
        eprintln!("{} {}", style::prefix(), entity_delete_force_deprecation());
    }
    let request = commands::delete::EntityDeleteRequest {
        entities: args.operands.clone(),
        renamed: args.renamed,
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        acknowledgements: acknowledged,
        legacy_force_used: force_used,
        backup: !args.no_backup,
        profile: args.profile.name().to_string(),
    };
    let mut notices = commands::Notices::default();
    let result = commands::delete::execute(solution, &request, server::Client::new, &mut notices);
    print_notices(&notices);
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let (report, date, apply) = match outcome {
        commands::delete::EntityDeleteOutcome::Plan { report, .. } => (report, None, false),
        commands::delete::EntityDeleteOutcome::Applied { report, date, .. } => {
            (report, Some(date), true)
        }
    };
    if args.json.json {
        let key = if apply { "applied" } else { "plan" };
        let value = serde_json::json!({ (key): true, "entities": report.entities, "backup": report.backup });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("delete report serialises")
        );
    } else {
        println!("{}:", if apply { "applied" } else { "plan" });
        for (at, entity) in report.entities.iter().enumerate() {
            println!(
                "  {}. {}/{}  {:?}  {}",
                at + 1,
                entity.collection,
                entity.name,
                entity.status,
                entity.method
            );
            for (code, refusal) in entity.refusal_pairs() {
                println!("     refused [{}]: {refusal}", code.as_str());
            }
            for dependent in &entity.dependents {
                println!(
                    "     dependent: {}/{}",
                    dependent.collection, dependent.name
                );
            }
            for warning in &entity.warnings {
                println!("     warning: {warning}");
            }
            if let Some(error) = &entity.error {
                println!("     failed: {error}");
            }
        }
        println!("limit: {}", report.dependency_limit);
        if let Some(dir) = &report.backup {
            println!("backup: the server's copies were saved to {dir}; `twaco entity restore` puts them back");
        }
        if !apply {
            println!("dry run: nothing was deleted; pass --apply to delete");
        } else if report.ledger_changed {
            println!(
                "rename ledger marked with {}",
                date.expect("an applied delete outcome has a date")
            );
        }
    }
    if apply && report.failed() {
        FAILED
    } else {
        OK
    }
}

pub(crate) fn entity_delete_force_deprecation() -> &'static str {
    entity_delete::FORCE_DEPRECATION
}

/// `twaco entity restore [<set> [<entity>...]]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct EntityRestore {
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// List backup sets, or plan (and with --apply perform) importing one back.
pub(crate) fn entity_restore_cmd(solution: &Solution, args: &EntityRestore) -> u8 {
    let json = args.json.json;
    let request = commands::restore::RestoreRequest {
        set: args.operands.first().cloned(),
        only: args.operands.iter().skip(1).cloned().collect(),
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        profile: args.profile.name().to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome =
        match commands::restore::execute(solution, &request, server::Client::new, &mut notices) {
            Ok(outcome) => outcome,
            Err(error) => {
                print_notices(&notices);
                eprintln!("{} {error}", style::prefix());
                return FAILED;
            }
        };
    print_notices(&notices);
    let (set, report, apply) = match outcome {
        commands::restore::RestoreOutcome::Sets { sets, .. } => {
            if json {
                let value = serde_json::json!({ "sets": sets.iter().map(|set| serde_json::json!({
                "id": set.id, "created": set.manifest.created, "reason": set.manifest.reason, "entities": set.manifest.entities.len(),
            })).collect::<Vec<_>>() });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).expect("sets serialise")
                );
            } else if sets.is_empty() {
                println!("no backup sets under {}", backup::DIR);
            } else {
                for set in &sets {
                    println!(
                        "{}  {}  {} entit{}",
                        set.id,
                        set.manifest.reason,
                        set.manifest.entities.len(),
                        if set.manifest.entities.len() == 1 {
                            "y"
                        } else {
                            "ies"
                        }
                    );
                }
            }
            return OK;
        }
        commands::restore::RestoreOutcome::Plan { set, entities, .. } => (set, entities, false),
        commands::restore::RestoreOutcome::Applied { set, entities, .. } => (set, entities, true),
    };
    if json {
        let value = serde_json::json!({ (if apply { "applied" } else { "plan" }): true, "set": set.id, "entities": report });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("restore report serialises")
        );
    } else {
        println!("{} of {}:", if apply { "applied" } else { "plan" }, set.id);
        for entry in &report {
            println!("  {}/{}  {:?}", entry.collection, entry.name, entry.status);
            if let Some(error) = &entry.error {
                println!("     failed: {error}");
            }
        }
        if !apply {
            println!("dry run: nothing was imported; pass --apply to restore");
        }
    }
    if report
        .iter()
        .any(|entry| entry.status == backup::Status::Failed)
    {
        FAILED
    } else {
        OK
    }
}

/// `twaco entity carry <old> <new>...|--renamed`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct EntityCarry {
    #[arg(long = "renamed")]
    pub(crate) renamed: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[command(flatten)]
    pub(crate) json: Json,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// Copy permissions from old entities to the ones that replaced them. A plan only reads; an apply
/// writes what differs and reports any failure as exit 2 after attempting the rest. Only an apply
/// that reads the ledger's pending entries writes the workspace, and so takes its lock.
pub(crate) fn entity_carry_cmd(solution: &Solution, args: &EntityCarry) -> u8 {
    let pairs = match entity_carry::pairs_from_names(&args.operands) {
        Ok(pairs) => pairs,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let request = commands::carry::CarryRequest {
        pairs,
        renamed: args.renamed,
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        detail: args.detail.detail,
        profile: args.profile.name().to_string(),
        lock_label: "entity carry",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        match commands::carry::execute(solution, &request, server::Client::new, &mut notices) {
            Ok(outcome) => outcome,
            Err(error) => {
                print_notices(&notices);
                eprintln!("{} {error}", style::prefix());
                return FAILED;
            }
        };
    print_notices(&notices);
    let (report, apply, date) = match outcome {
        commands::carry::CarryOutcome::Plan { report, .. } => (report, false, None),
        commands::carry::CarryOutcome::Applied { report, date, .. } => (report, true, Some(date)),
    };
    if args.json.json {
        let key = if apply { "applied" } else { "plan" };
        let value = serde_json::json!({ (key): true, "entities": report.entities });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("carry report serialises")
        );
    } else {
        println!("{}:", if apply { "applied" } else { "plan" });
        for entity in &report.entities {
            let kinds = if entity.kinds.is_empty() {
                String::new()
            } else {
                format!("  [{}]", entity.kinds.join(", "))
            };
            println!(
                "  {}/{} -> {}  {:?}{kinds}",
                entity.collection, entity.old, entity.new, entity.status
            );
            if let Some(count) = entity.differences {
                println!("     the platform reports {count} difference(s) between them");
            }
            if let Some(error) = &entity.error {
                println!("     failed: {error}");
            }
        }
        if !apply {
            println!("dry run: nothing was written; pass --apply to carry");
        } else if report.ledger_changed {
            println!(
                "rename ledger marked with {}",
                date.expect("an applied carry has a date")
            );
        }
    }
    if report
        .entities
        .iter()
        .any(|entity| entity.status == entity_carry::Status::Failed)
        && apply
    {
        FAILED
    } else {
        OK
    }
}

/// `twaco permissions <command>`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Permissions {
    Init(PermissionsInit),
    Audit(PermissionsAudit),
    Apply(PermissionsApply),
    Diff(PermissionsDiff),
    Push(PermissionsPush),
}

impl Permissions {
    /// The command's [`USAGE`](super::spec::USAGE) path.
    pub(crate) fn path(&self) -> &'static str {
        match self {
            Permissions::Init(_) => "permissions init",
            Permissions::Audit(_) => "permissions audit",
            Permissions::Apply(_) => "permissions apply",
            Permissions::Diff(_) => "permissions diff",
            Permissions::Push(_) => "permissions push",
        }
    }
}

/// `twaco permissions diff <name>...|--all`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct PermissionsDiff {
    #[command(flatten)]
    pub(crate) selection: Selection,
    #[command(flatten)]
    pub(crate) project: Project,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[command(flatten)]
    pub(crate) json: Json,
}

/// `twaco permissions push <name>...|--all|--platform`: what diff takes, and how to push.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct PermissionsPush {
    #[command(flatten)]
    pub(crate) diff: PermissionsDiff,
    #[arg(long = "platform")]
    pub(crate) platform: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
}

/// `twaco permissions audit [--server]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct PermissionsAudit {
    #[command(flatten)]
    pub(crate) project: Project,
    #[arg(long = "server")]
    pub(crate) server: bool,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) json: Json,
}

/// `twaco permissions apply`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct PermissionsApply {
    #[command(flatten)]
    pub(crate) project: Project,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) json: Json,
}

/// `twaco permissions init [--from-helper]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct PermissionsInit {
    #[command(flatten)]
    pub(crate) project: Project,
    #[arg(long = "from-helper")]
    pub(crate) from_helper: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
}

/// Compare entity permissions with the server's (`permissions diff`), or make them the
/// repository's (`permissions push`). A diff exits 1 when any entity differs, like a check; a push
/// is a plan without `--apply`, and an applied push that fails anywhere exits 2.
pub(crate) fn permissions_cmd(
    solution: &Solution,
    args: &PermissionsDiff,
    push_args: Option<&PermissionsPush>,
) -> u8 {
    let push = push_args.is_some();
    let apply = push_args.is_some_and(|push| push.apply.apply);
    if push_args.is_some_and(|push| push.platform) {
        if args.selection.all || !args.selection.operands.is_empty() {
            eprintln!(
                "{} --platform pushes the policy's platform entries; push entities separately",
                style::prefix()
            );
            return FAILED;
        }
        return permissions_platform_cmd(solution, args, apply);
    }
    let request = commands::permissions::PermissionsRequest {
        target: if args.selection.all {
            commands::status::StatusTarget::All
        } else {
            commands::status::StatusTarget::Names(args.selection.operands.clone())
        },
        project: args.project.project.clone(),
        mode: if apply { Mode::Apply } else { Mode::Plan },
        profile: args.profile.name().to_string(),
        lock_label: "permissions push",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::permissions::execute_with_progress(
        solution,
        &request,
        server::Client::new,
        &mut notices,
        &super::progress::reporter(),
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let report = &outcome.report;
    if args.json.json {
        let value = serde_json::json!({
            "applied": report.applied,
            "entities": report.entities,
            "recorded": outcome.recorded,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("permissions report serialises")
        );
    } else {
        let mut counts = Vec::new();
        for status in [
            twaco::core::permissions::Status::Same,
            twaco::core::permissions::Status::Differs,
            twaco::core::permissions::Status::Pushed,
            twaco::core::permissions::Status::NotOnServer,
            twaco::core::permissions::Status::Unmanaged,
            twaco::core::permissions::Status::Failed,
        ] {
            let count = report.count(status);
            if count > 0 {
                counts.push(format!("{count} {}", status.label()));
            }
        }
        println!(
            "{} entit{}: {}",
            report.entities.len(),
            if report.entities.len() == 1 {
                "y"
            } else {
                "ies"
            },
            counts.join(", ")
        );
        for entity in &report.entities {
            if entity.differences.is_empty() && entity.error.is_none() {
                continue;
            }
            println!(
                "{}/{}  {}",
                entity.collection,
                entity.name,
                entity.status.label()
            );
            for difference in &entity.differences {
                println!("  {difference}");
            }
            if let Some(error) = &entity.error {
                println!("  failed: {error}");
            }
        }
        if let Some(recorded) = outcome.recorded {
            println!(
                "baseline recorded for {recorded} pushed entit{} now matching the server",
                if recorded == 1 { "y" } else { "ies" }
            );
        }
        let differs = report.count(twaco::core::permissions::Status::Differs);
        if !push && differs > 0 {
            println!(
                "an import never removes a grant or changes the server's allow/deny; \
                 `twaco permissions push` makes these the repository's"
            );
        } else if push && !report.applied && differs > 0 {
            println!("dry run: nothing was written; pass --apply to push");
        }
    }
    if report.count(twaco::core::permissions::Status::Failed) > 0
        || (report.applied && report.count(twaco::core::permissions::Status::NotOnServer) > 0)
    {
        FAILED
    } else if !push && report.count(twaco::core::permissions::Status::Differs) > 0 {
        DRIFT
    } else {
        OK
    }
}

/// Audit each project's permission policy against its entity XML (`permissions audit`). Exits 1
/// when any finding is an error, like a check.
pub(crate) fn permissions_audit_cmd(solution: &Solution, args: &PermissionsAudit) -> u8 {
    use twaco::core::permissions::audit::Severity;
    let request = commands::permissions::AuditRequest {
        project: args.project.project.clone(),
        server: args.server.then(|| args.profile.name().to_string()),
    };
    let report = match commands::permissions::execute_audit_with_progress(
        solution,
        &request,
        server::Client::new,
        &super::progress::reporter(),
    ) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    let detail = args.detail.detail;
    if args.json.json {
        let mut value = serde_json::to_value(&report).expect("audit report serialises");
        if !detail {
            for project in value["projects"].as_array_mut().into_iter().flatten() {
                for finding in project["findings"].as_array_mut().into_iter().flatten() {
                    if let Some(object) = finding.as_object_mut() {
                        object.remove("details");
                    }
                }
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("audit report serialises")
        );
    } else {
        for project in &report.projects {
            let mut counts = Vec::new();
            for severity in [Severity::Error, Severity::Warning, Severity::Note] {
                let count = project.count(severity);
                if count > 0 {
                    counts.push(format!(
                        "{count} {}{}",
                        severity.label(),
                        if count == 1 { "" } else { "s" }
                    ));
                }
            }
            let mode = match &project.helper {
                Some(helper) => format!("helper mode, {helper}"),
                None => "plain mode".to_string(),
            };
            println!(
                "{} ({mode}): {} entities, {}",
                project.project,
                project.entities,
                if counts.is_empty() {
                    "as the policy says".to_string()
                } else {
                    counts.join(", ")
                }
            );
            for finding in &project.findings {
                println!("  {finding}");
                if detail {
                    for line in &finding.details {
                        println!("      {line}");
                    }
                }
            }
        }
        if !report.without_policy.is_empty() {
            println!(
                "without a permissions.toml: {}",
                report.without_policy.join(", ")
            );
        }
        if !detail
            && report
                .projects
                .iter()
                .any(|p| p.findings.iter().any(|f| !f.details.is_empty()))
        {
            println!("--detail lists every grant behind a finding");
        }
    }
    if report.count(Severity::Error) > 0 {
        DRIFT
    } else {
        OK
    }
}

/// Write each project's permission policy into its entity XML (`permissions apply`); a plan
/// unless `--apply`.
pub(crate) fn permissions_apply_cmd(solution: &Solution, args: &PermissionsApply) -> u8 {
    let request = commands::permissions::ApplyRequest {
        project: args.project.project.clone(),
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "permissions apply",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::permissions::execute_apply_with_progress(
        solution,
        &request,
        &mut notices,
        &super::progress::reporter(),
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    let detail = args.detail.detail;
    let plan = &outcome.plan;
    if args.json.json {
        let mut value = serde_json::to_value(plan).expect("apply plan serialises");
        if !detail {
            for project in value["projects"].as_array_mut().into_iter().flatten() {
                for change in project["changes"].as_array_mut().into_iter().flatten() {
                    if let Some(object) = change.as_object_mut() {
                        object.remove("details");
                    }
                }
                for finding in project["remaining"].as_array_mut().into_iter().flatten() {
                    if let Some(object) = finding.as_object_mut() {
                        object.remove("details");
                    }
                }
            }
        }
        value["applied"] = serde_json::json!(outcome.applied);
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("apply plan serialises")
        );
        return if plan.remaining_errors() > 0 {
            DRIFT
        } else {
            OK
        };
    }
    for project in &plan.projects {
        let mode = match &project.helper {
            Some(helper) => format!("helper mode, {helper}"),
            None => "plain mode".to_string(),
        };
        let (added, removed) = project
            .changes
            .iter()
            .fold((0, 0), |(a, r), c| (a + c.added, r + c.removed));
        if project.changes.is_empty() {
            println!(
                "{} ({mode}): the entity XML is as the policy says",
                project.project
            );
        } else {
            println!(
                "{} ({mode}): {} file{}, {added} grant{} added, {removed} removed",
                project.project,
                project.changes.len(),
                if project.changes.len() == 1 { "" } else { "s" },
                if added == 1 { "" } else { "s" },
            );
        }
        for change in &project.changes {
            let mut sets: Vec<&str> = change.sets.iter().map(|kind| kind.label()).collect();
            if change.helper {
                sets.push(if change.entity.starts_with("DataShapes/") {
                    "helper columns"
                } else {
                    "helper tables"
                });
            }
            println!(
                "  {}  {}  +{} -{}",
                change.entity,
                sets.join(", "),
                change.added,
                change.removed
            );
            if detail {
                for line in &change.details {
                    println!("      {line}");
                }
            }
        }
        if !project.remaining.is_empty() {
            println!("  left for you (`permissions audit` explains):");
            for finding in &project.remaining {
                println!("    {finding}");
            }
        }
    }
    let changed = plan.changes().count();
    if changed > 0 {
        if outcome.applied {
            println!(
                "wrote {changed} file{}; deploy them, then `twaco permissions push` removes on the server what an import cannot",
                if changed == 1 { "" } else { "s" }
            );
        } else {
            println!("dry run: nothing was written; pass --apply to write");
        }
    }
    if plan.remaining_errors() > 0 {
        DRIFT
    } else {
        OK
    }
}

/// Add the policies' platform grants and memberships the server lacks (`permissions push
/// --platform`); a plan unless `--apply`. Exits 2 when anything failed.
fn permissions_platform_cmd(solution: &Solution, args: &PermissionsDiff, apply: bool) -> u8 {
    use twaco::core::permissions::platform::State;
    let request = commands::permissions::PlatformRequest {
        project: args.project.project.clone(),
        mode: if apply { Mode::Apply } else { Mode::Plan },
        profile: args.profile.name().to_string(),
    };
    let report = match commands::permissions::execute_platform_with_progress(
        solution,
        &request,
        server::Client::new,
        &super::progress::reporter(),
    ) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    if args.json.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("platform report serialises")
        );
    } else {
        let counts: Vec<String> = [
            State::Present,
            State::Missing,
            State::Added,
            State::Skipped,
            State::Failed,
        ]
        .into_iter()
        .filter(|state| report.count(*state) > 0)
        .map(|state| format!("{} {}", report.count(state), state.label()))
        .collect();
        println!(
            "{} platform grant(s) and membership(s): {}",
            report.items.len(),
            if counts.is_empty() {
                "none in any policy".to_string()
            } else {
                counts.join(", ")
            }
        );
        for item in &report.items {
            if item.state == State::Present {
                continue;
            }
            print!(
                "  {:<8} {} {}: {}",
                item.state.label(),
                item.entity,
                item.what,
                item.group
            );
            match &item.error {
                Some(error) => println!(" ({error})"),
                None => println!(),
            }
        }
        if !report.applied && report.count(State::Missing) > 0 {
            println!("dry run: nothing was written; pass --apply to add what is missing (nothing is ever removed)");
        }
    }
    if report.count(State::Failed) > 0 {
        FAILED
    } else {
        OK
    }
}

/// Draft a permissions.toml for each project without one (`permissions init`); prints the drafts
/// unless `--apply` writes them.
pub(crate) fn permissions_init_cmd(solution: &Solution, args: &PermissionsInit) -> u8 {
    let request = commands::permissions::InitRequest {
        project: args.project.project.clone(),
        from_helper: args.from_helper,
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        lock_label: "permissions init",
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::permissions::execute_init(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) => {
            print_notices(&notices);
            eprintln!("{} {error}", style::prefix());
            return FAILED;
        }
    };
    print_notices(&notices);
    if args.json.json {
        let drafts: Vec<serde_json::Value> = outcome
            .drafts
            .iter()
            .map(|draft| {
                serde_json::json!({
                    "project": draft.project,
                    "path": draft.path.display().to_string(),
                    "source": draft.source,
                    "text": draft.text,
                    "notes": draft.notes,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "written": outcome.written,
                "drafts": drafts,
            }))
            .expect("drafts serialise")
        );
        return OK;
    }
    for draft in &outcome.drafts {
        if outcome.written {
            println!("wrote {} (from {})", draft.path.display(), draft.source);
        } else {
            println!("# {} would be:\n{}", draft.path.display(), draft.text);
        }
        for note in &draft.notes {
            println!("  note: {note}");
        }
    }
    if outcome.written {
        println!(
            "`twaco permissions audit` checks it; `permissions apply` changes nothing but what the notes name"
        );
    } else if !outcome.drafts.is_empty() {
        println!("dry run: nothing was written; pass --apply to write the drafts");
    }
    OK
}
