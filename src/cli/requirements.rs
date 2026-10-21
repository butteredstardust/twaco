//! Requirements command arguments and presentation.

use super::flags::{Apply, Detail, Json, Profile};
use crate::{print_notices, DRIFT, FAILED, OK};
use twaco::core::commands::{self, requirements, Mode};
use twaco::core::{config::Solution, server};

#[derive(Debug, clap::Subcommand)]
pub(crate) enum Requirements {
    Init(Init),
    Check(Check),
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Init {
    #[command(flatten)]
    apply: Apply,
    #[command(flatten)]
    json: Json,
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Check {
    #[arg(long = "server")]
    server: bool,
    #[command(flatten)]
    profile: Profile,
    #[command(flatten)]
    detail: Detail,
    #[command(flatten)]
    json: Json,
}

pub(crate) fn requirements_cmd(solution: &Solution, command: &Requirements) -> u8 {
    match command {
        Requirements::Init(args) => {
            let mut notices = commands::Notices::default();
            let outcome = requirements::execute_init(
                solution,
                &requirements::InitRequest {
                    mode: if args.apply.apply {
                        Mode::Apply
                    } else {
                        Mode::Plan
                    },
                    lock_label: "requirements init",
                },
                &mut notices,
            );
            print_notices(&notices);
            match outcome {
                Ok(outcome) => {
                    if args.json.json {
                        println!(
                            "{}",
                            serde_json::json!({"summary": outcome.report.summary(), "dry_run": !outcome.written, "written": outcome.written, "file": "requirements.toml", "draft": outcome.text, "requirements": outcome.report.json(true)})
                        );
                    } else {
                        println!("{}", outcome.report.summary());
                        if outcome.written {
                            println!("wrote requirements.toml");
                        } else {
                            println!("{}\nplan: --apply writes requirements.toml", outcome.text);
                        }
                    }
                    OK
                }
                Err(error) => {
                    eprintln!("{error}");
                    FAILED
                }
            }
        }
        Requirements::Check(args) => {
            let progress = super::progress::reporter();
            match requirements::execute_check(
                solution,
                &requirements::CheckRequest {
                    server: args.server.then(|| args.profile.name().to_string()),
                },
                server::Client::new,
                &progress,
            ) {
                Ok(report) => {
                    if args.json.json {
                        println!("{}", report.json(args.detail.detail));
                    } else {
                        println!("{}", report.summary());
                        for finding in report.findings.iter().take(if args.detail.detail {
                            usize::MAX
                        } else {
                            8
                        }) {
                            println!(
                                "  {} [{}]: {}",
                                finding.requirement, finding.rule, finding.message
                            );
                            if args.detail.detail {
                                for from in &finding.referrers {
                                    println!("    from {from}");
                                }
                            }
                        }
                        if !args.detail.detail && report.findings.len() > 8 {
                            println!(
                                "  pass --detail for all {} findings and referrers",
                                report.findings.len()
                            );
                        }
                    }
                    if report.blocks() {
                        DRIFT
                    } else {
                        OK
                    }
                }
                Err(error) => {
                    eprintln!("{error}");
                    FAILED
                }
            }
        }
    }
}
