//! Process identities that survive PID reuse.

use serde::{Deserialize, Serialize};

/// A PID together with its kernel start time. A recycled PID has a different
/// start time, so a stale ticket is never mistaken for a live one.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) start_ticks: u64,
}

impl ProcessIdentity {
    pub(crate) fn current() -> crate::Result<Self> {
        Self::of(std::process::id()).ok_or_else(|| "cannot read this process's start time".into())
    }

    pub(crate) fn of(pid: u32) -> Option<Self> {
        Some(Self {
            pid,
            start_ticks: oer_process::proc::start_ticks(pid)?,
        })
    }

    pub(crate) fn alive(&self) -> bool {
        Self::of(self.pid).is_some_and(|current| current == *self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_is_alive_until_it_exits() {
        let current = ProcessIdentity::current().unwrap();
        assert!(current.alive());
        let recycled = ProcessIdentity {
            start_ticks: current.start_ticks + 1,
            ..current
        };
        assert!(!recycled.alive());
        let mut child = oer_process::command("true").spawn().unwrap();
        let identity = ProcessIdentity::of(child.id());
        child.wait().unwrap();
        assert!(identity.is_none_or(|identity| !identity.alive()));
    }
}
