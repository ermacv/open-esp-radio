//! Control groups (cgroup v2) a process owns for the commands it runs.
//!
//! A service whose systemd unit delegates its control group to it
//! (`Delegate=yes`) takes that group ([`Delegated::take`]): it moves itself
//! into the leaf `service` and starts every command in a group of its own
//! under `commands`, so it never sits in a group it kills. A command enters
//! its group before `exec`, so it and everything it starts stay inside,
//! whatever session or process group they move to. Stopping a group first
//! closes its admission, also to a spawn under way, then the kernel kills
//! the whole subtree (`cgroup.kill`); `cgroup.events` reports when no
//! process is left (`populated 0`).

use std::{
    fs,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

use crate::Result;

/// Where the cgroup v2 hierarchy is mounted.
const MOUNT: &str = "/sys/fs/cgroup";
/// How often a stopping group is looked at.
const POLL: Duration = Duration::from_millis(20);

/// The control group a process took from its unit.
#[derive(Debug)]
pub struct Delegated {
    commands: PathBuf,
}

impl Delegated {
    /// Take the control group this process runs in: move this process into
    /// its leaf `service`, and stop and remove what a previous owner left
    /// under `commands`. An error when the group is not this process's to
    /// write, as when its unit does not delegate it.
    pub fn take() -> Result<Self> {
        let own = fs::read_to_string("/proc/self/cgroup")?;
        Self::take_at(Path::new(MOUNT), &own, std::process::id())
    }

    /// [`Self::take`] for the hierarchy at `mount`, of a process `pid` whose
    /// `/proc/PID/cgroup` reads `own`.
    pub fn take_at(mount: &Path, own: &str, pid: u32) -> Result<Self> {
        let relative = own
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .ok_or("this process is not in a cgroup v2 hierarchy")?
            .trim()
            .trim_start_matches('/');
        let mut group = mount.join(relative);
        // A process that took its group already sits in its leaf.
        if group.file_name().is_some_and(|name| name == "service")
            && let Some(parent) = group.parent()
        {
            group = parent.to_owned();
        }
        let refused = |error: std::io::Error| {
            format!(
                "cannot take the control group {}: {error}; run the service by its unit, which \
                 delegates the group to it (Delegate=yes)",
                group.display()
            )
        };
        let service = group.join("service");
        fs::create_dir_all(&service).map_err(refused)?;
        fs::write(service.join("cgroup.procs"), pid.to_string()).map_err(refused)?;
        let commands = group.join("commands");
        fs::create_dir_all(&commands).map_err(refused)?;
        let delegated = Self { commands };
        for left in fs::read_dir(&delegated.commands)? {
            let left = left?;
            if left.file_type()?.is_dir() {
                let name = left.file_name().to_string_lossy().into_owned();
                let group = Group::at(left.path(), name);
                group.kill()?;
                if !group.wait_empty(Duration::from_secs(10))? {
                    return Err(format!(
                        "the command group {} left by a previous service does not empty",
                        group.path.display()
                    )
                    .into());
                }
            }
        }
        Ok(delegated)
    }

    /// A new group `name` for one command.
    pub fn group(&self, name: &str) -> Result<Group> {
        if name.is_empty() || name.contains('/') || name.starts_with('.') {
            return Err(format!("`{name}` is not a command group name").into());
        }
        let path = self.commands.join(name);
        fs::create_dir(&path).map_err(|error| {
            format!(
                "cannot create the command group {}: {error}",
                path.display()
            )
        })?;
        Ok(Group::at(path, name.to_owned()))
    }
}

/// One command's control group; removed once empty when dropped.
#[derive(Debug)]
pub struct Group {
    path: PathBuf,
    name: String,
    /// Whether a process may still be started in it.
    open: Mutex<bool>,
}

impl Group {
    fn at(path: PathBuf, name: String) -> Self {
        Self {
            path,
            name,
            open: Mutex::new(true),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Spawn `command` in this group, leading a session of its own with
    /// `terminal` as its controlling terminal when given. Refused once the
    /// group closed; a stop waits for a spawn under way, so the process it
    /// starts is in the group before the stop kills it.
    pub fn spawn(&self, command: &mut Command, terminal: Option<OwnedFd>) -> Result<Child> {
        let open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if !*open {
            return Err(format!("the command group {} is closed", self.name).into());
        }
        let procs: OwnedFd = fs::OpenOptions::new()
            .write(true)
            .open(self.path.join("cgroup.procs"))?
            .into();
        crate::context::prepare(command);
        crate::child_setup::new_session(command, Some(procs), terminal);
        // `spawn` returns once the child ran `exec`, after it entered the
        // group.
        Ok(command.spawn()?)
    }

    /// Admit no more processes.
    pub fn close(&self) {
        *self.open.lock().unwrap_or_else(PoisonError::into_inner) = false;
    }

    /// Close the group and kill every process in it and below it.
    pub fn kill(&self) -> Result<()> {
        self.close();
        fs::write(self.path.join("cgroup.kill"), "1")
            .map_err(|error| format!("cannot kill the command group {}: {error}", self.name))?;
        Ok(())
    }

    /// Whether a process is still in the group or below it.
    pub fn populated(&self) -> Result<bool> {
        populated(&fs::read_to_string(self.path.join("cgroup.events"))?)
    }

    /// Wait up to `timeout` until no process is left; whether none is.
    pub fn wait_empty(&self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.populated()? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.close();
        if self.populated().is_ok_and(|populated| !populated) {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

/// Whether `events`, the text of a group's `cgroup.events`, reports a
/// process in it or below it.
pub fn populated(events: &str) -> Result<bool> {
    events
        .lines()
        .find_map(|line| line.strip_prefix("populated "))
        .map(|value| value.trim() == "1")
        .ok_or_else(|| "cgroup.events has no `populated` line".into())
}

#[cfg(test)]
mod tests;
