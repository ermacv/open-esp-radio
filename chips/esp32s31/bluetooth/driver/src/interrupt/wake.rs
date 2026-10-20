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

use crate::interrupt::SchedulerWorkerWakeClass;

const PENDING: u32 = 1 << 0;
const MARKED: u32 = 1 << 1;

/// Result of publishing one classified scheduler wake from interrupt context.
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

    fn from_facts(facts: u32) -> Option<Self> {
        (facts & PENDING != 0).then_some(Self {
            marked: facts & MARKED != 0,
        })
    }
}

/// Pending/marker facts shared by the hard handler and async worker.
///
/// There is intentionally no count. The reference worker drains scheduler
/// state rather than executing once per interrupt, and the public OSAL drops
/// duplicate insertion of its same static event. The marker is accumulated by
/// OR, so an ordinary publication can never clear an earlier marked one.
pub struct SchedulerWakeCell<X: SignalExecutor> {
    signal: &'static IrqSignal<X>,
}

impl<X: SignalExecutor> SchedulerWakeCell<X> {
    /// The empty handoff of `signal`, the image's scheduler signal of the
    /// worker's executor.
    pub const fn new(signal: &'static IrqSignal<X>) -> Self {
        Self { signal }
    }

    /// Publish one scheduler-work classification from the hard handler,
    /// after its register work.
    ///
    /// This method is finite, lock-free and allocation-free; its only MMIO
    /// is the executor's pend when the publication opens a fresh epoch.
    pub fn publish_from_interrupt(
        &self,
        class: SchedulerWorkerWakeClass,
    ) -> SchedulerWakePublication {
        let publication = PENDING
            | match class {
                SchedulerWorkerWakeClass::Ordinary => 0,
                SchedulerWorkerWakeClass::Marked => MARKED,
            };
        let raised = self.signal.raise_deferred(publication);
        // A marker joining an unserviced epoch pends once more; harmless.
        if let Some(pend) = raised.pend {
            pend.pend();
        }
        if raised.previous & PENDING == 0 {
            SchedulerWakePublication::Pended
        } else {
            SchedulerWakePublication::Coalesced
        }
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
    pub fn take(&self) -> Result<Option<SchedulerWakeBatch>, ForeignContext> {
        self.signal.take().map(SchedulerWakeBatch::from_facts)
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
    ) -> Poll<Result<SchedulerWakeBatch, ForeignContext>> {
        loop {
            match self.signal.poll_take(cx) {
                Poll::Ready(Ok(facts)) => {
                    if let Some(batch) = SchedulerWakeBatch::from_facts(facts) {
                        return Poll::Ready(Ok(batch));
                    }
                    // Every publication carries PENDING; no other fact
                    // exists, so this word was empty of work.
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
    pub async fn wait(&self) -> Result<SchedulerWakeBatch, ForeignContext> {
        poll_fn(|cx| self.poll_take(cx)).await
    }
}

#[cfg(test)]
mod tests;
