//! Process identities that survive PID reuse.

use serde::{Deserialize, Serialize};

/// A PID together with its kernel start time. A recycled PID has a different
/// start time, so a stale ticket is never mistaken for a live one.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_ticks: u64,
}

impl ProcessIdentity {
    pub fn of(pid: u32) -> Option<Self> {
        Some(Self {
            pid,
            start_ticks: oer_os_process::proc::start_ticks(pid)?,
        })
    }
}
