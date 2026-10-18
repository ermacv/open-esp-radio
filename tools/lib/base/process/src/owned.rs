//! A Unix process group owns the command and its ordinary descendants.
//!
//! This is lifecycle cleanup, not a resource limit or a sandbox for processes
//! that deliberately move to another process group or session.

use crate::Result;
use std::{
    process::{ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Output},
    time::{Duration, Instant},
};

/// Owned children one process keeps alive at once: [`Child::spawn`] fails
/// beyond this, since the guardian that stops them after a lost owner tracks
/// no more.
#[cfg(unix)]
pub const MAX_LIVE_CHILDREN: usize = super::guardian::CAPACITY;

pub struct Child {
    child: std::process::Child,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    interruptible: bool,
    deadline: Option<Instant>,
    group: i32,
    /// Whether the guardian holds a registration of `group` for this child.
    guarded: bool,
    finished: bool,
    shutdown_grace: Duration,
}
impl Child {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        Self::spawn_with_shutdown_grace(command, Duration::from_secs(1))
    }
    pub fn spawn_with_shutdown_grace(
        command: &mut Command,
        shutdown_grace: Duration,
    ) -> Result<Self> {
        super::check_cancelled()?;
        super::context::prepare(command);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
            let mut child = command.spawn()?;
            let group = i32::try_from(child.id()).expect("Unix PID fits pid_t");
            let registered = super::guardian::register(group);
            let owned = Self {
                stdin: child.stdin.take(),
                stdout: child.stdout.take(),
                stderr: child.stderr.take(),
                child,
                interruptible: !super::cancellation::in_cleanup(),
                deadline: super::cancellation::cleanup_deadline(),
                group,
                guarded: registered.is_ok(),
                finished: false,
                shutdown_grace,
            };
            // A group the guardian does not track would outlive a lost
            // owner: dropping `owned` stops it before the error returns.
            registered.map(|()| owned)
        }
        #[cfg(not(unix))]
        {
            let _ = command;
            Err("host process-group ownership is unsupported on this host".into())
        }
    }
    /// Bound the remaining lifetime, including work before the caller waits.
    /// A cleanup scope's earlier deadline always takes precedence.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.deadline = self
            .deadline
            .into_iter()
            .chain(Some(Instant::now() + timeout))
            .min();
        self
    }
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.stderr.take()
    }
    pub fn wait(&mut self) -> Result<ExitStatus> {
        self.wait_timeout(None)
    }
    /// Forward cancellation and retain the child's final status after its shutdown grace.
    pub fn wait_forwarding_cancellation(&mut self) -> Result<ExitStatus> {
        match self.wait() {
            Err(error) if crate::is_cancelled(&*error) => Ok(self.child.wait()?),
            result => result,
        }
    }
    pub fn wait_timeout(&mut self, timeout: Option<Duration>) -> Result<ExitStatus> {
        let deadline = timeout
            .map(|timeout| Instant::now() + timeout)
            .into_iter()
            .chain(self.deadline)
            .min();
        loop {
            if self.interruptible && super::cancellation_requested() {
                self.cleanup();
                return Err(super::Cancelled.into());
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                self.cleanup();
                return Err(DeadlineExceeded.into());
            }
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.cleanup();
                    return Ok(status);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    self.cleanup();
                    return Err(error.into());
                }
            }
        }
    }
    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
    pub fn kill(&mut self) -> std::io::Result<()> {
        self.cleanup();
        Ok(())
    }
    pub fn wait_with_output(self) -> Result<Output> {
        self.wait_with_output_timeout(None)
    }
    pub fn wait_with_output_timeout(self, timeout: Option<Duration>) -> Result<Output> {
        super::collect_output(self, timeout)
    }
    #[cfg(unix)]
    fn signal(&self, signal: i32) {
        // SAFETY: the negative PID names only the process group created for
        // this owned child; no caller supplies an arbitrary process identifier.
        unsafe {
            libc::kill(-self.group, signal);
        }
    }
    #[cfg(unix)]
    fn group_exists(&self) -> bool {
        // SAFETY: signal zero only observes the group created during spawn.
        unsafe { libc::kill(-self.group, 0) == 0 }
    }
    fn cleanup(&mut self) {
        if self.finished {
            return;
        }
        #[cfg(unix)]
        {
            self.signal(libc::SIGTERM);
            let deadline = Instant::now() + self.shutdown_grace;
            while self.group_exists() && Instant::now() < deadline {
                let _ = self.child.try_wait();
                std::thread::sleep(Duration::from_millis(10));
            }
            self.signal(libc::SIGKILL);
            if self.guarded {
                super::guardian::unregister(self.group);
            }
        }
        let _ = self.child.wait();
        self.finished = true;
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[derive(Debug)]
pub struct DeadlineExceeded;
impl std::fmt::Display for DeadlineExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("subprocess exceeded its deadline")
    }
}
impl std::error::Error for DeadlineExceeded {}
