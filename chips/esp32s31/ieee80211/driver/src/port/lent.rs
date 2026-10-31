//! Receive buffers lent to a network owner.
//!
//! A lend moves one received unit out of the installation's receive count to
//! the network: the network's count rises first, then the P2 driver takes
//! the unit off its own count. Retirement therefore never sees a unit in
//! neither count. Dropping the network owner, in a task, returns the unit's
//! memory to its pool, lowers the network's count and wakes the driver to
//! rearm what the ring got back and the task waiting for the network.

use super::{ReceiveBuffer, ReceiveUnit};
use atomic_waker::AtomicWaker;
use core::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::{Context, Poll},
};
use oer_ieee80211_contracts_port::{LendRefusal, LendRefused, LentRxBuffer};
use oer_radio_port::SlotIdentity;
use oer_sync::PriorityWaker;

/// The cross-context count of the receive units the network holds.
pub(super) struct NetworkCredit {
    /// Units lent and not yet dropped.
    held: AtomicUsize,
    /// Lends the driver has not yet taken off its receive count.
    transferred: AtomicUsize,
    /// A lent unit was dropped since the driver last looked.
    returned: AtomicBool,
    /// The task that waits for the network's units. Their owners drop in
    /// tasks of the network's executor, never in an interrupt handler.
    network: AtomicWaker,
}

impl NetworkCredit {
    pub(super) const fn new() -> Self {
        Self {
            held: AtomicUsize::new(0),
            transferred: AtomicUsize::new(0),
            returned: AtomicBool::new(false),
            network: AtomicWaker::new(),
        }
    }

    /// Units the network holds.
    pub(super) fn held(&self) -> usize {
        self.held.load(Ordering::Acquire)
    }

    /// Lends since the last call, for the driver's receive count.
    pub(super) fn take_transferred(&self) -> usize {
        self.transferred.swap(0, Ordering::AcqRel)
    }

    /// Whether a lent unit was dropped since the last call.
    pub(super) fn take_returned(&self) -> bool {
        self.returned.swap(false, Ordering::AcqRel)
    }

    /// Ready once the network holds no unit.
    pub(super) fn poll_returned(&self, cx: &mut Context<'_>) -> Poll<()> {
        self.network.register(cx.waker());
        if self.held() == 0 {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    /// Start a new installation's count. The caller has seen the previous
    /// installation return every unit.
    pub(super) fn reset(&self) {
        self.transferred.store(0, Ordering::Release);
        self.returned.store(false, Ordering::Release);
    }
}

/// One received unit the network owns ([`LentRxBuffer`]).
pub struct LentReceive<R: 'static> {
    unit: Option<R>,
    start: usize,
    len: usize,
    credit: &'static NetworkCredit,
    driver: &'static PriorityWaker,
}

impl<R: ReceiveUnit + Send> LentRxBuffer for LentReceive<R> {
    fn storage(&self) -> &[u8] {
        self.unit
            .as_ref()
            .and_then(|unit| unit.storage().get(self.start..))
            .unwrap_or_default()
    }
    fn storage_mut(&mut self) -> &mut [u8] {
        let start = self.start;
        self.unit
            .as_mut()
            .and_then(|unit| unit.storage_mut().get_mut(start..))
            .unwrap_or_default()
    }
    fn mpdu_len(&self) -> usize {
        self.len
    }
}

impl<R: 'static> Drop for LentReceive<R> {
    fn drop(&mut self) {
        // The unit's own drop returns its memory to the pool before the
        // count falls, so a zero count means every unit came back.
        drop(self.unit.take());
        self.credit.held.fetch_sub(1, Ordering::AcqRel);
        self.credit.returned.store(true, Ordering::Release);
        self.credit.network.wake();
        self.driver.wake();
    }
}

impl<Id: SlotIdentity, R: ReceiveUnit + Send> ReceiveBuffer<'_, Id, R> {
    /// Move this unit to the network, at most `limit` units at once.
    pub(super) fn lend_to(
        mut self,
        credit: &'static NetworkCredit,
        limit: usize,
        stopping: bool,
    ) -> Result<LentReceive<R>, LendRefused<Self>> {
        let refused = if stopping {
            Some(LendRefusal::Stopping)
        } else if credit.held() >= limit {
            Some(LendRefusal::Capacity)
        } else if self
            .unit
            .as_ref()
            .is_none_or(|unit| self.range.end > unit.storage().len())
        {
            // The MPDU this buffer lends lies outside the unit's storage.
            Some(LendRefusal::Unsupported)
        } else {
            None
        };
        if let Some(reason) = refused {
            return Err(LendRefused {
                buffer: self,
                reason,
            });
        }
        // Credit first: the network counts the unit before the driver stops.
        credit.held.fetch_add(1, Ordering::AcqRel);
        credit.transferred.fetch_add(1, Ordering::AcqRel);
        let lent = LentReceive {
            unit: self.unit.take(),
            start: self.range.start,
            len: self.range.len(),
            credit,
            driver: self.driver,
        };
        self.driver.wake();
        Ok(lent)
    }
}
