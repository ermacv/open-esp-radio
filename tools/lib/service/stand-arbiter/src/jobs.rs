//! Deferred `cargo hil` runs as the arbiter's job tickets.
//!
//! A job is a `cargo hil run …` (or an experiment) recorded in the arbiter
//! directory's `jobs/<id>.json`: enqueued detached, or started in the
//! foreground. Its process, and the runner it starts, carry the job's id in
//! [`JOB_KEY`]; every stand request they make is a ticket of that job in the
//! one queue ([`crate::Status`] names each holder's and waiter's job), so a
//! job's phase is read from the queue rather than guessed from process ids.
//! The job's process moves its record through its states (waiting for the
//! job it runs after, started, finished with the outcome of its runs). A job
//! whose process is gone without finishing is abandoned.

use std::{fs, path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

/// Names the job a `cargo hil` process, and the runner it starts, run as;
/// the arbiter tags their requests with it.
pub const JOB_KEY: &str = "stand.job";
/// How long a job's record is kept.
const RECORD_RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);
/// How often a waiting process reads the job it waits for.
const POLL: Duration = Duration::from_secs(2);

/// The job this process runs as, from [`JOB_KEY`].
pub fn current() -> crate::Result<Option<String>> {
    Ok(oer_process::Context::current()?
        .get(JOB_KEY)
        .map(str::to_owned))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Job {
    pub id: String,
    pub owner: String,
    /// The `cargo hil` arguments the job runs.
    pub command: Vec<String>,
    pub checkout: PathBuf,
    /// The job this one starts after.
    pub after: Option<String>,
    /// Whether it starts after that job whatever its outcome.
    #[serde(default)]
    pub after_any: bool,
    pub enqueued_unix_millis: u64,
    /// The job's process and its start time, which survive PID reuse.
    pub pid: Option<u32>,
    pub pid_started_unix_millis: Option<u64>,
    /// Where a detached job's output goes; a run in the foreground has none.
    pub log: Option<PathBuf>,
    /// The files the job was fixed with when it was enqueued; a sweep keeps
    /// them while the job has not ended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed: Vec<PathBuf>,
    #[serde(flatten)]
    pub state: JobState,
}

impl Job {
    /// A new pending job of `owner` running `command` in `checkout`, after
    /// `after`; [`Self::run_by`] names its process once it has one.
    pub fn new(
        owner: &str,
        command: Vec<String>,
        checkout: PathBuf,
        after: Option<&After>,
    ) -> Self {
        let enqueued_unix_millis = oer_durable::unix_millis();
        Self {
            id: format!("{enqueued_unix_millis}-{:x}", std::process::id()),
            owner: owner.to_owned(),
            command,
            checkout,
            after: after.map(|after| after.job.clone()),
            after_any: after.is_some_and(|after| after.any),
            enqueued_unix_millis,
            pid: None,
            pid_started_unix_millis: None,
            log: None,
            fixed: Vec::new(),
            state: JobState::Pending,
        }
    }

    /// Run by the process `pid`.
    pub fn run_by(&mut self, pid: u32) {
        self.pid = Some(pid);
        self.pid_started_unix_millis = oer_process::proc::started_unix_millis(pid);
    }

    /// Whether the job's process still runs; a job not yet given one counts
    /// as running for a minute after it was enqueued.
    fn alive(&self) -> bool {
        match (self.pid, self.pid_started_unix_millis) {
            (Some(pid), Some(started)) => {
                oer_process::proc::started_unix_millis(pid) == Some(started)
            }
            _ => oer_durable::unix_millis().saturating_sub(self.enqueued_unix_millis) < 60_000,
        }
    }
}

/// Where an unfinished job is, from its ticket in the arbiter's queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// Waiting for the job it runs after.
    WaitingForJob,
    /// Building its images; it has not asked for the stand yet.
    Building,
    /// Queued in the arbiter.
    WaitingForStand,
    /// Holding its lease.
    Holding,
    Finished,
}

impl Phase {
    pub const fn describe(self) -> &'static str {
        match self {
            Self::WaitingForJob => "waits for its job",
            Self::Building => "building images",
            Self::WaitingForStand => "waiting for the stand",
            Self::Holding => "holding the stand",
            Self::Finished => "finished",
        }
    }
}

/// The phase of `job` given the arbiter's holders and queue: a lease held
/// or a request queued with the job's id is the job's.
pub fn phase(job: &Job, status: &crate::Status) -> Phase {
    phase_of(
        job,
        status.holders.iter().map(|holder| holder.job.as_deref()),
        status.queue.iter().map(|queued| queued.job.as_deref()),
    )
}

/// [`phase`] from the job ids of the holders and of the queued tickets.
fn phase_of<'a>(
    job: &Job,
    mut holders: impl Iterator<Item = Option<&'a str>>,
    mut queued: impl Iterator<Item = Option<&'a str>>,
) -> Phase {
    let ours = |ticket: Option<&str>| ticket == Some(job.id.as_str());
    match job.state {
        JobState::Finished { .. } => Phase::Finished,
        JobState::Pending if job.after.is_some() => Phase::WaitingForJob,
        _ if holders.any(ours) => Phase::Holding,
        _ if queued.any(ours) => Phase::WaitingForStand,
        _ => Phase::Building,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum JobState {
    /// Waiting for its `after` job, or not started yet.
    Pending,
    /// Building, waiting for the stand, or running.
    Started,
    Finished {
        outcome: JobOutcome,
        runs: Vec<String>,
    },
}

/// A job's outcome, and `wait`'s exit status for it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobOutcome {
    Passed,
    Failed,
    Interrupted,
    Blocked,
    Broken,
    /// The command ended without creating a run.
    NoRun,
    /// The job's process is gone without finishing.
    Abandoned,
}

impl JobOutcome {
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Passed => 0,
            Self::Failed => 1,
            Self::Interrupted => 2,
            Self::Blocked => 3,
            Self::Broken => 4,
            Self::NoRun => 5,
            Self::Abandoned => 6,
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Blocked => "blocked",
            Self::Broken => "broken",
            Self::NoRun => "no-run",
            Self::Abandoned => "abandoned",
        }
    }

    /// How bad the outcome is, for the worst of several runs.
    pub const fn severity(self) -> u8 {
        match self {
            Self::Passed => 0,
            Self::Failed => 1,
            Self::Blocked => 2,
            Self::Broken => 3,
            Self::Interrupted => 4,
            Self::NoRun => 5,
            Self::Abandoned => 6,
        }
    }

    /// Whether a job with `--after` (not `--after-any`) starts after one
    /// that ended so: after a judged run, passed or failed, not after one
    /// that created none, was blocked, broken, interrupted or abandoned.
    pub const fn lets_dependents_start(self) -> bool {
        matches!(self, Self::Passed | Self::Failed)
    }
}

impl std::fmt::Display for JobOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

/// The job a run starts after, and whether it starts whatever that job's
/// outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct After {
    pub job: String,
    /// `--after-any`: start even when the job created no judged run.
    pub any: bool,
}

/// The jobs of the stand, in its arbiter directory.
pub struct Jobs {
    directory: PathBuf,
}

impl crate::Arbiter {
    /// The stand's jobs.
    pub fn jobs(&self) -> Jobs {
        Jobs::at(self.directory().join("jobs"))
    }
}

impl Jobs {
    pub fn at(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn path(&self, id: &str) -> crate::Result<PathBuf> {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(format!("`{id}` is not a job id").into());
        }
        Ok(self.directory.join(format!("{id}.json")))
    }

    pub fn read(&self, id: &str) -> crate::Result<Job> {
        let path = self.path(id)?;
        let bytes = fs::read(&path).map_err(|_| format!("no job `{id}`"))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn write(&self, job: &Job) -> crate::Result<()> {
        oer_durable::atomic_json(&self.path(&job.id)?, job)
    }

    /// Every job that has not finished, oldest first. A job whose process is
    /// gone is recorded as abandoned and left out.
    pub fn unfinished(&self) -> Vec<Job> {
        let mut jobs = self
            .all()
            .filter(|job| !matches!(job.state, JobState::Finished { .. }))
            .filter(|job| {
                if job.alive() {
                    return true;
                }
                let mut abandoned = job.clone();
                abandoned.state = JobState::Finished {
                    outcome: JobOutcome::Abandoned,
                    runs: Vec::new(),
                };
                let _ = self.write(&abandoned);
                false
            })
            .collect::<Vec<_>>();
        jobs.sort_by(|a, b| (a.enqueued_unix_millis, &a.id).cmp(&(b.enqueued_unix_millis, &b.id)));
        jobs
    }

    fn all(&self) -> impl Iterator<Item = Job> + '_ {
        fs::read_dir(&self.directory)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                self.read(name.strip_suffix(".json")?).ok()
            })
    }

    /// Jobs that ended without a judged run within the last `window`,
    /// newest first, at most `limit`: what `queue` shows so a lost chain is
    /// seen. Records older than a week are removed.
    pub fn recently_ended_unjudged(&self, window: Duration, limit: usize) -> Vec<Job> {
        let now = oer_durable::unix_millis();
        let mut jobs = self
            .all()
            .filter(|job| {
                if now.saturating_sub(job.enqueued_unix_millis)
                    > RECORD_RETENTION.as_millis() as u64
                {
                    if let Ok(path) = self.path(&job.id) {
                        let _ = fs::remove_file(path);
                    }
                    return false;
                }
                true
            })
            .filter(|job| {
                matches!(
                    job.state,
                    JobState::Finished { outcome, .. } if !outcome.lets_dependents_start()
                ) && now.saturating_sub(job.enqueued_unix_millis) <= window.as_millis() as u64
            })
            .collect::<Vec<_>>();
        jobs.sort_by(|a, b| (b.enqueued_unix_millis, &b.id).cmp(&(a.enqueued_unix_millis, &a.id)));
        jobs.truncate(limit);
        jobs
    }

    /// The job's outcome once it has one: finished, or abandoned by its
    /// process.
    pub fn settled(&self, id: &str) -> crate::Result<Option<JobOutcome>> {
        let job = self.read(id)?;
        Ok(match job.state {
            JobState::Finished { outcome, .. } => Some(outcome),
            _ if !job.alive() => Some(JobOutcome::Abandoned),
            _ => None,
        })
    }

    /// Block until job `id` settles.
    pub fn wait(&self, id: &str) -> crate::Result<JobOutcome> {
        loop {
            if let Some(outcome) = self.settled(id)? {
                return Ok(outcome);
            }
            oer_process::check_cancelled()?;
            std::thread::sleep(POLL);
        }
    }
}

/// The last line of a job's log, its reason when it ended without a run.
fn last_log_line(job: &Job) -> Option<String> {
    let text = fs::read_to_string(job.log.as_ref()?).ok()?;
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Jobs that ended without a judged run, for `cargo stand queue`.
pub fn describe_ended(jobs: &[Job]) -> String {
    let mut text = String::new();
    for job in jobs {
        let JobState::Finished { outcome, .. } = &job.state else {
            continue;
        };
        text.push_str(&format!(
            "ended:   {} {} `cargo hil {}` {outcome}{}\n",
            job.id,
            job.owner,
            job.command.join(" "),
            last_log_line(job)
                .map(|line| format!(": {line}"))
                .unwrap_or_default()
        ));
    }
    text
}

/// A job with its phase, as `queue --json` and the dashboard show it.
#[derive(Serialize)]
pub struct JobView<'a> {
    #[serde(flatten)]
    pub job: &'a Job,
    pub phase: Phase,
}

pub fn views<'a>(jobs: &'a [Job], status: &crate::Status) -> Vec<JobView<'a>> {
    jobs.iter()
        .map(|job| JobView {
            job,
            phase: phase(job, status),
        })
        .collect()
}

/// The unfinished jobs, for `cargo stand queue`: runs still building, and
/// who waits for whom.
pub fn describe(jobs: &[Job], status: &crate::Status) -> String {
    describe_views(&views(jobs, status))
}

fn describe_views(views: &[JobView<'_>]) -> String {
    let mut text = String::new();
    for JobView { job, phase } in views {
        let state = match (phase, &job.after) {
            (Phase::WaitingForJob, Some(after)) => format!("waits for job {after}"),
            (phase, _) => String::from(phase.describe()),
        };
        text.push_str(&format!(
            "job:     {} {} `cargo hil {}` {state}\n",
            job.id,
            job.owner,
            job.command.join(" ")
        ));
    }
    text
}

#[cfg(test)]
mod tests;
