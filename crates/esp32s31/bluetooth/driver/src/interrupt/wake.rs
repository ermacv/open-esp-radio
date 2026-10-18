//! Lock-free handoff from the bounded Bluetooth hard handler to one worker.
//!
//! The recovered deferred-work path owns one static scheduler event. Repeated
//! queue insertion is coalesced, while its one-bit marker remains sticky until
//! the worker dequeues the event. This cell preserves exactly that contract
//! without an RTOS queue, allocation, or a callback-list ABI.
//!
//! Waker storage is deliberately outside this type. A platform integration
//! must register its worker waker before rechecking
//! [`SchedulerWakeCell::take`] and must deliver every
//! [`SchedulerWakePublication::WakeWorker`] result.

#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU8, Ordering};

use crate::interrupt::SchedulerWorkerWakeClass;

const PENDING: u8 = 1 << 0;
const MARKED: u8 = 1 << 1;

/// Result of publishing one classified scheduler wake from interrupt context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerWakePublication {
    /// This publication created a fresh pending epoch; wake the sole worker.
    WakeWorker,
    /// A pending epoch already exists and now covers this publication.
    Coalesced,
}

/// One coalesced scheduler-work batch consumed by the sole worker.
///
/// The batch is affine because one dequeued hardware-work notification may
/// authorize at most one fresh finished-list transfer:
///
/// ```compile_fail
/// use oer_esp32s31_bluetooth::interrupt::SchedulerWakeBatch;
///
/// fn replay(batch: SchedulerWakeBatch) {
///     consume(batch);
///     consume(batch);
/// }
///
/// fn consume(_batch: SchedulerWakeBatch) {}
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct SchedulerWakeBatch {
    marked: bool,
}

impl SchedulerWakeBatch {
    /// Whether any publication in this coalesced batch carried the marker.
    pub const fn is_marked(&self) -> bool {
        self.marked
    }
}

/// Atomic pending/marker state shared by the hard handler and async worker.
///
/// There is intentionally no count. The reference worker drains scheduler
/// state rather than executing once per interrupt, and the public OSAL drops
/// duplicate insertion of its same static event. The marker is accumulated by
/// OR, so an ordinary publication can never clear an earlier marked one.
pub struct SchedulerWakeCell {
    state: AtomicU8,
}

impl SchedulerWakeCell {
    /// Construct an empty handoff cell.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
        }
    }

    /// Publish one scheduler-work classification from the hard handler.
    ///
    /// This method is finite, lock-free, allocation-free, and performs no
    /// MMIO. The caller remains responsible for waking the worker when the
    /// return value is [`SchedulerWakePublication::WakeWorker`].
    pub fn publish_from_interrupt(
        &self,
        class: SchedulerWorkerWakeClass,
    ) -> SchedulerWakePublication {
        let publication = PENDING
            | match class {
                SchedulerWorkerWakeClass::Ordinary => 0,
                SchedulerWorkerWakeClass::Marked => MARKED,
            };
        let previous = self.state.fetch_or(publication, Ordering::AcqRel);

        if previous & PENDING == 0 {
            SchedulerWakePublication::WakeWorker
        } else {
            SchedulerWakePublication::Coalesced
        }
    }

    /// Atomically dequeue the current coalesced batch.
    ///
    /// A publication racing after the swap creates a distinct pending epoch
    /// and returns `WakeWorker`, so it cannot be consumed by this batch without
    /// an accompanying wake edge.
    pub fn take(&self) -> Option<SchedulerWakeBatch> {
        let state = self.state.swap(0, Ordering::AcqRel);
        if state & PENDING == 0 {
            None
        } else {
            Some(SchedulerWakeBatch {
                marked: state & MARKED != 0,
            })
        }
    }

    /// Whether a worker batch is currently pending.
    pub fn is_pending(&self) -> bool {
        self.state.load(Ordering::Acquire) & PENDING != 0
    }
}

impl Default for SchedulerWakeCell {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
