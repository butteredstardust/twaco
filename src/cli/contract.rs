use super::flags::{Detail, Json, Project};
use crate::{FAILED, OK};
use twaco::core::{config::Solution, contract};

#[derive(Debug, clap::Subcommand)]
pub(crate) enum Contract {
    Diff(Diff),
}

#[derive(Debug, clap::Args)]
#[group(skip)]
pub(crate) struct Diff {
    #[arg(long = "against", value_name = "git rev")]
    pub(crate) against: Option<String>,
    #[command(flatten)]
    pub(crate) project: Project,
    #[command(flatten)]
    pub(crate) detail: Detail,
    #[command(flatten)]
    pub(crate) json: Json,
}

pub(crate) fn diff_cmd(solution: &Solution, args: &Diff) -> u8 {
    let Some(against) = args.against.as_deref() else {
        eprintln!("twaco: contract diff needs --against <git rev>; try --against origin/main");
        return FAILED;
    };
    match contract::diff(solution, against, args.project.project.as_deref()) {
        Ok(report) => {
            if args.json.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("contract report serialises")
                );
            } else {
                print!("{}", contract::render(&report, args.detail.detail));
            }
            if report.breaking.is_empty() {
                OK
            } else {
                1
            }
        }
        Err(error) => {
            eprintln!("twaco: contract diff: {error}");
            FAILED
        }
    }
}
