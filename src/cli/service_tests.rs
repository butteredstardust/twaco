use super::flags::{Apply, Detail, Json, Profile};
use super::style;
use crate::{DRIFT, FAILED, OK};
use twaco::core::commands::service_tests::{self as command, TestOutcome, TestRequest};
use twaco::core::commands::Mode;
use twaco::core::config::Solution;
use twaco::core::server;
use twaco::core::testing::cases::{outcomes_json, plan_json, Case};

/// `twaco test [<filter>...] [--apply] [--json] [--detail]`.
#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Test {
    #[command(flatten)]
    pub(crate) apply: Apply,
    #[command(flatten)]
    pub(crate) json: Json,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) profile: Profile,
    #[arg(id = "operands", value_name = "operand")]
    pub(crate) operands: Vec<String>,
}

/// `twaco test [<filter>...]`: the service cases of `tests/*.toml`, called as the test users.
pub(crate) fn test_cmd(solution: &Solution, args: &Test) -> u8 {
    let request = TestRequest {
        filters: args.operands.clone(),
        mode: if args.apply.apply {
            Mode::Apply
        } else {
            Mode::Plan
        },
        profile: args.profile.name().to_string(),
    };
    match command::execute(solution, &request, server::Client::new) {
        Ok(TestOutcome::Plan { cases, .. }) if args.json.json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&plan_json(&cases)).expect("a plan serialises")
            );
            OK
        }
        Ok(TestOutcome::Plan { cases, .. }) => {
            let calls: usize = cases.iter().map(|case| case.users.len()).sum();
            for case in &cases {
                println!(
                    "{}: {}.{} as {}, expects {}",
                    case.name,
                    case.target,
                    case.service,
                    case.users.join(", "),
                    case.expect
                );
            }
            println!(
                "dry run: {} case(s), {calls} call(s); nothing was sent; pass --apply to run them",
                cases.len()
            );
            OK
        }
        Ok(TestOutcome::Ran {
            cases, outcomes, ..
        }) => {
            if args.json.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&outcomes_json(&cases, &outcomes))
                        .expect("a result serialises")
                );
            } else {
                print_outcomes(&cases, &outcomes, args.detail.detail);
            }
            if outcomes.iter().all(|outcome| outcome.failure.is_none()) {
                OK
            } else {
                DRIFT
            }
        }
        Err(error) => {
            eprintln!("{} test: {error}", style::prefix());
            FAILED
        }
    }
}

fn print_outcomes(cases: &[Case], outcomes: &[twaco::core::testing::cases::Outcome], all: bool) {
    let failed = outcomes
        .iter()
        .filter(|outcome| outcome.failure.is_some())
        .count();
    for outcome in outcomes {
        let case = &cases[outcome.case];
        match &outcome.failure {
            Some(why) => println!(
                "{} {} as {}: {why}",
                style::warn("fail"),
                case.name,
                outcome.user
            ),
            None if all => println!("{} {} as {}", style::ok("pass"), case.name, outcome.user),
            None => {}
        }
    }
    println!(
        "{} call(s): {} passed, {failed} failed",
        outcomes.len(),
        outcomes.len() - failed
    );
}
