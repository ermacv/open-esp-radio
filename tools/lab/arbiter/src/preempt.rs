//! Stopping another owner's lease on request.
//!
//! `cargo stand preempt ID --reason TEXT` marks the holder of lease `ID` as
//! preempted, which stops charging its owner from that moment; the service
//! then stops the lease's operations and tells its client why. The lease's
//! history record names who preempted it and why.

use serde::{Deserialize, Serialize};

use crate::state::Holder;

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
