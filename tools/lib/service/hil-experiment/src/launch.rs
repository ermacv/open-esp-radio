//! The one way a run is launched: [`launch_run`].
//!
//! Whoever starts a runner (a `cargo hil run`, an A/B arm, a bisection
//! step) describes it as a [`Launch`]; the runner executable runs
//! supervised, with cancellation forwarded and a shutdown grace for fixture
//! cleanup and sealing, and the runs it created come back from its run
//! receipt ([`oer_tool_service_hil_run_bundle::receipt`]), never from the store's newest
//! directories. No launcher runs `cargo hil`.

use std::{
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    time::Duration,
};

use oer_tool_service_hil_run_bundle::RunId;
use oer_tool_service_hil_run_bundle::receipt::Receipt;

use crate::Result;

/// How long a cancelled runner may take to restore its fixtures and seal
/// its run: a shutdown allowance, never a run timeout.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(300);

/// A runner executable and the observer receipt it runs under.
#[derive(Clone, Debug)]
pub struct Runner {
    pub executable: PathBuf,
    /// The prepared observer receipt; `None` runs the executable without
    /// one, which records no observer artifacts.
    pub receipt: Option<PathBuf>,
}

impl Runner {
    /// The runner of the checkout at `root`, prepared with its receipt.
    pub fn prepare(root: &Path) -> Result<Self> {
        let prepared = oer_tool_service_hil_observer::prepare::prepare(root)?;
        Ok(Self {
            executable: prepared.runner,
            receipt: Some(prepared.receipt),
        })
    }
    /// The runner of a revision checked out at `root`, without an observer
    /// receipt: its embedded build and receipt format are the revision's
    /// own, which this checkout does not read.
    pub fn revision(root: &Path) -> Result<Self> {
        Ok(Self {
            executable: oer_tool_service_hil_observer::prepare::build(root)?,
            receipt: None,
        })
    }
}

/// One invocation of a runner.
#[derive(Debug)]
pub struct Launch<'a> {
    /// The checkout the runner runs in.
    pub checkout: &'a Path,
    pub runner: &'a Runner,
    pub arguments: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
    /// Variables the runner must not inherit.
    pub removed: Vec<OsString>,
    /// Where the runner's output goes instead of this process's.
    pub log: Option<PathBuf>,
    pub context: oer_tool_base_process::Context,
}

impl<'a> Launch<'a> {
    pub fn new(checkout: &'a Path, runner: &'a Runner) -> Self {
        Self {
            checkout,
            runner,
            arguments: Vec::new(),
            environment: Vec::new(),
            removed: Vec::new(),
            log: None,
            context: oer_tool_base_process::Context::default(),
        }
    }

    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.arguments.extend(arguments.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.push((name.into(), value.into()));
        self
    }

    pub fn env_remove(mut self, name: impl Into<OsString>) -> Self {
        self.removed.push(name.into());
        self
    }

    pub fn context(mut self, context: oer_tool_base_process::Context) -> Self {
        self.context = context;
        self
    }

    pub fn log(mut self, path: PathBuf) -> Self {
        self.log = Some(path);
        self
    }

    /// The command the launch runs, with `receipt` as its run receipt.
    fn command(&self, receipt: &Path) -> Result<Command> {
        let mut command = oer_tool_base_process::command(&self.runner.executable);
        command.current_dir(self.checkout).args(&self.arguments);
        match &self.runner.receipt {
            Some(observer) => command.env(
                oer_tool_data_hil_run_bundle_format::observer::receipt::ENV,
                observer,
            ),
            None => command.env_remove(oer_tool_data_hil_run_bundle_format::observer::receipt::ENV),
        };
        for (name, value) in &self.environment {
            command.env(name, value);
        }
        for name in &self.removed {
            command.env_remove(name);
        }
        command.env(oer_tool_service_hil_run_bundle::receipt::ENV, receipt);
        self.context.apply(&mut command)?;
        if let Some(path) = &self.log {
            let log = File::create(path)?;
            command.stdout(log.try_clone()?).stderr(log);
        }
        Ok(command)
    }
}

/// What a launch did: the runs its runner created, in creation order, and
/// how the runner ended.
#[derive(Debug)]
pub struct Launched {
    pub runs: Vec<RunId>,
    pub status: ExitStatus,
}

impl Launched {
    /// The last run the runner created.
    pub fn run(&self) -> Result<RunId> {
        self.runs
            .last()
            .cloned()
            .ok_or_else(|| format!("the runner created no run ({})", self.status).into())
    }
}

/// Run `launch` to its end; the runs its runner created.
pub fn launch_run(launch: &Launch<'_>) -> Result<Launched> {
    let receipt = Receipt::new()?;
    let mut command = launch.command(receipt.path())?;
    let mut child = oer_tool_base_process::owned::Child::spawn_with_shutdown_grace(
        &mut command,
        SHUTDOWN_GRACE,
    )?;
    let status = child.wait_forwarding_cancellation()?;
    Ok(Launched {
        runs: receipt.runs()?,
        status,
    })
}

#[cfg(test)]
mod tests;
