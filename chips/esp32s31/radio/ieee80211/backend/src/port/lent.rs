//! Receive buffers lent to a network owner.
//!
//! A lend moves one received unit out of the installation's receive count to
//! the network: the network's count rises first, then the port takes the
//! unit off its own count. Retirement therefore never sees a unit in
//! neither count. Dropping the network owner, in a task, returns the unit's
//! memory to its pool, lowers the network's count and tells the task
//! waiting for returns, whose port rearms what the ring got back.

use super::{ReceiveBuffer, ReceiveUnit};
use core::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::{Context, Poll},
};
use oer_portable_memory::QueueNotify;
use oer_portable_waker::AtomicWaker;
use oer_protocols_ieee80211_contracts_port::{
    AdvanceRefused, LendRefusal, LendRefused, LentRxBuffer,
};
use oer_protocols_radio_port::SlotIdentity;

/// How a lent unit's return tells the task waiting for returns (the
/// datapath actor). Every call is direct: the composition names the type.
pub trait LentNotifies: Sync + 'static {
    /// Make the polling task the one a return tells; it checks the count
    /// next.
    fn register_returned(&self, cx: &mut Context<'_>);
    /// Tell the waiting task that a unit returned.
    fn returned(&self);
}

/// The network on the waiting task's hart: that task's waker.
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
    // A network that drops lent units on several paths (a refusal, a full
    // queue) can otherwise merge their inlined returns into one block whose
    // indirect wake keeps no Waker operation scope.
    #[inline(never)]
    fn returned(&self) {
        self.network.wake();
    }
}

/// The network on another hart: a return raises the waiting task's
/// executor signal instead of a waker (no waker crosses harts).
#[derive(Clone, Copy)]
pub struct NetworkPeers<R> {
    /// The task that waits for the units' returns.
    pub returned: R,
}

impl<R: QueueNotify + 'static> LentNotifies for NetworkPeers<R> {
    fn register_returned(&self, cx: &mut Context<'_>) {
        self.returned.register(cx);
    }
    fn returned(&self) {
        self.returned.notify();
    }
}

/// The cross-context count of the receive units the network holds.
pub(super) struct NetworkCredit<P = SameHart> {
    /// Units lent and not yet dropped.
    held: AtomicUsize,
    /// Lends not yet taken off the port's receive count.
    transferred: AtomicUsize,
    /// A lent unit was dropped since the port last looked.
    returned: AtomicBool,
    /// A return need not tell the waiting task: the ring's next service
    /// comes anyway, timed or with a frame, and rearms what came back.
    quiet: AtomicBool,
    /// How a return tells the waiting task. The units'
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
            quiet: AtomicBool::new(false),
            notifies,
        }
    }

    /// Units the network holds.
    pub(super) fn held(&self) -> usize {
        self.held.load(Ordering::Acquire)
    }

    /// Lends since the last call, for the port's receive count.
    #[cfg(test)]
    pub(super) fn take_transferred(&self) -> usize {
        self.transferred.swap(0, Ordering::AcqRel)
    }

    /// Lends not yet settled against the port's receive count.
    pub(super) fn transferred(&self) -> usize {
        self.transferred.load(Ordering::Acquire)
    }

    /// Settle `lends` the port's receive count just gave up; a lend counted
    /// since they were read stays for the next settlement.
    pub(super) fn settle_transferred(&self, lends: usize) {
        // `lends` were read from this count and only a settlement lowers
        // it, so it holds them: the subtraction does not wrap.
        self.transferred.fetch_sub(lends, Ordering::AcqRel);
    }

    /// Whether a lent unit was dropped since [`Self::take_returned`].
    pub(super) fn returned_unseen(&self) -> bool {
        self.returned.load(Ordering::Acquire)
    }

    /// Whether a lent unit was dropped since the last call.
    pub(super) fn take_returned(&self) -> bool {
        self.returned.swap(false, Ordering::AcqRel)
    }

    /// Whether the network returned a unit the ring has not rearmed yet;
    /// the next return tells the polling task unless `quiet`, when the
    /// ring's next service comes anyway. The port's facts ask: a return may
    /// be the only edge left when the walker ran out of armed descriptors.
    pub(super) fn poll_returns(&self, cx: &mut Context<'_>, quiet: bool) -> bool {
        self.quiet.store(quiet, Ordering::SeqCst);
        self.notifies.register_returned(cx);
        self.returned.load(Ordering::Acquire)
    }

    /// Ready once `ready` holds of the network's count; otherwise the next
    /// return tells the polling task. The count is read after the task
    /// asked to hear: a return either sees that or is in the count.
    fn poll_count(&self, cx: &mut Context<'_>, ready: impl Fn(usize) -> bool) -> Poll<()> {
        self.notifies.register_returned(cx);
        if ready(self.held()) {
            return Poll::Ready(());
        }
        self.quiet.store(false, Ordering::SeqCst);
        if ready(self.held.load(Ordering::SeqCst)) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    /// Ready once the network holds no unit.
    pub(super) fn poll_returned(&self, cx: &mut Context<'_>) -> Poll<()> {
        self.poll_count(cx, |held| held == 0)
    }

    /// Ready while the network holds fewer than `limit` units.
    pub(super) fn poll_room(&self, cx: &mut Context<'_>, limit: usize) -> Poll<()> {
        self.poll_count(cx, |held| held < limit)
    }

    /// Start a new installation's count. The lend credit outlives the
    /// installation: units the network still holds stay counted against
    /// its limit, and a return not yet rearmed stays reported, so each
    /// unit rejoins the ring of whichever installation runs when the
    /// network drops it. Only the previous installation's pending
    /// transfers (its receive count restarts at zero) end here.
    pub(super) fn reset(&self) {
        self.transferred.store(0, Ordering::Release);
    }
}

/// One received unit the network owns ([`LentRxBuffer`]).
pub struct LentReceive<R: 'static, P: LentNotifies = SameHart> {
    /// The unit, its view the MPDU. It drops before `_returned`, so its
    /// memory is back in its pool before the count falls.
    unit: R,
    _returned: ReturnOnDrop<P>,
}

/// Lowers the network's count and tells the waiting task when its lent
/// unit was dropped.
struct ReturnOnDrop<P: LentNotifies> {
    credit: &'static NetworkCredit<P>,
}

impl<P: LentNotifies> Drop for ReturnOnDrop<P> {
    fn drop(&mut self) {
        self.credit.held.fetch_sub(1, Ordering::SeqCst);
        // The port takes the flag at its next facts.
        self.credit.returned.store(true, Ordering::Release);
        if !self.credit.quiet.load(Ordering::SeqCst) {
            self.credit.notifies.returned();
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
        // Credit first: the network counts the unit before the port stops.
        credit.held.fetch_add(1, Ordering::AcqRel);
        // The caller lends in its own turn and the port takes the count at
        // its next pull, before it reads its receive room.
        credit.transferred.fetch_add(1, Ordering::AcqRel);
        Ok(LentReceive {
            unit: self.into_unit(),
            _returned: ReturnOnDrop { credit },
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

    impl oer_portable_memory::QueueNotify for Counted {
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
        drop(ReturnOnDrop { credit });
    }

    #[test]
    fn a_return_on_another_hart_raises_the_waiting_tasks_signal_and_the_polling_task_registers() {
        let returned = leaked(Counted::new());
        let credit = leaked(NetworkCredit::with(NetworkPeers { returned }));
        let mut cx = Context::from_waker(Waker::noop());

        // Polling registers the waiting task's signal before it reads.
        assert!(!credit.poll_returns(&mut cx, false), "nothing returned yet");
        assert_eq!(returned.counts(), (1, 0));
        // Each return raises it and stays reported until the port takes it.
        return_one(credit);
        return_one(credit);
        assert_eq!(returned.counts(), (1, 2));
        assert!(credit.poll_returns(&mut cx, false));
        assert!(credit.take_returned());
        assert!(!credit.take_returned(), "taken once");
    }

    #[test]
    fn a_return_stays_quiet_while_the_ring_waits_and_is_still_reported() {
        let returned = leaked(Counted::new());
        let credit = leaked(NetworkCredit::with(NetworkPeers { returned }));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(!credit.poll_returns(&mut cx, true));
        return_one(credit);
        assert_eq!(returned.counts().1, 0, "the ring's timed service takes it");
        assert!(credit.poll_returns(&mut cx, false), "the flag stays set");
        return_one(credit);
        assert_eq!(returned.counts().1, 1, "no timed service: the return tells");
    }

    #[test]
    fn a_task_at_the_lend_limit_hears_the_next_return_while_the_ring_waits() {
        let returned = leaked(Counted::new());
        let credit = leaked(NetworkCredit::with(NetworkPeers { returned }));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(!credit.poll_returns(&mut cx, true));
        credit.held.store(2, Ordering::SeqCst);
        assert!(credit.poll_room(&mut cx, 2).is_pending());
        drop(ReturnOnDrop { credit });
        assert_eq!(returned.counts().1, 1, "the waiting task hears it");
        assert!(credit.poll_room(&mut cx, 2).is_ready());
    }

    #[test]
    fn a_task_waiting_for_the_last_unit_hears_it_while_the_ring_waits() {
        let returned = leaked(Counted::new());
        let credit = leaked(NetworkCredit::with(NetworkPeers { returned }));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(!credit.poll_returns(&mut cx, true));
        credit.held.store(1, Ordering::SeqCst);
        assert!(credit.poll_returned(&mut cx).is_pending());
        drop(ReturnOnDrop { credit });
        assert_eq!(returned.counts().1, 1);
        assert!(credit.poll_returned(&mut cx).is_ready());
    }

    #[test]
    fn the_lend_credit_outlives_a_reinstall_and_late_returns_rejoin_the_new_ring() {
        let credit = leaked(NetworkCredit::with(SameHart::new()));
        // Two units lent by the old installation, one returned before the
        // reinstall and not yet rearmed.
        credit.held.fetch_add(2, Ordering::AcqRel);
        credit.transferred.fetch_add(2, Ordering::AcqRel);
        drop(ReturnOnDrop { credit });
        credit.reset();
        // The network still holds one: the new installation counts it
        // against its limit, and owes the ring the earlier return.
        assert_eq!(credit.held(), 1);
        assert_eq!(
            credit.take_transferred(),
            0,
            "the old receive count is gone"
        );
        assert!(credit.take_returned(), "the earlier return is rearmed now");
        // The late return reaches the new installation's ring.
        drop(ReturnOnDrop { credit });
        assert_eq!(credit.held(), 0);
        assert!(credit.take_returned());
    }
}
