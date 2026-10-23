//! Lock-free handoff from the bounded Bluetooth hard handler to one worker.
//!
//! The recovered deferred-work path owns one static scheduler event. Repeated
//! queue insertion is coalesced, while its one-bit marker remains sticky until
//! the worker dequeues the event. This cell preserves exactly that contract
//! without an RTOS queue, allocation, or a callback-list ABI.
//!
//! The facts travel in the image's static [`IrqSignal`] of the worker's
//! interrupt-mode executor: the handler ORs them in and pends that executor,
//! never a waker; the executor wakes the worker in its own context. A worker
//! polled outside that executor reports [`ForeignContext`].

use core::{
    future::poll_fn,
    task::{Context, Poll},
};

use oer_interrupt_table::signal::{ForeignContext, IrqSignal, SignalExecutor};

use super::PrimaryInterruptCapture;

// These are software latch keys, not register fields. A bit records one
// correlated set of source inputs and its own later deferred-work sample.
const MARKED_INPUTS: u32 = {
    let mut key = 0;
    let mut mask = 0;
    while key < 16 {
        if key & 2 != 0 && key & 8 != 0 {
            mask |= 1 << key;
        }
        key += 1;
    }
    mask
};

/// Readiness disposition after latching captured primary inputs in P3.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerWakePublication {
    /// No earlier publication waited for the executor to service it: this
    /// one pended the worker's executor.
    Pended,
    /// An unserviced publication already pended the executor and now
    /// covers this one.
    Coalesced,
}

/// One coalesced scheduler-work batch consumed by the sole worker.
///
/// The batch is affine because one dequeued hardware-work notification may
/// authorize at most one fresh finished-list transfer:
///
/// ```compile_fail
/// use oer_esp32s31_bluetooth_driver::interrupt::SchedulerWakeBatch;
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

/// Positional primary faults retained until P2 chooses fail-stop handling.
/// Multiple acknowledged epochs may contribute source presence to one batch.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "retain the captured fault until P2's fail-stop handling"]
pub struct PrimaryWakeFault {
    facts: u32,
}
impl PrimaryWakeFault {
    /// Captured bank-zero source 15.
    pub const fn bank_0_source_15_pending(&self) -> bool {
        self.facts & (1 << 16) != 0
    }
    /// Captured bank-one source 8.
    pub const fn bank_1_source_8_pending(&self) -> bool {
        self.facts & (1 << 17) != 0
    }
    /// Captured bank-one source 9.
    pub const fn bank_1_source_9_pending(&self) -> bool {
        self.facts & (1 << 18) != 0
    }
    /// Captured bank-one source 12.
    pub const fn bank_1_source_12_pending(&self) -> bool {
        self.facts & (1 << 19) != 0
    }
    /// A source without reviewed handling was captured.
    pub const fn unclassified_pending(&self) -> bool {
        self.facts & (1 << 20) != 0
    }
    /// A bounded reference-gate sample did not settle.
    pub const fn diagnostic_unsettled(&self) -> bool {
        self.facts & PrimaryInterruptCapture::UNSETTLED != 0
    }
}

/// Affine inputs dequeued only in P2's signal-executor context. Classification
/// cannot be called on an IRQ-produced capture or a copied readiness bit.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "classify the captured inputs before admitting scheduler work"]
pub struct CapturedWakeBatch {
    facts: u32,
}
impl CapturedWakeBatch {
    fn from_facts(facts: u32) -> Option<Self> {
        (facts != 0).then_some(Self { facts })
    }
    /// Classify on P2. Faults preempt ordinary work without creating a
    /// finished-list authority; all fault source presence remains retained.
    #[inline(never)]
    pub fn classify(self) -> Result<SchedulerWakeBatch, PrimaryWakeFault> {
        if self.facts & (PrimaryInterruptCapture::FAULT_MASK | PrimaryInterruptCapture::UNSETTLED)
            != 0
        {
            Err(PrimaryWakeFault { facts: self.facts })
        } else {
            Ok(SchedulerWakeBatch {
                marked: self.facts & MARKED_INPUTS != 0,
            })
        }
    }
}

/// Captured positional inputs shared by the hard handler and P2 worker.
///
/// There is intentionally no count. The reference worker drains scheduler
/// state rather than executing once per interrupt, and the public OSAL drops
/// duplicate insertion of its same static event. One-hot input keys preserve
/// source/sample pairs across OR; P2 computes the sticky marker after dequeue.
pub struct SchedulerWakeCell<X: SignalExecutor> {
    signal: &'static IrqSignal<X>,
}

impl<X: SignalExecutor> SchedulerWakeCell<X> {
    /// The empty handoff of `signal`, the image's scheduler signal of the
    /// worker's executor.
    pub const fn new(signal: &'static IrqSignal<X>) -> Self {
        Self { signal }
    }

    /// Latch positional inputs in P3 and pend P2. Empty epochs publish
    /// nothing. The keys preserve each source/sample correlation across OR,
    /// while the worker owns classification, fault handling and accounting.
    pub fn publish_from_interrupt(
        &self,
        capture: PrimaryInterruptCapture,
    ) -> Option<SchedulerWakePublication> {
        if capture.0 == 0 {
            None
        } else {
            Some(self.latch(capture.0))
        }
    }

    fn latch(&self, facts: u32) -> SchedulerWakePublication {
        let raised = self.signal.raise_deferred(facts);
        if let Some(pend) = raised.pend {
            pend.pend();
        }
        if raised.previous == 0 {
            SchedulerWakePublication::Pended
        } else {
            SchedulerWakePublication::Coalesced
        }
    }

    #[cfg(test)]
    pub(crate) fn publish_class_for_test(
        &self,
        class: crate::interrupt::SchedulerWorkerWakeClass,
    ) -> SchedulerWakePublication {
        use crate::interrupt::SchedulerWorkerWakeClass;
        self.latch(match class {
            SchedulerWorkerWakeClass::Ordinary => 1 << 1,
            SchedulerWorkerWakeClass::Marked => 1 << 10,
        })
    }

    /// Dequeue the current coalesced batch.
    ///
    /// A publication racing after the take creates a distinct pending epoch
    /// and pends the executor, so it cannot be consumed by this batch
    /// without an accompanying wake edge.
    ///
    /// # Errors
    ///
    /// The caller runs outside the signal's executor; the batch stays.
    pub fn take(&self) -> Result<Option<CapturedWakeBatch>, ForeignContext> {
        self.signal.take().map(CapturedWakeBatch::from_facts)
    }

    /// Whether a worker batch is currently pending.
    ///
    /// # Errors
    ///
    /// The caller runs outside the signal's executor.
    pub fn is_pending(&self) -> Result<bool, ForeignContext> {
        self.signal.has_pending()
    }

    /// Register the worker's waker, then dequeue the pending batch; pending
    /// while there is none.
    ///
    /// # Errors
    ///
    /// The caller runs outside the signal's executor.
    pub fn poll_take(
        &self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<CapturedWakeBatch, ForeignContext>> {
        loop {
            match self.signal.poll_take(cx) {
                Poll::Ready(Ok(facts)) => {
                    if let Some(batch) = CapturedWakeBatch::from_facts(facts) {
                        return Poll::Ready(Ok(batch));
                    }
                    // An empty signal epoch grants no captured work.
                }
                Poll::Ready(Err(foreign)) => return Poll::Ready(Err(foreign)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    /// Wait for the next batch.
    ///
    /// # Errors
    ///
    /// As [`Self::poll_take`].
    pub async fn wait(&self) -> Result<CapturedWakeBatch, ForeignContext> {
        poll_fn(|cx| self.poll_take(cx)).await
    }
}

#[cfg(test)]
mod tests;
