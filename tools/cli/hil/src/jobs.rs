//! Enqueued HIL runs at the command line: `cargo hil run … --enqueue
//! [--after JOB]`, `cargo hil wait JOB`, and `--after JOB` in the
//! foreground.
//!
//! A job is the arbiter's ticket model ([`oer_stand_arbiter::jobs`]): its
//! record lives in the arbiter directory and every stand request its process
//! and runner make carries its id. This module keeps what a job needs of the
//! command line: a copy of this binary, the checkout's runner and the source
//! snapshot fixed when it was enqueued, and its detached process.

use std::{ffi::OsString, fs, path::PathBuf};

use oer_stand_arbiter::jobs::{After, Job, JobState};

use crate::Result;
use oer_process::Checkout;

/// The `cargo hil` binary, runner and runner receipt a job runs with, fixed
/// when it was enqueued (or, for an experiment, when it started).
pub const FROZEN_CLI_ENV: &str = "OER_HIL_JOB_CLI";
pub const FROZEN_RUNNER_ENV: &str = "OER_HIL_JOB_RUNNER";
pub const FROZEN_RECEIPT_ENV: &str = "OER_HIL_JOB_RECEIPT";
/// The source snapshot an enqueued `run` builds from, captured when it was
/// enqueued.
pub const FROZEN_SNAPSHOT_ENV: &str = "OER_HIL_JOB_SNAPSHOT";
pub const ENQUEUE: &str = "--enqueue";
pub use oer_hil_experiment::job::{AFTER, AFTER_ANY};

fn jobs() -> Result<oer_stand_arbiter::jobs::Jobs> {
    Ok(oer_stand_arbiter::Arbiter::open()?.jobs())
}

/// What a job runs with, fixed before it waits: a copy of this binary, the
/// checkout's runner with its receipt, and for a `run` the source snapshot.
/// Edits, pulls and rebuilds of the checkout after that do not reach the job,
/// which otherwise built them in the middle of an experiment.
#[derive(Clone, Debug)]
pub struct Frozen {
    pub cli: PathBuf,
    pub runner: PathBuf,
    pub receipt: PathBuf,
    pub snapshot: Option<PathBuf>,
}

impl Frozen {
    /// This binary and the checkout's runner as they are now.
    pub fn capture(ctx: &Checkout) -> Result<Self> {
        let oer_hil_observer::prepare::Prepared { runner, receipt } =
            oer_hil_observer::prepare::prepare(&ctx.root)?;
        let bytes = fs::read(std::env::current_exe()?)?;
        let directory = ctx
            .root
            .join("target/hil/jobs/cli")
            .join(oer_durable::sha256_bytes(&bytes));
        fs::create_dir_all(&directory)?;
        let cli = directory.join("oer-hil-cli");
        if !cli.is_file() {
            let mut copy = tempfile::NamedTempFile::new_in(&directory)?;
            std::io::Write::write_all(&mut copy, &bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                copy.as_file()
                    .set_permissions(fs::Permissions::from_mode(0o755))?;
            }
            copy.persist(&cli)?;
        }
        Ok(Self {
            cli,
            runner,
            receipt,
            snapshot: None,
        })
    }

    /// The parts this process's job was fixed with, if it is such a job.
    pub fn inherited() -> Result<Option<Self>> {
        let variable = |name| std::env::var_os(name).map(PathBuf::from);
        let (Some(cli), Some(runner), Some(receipt)) = (
            variable(FROZEN_CLI_ENV),
            variable(FROZEN_RUNNER_ENV),
            variable(FROZEN_RECEIPT_ENV),
        ) else {
            return Ok(None);
        };
        for path in [&cli, &runner, &receipt] {
            if !path.is_file() {
                return Err(format!(
                    "{} was fixed for this job when it was enqueued and is gone; enqueue it again",
                    path.display()
                )
                .into());
            }
        }
        Ok(Some(Self {
            cli,
            runner,
            receipt,
            snapshot: variable(FROZEN_SNAPSHOT_ENV),
        }))
    }

    /// Every file or directory the job needs to stay.
    pub fn paths(&self) -> Vec<PathBuf> {
        [&self.cli, &self.runner, &self.receipt]
            .into_iter()
            .cloned()
            .chain(self.snapshot.clone())
            .collect()
    }

    /// A command running this frozen `cargo hil` in `ctx`'s checkout, with
    /// the frozen parts in its environment.
    pub fn hil_command(&self, ctx: &Checkout) -> std::process::Command {
        let mut command = ctx.command(&self.cli);
        command
            .arg("--root")
            .arg(&ctx.root)
            .env(FROZEN_CLI_ENV, &self.cli)
            .env(FROZEN_RUNNER_ENV, &self.runner)
            .env(FROZEN_RECEIPT_ENV, &self.receipt);
        match &self.snapshot {
            Some(snapshot) => command.env(FROZEN_SNAPSHOT_ENV, snapshot),
            None => command.env_remove(FROZEN_SNAPSHOT_ENV),
        };
        command
    }
}

/// `--enqueue` and `--after JOB` (or `--after-any JOB`) taken from `cargo
/// hil` arguments, up to a `--`.
pub fn take(args: Vec<OsString>) -> Result<(bool, Option<After>, Vec<OsString>)> {
    let mut enqueue = false;
    let mut after = None;
    let mut remaining = Vec::new();
    let mut rest = args.into_iter();
    while let Some(argument) = rest.next() {
        let text = argument.to_str().unwrap_or_default();
        let dependency = [(AFTER, false), (AFTER_ANY, true)]
            .into_iter()
            .find_map(|(flag, any)| {
                if text == flag {
                    Some((flag, any, None))
                } else {
                    text.strip_prefix(flag)
                        .and_then(|value| value.strip_prefix('='))
                        .map(|value| (flag, any, Some(value.to_owned())))
                }
            });
        if text == "--" {
            remaining.push(argument);
            remaining.extend(rest.by_ref());
            break;
        } else if text == ENQUEUE {
            enqueue = true;
        } else if let Some((flag, any, value)) = dependency {
            let job = match value {
                Some(value) => value,
                None => rest
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(|| format!("{flag} requires a job id"))?,
            };
            if after.replace(After { job, any }).is_some() {
                return Err("--after or --after-any is given twice".into());
            }
        } else {
            remaining.push(argument);
        }
    }
    Ok((enqueue, after, remaining))
}

/// Record a job for `args` and start it detached; returns its id.
pub fn enqueue(
    ctx: &Checkout,
    owner: &str,
    args: &[OsString],
    after: Option<After>,
    frozen: &Frozen,
) -> Result<String> {
    let jobs = jobs()?;
    if let Some(after) = &after {
        jobs.read(&after.job)?;
    }
    let mut job = Job::new(
        owner,
        oer_hil_experiment::job::command_of(args),
        ctx.root.clone(),
        after.as_ref(),
    );
    let id = job.id.clone();
    let log_directory = ctx.root.join("target/hil/jobs");
    fs::create_dir_all(&log_directory)?;
    let log = log_directory.join(format!("{id}.log"));
    job.log = Some(log.clone());
    job.fixed = frozen.paths();
    jobs.write(&job)?;
    let output = fs::File::create(&log)?;
    let mut command = frozen.hil_command(ctx);
    command
        .args(args)
        .args(after.iter().flat_map(|after| {
            [
                if after.any { AFTER_ANY } else { AFTER },
                after.job.as_str(),
            ]
        }))
        .env(oer_stand_owners::OWNER_ENV, owner)
        .stdin(std::process::Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Its own process group: the job outlives this command and its
        // terminal's signals.
        command.process_group(0);
    }
    oer_process::Context::default()
        .with(oer_stand_arbiter::jobs::JOB_KEY, &id)
        .apply(&mut command)?;
    let child = command.spawn()?;
    job.run_by(child.id());
    jobs.write(&job)?;
    Ok(id)
}

/// `cargo hil wait JOB`: block until the job settles, print its outcome and
/// runs, and exit with the outcome's code.
pub fn wait_command(args: &[OsString]) -> Result<std::process::ExitCode> {
    let [id] = args else {
        return Err("usage: cargo hil wait JOB".into());
    };
    let id = id.to_str().ok_or("a job id is text")?;
    let jobs = jobs()?;
    let outcome = jobs.wait(id)?;
    let job = jobs.read(id)?;
    let runs = match &job.state {
        JobState::Finished { runs, .. } => runs.join(" "),
        _ => String::new(),
    };
    println!(
        "job {id}: {outcome}; runs: {runs}{}",
        job.log
            .as_deref()
            .map(|log| format!("; log: {}", log.display()))
            .unwrap_or_default()
    );
    Ok(std::process::ExitCode::from(outcome.exit_code()))
}

#[cfg(test)]
mod tests;

/// `--board` and `--peer-board` of a command that starts runs, passed on to
/// each run: the pool's boards to use where several of a chip qualify,
/// until the stand's scheduler assigns them.
#[derive(Clone, Debug, Default, Eq, PartialEq, clap::Args)]
pub(crate) struct BoardChoiceArgs {
    /// The device under test, by its stand-file id.
    #[arg(long)]
    pub board: Option<String>,
    /// The peer board, by its stand-file id.
    #[arg(long)]
    pub peer_board: Option<String>,
}

impl BoardChoiceArgs {
    /// The runner arguments that name the chosen boards.
    pub(crate) fn arguments(&self) -> Vec<String> {
        let mut arguments = Vec::new();
        if let Some(board) = &self.board {
            arguments.extend([String::from("--board"), board.clone()]);
        }
        if let Some(board) = &self.peer_board {
            arguments.extend([String::from("--peer-board"), board.clone()]);
        }
        arguments
    }
}

#[cfg(test)]
mod board_choice_tests {
    use super::BoardChoiceArgs;

    #[test]
    fn the_chosen_boards_reach_each_run() {
        assert!(BoardChoiceArgs::default().arguments().is_empty());
        let both = BoardChoiceArgs {
            board: Some("dut-b".into()),
            peer_board: Some("peer-a".into()),
        };
        assert_eq!(
            both.arguments(),
            ["--board", "dut-b", "--peer-board", "peer-a"]
        );
    }
}
