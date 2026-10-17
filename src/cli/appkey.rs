use super::flags::{Apply, Json, Profile, Project};
use super::style;
use crate::{FAILED, OK};
use twaco::core::appkey;
use twaco::core::commands::appkey::{
    self as command, AppKeyAction, AppKeyCommandError, AppKeyOutcome, AppKeyRequest,
};
use twaco::core::commands::Mode;
use twaco::core::config::Solution;
use twaco::core::server;

/// `twaco appkey create|show|delete <name>`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct AppKey {
    #[arg(long = "user", value_name = "value")]
    pub(crate) user: Option<String>,
    #[arg(long = "expires", value_name = "when")]
    pub(crate) expires: Option<String>,
    #[command(flatten)]
    pub(crate) project: Project,
    #[arg(long = "description", value_name = "text")]
    pub(crate) description: Option<String>,
    #[arg(long = "whitelist", value_name = "ip,...")]
    pub(crate) whitelist: Option<String>,
    #[arg(long = "save", value_name = "profile")]
    pub(crate) save: Option<String>,
    #[arg(long = "show")]
    pub(crate) show: bool,
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[command(flatten)]
    pub(crate) profile: Profile,
    /// What to do; clap refuses any other word, with the ones it takes.
    #[arg(id = "action", value_name = "action", value_enum)]
    pub(crate) action: Option<AppKeyVerb>,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// What `appkey` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum AppKeyVerb {
    Create,
    Show,
    Delete,
}

/// The default lifetime of a new key. The server's own, without an expiration date, is a day.
const DEFAULT_EXPIRES: &str = "30d";

pub(crate) fn appkey_cmd(solution: &Solution, args: &AppKey) -> u8 {
    let Some(verb) = args.action else {
        eprintln!("{} appkey takes: create | show | delete", style::prefix());
        return FAILED;
    };
    let [name] = args.operands.as_slice() else {
        eprintln!("{} appkey needs one key name", style::prefix());
        return FAILED;
    };
    let only_create = [
        ("--user", args.user.is_some()),
        ("--expires", args.expires.is_some()),
        ("--project", args.project.project.is_some()),
        ("--description", args.description.is_some()),
        ("--whitelist", args.whitelist.is_some()),
    ];
    if verb != AppKeyVerb::Create {
        if let Some((flag, _)) = only_create.iter().find(|(_, given)| *given) {
            eprintln!("{} {flag} is only for appkey create", style::prefix());
            return FAILED;
        }
    }
    let action = match verb {
        AppKeyVerb::Delete => return delete(solution, args, name),
        AppKeyVerb::Show => {
            if args.apply.apply {
                eprintln!(
                    "{} appkey show changes nothing on the server; leave out --apply",
                    style::prefix()
                );
                return FAILED;
            }
            AppKeyAction::Show { name: name.clone() }
        }
        AppKeyVerb::Create => {
            let expires = match appkey::parse_expires(
                args.expires.as_deref().unwrap_or(DEFAULT_EXPIRES),
                jiff::Timestamp::now(),
            ) {
                Ok(when) => when,
                Err(error) => {
                    eprintln!("{} {error}", style::prefix());
                    return FAILED;
                }
            };
            AppKeyAction::Create {
                key: appkey::NewKey {
                    name: name.clone(),
                    user: args.user.clone().unwrap_or_default(),
                    expires,
                    project: args.project.project.clone(),
                    description: args.description.clone().unwrap_or_default(),
                    whitelist: args
                        .whitelist
                        .as_deref()
                        .map(|list| {
                            list.split(',')
                                .map(str::trim)
                                .filter(|ip| !ip.is_empty())
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                },
                mode: if args.apply.apply {
                    Mode::Apply
                } else {
                    Mode::Plan
                },
            }
        }
    };
    let request = AppKeyRequest {
        action,
        profile: args.profile.name().to_string(),
        save: args.save.clone(),
    };
    match command::execute(solution, &request, server::Client::new) {
        Ok(outcome) => {
            print(&outcome, args);
            OK
        }
        Err(error) => {
            eprintln!("{} appkey: {error}", style::prefix());
            if let AppKeyCommandError::Save { .. } = error {
                // The key exists; say so plainly so nobody creates a second one.
                eprintln!(
                    "{} the key was not printed; `appkey show` reads it",
                    style::prefix()
                );
            }
            FAILED
        }
    }
}

fn print(outcome: &AppKeyOutcome, args: &AppKey) {
    match outcome {
        AppKeyOutcome::Planned { key, .. } => {
            if args.json.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "applied": false,
                        "name": key.name,
                        "user": key.user,
                        "expires": key.expires.to_string(),
                        "project": key.project,
                        "whitelist": key.whitelist,
                        "save": args.save,
                    })
                );
                return;
            }
            println!(
                "would create application key {} for user {}, expiring {}",
                key.name, key.user, key.expires
            );
            if let Some(project) = &key.project {
                println!("  project {project}");
            }
            if !key.whitelist.is_empty() {
                println!("  only from {}", key.whitelist.join(", "));
            }
            match &args.save {
                Some(profile) => {
                    println!("  the key would be saved as app_key of profile {profile}")
                }
                None if args.show => println!("  the key would be printed"),
                None => println!("  the key would not be printed: `appkey show` reads it later"),
            }
            println!("dry run: nothing was sent; pass --apply");
        }
        AppKeyOutcome::Done {
            created,
            key,
            saved,
            ..
        } => {
            let expires = key.expires.map(|when| when.to_string());
            if args.json.json {
                let mut json = serde_json::json!({
                    "applied": *created,
                    "name": key.name,
                    "expires": expires,
                    "saved_to": saved.as_ref().map(|path| path.display().to_string()),
                });
                if args.show {
                    json["key"] = key.reveal().into();
                }
                println!("{json}");
                return;
            }
            let verb = if *created { "created" } else { "found" };
            println!(
                "{verb} application key {}{}",
                key.name,
                expires.map_or(String::new(), |when| format!(", expiring {when}"))
            );
            if let Some(path) = saved {
                println!("  saved as app_key in {}", path.display());
            }
            if args.show {
                println!("{}", key.reveal());
            } else if saved.is_none() {
                println!("  not printed: pass --show to see it, or --save <profile> to keep it");
            }
        }
    }
}

/// `appkey delete <name>` is `entity delete ApplicationKeys/<name>`: the same guards, plan and
/// backup as deleting any entity.
fn delete(solution: &Solution, args: &AppKey, name: &str) -> u8 {
    if args.save.is_some() || args.show {
        eprintln!(
            "{} --save and --show are for create and show",
            style::prefix()
        );
        return FAILED;
    }
    let delete = super::entity::EntityDelete {
        renamed: false,
        force: false,
        allow_repository_defined: false,
        allow_outside_dependents: false,
        allow_file_repository_data_loss: false,
        apply: Apply {
            apply: args.apply.apply,
        },
        no_backup: false,
        profile: Profile {
            profile: args.profile.profile.clone(),
        },
        json: Json {
            json: args.json.json,
        },
        operands: vec![format!("ApplicationKeys/{name}")],
    };
    super::entity::entity_delete_cmd(solution, &delete)
}
