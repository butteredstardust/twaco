use super::args::Args;
use super::style;
use crate::{DRIFT, FAILED, OK};
use twaco::core::commands::users::{self as command, UsersAction, UsersOutcome, UsersRequest};
use twaco::core::commands::Mode;
use twaco::core::config::Solution;
use twaco::core::server;
use twaco::core::testing::users::{users_json, Membership, UserState};

/// `twaco users list|seed|delete`: the test users of `[[test.user]]` on the profile's server.
pub(crate) fn users_cmd(solution: &Solution, args: &Args) -> u8 {
    let mode = if args.has("--apply") {
        Mode::Apply
    } else {
        Mode::Plan
    };
    let action = match args.names.as_slice() {
        [action] if action == "list" => UsersAction::List,
        [action] if action == "seed" => UsersAction::Seed { mode },
        [action] if action == "delete" => UsersAction::Delete { mode },
        _ => {
            eprintln!("{} users takes: list | seed | delete", style::prefix());
            return FAILED;
        }
    };
    if action == UsersAction::List && args.has("--apply") {
        eprintln!(
            "{} users list changes nothing; leave out --apply",
            style::prefix()
        );
        return FAILED;
    }
    let request = UsersRequest {
        action,
        profile: args.profile.as_deref().unwrap_or("default").to_string(),
    };
    match command::execute(solution, &request, server::Client::new) {
        Ok(UsersOutcome::Listed { users, .. }) => {
            if args.has("--json") {
                println!("{}", json(&users));
            } else {
                print_users(&users);
            }
            if users.iter().all(ready) {
                OK
            } else {
                DRIFT
            }
        }
        Ok(UsersOutcome::Changed { changed, .. }) => {
            if changed.steps.is_empty() {
                println!("nothing to do");
            }
            for step in &changed.steps {
                if changed.applied {
                    println!("{}: {step}", style::ok("done"));
                } else {
                    println!("would {step}");
                }
            }
            if changed.applied {
                print_users(&changed.users);
                if changed.users.iter().all(ready) {
                    OK
                } else {
                    DRIFT
                }
            } else {
                if !changed.steps.is_empty() {
                    println!("dry run: no change was sent; pass --apply");
                }
                OK
            }
        }
        Err(error) => {
            eprintln!("{} users: {error}", style::prefix());
            FAILED
        }
    }
}

/// A user a test can sign in as, in every group it should be in.
fn ready(user: &UserState) -> bool {
    user.exists
        && user.signs_in != Some(false)
        && user
            .groups
            .iter()
            .all(|(_, state)| *state == Membership::Member)
}

fn print_users(users: &[UserState]) {
    for user in users {
        let state = if !user.exists {
            style::warn("absent").to_string()
        } else {
            match user.signs_in {
                Some(true) => style::ok("signs in").to_string(),
                Some(false) => style::warn("cannot sign in with test_password").to_string(),
                None => "present (no test_password to sign in with)".to_string(),
            }
        };
        println!("{}: {state}", user.name);
        for (group, membership) in &user.groups {
            let text = match membership {
                Membership::Member => "member".to_string(),
                Membership::Missing => style::warn("not a member").to_string(),
                Membership::NoGroup => style::warn("group not on the server").to_string(),
            };
            println!("  {group}: {text}");
        }
    }
}

fn json(users: &[UserState]) -> String {
    serde_json::to_string_pretty(&users_json(users)).expect("a list serialises")
}
