//! `cargo hil ab` and `cargo hil bisect`: their
//! arguments, and calls into `oer-hil-experiment`.

use std::{ffi::OsString, num::NonZeroU32};

use crate::experiment::{ab, bisect, launch::Runner};
use oer_hil_model::schema::run::Outcome;
use oer_hil_runs::RunId;
use oer_os_process::Checkout;

use crate::Result;

#[derive(clap::Parser)]
#[command(name = "cargo hil ab")]
pub(crate) struct AbCli {
    /// Variant A: `rev=<revision>`, then `;override:<esp-hal|embassy|xarxa>=<path>`
    /// for each replaced dependency; `rev=` defaults to HEAD.
    #[arg(long)]
    a: String,
    /// Variant B, in the same form.
    #[arg(long)]
    b: String,
    #[arg(long = "scenario", required = true)]
    scenarios: Vec<String>,
    /// Measured rounds (pairs) per layout seed, after its preparation
    /// round that builds both arms' images and is never paired.
    #[arg(long, default_value_t = 3)]
    repetitions: u32,
    /// Layout seeds 1..=K, each built and run for both arms.
    #[arg(long, default_value_t = 1)]
    layout_seeds: u32,
    /// Seed of the balanced AB/BA order of the rounds; defaults to the
    /// experiment id. Recorded in the report and every run's manifest.
    #[arg(long, value_name = "SEED")]
    order_seed: Option<u64>,
    #[command(flatten)]
    boards: crate::jobs::BoardChoiceArgs,
}

/// Run the comparison `args` describe with `runner`; returns each created
/// run with its outcome.
pub(crate) fn ab(
    ctx: &Checkout,
    owner: &str,
    runner: &Runner,
    args: &[OsString],
) -> Result<Vec<(RunId, Option<Outcome>)>> {
    use clap::Parser as _;
    let cli = AbCli::try_parse_from(
        std::iter::once(OsString::from("cargo hil ab")).chain(args.iter().cloned()),
    )?;
    let finished = ab::run(
        &ctx.root,
        owner,
        runner,
        &ab::Spec {
            a: cli.a.parse()?,
            b: cli.b.parse()?,
            scenarios: cli.scenarios,
            repetitions: cli.repetitions,
            layout_seeds: cli.layout_seeds,
            boards: cli.boards.arguments(),
            order_seed: cli.order_seed,
        },
    )?;
    print!("{}", ab::summary(&finished.report));
    println!("report: {}", finished.path.display());
    Ok(finished.runs)
}

#[derive(clap::Parser)]
#[command(name = "cargo hil bisect")]
/// One step of a manual bisection: run SCENARIO on REVISION and exit as
/// `git bisect run` reads it (0 pass, 1 fail, 125 skip, 128 stop). Run it
/// from `git bisect start --no-checkout BAD GOOD` with
/// `git bisect run cargo hil bisect BISECT_HEAD --scenario S [--chip C]`.
pub(crate) struct BisectCli {
    /// The revision to test: a commit, tag, branch or `BISECT_HEAD`.
    revision: String,
    #[arg(long)]
    scenario: String,
    /// The chip, for a scenario that runs on more than one.
    #[arg(long)]
    chip: Option<String>,
    /// Build the image with this code layout seed.
    #[arg(long, value_name = "SEED")]
    layout_seed: Option<NonZeroU32>,
    #[command(flatten)]
    boards: crate::jobs::BoardChoiceArgs,
}

pub(crate) fn bisect(
    ctx: &Checkout,
    owner: &str,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let cli = BisectCli::try_parse_from(
        std::iter::once(OsString::from("cargo hil bisect")).chain(args.iter().cloned()),
    )?;
    // The commit names the step in its line, even when the step fails.
    let revision = oer_os_process::git::commit(&ctx.root, &cli.revision).unwrap_or(cli.revision);
    let spec = bisect::Spec {
        revision,
        scenario: cli.scenario,
        chip: cli.chip,
        layout_seed: cli.layout_seed,
        boards: cli.boards.arguments(),
    };
    // Whatever stops the step before it judged the revision stops the
    // search too: an error exits as an unjudged step.
    let (commit, step) = match bisect::step(&ctx.root, owner, &spec) {
        Ok(found) => found,
        Err(error) => {
            let step = bisect::Step::Unjudged {
                why: error.to_string(),
            };
            println!("bisect {}", step.line(&spec.revision));
            return Ok(std::process::ExitCode::from(step.code()));
        }
    };
    println!("bisect {}", step.line(&commit));
    Ok(std::process::ExitCode::from(step.code()))
}
