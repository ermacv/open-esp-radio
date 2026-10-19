//! `cargo hil ab`, `cargo hil bisect` and `cargo hil revision`: their
//! arguments, and calls into `oer-hil-experiment`.

use std::{ffi::OsString, num::NonZeroU32};

use crate::experiment::{ab, bisect, launch::Runner, revision};
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
pub(crate) struct BisectCli {
    /// A commit at which the scenario passes.
    #[arg(long)]
    good: String,
    /// A later commit, descending from GOOD, at which it does not.
    #[arg(long)]
    bad: String,
    #[arg(long)]
    scenario: String,
    /// Build every tested image with this code layout seed.
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
    let finished = bisect::run(
        &ctx.root,
        owner,
        &bisect::Spec {
            good: cli.good,
            bad: cli.bad,
            scenario: cli.scenario,
            layout_seed: cli.layout_seed,
            boards: cli.boards.arguments(),
        },
        |line| eprintln!("hil: {line}"),
    )?;
    print!("{}", bisect::summary(&finished.report));
    Ok(std::process::ExitCode::from(finished.code))
}

#[derive(clap::Parser)]
#[command(name = "cargo hil revision")]
pub(crate) struct RevisionCli {
    /// The revision: a commit, tag or branch.
    #[arg(long)]
    rev: String,
    /// A scenario to run, in run order; repeat for each. The revision's own
    /// runner takes the revision's scenario ids, this checkout's runner (the
    /// same HIL wire) this checkout's.
    #[arg(long = "scenario", required = true)]
    scenarios: Vec<String>,
    /// Build every image with this code layout seed.
    #[arg(long, value_name = "SEED")]
    layout_seed: Option<NonZeroU32>,
    #[command(flatten)]
    boards: crate::jobs::BoardChoiceArgs,
}

/// Run the scenario set `args` names on its revision; exits 0 when every
/// scenario passed, 1 when one failed, 2 when the revision was not judged.
pub(crate) fn revision(
    ctx: &Checkout,
    owner: &str,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let cli = RevisionCli::try_parse_from(
        std::iter::once(OsString::from("cargo hil revision")).chain(args.iter().cloned()),
    )?;
    let finished = revision::run(
        &ctx.root,
        owner,
        &revision::Spec {
            revision: cli.rev,
            scenarios: cli.scenarios,
            layout_seed: cli.layout_seed,
            boards: cli.boards.arguments(),
        },
        |line| eprintln!("hil: {line}"),
    )?;
    print!("{}", revision::summary(&finished.report));
    println!("report: {}", finished.path.display());
    Ok(std::process::ExitCode::from(finished.report.code()))
}
