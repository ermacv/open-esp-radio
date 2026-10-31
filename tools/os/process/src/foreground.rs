//! A child that owns the terminal: it stays in its parent's process group,
//! the terminal's foreground job, so it reads the terminal and gets the
//! terminal's Ctrl-C itself. While one runs, SIGINT cancels nothing in the
//! parent (the child decides what Ctrl-C means, as a shell or a debugger
//! does); SIGTERM and SIGHUP still cancel. The parent stops only the child:
//! what the child started is its own to end.

use crate::Result;
use std::{
    process::{Command, ExitStatus},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

/// Foreground children running in this process.
static RUNNING: AtomicUsize = AtomicUsize::new(0);

/// Whether `signal` cancels this process now.
pub(crate) fn cancels_now(signal: i32) -> bool {
    cancels(signal, RUNNING.load(Ordering::Relaxed))
}

/// Whether `signal` cancels a process running `foreground` foreground
/// children.
fn cancels(signal: i32, foreground: usize) -> bool {
    #[cfg(unix)]
    if signal == signal_hook::consts::SIGINT {
        return foreground == 0;
    }
    let _ = (signal, foreground);
    true
}

pub struct Child {
    child: std::process::Child,
    finished: bool,
}

impl Child {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        super::check_cancelled()?;
        super::context::prepare(command);
        RUNNING.fetch_add(1, Ordering::Relaxed);
        match command.spawn() {
            Ok(child) => Ok(Self {
                child,
                finished: false,
            }),
            Err(error) => {
                RUNNING.fetch_sub(1, Ordering::Relaxed);
                Err(error.into())
            }
        }
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.finish();
        }
        Ok(status)
    }

    /// Stop the child: SIGTERM, then SIGKILL once `grace` passed.
    pub fn terminate(&mut self, grace: Duration) -> Result<ExitStatus> {
        if let Some(status) = self.try_wait()? {
            return Ok(status);
        }
        #[cfg(unix)]
        {
            let pid = rustix::process::Pid::from_child(&self.child);
            // A child that already exited is reaped below.
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline {
                if let Some(status) = self.try_wait()? {
                    return Ok(status);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        #[cfg(not(unix))]
        let _ = (grace, Instant::now());
        // Kill fails only for a child that already exited.
        let _ = self.child.kill();
        let status = self.child.wait()?;
        self.finish();
        Ok(status)
    }

    fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            RUNNING.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.terminate(Duration::from_secs(1));
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn interrupts_cancel_only_while_no_foreground_child_runs() {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        assert!(cancels(SIGINT, 0));
        assert!(!cancels(SIGINT, 1), "Ctrl-C belongs to the child");
        assert!(cancels(SIGTERM, 1) && cancels(SIGHUP, 1));
    }
}
