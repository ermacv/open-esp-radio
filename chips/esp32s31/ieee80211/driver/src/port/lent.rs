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
use oer_ieee80211_contracts_port::{AdvanceRefused, LendRefusal, LendRefused, LentRxBuffer};
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
    /// The driver ran out of receive room: the next lend wakes it.
    wants_room: AtomicBool,
    /// The network holds many units: the next return wakes the driver.
    wants_units: AtomicBool,
    /// The task that waits for the network's units. Their owners drop in
    /// tasks of the network's executor, never in an interrupt handler.
    network: AtomicWaker,
    /// When the network runs on another hart: the driver's and the waiting
    /// task's executor signals, which a return raises instead of a waker
    /// (no waker crosses harts).
    peers: Option<NetworkPeers>,
}

/// The executor signals a return on another hart raises.
#[derive(Clone, Copy)]
pub struct NetworkPeers {
    /// The driver's task.
    pub driver: &'static dyn oer_memory::QueueNotify,
    /// The task that waits for every unit's return (the station's stop).
    pub returned: &'static dyn oer_memory::QueueNotify,
}

impl NetworkCredit {
    pub(super) const fn new() -> Self {
        Self {
            held: AtomicUsize::new(0),
            transferred: AtomicUsize::new(0),
            returned: AtomicBool::new(false),
            wants_room: AtomicBool::new(false),
            wants_units: AtomicBool::new(false),
            network: AtomicWaker::new(),
            peers: None,
        }
    }

    /// The count of a network on another hart, whose returns raise `peers`.
    pub(super) const fn across_harts(peers: NetworkPeers) -> Self {
        let mut credit = Self::new();
        credit.peers = Some(peers);
        credit
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
        match self.peers {
            Some(peers) => peers.returned.register(cx),
            None => self.network.register(cx.waker()),
        }
        if self.held() == 0 {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    /// Ask the next lend to wake the driver, which has no receive room.
    /// Whether a lend came first: the driver then polls again itself.
    pub(super) fn want_room(&self) -> bool {
        self.wants_room.store(true, Ordering::Release);
        self.transferred.load(Ordering::Acquire) != 0
    }

    /// Ask the next return to wake the driver, while the network holds many
    /// units. Whether a return came first: the driver then polls again.
    pub(super) fn want_units(&self) -> bool {
        self.wants_units.store(true, Ordering::Release);
        self.returned.load(Ordering::Acquire)
    }

    /// Start a new installation's count. The caller has seen the previous
    /// installation return every unit.
    pub(super) fn reset(&self) {
        self.transferred.store(0, Ordering::Release);
        self.returned.store(false, Ordering::Release);
        self.wants_room.store(false, Ordering::Release);
        self.wants_units.store(false, Ordering::Release);
    }
}

/// One received unit the network owns ([`LentRxBuffer`]).
pub struct LentReceive<R: 'static> {
    /// The unit, its view the MPDU. It drops before `returned`, so its
    /// memory is back in its pool before the count falls.
    unit: R,
    returned: ReturnOnDrop,
}

/// Lowers the network's count and wakes both waiters when its lent unit
/// was dropped.
struct ReturnOnDrop {
    credit: &'static NetworkCredit,
    driver: &'static PriorityWaker,
}

impl Drop for ReturnOnDrop {
    fn drop(&mut self) {
        self.credit.held.fetch_sub(1, Ordering::AcqRel);
        // The driver takes the flag on its next poll, which an interrupt
        // brings: a return wakes it only when it asked, holding few units.
        self.credit.returned.store(true, Ordering::Release);
        let wants = self.credit.wants_units.swap(false, Ordering::AcqRel);
        match self.credit.peers {
            Some(peers) => {
                peers.returned.notify();
                if wants {
                    peers.driver.notify();
                }
            }
            None => {
                self.credit.network.wake();
                if wants {
                    self.driver.wake();
                }
            }
        }
    }
}

impl<R: ReceiveUnit + Send> LentRxBuffer for LentReceive<R> {
    fn storage(&self) -> &[u8] {
        self.unit.storage()
    }
    fn storage_mut(&mut self) -> &mut [u8] {
        self.unit.storage_mut()
    }
    fn mpdu_len(&self) -> usize {
        self.unit.bytes().len()
    }
    fn advance(&mut self, by: usize) -> Result<(), AdvanceRefused> {
        let mpdu_len = self.mpdu_len();
        let refused = AdvanceRefused { by, mpdu_len };
        let rest = mpdu_len.checked_sub(by).ok_or(refused)?;
        self.unit.narrow(by, rest).map_err(|_| refused)
    }
}

impl<Id: SlotIdentity, R: ReceiveUnit + Send> ReceiveBuffer<'_, Id, R> {
    /// Move this unit to the network, at most `limit` units at once.
    pub(super) fn lend_to(
        self,
        credit: &'static NetworkCredit,
        limit: usize,
        stopping: bool,
    ) -> Result<LentReceive<R>, LendRefused<Self>> {
        let refused = if stopping {
            Some(LendRefusal::Stopping)
        } else if credit.held() >= limit {
            Some(LendRefusal::Capacity)
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
        // The driver takes the count on its next poll: a lend wakes it only
        // when it asked, out of receive room.
        credit.transferred.fetch_add(1, Ordering::AcqRel);
        let (unit, driver) = self.into_unit();
        if credit.wants_room.swap(false, Ordering::AcqRel) {
            driver.wake();
        }
        Ok(LentReceive {
            unit,
            returned: ReturnOnDrop { credit, driver },
        })
    }
}
