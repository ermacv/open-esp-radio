//! One step of a manual bisection: run a scenario on one revision and say
//! whether it passes, in the exit code `git bisect run` reads.
//!
//! The revision is checked out, detached, in a worktree of its own below
//! `target/hil/bisect/`, and this checkout's runner builds its firmware
//! from there (`cargo hil run --source`), as an A/B arm's; the run is an
//! ordinary run of the store. The search is git's: with
//! `git bisect start --no-checkout`, the working copy, and with it this
//! tool, stays at the current revision while git moves `BISECT_HEAD`.
//!
//! A revision whose image does not build or does not link, or that speaks
//! another HIL wire than this checkout, cannot be judged by this runner: git
//! skips it. A run that judged nothing for another reason (a blocked or
//! broken run, an interrupted one, a quarantined board) stops the search,
//! since its later steps would meet the same stand.

use std::{
    ffi::OsString,
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use oer_hil_analysis::Run;
use oer_hil_model::schema::run::{FailureKind, Outcome};
use oer_hil_runs::RunStore;
use oer_os_process::git::Worktree;

use crate::experiment::{
    Result,
    job::Running,
    launch::{self, Launch, launch_run},
};

/// The file naming a revision's HIL wire: revisions with equal locks speak
/// the same protocol.
const MESSAGES_LOCK: &str = "protocols/hil/protocol/messages.lock";

/// The HIL wire of the tree at `root`, or `None` for a revision older than
/// the keyed protocol, whose wire no lock names.
pub(crate) fn messages_lock(root: &Path) -> Option<String> {
    fs::read_to_string(root.join(MESSAGES_LOCK)).ok()
}

/// What one step showed, and the exit code `git bisect run` reads from it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Step {
    /// The scenario passed in `run`.
    Pass { run: String },
    /// The scenario failed in `run`.
    Fail { run: String },
    /// This runner cannot judge the revision; git skips it.
    Skip { why: String },
    /// The run judged nothing; git stops the search.
    Unjudged { why: String },
}

impl Step {
    /// `git bisect run`'s convention: 0 good, 1 bad, 125 skip, and above
    /// 127 an abort.
    pub fn code(&self) -> u8 {
        match self {
            Self::Pass { .. } => 0,
            Self::Fail { .. } => 1,
            Self::Skip { .. } => 125,
            Self::Unjudged { .. } => 128,
        }
    }

    pub fn line(&self, commit: &str) -> String {
        let short = &commit[..12.min(commit.len())];
        match self {
            Self::Pass { run } => format!("{short} pass (run {run})"),
            Self::Fail { run } => format!("{short} fail (run {run})"),
            Self::Skip { why } => format!("{short} skip: {why}"),
            Self::Unjudged { why } => format!("{short} not judged: {why}"),
        }
    }
}

/// Whether an image build log shows a link failure rather than a compile
/// failure.
fn build_failure(log: &str) -> &'static str {
    const LINK_ERRORS: [&str; 5] = [
        "error: linking with",
        "rust-lld: error:",
        "ld.lld: error:",
        "undefined symbol:",
        "relocation R_",
    ];
    if LINK_ERRORS.iter().any(|marker| log.contains(marker)) {
        "the image does not link"
    } else {
        "the image does not build"
    }
}

/// The step a finished run of the scenario shows: the scenario's own
/// outcome decides, and a scenario whose image did not build is skipped,
/// told from a link failure by its build log.
fn judge(run: &Run) -> Step {
    let id = run.id().to_owned();
    let Some(scenario) = run.scenarios().first() else {
        return Step::Unjudged {
            why: format!("run {id} ended {} without the scenario", run.status()),
        };
    };
    match scenario.outcome {
        Outcome::Passed => return Step::Pass { run: id },
        Outcome::Failed => return Step::Fail { run: id },
        _ => {}
    }
    let build = scenario
        .failure
        .as_ref()
        .or_else(|| {
            scenario
                .repetitions
                .iter()
                .find_map(|repetition| repetition.failure.as_ref())
        })
        .filter(|failure| failure.kind == FailureKind::ImageBuild);
    match build {
        Some(failure) => {
            let log = fs::read_to_string(
                run.directory()
                    .join("firmware")
                    .join(scenario.image.id())
                    .join("build.log"),
            )
            .unwrap_or_default();
            Step::Skip {
                why: format!(
                    "{} (run {id})",
                    build_failure(&format!("{}\n{log}", failure.message))
                ),
            }
        }
        None => Step::Unjudged {
            why: format!("run {id} ended {}", scenario.outcome),
        },
    }
}

/// The step the run at `directory` shows; why it does not read, with its
/// path, when it does not.
fn judge_run(directory: &Path) -> Step {
    let unreadable = |error: &dyn std::fmt::Display| Step::Unjudged {
        why: format!("the run at {} does not read: {error}", directory.display()),
    };
    let bundle = match oer_hil_runs::format::RunBundle::open(directory) {
        Ok(Some(bundle)) => bundle,
        Ok(None) => return unreadable(&"it has no manifest"),
        Err(error) => return unreadable(&error),
    };
    match Run::of(bundle) {
        Ok(run) => judge(&run),
        Err(error) => unreadable(&error),
    }
}

/// What one step runs.
#[derive(Clone, Debug)]
pub struct Spec {
    /// The revision: a commit, tag, branch or `BISECT_HEAD`.
    pub revision: String,
    pub scenario: String,
    /// The chip, for a scenario that runs on more than one.
    pub chip: Option<String>,
    /// Build the image with this code layout seed.
    pub layout_seed: Option<NonZeroU32>,
    /// The runner arguments naming the boards the run uses.
    pub boards: Vec<String>,
}

/// Run one step of `spec` in the checkout at `checkout` for `owner`; returns
/// the revision's commit and what the step showed.
pub fn step(checkout: &Path, owner: &str, spec: &Spec) -> Result<(String, Step)> {
    let commit = oer_os_process::git::commit(checkout, &spec.revision)?;
    let current = messages_lock(checkout).ok_or("this checkout names no HIL wire")?;
    let directory = checkout.join("target/hil/bisect").join(&commit);
    let worktree = Worktree::detached(checkout, &directory, &commit)?;
    // A warm start: the step recompiles only what the revision changes.
    oer_image_build::seed_build_cache(checkout, worktree.path())?;
    let step = if messages_lock(worktree.path()).as_deref() != Some(&current) {
        Ok(Step::Skip {
            why: String::from(
                "it speaks another HIL wire than this checkout (its \
                 protocols/hil/protocol/messages.lock differs)",
            ),
        })
    } else {
        run_scenario(checkout, owner, worktree.path(), spec)
    };
    if let Err(error) = worktree.remove() {
        eprintln!("hil: the step's worktree was not removed: {error}");
    }
    Ok((commit, step?))
}

/// This checkout's runner on the scenario, building the revision's firmware
/// from `worktree`, as a job of its own.
fn run_scenario(checkout: &Path, owner: &str, worktree: &Path, spec: &Spec) -> Result<Step> {
    let mut arguments = vec![
        String::from("run"),
        spec.scenario.clone(),
        String::from("--source"),
        worktree.to_string_lossy().into_owned(),
    ];
    if let Some(chip) = &spec.chip {
        arguments.extend([String::from("--chip"), chip.clone()]);
    }
    if let Some(seed) = spec.layout_seed {
        arguments.extend([String::from("--layout-seed"), seed.to_string()]);
    }
    arguments.extend(spec.boards.iter().cloned());
    let arguments: Vec<OsString> = arguments.into_iter().map(OsString::from).collect();
    let store = RunStore::shared()?;
    let runner = launch::Runner::prepare(checkout)?;
    let mut job = Running::new(checkout, owner, &arguments)?;
    let launched = launch_run(
        &Launch::new(checkout, &runner)
            .args(arguments)
            .env(oer_lab_arbiter::owner::OWNER_ENV, owner)
            .context(
                oer_os_process::Context::current()?
                    .clone()
                    .with(oer_lab_arbiter::owner::OWNER_KEY, owner)
                    .with(oer_lab_arbiter::jobs::JOB_KEY, job.id()),
            ),
    )?;
    job.finish_with(&store, &launched.runs)?;
    let directory: PathBuf = store.runs().join(launched.run()?.to_string());
    Ok(judge_run(&directory))
}

#[cfg(test)]
pub(crate) mod tests;
