//! A revision's scenario set, run with the runner that speaks its HIL wire.
//!
//! The revision is checked out, detached, in a worktree below the
//! experiment's directory. A revision whose HIL protocol version
//! (`hil/protocol/messages.lock`) is this checkout's runs on this
//! checkout's runner and host code with the revision's firmware, built from
//! a source snapshot of the worktree. A revision with another protocol
//! version runs its own runner, built in the worktree, inside a whole-stand
//! lease: that runner gets a private arbiter directory holding a copy of the
//! stand file (and of an older device registry), so it neither waits for nor
//! disturbs the shared arbiter, whose schema it may not read, and the
//! worktree's run directory links to the shared store. An older runner
//! reads the same settings under older names (`OER_HIL_ARBITER_DIR`,
//! `OER_HIL_OWNER`) and may keep its runs in a directory of its own below
//! the worktree's `target/hil`: such a run is moved into the store's
//! `revision-runs` directory before the worktree goes, where it outlives the
//! checkout. One launch on a revision is shared by [`run`]
//! and every [`crate::bisect`] step; every run goes
//! through [`crate::launch::launch_run`], and its run is the one its receipt
//! names.
//!
//! [`run`] records one typed report, `report.json`: the revision, the
//! runner that ran it and each scenario's outcome. It pairs no measurement
//! with another revision's: a revision with another wire may measure
//! differently.

use std::{
    ffi::OsString,
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use oer_hil_run_bundle::RunStore;
use oer_hil_schema::run::Outcome;
use oer_os_process::git::Worktree;
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    job::Running,
    launch::{self, Launch, launch_run},
};

/// The file naming a revision's HIL wire: revisions with equal locks speak
/// the same protocol.
const MESSAGES_LOCK: &str = "portable/hil/protocol/messages.lock";
const REPORT_SCHEMA: u16 = 1;

/// Every name a revision's runner has read its arbiter directory from:
/// this checkout's first, then the HIL arbiter's before the stand packages.
const ARBITER_DIRECTORY: [&str; 2] = [oer_lab_model::paths::ARBITER_ENV, "OER_HIL_ARBITER_DIR"];

/// Every name a revision's runner has read its stand owner from.
const OWNER: [&str; 2] = [oer_lab_stand_owners::OWNER_ENV, "OER_HIL_OWNER"];

/// The report every runner writes into its run directory: each scenario's
/// outcome, in a form every revision shares.
const SUITE: &str = "suite.json";

/// The HIL wire of the tree at `root`, or `None` for a revision older than
/// the keyed protocol, whose wire no lock names.
pub(crate) fn messages_lock(root: &Path) -> Option<String> {
    fs::read_to_string(root.join(MESSAGES_LOCK)).ok()
}

/// Which runner runs a revision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "runner", rename_all = "kebab-case")]
pub enum Runner {
    /// This checkout's runner with the revision's firmware.
    Current,
    /// The revision's own runner, for its other wire.
    Revision,
}

impl Runner {
    /// The runner for a revision whose wire is `revision`, from a checkout
    /// whose wire is `current`: this checkout's only for the same wire.
    pub fn for_wire(current: &str, revision: Option<&str>) -> Self {
        if revision == Some(current) {
            Self::Current
        } else {
            Self::Revision
        }
    }
}

/// What a launch on a revision produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ran {
    /// The run the runner created last, and its directory.
    Run { id: String, directory: PathBuf },
    /// The revision's own runner did not build or created no run.
    RunnerUnavailable,
}

/// Launches on revisions checked out in one worktree; see the
/// [module](self) documentation.
pub(crate) struct Revision<'a> {
    pub(crate) checkout: &'a Path,
    pub(crate) owner: &'a str,
    /// This checkout's wire.
    pub(crate) current: String,
    /// This checkout's runner, prepared once for every launch it runs.
    pub(crate) runner: launch::Runner,
    pub(crate) store: RunStore,
    pub(crate) worktree: PathBuf,
    pub(crate) arbiter: PathBuf,
    /// Where the runs a revision's runner kept below its worktree go: the
    /// store's [`RunStore::revision_runs`], so they outlive this checkout.
    pub(crate) runs: PathBuf,
}

impl<'a> Revision<'a> {
    /// Launches for `checkout`, with the worktree and private arbiter below
    /// `directory`.
    pub(crate) fn new(checkout: &'a Path, owner: &'a str, directory: &Path) -> Result<Self> {
        let store = RunStore::shared()?;
        Ok(Self {
            checkout,
            owner,
            current: messages_lock(checkout).ok_or("this checkout names no HIL wire")?,
            runner: launch::Runner::prepare(checkout)?,
            runs: store.revision_runs(),
            store,
            worktree: directory.join("worktree"),
            arbiter: directory.join("arbiter"),
        })
    }

    /// Check out `commit` in `worktree`, creating it on first use.
    pub(crate) fn checkout(&self, worktree: &mut Option<Worktree>, commit: &str) -> Result<()> {
        match worktree {
            Some(worktree) => worktree.checkout(commit)?,
            None => *worktree = Some(Worktree::detached(self.checkout, &self.worktree, commit)?),
        }
        Ok(())
    }

    /// The checked-out revision's subject line.
    pub(crate) fn subject(&self) -> Result<String> {
        oer_os_process::git::text(&self.worktree, ["log", "-1", "--format=%s"])
    }

    /// The runner for the checked-out revision.
    pub(crate) fn runner(&self) -> Runner {
        Runner::for_wire(&self.current, messages_lock(&self.worktree).as_deref())
    }

    /// Run `scenarios` on the checked-out revision with `arguments` after
    /// them (a layout seed, the chosen boards); `work` names the lease.
    pub(crate) fn launch(
        &self,
        scenarios: &[String],
        arguments: &[String],
        work: &str,
    ) -> Result<(Runner, Ran)> {
        let runner = self.runner();
        let ran = match runner {
            Runner::Current => self.with_current_runner(scenarios, arguments)?,
            Runner::Revision => self.with_revision_runner(scenarios, arguments, work)?,
        };
        Ok((runner, ran))
    }

    /// This checkout's runner, building the revision's firmware from a
    /// snapshot of the worktree, as a job of its own.
    pub(crate) fn with_current_runner(
        &self,
        scenarios: &[String],
        arguments: &[String],
    ) -> Result<Ran> {
        let snapshot = oer_hil_source_snapshot::capture(
            &self.worktree,
            &[],
            false,
            &oer_hil_image::source_snapshot_store()?,
            &oer_hil_image::source_objects()?,
        )?;
        let arguments = std::iter::once(String::from("run"))
            .chain(scenarios.iter().cloned())
            .chain([
                String::from("--source-snapshot"),
                snapshot.directory().to_string_lossy().into_owned(),
            ])
            .chain(arguments.iter().cloned())
            .map(OsString::from)
            .collect::<Vec<_>>();
        let mut job = Running::new(self.checkout, self.owner, &arguments)?;
        let launched = launch_run(
            &Launch::new(self.checkout, &self.runner)
                .args(arguments)
                .env(oer_lab_stand_owners::OWNER_ENV, self.owner)
                .context(
                    oer_os_process::Context::current()?
                        .clone()
                        .with(oer_lab_stand_owners::OWNER_KEY, self.owner)
                        .with(oer_lab_stand_arbiter::jobs::JOB_KEY, job.id()),
                ),
        )?;
        job.finish_with(&self.store, &launched.runs)?;
        let id = launched.run()?.to_string();
        Ok(Ran::Run {
            directory: self.store.runs().join(&id),
            id,
        })
    }

    /// The revision's own runner, inside a whole-stand lease of the shared
    /// arbiter, with a private arbiter holding a copy of the stand file.
    fn with_revision_runner(
        &self,
        scenarios: &[String],
        arguments: &[String],
        work: &str,
    ) -> Result<Ran> {
        let arbiter = oer_lab_stand_arbiter::Arbiter::open()?;
        fs::create_dir_all(&self.arbiter)?;
        // A revision's runner reads the user's stand file; a revision before
        // the stand model read it beside its arbiter's state, and one before
        // the stand file the arbiter's device registry the shared directory
        // may still hold: the private arbiter gets copies of both.
        fs::copy(arbiter.stand_file(), self.arbiter.join("stand.toml"))?;
        let devices = arbiter.directory().join("devices.json");
        if devices.exists() {
            fs::copy(&devices, self.arbiter.join("devices.json"))?;
        }
        let _grant = arbiter.acquire(&oer_lab_stand_arbiter::Request {
            owner: self.owner.to_owned(),
            work: work.to_owned(),
            scenarios: scenarios.to_vec(),
            run: None,
            claims: vec![oer_lab_stand_claims::Claim::stand()],
        })?;
        // The revision's runner records its runs in the shared store.
        let runner = match launch::Runner::prepare(&self.worktree)
            .and_then(|runner| Ok((runner, self.store.link(&self.worktree)?)))
        {
            Ok((runner, _)) => runner,
            Err(error) => {
                eprintln!("hil: the revision's runner does not build: {error}");
                return Ok(Ran::RunnerUnavailable);
            }
        };
        let launched = launch_run(&self.own_runner_launch(&runner, scenarios, arguments))?;
        match launched.runs.last() {
            Some(run) => {
                let id = run.to_string();
                Ok(Ran::Run {
                    directory: self.keep(&id)?,
                    id,
                })
            }
            None => {
                eprintln!(
                    "hil: the revision's runner created no run ({})",
                    launched.status
                );
                Ok(Ran::RunnerUnavailable)
            }
        }
    }
}

impl Revision<'_> {
    /// The launch of the revision's own `runner` on `scenarios`: in the
    /// worktree, with the private arbiter and the owner under every name a
    /// revision has read them from.
    fn own_runner_launch<'r>(
        &'r self,
        runner: &'r launch::Runner,
        scenarios: &[String],
        arguments: &[String],
    ) -> Launch<'r> {
        let arguments = std::iter::once(String::from("run"))
            .chain(scenarios.iter().cloned())
            .chain(arguments.iter().cloned());
        let mut launch = Launch::new(&self.worktree, runner).args(arguments).context(
            oer_os_process::Context::default().with(oer_lab_stand_owners::OWNER_KEY, self.owner),
        );
        for name in ARBITER_DIRECTORY {
            launch = launch.env(name, &self.arbiter);
        }
        for name in OWNER {
            launch = launch.env(name, self.owner);
        }
        launch
    }

    /// The directory of the run `id` a revision's runner created: in the
    /// shared store, or moved out of the worktree's `target/hil` into
    /// [`Self::runs`].
    fn keep(&self, id: &str) -> Result<PathBuf> {
        let shared = self.store.runs().join(id);
        if shared.is_dir() {
            return Ok(shared);
        }
        let found = find_run(&self.worktree.join("target/hil"), id)?
            .ok_or_else(|| format!("the revision's run {id} is in no run directory"))?;
        fs::create_dir_all(&self.runs)?;
        let kept = self.runs.join(id);
        move_directory(&found, &kept)?;
        Ok(kept)
    }
}

/// Move the directory `from` to `to`, also onto another file system (the
/// store may live elsewhere than the checkout). There the tree is copied
/// beside `to` under a partial name, renamed into place once complete and
/// only then removed from `from`; a failed copy leaves no partial tree.
fn move_directory(from: &Path, to: &Path) -> std::io::Result<()> {
    match fs::rename(from, to) {
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => move_across(from, to),
        moved => moved,
    }
}

/// [`move_directory`] onto another file system: copy under a partial name,
/// rename into place, then remove `from`. A failed copy removes the partial
/// tree and leaves `from` whole.
fn move_across(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut name = std::ffi::OsString::from(".");
    name.push(to.file_name().unwrap_or_default());
    name.push(".partial");
    let partial = to.with_file_name(name);
    let copied = copy_tree(from, &partial).and_then(|()| fs::rename(&partial, to));
    if let Err(error) = copied {
        // The copy's own error is the one to report; a partial tree that
        // cannot be removed either stays under its hidden partial name.
        let _ = fs::remove_dir_all(&partial);
        return Err(error);
    }
    fs::remove_dir_all(from)
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(entry.path())?, &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// The directory named `id` holding a run report below `root`, at most
/// three levels down (`<chip>/runs/<id>`).
pub(crate) fn find_run(root: &Path, id: &str) -> Result<Option<PathBuf>> {
    fn walk(directory: &Path, id: &str, depth: u8) -> std::io::Result<Option<PathBuf>> {
        let candidate = directory.join(id);
        if candidate.join(SUITE).is_file() {
            return Ok(Some(candidate));
        }
        if depth == 0 {
            return Ok(None);
        }
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            // A link leads out of the worktree, to the shared store.
            if entry.file_type()?.is_dir()
                && let Some(found) = walk(&entry.path(), id, depth - 1)?
            {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }
    Ok(walk(root, id, 3)?)
}

/// What one revision run runs.
#[derive(Clone, Debug)]
pub struct Spec {
    /// The revision: a commit, tag or branch.
    pub revision: String,
    /// The scenario set, in the order the runner runs it.
    pub scenarios: Vec<String>,
    /// Build every image with this code layout seed.
    pub layout_seed: Option<NonZeroU32>,
    /// The runner arguments naming the boards the run uses; a revision's
    /// own runner from before the stand file does not know them.
    pub boards: Vec<String>,
}

impl Spec {
    /// The runner arguments after the scenarios.
    pub(crate) fn arguments(&self) -> Vec<String> {
        seed_and_boards(self.layout_seed, &self.boards)
    }
}

/// The run's layout seed and chosen boards, as runner arguments.
pub(crate) fn seed_and_boards(layout_seed: Option<NonZeroU32>, boards: &[String]) -> Vec<String> {
    let mut arguments = layout_seed
        .map(|seed| vec![String::from("--layout-seed"), seed.to_string()])
        .unwrap_or_default();
    arguments.extend(boards.iter().cloned());
    arguments
}

/// One scenario's outcome on the revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScenarioOutcome {
    pub scenario: String,
    pub outcome: Outcome,
}

/// What the revision's run showed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum RevisionResult {
    /// The run ended; each scenario's outcome, in the run's order.
    Ran {
        run: String,
        /// The run's directory: in the store's `runs/`, or in its
        /// `revision-runs/` for a revision's runner that kept the run below
        /// its worktree in its own format.
        directory: PathBuf,
        scenarios: Vec<ScenarioOutcome>,
    },
    /// The revision's own runner did not build or created no run.
    RunnerUnavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Report {
    pub schema: u16,
    /// The revision as given.
    pub revision: String,
    pub commit: String,
    pub subject: String,
    pub layout_seed: Option<NonZeroU32>,
    #[serde(flatten)]
    pub runner: Runner,
    #[serde(flatten)]
    pub result: RevisionResult,
}

impl Report {
    /// The exit status the report gives: 0 every scenario passed, 1 one
    /// failed, 2 the revision was not judged (a scenario neither passed nor
    /// failed, or no run).
    pub fn code(&self) -> u8 {
        match &self.result {
            RevisionResult::Ran { scenarios, .. } => {
                if scenarios
                    .iter()
                    .any(|s| !matches!(s.outcome, Outcome::Passed | Outcome::Failed))
                {
                    2
                } else if scenarios.iter().any(|s| s.outcome == Outcome::Failed) {
                    1
                } else {
                    0
                }
            }
            RevisionResult::RunnerUnavailable => 2,
        }
    }
}

/// A finished revision run: its report and where it is written.
pub struct Finished {
    pub report: Report,
    pub path: PathBuf,
}

/// Run `spec` in the checkout at `checkout` for `owner`.
pub fn run(
    checkout: &Path,
    owner: &str,
    spec: &Spec,
    mut progress: impl FnMut(&str),
) -> Result<Finished> {
    if spec.scenarios.is_empty() {
        return Err("a revision run needs at least one scenario".into());
    }
    let commit = oer_os_process::git::commit(checkout, &spec.revision)?;
    let id = oer_os_durable::unix_millis().to_string();
    let directory = checkout.join("target/hil/revision").join(&id);
    fs::create_dir_all(&directory)?;
    let revision = Revision::new(checkout, owner, &directory)?;
    let mut worktree = None;
    let launched = revision.checkout(&mut worktree, &commit).and_then(|()| {
        let subject = revision.subject()?;
        progress(&format!(
            "running {} scenario(s) on {} ({subject}) with {} runner",
            spec.scenarios.len(),
            &commit[..12.min(commit.len())],
            match revision.runner() {
                Runner::Current => "this checkout's",
                Runner::Revision => "the revision's own",
            }
        ));
        let work = format!("run {} on {}", spec.scenarios.join(" "), spec.revision);
        let (runner, ran) = revision.launch(&spec.scenarios, &spec.arguments(), &work)?;
        Ok((subject, runner, ran))
    });
    if let Some(worktree) = worktree
        && let Err(error) = worktree.remove()
    {
        progress(&format!("the revision's worktree was not removed: {error}"));
    }
    let (subject, runner, ran) = launched?;
    let result = match ran {
        Ran::Run { id, directory } => RevisionResult::Ran {
            scenarios: outcomes(&directory)?,
            directory,
            run: id,
        },
        Ran::RunnerUnavailable => RevisionResult::RunnerUnavailable,
    };
    let report = Report {
        schema: REPORT_SCHEMA,
        revision: spec.revision.clone(),
        commit,
        subject,
        layout_seed: spec.layout_seed,
        runner,
        result,
    };
    let path = directory.join("report.json");
    oer_os_durable::atomic_json(&path, &report)?;
    Ok(Finished { report, path })
}

/// Each scenario's outcome in the run at `directory`, in its order, from
/// its suite report.
pub(crate) fn outcomes(directory: &Path) -> Result<Vec<ScenarioOutcome>> {
    #[derive(Deserialize)]
    struct Suite {
        scenarios: Vec<ScenarioOutcome>,
    }
    let path = directory.join(SUITE);
    let suite: Suite = serde_json::from_slice(&fs::read(&path).map_err(|error| {
        format!(
            "the run's suite report {} is unreadable: {error}",
            path.display()
        )
    })?)?;
    Ok(suite.scenarios)
}

pub fn summary(report: &Report) -> String {
    let short = &report.commit[..12.min(report.commit.len())];
    let runner = match report.runner {
        Runner::Current => "",
        Runner::Revision => " [own runner]",
    };
    let mut text = format!(
        "{} at {short}{runner}  {}\n",
        report.revision, report.subject
    );
    match &report.result {
        RevisionResult::Ran { run, scenarios, .. } => {
            text.push_str(&format!("run {run}:\n"));
            for scenario in scenarios {
                text.push_str(&format!(
                    "  {} {}\n",
                    scenario.outcome.id(),
                    scenario.scenario
                ));
            }
        }
        RevisionResult::RunnerUnavailable => text.push_str("the revision's runner cannot run\n"),
    }
    text
}

#[cfg(test)]
mod tests;
