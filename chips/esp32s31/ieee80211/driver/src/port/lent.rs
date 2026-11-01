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
use oer_memory::QueueNotify;
use oer_radio_port::SlotIdentity;
use oer_sync::PriorityWaker;

/// How a lent unit's return tells the task waiting for every return (the
/// station's stop) and the driver when it asked for its units. Every call
/// is direct: the composition names the type.
pub trait LentNotifies: Sync + 'static {
    /// Make the polling task the one a return tells; it checks the count
    /// next.
    fn register_returned(&self, cx: &mut Context<'_>);
    /// Tell the waiting task that a unit returned.
    fn returned(&self);
    /// Make the driver's task the one a return it asked for tells.
    fn register_driver(&self, cx: &mut Context<'_>);
    /// Tell the driver, whose own waker is `local`.
    fn wake_driver(&self, local: &PriorityWaker);
}

/// The network on the driver's hart: the waiting task's waker, and the
/// driver's own waker, which its poll registers.
pub struct SameHart {
    network: AtomicWaker,
}

impl SameHart {
    pub const fn new() -> Self {
        Self {
            network: AtomicWaker::new(),
        }
    }
}

impl Default for SameHart {
    fn default() -> Self {
        Self::new()
    }
}

impl LentNotifies for SameHart {
    fn register_returned(&self, cx: &mut Context<'_>) {
        self.network.register(cx.waker());
    }
    fn returned(&self) {
        self.network.wake();
    }
    fn register_driver(&self, _: &mut Context<'_>) {}
    fn wake_driver(&self, local: &PriorityWaker) {
        local.wake();
    }
}

/// The network on another hart: a return raises the driver's and the
/// waiting task's executor signals instead of a waker (no waker crosses
/// harts).
#[derive(Clone, Copy)]
pub struct NetworkPeers<D, R> {
    /// The driver's task.
    pub driver: D,
    /// The task that waits for every unit's return (the station's stop).
    pub returned: R,
}

impl<D: QueueNotify + 'static, R: QueueNotify + 'static> LentNotifies for NetworkPeers<D, R> {
    fn register_returned(&self, cx: &mut Context<'_>) {
        self.returned.register(cx);
    }
    fn returned(&self) {
        self.returned.notify();
    }
    fn register_driver(&self, cx: &mut Context<'_>) {
        self.driver.register(cx);
    }
    fn wake_driver(&self, _: &PriorityWaker) {
        self.driver.notify();
    }
}

/// The cross-context count of the receive units the network holds.
pub(super) struct NetworkCredit<P = SameHart> {
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
    /// How a return tells the waiting task and the driver. The units'
    /// owners drop in tasks of the network's executor, never in an
    /// interrupt handler.
    notifies: P,
}

impl<P: LentNotifies> NetworkCredit<P> {
    /// The count of a network whose returns tell through `notifies`.
    pub(super) const fn with(notifies: P) -> Self {
        Self {
            held: AtomicUsize::new(0),
            transferred: AtomicUsize::new(0),
            returned: AtomicBool::new(false),
            wants_room: AtomicBool::new(false),
            wants_units: AtomicBool::new(false),
            notifies,
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
        self.notifies.register_returned(cx);
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
    ///
    /// `cx` is the driver's: a network on another hart wakes it through the
    /// driver's executor signal ([`NetworkPeers::driver`]), which the driver
    /// registers here, before it asks.
    pub(super) fn want_units(&self, cx: &mut Context<'_>) -> bool {
        self.notifies.register_driver(cx);
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
pub struct LentReceive<R: 'static, P: LentNotifies = SameHart> {
    /// The unit, its view the MPDU. It drops before `returned`, so its
    /// memory is back in its pool before the count falls.
    unit: R,
    returned: ReturnOnDrop<P>,
}

/// Lowers the network's count and wakes both waiters when its lent unit
/// was dropped.
struct ReturnOnDrop<P: LentNotifies> {
    credit: &'static NetworkCredit<P>,
    driver: &'static PriorityWaker,
}

impl<P: LentNotifies> Drop for ReturnOnDrop<P> {
    fn drop(&mut self) {
        self.credit.held.fetch_sub(1, Ordering::AcqRel);
        // The driver takes the flag on its next poll, which an interrupt
        // brings: a return wakes it only when it asked, holding few units.
        self.credit.returned.store(true, Ordering::Release);
        let wants = self.credit.wants_units.swap(false, Ordering::AcqRel);
        self.credit.notifies.returned();
        if wants {
            self.credit.notifies.wake_driver(self.driver);
        }
    }
}

impl<R: ReceiveUnit + Send, P: LentNotifies> LentRxBuffer for LentReceive<R, P> {
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
    pub(super) fn lend_to<P: LentNotifies>(
        self,
        credit: &'static NetworkCredit<P>,
        limit: usize,
        stopping: bool,
    ) -> Result<LentReceive<R, P>, LendRefused<Self>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use core::task::Waker;
    use std::boxed::Box;

    /// A notify that counts its registrations and notifications.
    struct Counted {
        registered: AtomicUsize,
        notified: AtomicUsize,
    }

    impl Counted {
        const fn new() -> Self {
            Self {
                registered: AtomicUsize::new(0),
                notified: AtomicUsize::new(0),
            }
        }
        fn counts(&self) -> (usize, usize) {
            (
                self.registered.load(Ordering::Relaxed),
                self.notified.load(Ordering::Relaxed),
            )
        }
    }

    impl oer_memory::QueueNotify for Counted {
        fn register(&self, _: &mut Context<'_>) {
            self.registered.fetch_add(1, Ordering::Relaxed);
        }
        fn notify(&self) {
            self.notified.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn leaked<T>(value: T) -> &'static T {
        Box::leak(Box::new(value))
    }

    /// One unit's return, as the network's drop of its lent unit makes it.
    fn return_one<P: LentNotifies>(credit: &'static NetworkCredit<P>) {
        credit.held.fetch_add(1, Ordering::AcqRel);
        drop(ReturnOnDrop {
            credit,
            driver: leaked(PriorityWaker::new()),
        });
    }

    #[test]
    fn a_driver_asking_for_units_registers_its_signal_which_a_return_on_another_hart_raises() {
        let driver = leaked(Counted::new());
        let returned = leaked(Counted::new());
        let credit = leaked(NetworkCredit::with(NetworkPeers { driver, returned }));
        let mut cx = Context::from_waker(Waker::noop());

        // A return the driver did not ask for raises only the waiting task.
        return_one(credit);
        assert_eq!((driver.counts(), returned.counts()), ((0, 0), (0, 1)));
        assert!(credit.take_returned());

        // Asking registers the driver's signal before the request is
        // published, so the next return's raise reaches the driver's task.
        assert!(!credit.want_units(&mut cx), "no return since");
        assert_eq!(driver.counts(), (1, 0));
        return_one(credit);
        assert_eq!((driver.counts(), returned.counts()), ((1, 1), (0, 2)));
        // The request is spent: a further return leaves the driver alone.
        return_one(credit);
        assert_eq!(driver.counts(), (1, 1));
    }

    #[test]
    fn a_return_before_the_request_is_reported_to_the_asking_driver() {
        let driver = leaked(Counted::new());
        let returned = leaked(Counted::new());
        let credit = leaked(NetworkCredit::with(NetworkPeers { driver, returned }));
        let mut cx = Context::from_waker(Waker::noop());
        return_one(credit);
        assert!(credit.want_units(&mut cx), "the driver polls again itself");
        assert_eq!(driver.counts(), (1, 0));
    }
}
