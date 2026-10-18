//! The job a launch runs as: the arbiter's ticket model
//! ([`oer_stand_arbiter::jobs`]). Every run is a job, so `cargo stand queue`
//! shows it while it builds, and every stand request its runner makes
//! carries the job's id.

use std::{ffi::OsString, path::Path};

use oer_hil_run_bundle::RunId;
use oer_hil_run_bundle::RunStore;
use oer_hil_schema::run::Outcome;
use oer_stand_arbiter::jobs::{After, Job, JobOutcome, JobState};

use crate::Result;

/// `--after` names a job a launch waits for; `--after-any` starts it
/// whatever that job's outcome.
pub const AFTER: &str = "--after";
pub const AFTER_ANY: &str = "--after-any";

/// The outcome of a job whose runs ended with these suite outcomes: the
/// worst of them; a job without a run created none.
pub fn outcome_of_runs(outcomes: &[Option<Outcome>]) -> JobOutcome {
    outcomes
        .iter()
        .map(|outcome| match outcome {
            Some(Outcome::Passed) => JobOutcome::Passed,
            Some(Outcome::Failed) => JobOutcome::Failed,
            Some(Outcome::Blocked | Outcome::Skipped) => JobOutcome::Blocked,
            Some(Outcome::Broken) => JobOutcome::Broken,
            Some(Outcome::Interrupted | Outcome::BoardQuarantined) | None => {
                JobOutcome::Interrupted
            }
        })
        .max_by_key(|outcome| outcome.severity())
        .unwrap_or(JobOutcome::NoRun)
}

/// The job a launch runs as: recorded and started when it begins, finished
/// with its runs' outcomes, and finished without a run when dropped first.
pub struct Running {
    jobs: oer_stand_arbiter::jobs::Jobs,
    id: String,
    finished: bool,
}

impl Running {
    /// The job of this process: the one [`oer_stand_arbiter::jobs::JOB_KEY`]
    /// names (a detached job already has its record) or a new one for
    /// `args`; waits for `after` (a job id from `--after`) and marks the job
    /// started.
    pub fn begin(
        checkout: &Path,
        owner: &str,
        args: &[OsString],
        after: Option<&After>,
    ) -> Result<Self> {
        let jobs = oer_stand_arbiter::Arbiter::open()?.jobs();
        let id = match oer_stand_arbiter::jobs::current()? {
            Some(id) => id,
            None => record(&jobs, checkout, owner, args, after)?,
        };
        Self::start(jobs, id, after)
    }

    /// A new job of its own for `args`, started at once: a launch on behalf
    /// of another job, such as an A/B arm's run.
    pub fn new(checkout: &Path, owner: &str, args: &[OsString]) -> Result<Self> {
        let jobs = oer_stand_arbiter::Arbiter::open()?.jobs();
        let id = record(&jobs, checkout, owner, args, None)?;
        Self::start(jobs, id, None)
    }

    fn start(
        jobs: oer_stand_arbiter::jobs::Jobs,
        id: String,
        after: Option<&After>,
    ) -> Result<Self> {
        let running = Self {
            jobs,
            id,
            finished: false,
        };
        if let Some(After { job, any }) = after {
            eprintln!("hil: waiting for job {job}");
            let outcome = running.jobs.wait(job)?;
            if !any && !outcome.lets_dependents_start() {
                return Err(format!(
                    "job {job} ended {outcome} without a judged run; not starting (pass \
                     {AFTER_ANY} to start after it whatever its outcome)"
                )
                .into());
            }
            eprintln!("hil: job {job} ended {outcome}; starting");
        }
        let mut job = running.jobs.read(&running.id)?;
        job.state = JobState::Started;
        running.jobs.write(&job)?;
        Ok(running)
    }

    /// The job's id, which the runner a launch starts carries in
    /// [`oer_stand_arbiter::jobs::JOB_KEY`], so its requests are the job's
    /// tickets.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Finish the job with its runs' outcomes.
    pub fn finish(&mut self, runs: &[RunId], outcomes: &[Option<Outcome>]) -> Result<()> {
        self.finished = true;
        let mut job = self.jobs.read(&self.id)?;
        job.state = JobState::Finished {
            outcome: outcome_of_runs(outcomes),
            runs: runs.iter().map(ToString::to_string).collect(),
        };
        self.jobs.write(&job)
    }

    /// Finish the job with `runs` of `store` and the outcomes their suites
    /// record.
    pub fn finish_with(&mut self, store: &RunStore, runs: &[RunId]) -> Result<()> {
        self.finish(runs, &outcomes(store, runs))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.finished
            && let Err(error) = self.finish(&[], &[])
        {
            eprintln!("hil: the job's record was not finished: {error}");
        }
    }
}

/// Record a pending job for `args`, run by this process.
fn record(
    jobs: &oer_stand_arbiter::jobs::Jobs,
    checkout: &Path,
    owner: &str,
    args: &[OsString],
    after: Option<&After>,
) -> Result<String> {
    let mut job = Job::new(owner, command_of(args), checkout.to_owned(), after);
    job.run_by(std::process::id());
    jobs.write(&job)?;
    Ok(job.id)
}

/// The suite outcome of each of `runs` in `store`; `None` for a run without
/// a readable suite.
pub fn outcomes(store: &RunStore, runs: &[RunId]) -> Vec<Option<Outcome>> {
    runs.iter()
        .map(|run| {
            store
                .open(run.as_str())
                .ok()
                .and_then(|bundle| bundle.suite().ok().flatten())
                .map(|suite| suite.outcome)
        })
        .collect()
}

/// The `cargo hil` arguments of a job, as its record shows them.
pub fn command_of(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests;
