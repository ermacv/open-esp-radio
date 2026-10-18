//! Stopping another owner's lease on request.
//!
//! `cargo stand preempt ID --reason TEXT` marks the holder of lease `ID` as
//! preempted in the shared state, which stops charging its owner from that
//! moment, then sends its process `SIGTERM`: the same ordinary cancellation
//! and fixture cleanup as the hard limit. A holder still present after the
//! shutdown grace receives `SIGKILL`. The lease's history record names who
//! preempted it and why, the holder prints that when it releases, and the
//! user is notified.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{Arbiter, process::ProcessIdentity, state::Holder};

/// Who stopped a lease, why and when.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Preemption {
    pub by: String,
    pub reason: String,
    pub unix_ms: u64,
    /// Fields a newer build wrote, kept when this build rewrites the record.
    #[serde(flatten)]
    pub unknown: crate::Unknown,
}

/// How a preempted lease ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreemptEnd {
    /// Its holder released it after `SIGTERM`.
    Released,
    /// Its holder was still present after the grace and received `SIGKILL`.
    Killed,
}

/// The time to charge `holder`'s lease: up to its preemption, when it was
/// preempted, else `held_ms`.
pub(crate) fn charged_ms(holder: &Holder, held_ms: u64) -> u64 {
    holder.preempted.as_ref().map_or(held_ms, |preemption| {
        preemption
            .unix_ms
            .saturating_sub(holder.granted_unix.saturating_mul(1000))
            .min(held_ms)
    })
}

impl Arbiter {
    /// Mark lease `id` preempted by `by` for `reason` and return its owner,
    /// work and process. Only a held lease can be preempted.
    fn mark_preempted(
        &self,
        id: u64,
        by: &str,
        reason: &str,
    ) -> crate::Result<(String, String, ProcessIdentity)> {
        self.transaction(|state| {
            let Some(holder) = state
                .holders
                .iter_mut()
                .find(|holder| holder.ticket.id == id)
            else {
                return Err(if state.queue.iter().any(|ticket| ticket.id == id) {
                    format!("lease #{id} is waiting, not held; nothing to preempt")
                } else {
                    format!("no lease #{id} is held")
                }
                .into());
            };
            // A repeated preemption keeps the first, which charging stopped at.
            holder.preempted.get_or_insert_with(|| Preemption {
                by: by.to_owned(),
                reason: reason.to_owned(),
                unix_ms: oer_durable::unix_millis(),
                unknown: crate::Unknown::default(),
            });
            Ok((
                holder.ticket.owner.clone(),
                holder.ticket.work.clone(),
                holder.ticket.process,
            ))
        })
    }

    fn holds(&self, id: u64) -> crate::Result<bool> {
        self.transaction(|state| Ok(state.holders.iter().any(|holder| holder.ticket.id == id)))
    }

    /// Preempt lease `id` for `by` with `reason`: stop charging it, send its
    /// holder `SIGTERM`, and `SIGKILL` once `grace` passed without release.
    pub fn preempt(
        &self,
        id: u64,
        by: &str,
        reason: &str,
        grace: Duration,
    ) -> crate::Result<PreemptEnd> {
        if reason.trim().is_empty() {
            return Err("a preemption needs a --reason".into());
        }
        let (owner, work, process) = self.mark_preempted(id, by, reason)?;
        crate::notify::send(
            "HIL stand lease preempted",
            &format!("{by} preempted {owner}'s `{work}`: {reason}"),
        );
        eprintln!("hil-arbiter: preempting lease #{id} of {owner} (`{work}`): {reason}");
        signal(process, rustix::process::Signal::TERM);
        let started = Instant::now();
        while started.elapsed() < grace {
            if !self.holds(id)? {
                return Ok(PreemptEnd::Released);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        if !self.holds(id)? {
            return Ok(PreemptEnd::Released);
        }
        signal(process, rustix::process::Signal::KILL);
        // The next transaction reaps the killed holder into the history.
        let _ = self.holds(id);
        Ok(PreemptEnd::Killed)
    }
}

/// Signal `process` if it is still the process that took the lease.
fn signal(process: ProcessIdentity, signal: rustix::process::Signal) {
    if process.alive()
        && let Some(pid) = rustix::process::Pid::from_raw(process.pid as i32)
    {
        let _ = rustix::process::kill_process(pid, signal);
    }
}

#[cfg(test)]
mod tests;
