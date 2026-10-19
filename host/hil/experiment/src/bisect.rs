//! Bisection: find the first commit at which a scenario stops passing.
//!
//! Every tested revision is checked out, detached, in the bisection's own
//! worktree below `target/hil/bisect/<id>/`, and runs its scenario with the
//! runner that speaks its HIL wire, as [`crate::revision`] launches it: this
//! checkout's with the revision's firmware for the same wire, otherwise the
//! revision's own inside the bisection's whole-stand lease.
//!
//! A revision whose image does not build or does not link, or whose own
//! runner cannot run, is broken: it is neither good nor bad, and the search
//! steps around it. A step that could not judge the revision for another
//! reason (a blocked or broken run, an interrupted one, a quarantined board)
//! ends the bisection, since its later steps would meet the same stand. One
//! typed report, `report.json`, records every step and the result.

use std::{
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use oer_hil_analysis::Run;
use oer_hil_schema::run::{FailureKind, Outcome};
use oer_os_process::git::Worktree;
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    revision::{Ran, Revision, Runner},
};

const REPORT_SCHEMA: u16 = 2;

/// What one tested revision showed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub enum Verdict {
    Good { run: String },
    Bad { run: String },
    Broken { why: Broken, run: Option<String> },
}

/// Why a revision could not be judged good or bad.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Broken {
    /// The image failed to compile.
    DoesNotBuild,
    /// The image compiled but did not link.
    DoesNotLink,
    /// The revision's own runner could not build or start a run.
    RunnerUnavailable,
}

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DoesNotBuild => "does not build",
            Self::DoesNotLink => "does not link",
            Self::RunnerUnavailable => "its runner cannot run",
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Step {
    pub commit: String,
    pub subject: String,
    #[serde(flatten)]
    pub runner: Runner,
    #[serde(flatten)]
    pub verdict: Verdict,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum Conclusion {
    /// The first commit at which the scenario does not pass.
    FirstBad { commit: String },
    /// Broken revisions hide which of these is the first bad one.
    Ambiguous { candidates: Vec<String> },
    /// A step could not judge its revision; the search stopped.
    Aborted { reason: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Report {
    pub schema: u16,
    pub scenario: String,
    pub layout_seed: Option<NonZeroU32>,
    pub good: String,
    pub bad: String,
    /// The commits between GOOD (excluded) and BAD (included), oldest first.
    pub commits: Vec<String>,
    pub steps: Vec<Step>,
    pub conclusion: Option<Conclusion>,
}

/// What a probe of one index showed, for the search.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Probe {
    Good,
    Bad,
    Broken,
}

/// Where the search ended, as indices into the commits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Found {
    FirstBad(usize),
    Ambiguous(std::ops::RangeInclusive<usize>),
}

/// Binary search over `len` commits whose last is known bad and whose
/// predecessor (outside the list) is known good. A broken index is skipped:
/// the next probe is the untested index closest to the middle.
pub fn search(len: usize, mut probe: impl FnMut(usize) -> Result<Probe>) -> Result<Found> {
    if len == 0 {
        return Err("the bisection has no commits".into());
    }
    let mut good: Option<usize> = None;
    let mut bad = len - 1;
    let mut broken = std::collections::BTreeSet::new();
    loop {
        let low = good.map_or(0, |index| index + 1);
        let middle = (low + bad) / 2;
        let next = (low..bad)
            .filter(|index| !broken.contains(index))
            .min_by_key(|index| index.abs_diff(middle));
        let Some(index) = next else {
            return Ok(if (low..bad).any(|index| broken.contains(&index)) {
                Found::Ambiguous(low..=bad)
            } else {
                Found::FirstBad(bad)
            });
        };
        match probe(index)? {
            Probe::Good => good = Some(index),
            Probe::Bad => bad = index,
            Probe::Broken => {
                broken.insert(index);
            }
        }
    }
}

/// Whether an image build log shows a link failure rather than a compile
/// failure.
fn build_failure(log: &str) -> Broken {
    const LINK_ERRORS: [&str; 5] = [
        "error: linking with",
        "rust-lld: error:",
        "ld.lld: error:",
        "undefined symbol:",
        "relocation R_",
    ];
    if LINK_ERRORS.iter().any(|marker| log.contains(marker)) {
        Broken::DoesNotLink
    } else {
        Broken::DoesNotBuild
    }
}

/// The verdict of a finished run of the bisected scenario, or why it judged
/// nothing: the scenario's own outcome decides, and a scenario whose image
/// did not build is broken, told from a link failure by its build log.
fn judge(run: &Run) -> std::result::Result<Verdict, String> {
    let id = run.id().to_owned();
    let Some(scenario) = run.scenarios().first() else {
        return Err(format!(
            "run {id} ended {} without judging the revision",
            run.status()
        ));
    };
    match scenario.outcome {
        Outcome::Passed => return Ok(Verdict::Good { run: id }),
        Outcome::Failed => return Ok(Verdict::Bad { run: id }),
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
            Ok(Verdict::Broken {
                why: build_failure(&format!("{}\n{log}", failure.message)),
                run: Some(id),
            })
        }
        None => Err(format!(
            "run {id} ended {} without judging the revision",
            scenario.outcome
        )),
    }
}

/// What one bisection searches.
#[derive(Clone, Debug)]
pub struct Spec {
    /// A commit at which the scenario passes.
    pub good: String,
    /// A later commit, descending from `good`, at which it does not.
    pub bad: String,
    pub scenario: String,
    /// Build every tested image with this code layout seed.
    pub layout_seed: Option<NonZeroU32>,
    /// The runner arguments naming the boards every run uses; a revision's
    /// own runner from before the stand file does not know them.
    pub boards: Vec<String>,
}

/// A finished bisection: its report, where it is written, and the exit
/// status its conclusion gives: 0 first bad commit found, 1 ambiguous, 2
/// aborted.
pub struct Finished {
    pub report: Report,
    pub path: PathBuf,
    pub code: u8,
}

/// Bisect `spec` in the checkout at `checkout` for `owner`, reporting each
/// step as it ends.
pub fn run(
    checkout: &Path,
    owner: &str,
    spec: &Spec,
    mut progress: impl FnMut(&str),
) -> Result<Finished> {
    let good = oer_os_process::git::commit(checkout, &spec.good)?;
    let bad = oer_os_process::git::commit(checkout, &spec.bad)?;
    if oer_os_process::git::run(checkout, ["merge-base", "--is-ancestor", &good, &bad]).is_err() {
        return Err(format!(
            "--good {} is not an ancestor of --bad {}",
            spec.good, spec.bad
        )
        .into());
    }
    let commits = oer_os_process::git::lines(
        checkout,
        [
            "rev-list",
            "--ancestry-path",
            "--reverse",
            &format!("{good}..{bad}"),
        ],
    )?;
    let id = oer_os_durable::unix_millis().to_string();
    let directory = checkout.join("target/hil/bisect").join(&id);
    fs::create_dir_all(&directory)?;
    let mut report = Report {
        schema: REPORT_SCHEMA,
        scenario: spec.scenario.clone(),
        layout_seed: spec.layout_seed,
        good,
        bad,
        commits: commits.clone(),
        steps: Vec::new(),
        conclusion: None,
    };
    let path = directory.join("report.json");
    write_report(&path, &report)?;
    progress(&format!(
        "bisecting {} over {} commits; report {}",
        spec.scenario,
        commits.len(),
        path.display()
    ));
    let bisection = Bisection {
        spec,
        revision: Revision::new(checkout, owner, &directory)?,
    };
    let mut worktree = None;
    let found = search(commits.len(), |index| {
        let step = bisection.step(&mut worktree, &commits[index])?;
        progress(&format!("bisect {}", step_line(&step)));
        let probe = match step.verdict {
            Verdict::Good { .. } => Probe::Good,
            Verdict::Bad { .. } => Probe::Bad,
            Verdict::Broken { .. } => Probe::Broken,
        };
        report.steps.push(step);
        write_report(&path, &report)?;
        Ok(probe)
    });
    if let Some(worktree) = worktree
        && let Err(error) = worktree.remove()
    {
        progress(&format!(
            "the bisection's worktree was not removed: {error}"
        ));
    }
    let (conclusion, code) = match found {
        Ok(Found::FirstBad(index)) => (
            Conclusion::FirstBad {
                commit: commits[index].clone(),
            },
            0,
        ),
        Ok(Found::Ambiguous(range)) => (
            Conclusion::Ambiguous {
                candidates: commits[range].to_vec(),
            },
            1,
        ),
        Err(error) => (
            Conclusion::Aborted {
                reason: error.to_string(),
            },
            2,
        ),
    };
    report.conclusion = Some(conclusion);
    write_report(&path, &report)?;
    Ok(Finished { report, path, code })
}

struct Bisection<'a> {
    spec: &'a Spec,
    revision: Revision<'a>,
}

impl Bisection<'_> {
    fn step(&self, worktree: &mut Option<Worktree>, commit: &str) -> Result<Step> {
        self.revision.checkout(worktree, commit)?;
        let subject = self.revision.subject()?;
        let (runner, ran) = self.revision.launch(
            std::slice::from_ref(&self.spec.scenario),
            &crate::revision::seed_and_boards(self.spec.layout_seed, &self.spec.boards),
            &format!("bisect {} with the revision's runner", self.spec.scenario),
        )?;
        let verdict = match ran {
            Ran::Run { directory, .. } => judge_run(&directory)?,
            Ran::RunnerUnavailable => Verdict::Broken {
                why: Broken::RunnerUnavailable,
                run: None,
            },
        };
        Ok(Step {
            commit: commit.to_owned(),
            subject,
            runner,
            verdict,
        })
    }
}

/// The verdict of the run at `directory`; why it does not read, with its
/// path, when it does not.
fn judge_run(directory: &Path) -> Result<Verdict> {
    let unreadable = |error: &dyn std::fmt::Display| {
        format!("the run at {} does not read: {error}", directory.display())
    };
    let bundle = oer_hil_run_bundle_format::RunBundle::open(directory)
        .map_err(|error| unreadable(&error))?
        .ok_or_else(|| unreadable(&"it has no manifest"))?;
    let run = Run::of(bundle).map_err(|error| unreadable(&error))?;
    Ok(judge(&run)?)
}

fn step_line(step: &Step) -> String {
    let short = &step.commit[..12.min(step.commit.len())];
    let runner = match step.runner {
        Runner::Current => String::new(),
        Runner::Revision => String::from(" [own runner]"),
    };
    let verdict = match &step.verdict {
        Verdict::Good { run } => format!("good (run {run})"),
        Verdict::Bad { run } => format!("bad (run {run})"),
        Verdict::Broken { why, run } => match run {
            Some(run) => format!("broken: {why} (run {run})"),
            None => format!("broken: {why}"),
        },
    };
    format!("{short} {verdict}{runner}  {}", step.subject)
}

pub fn summary(report: &Report) -> String {
    let mut text = format!("bisect {}:\n", report.scenario);
    for step in &report.steps {
        text.push_str(&format!("  {}\n", step_line(step)));
    }
    match &report.conclusion {
        Some(Conclusion::FirstBad { commit }) => {
            text.push_str(&format!("first bad commit: {commit}\n"));
        }
        Some(Conclusion::Ambiguous { candidates }) => {
            text.push_str("broken revisions hide the first bad commit among:\n");
            for commit in candidates {
                text.push_str(&format!("  {commit}\n"));
            }
        }
        Some(Conclusion::Aborted { reason }) => {
            text.push_str(&format!("bisection stopped: {reason}\n"));
        }
        None => text.push_str("bisection unfinished\n"),
    }
    text
}

fn write_report(path: &Path, report: &Report) -> Result<()> {
    oer_os_durable::atomic_json(path, report)
}

#[cfg(test)]
pub(crate) mod tests;
