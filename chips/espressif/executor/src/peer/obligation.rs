//! The supervised obligations of one executor's peer clients: one static
//! seat per obligated channel, reserved when the channel is made, which the
//! liveness supervisor of the same executor watches independently of
//! whether anyone still waits for the answer.
//!
//! A seat holds only metadata: whether the channel has a posted request and
//! that request's deadline, the full range of [`Instant`]. The channel's
//! client is its one writer and the supervisor its one reader. The client
//! publishes the deadline before the request reaches the server and wakes
//! the supervisor; it withdraws it only when it took the answer before the
//! deadline. An expiry, an answer taken late, a wait cancelled for good or
//! never started all leave the obligation in the seat, which the supervisor
//! reports once its own clock reaches the deadline.
//!
//! Executor locality is checked, not assumed: every write takes the proof
//! that the caller runs in executor `C`'s context
//! ([`signal::enter`](oer_espressif_interrupt_table::signal::enter)), and
//! the supervisor registers its waker only after proving that one of `C`'s
//! tasks polls ([`signal::enter_task`]). So writer and reader never run at
//! once (the executor polls one task at a time, and a context is one
//! executor even beside another on the same hart), and the wake stays on
//! that executor's hart. The deadline's two words still go through a
//! [`DeadlineCell`]: a torn read is skipped, never waited for.
//!
//! [`signal::enter_task`]: oer_espressif_interrupt_table::signal::enter_task

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    task::Context,
};

use atomic_waker::AtomicWaker;
use oer_espressif_interrupt_table::signal::{ContextToken, SignalExecutor};
use oer_time::Instant;

use crate::deadline_cell::DeadlineCell;

/// One channel's seat.
struct Seat {
    /// Whether the channel has a posted request; written after `deadline`.
    posted: AtomicBool,
    deadline: DeadlineCell,
}

/// The obligation seats of the supervisor of executor `C`, `N` of them.
pub struct Obligations<C: SignalExecutor, const N: usize> {
    seats: [Seat; N],
    /// Seats handed to channels so far.
    issued: AtomicU32,
    /// Whether a supervisor claimed this table.
    claimed: AtomicBool,
    /// Set by every write, taken by the supervisor's scan.
    changed: AtomicBool,
    supervisor: AtomicWaker,
    _executor: PhantomData<fn() -> C>,
}

impl<C: SignalExecutor, const N: usize> Obligations<C, N> {
    pub const fn new() -> Self {
        const {
            if N > u32::MAX as usize {
                panic!("seats are numbered in u32");
            }
        };
        Self {
            seats: [const {
                Seat {
                    posted: AtomicBool::new(false),
                    deadline: DeadlineCell::new(0),
                }
            }; N],
            issued: AtomicU32::new(0),
            claimed: AtomicBool::new(false),
            changed: AtomicBool::new(false),
            supervisor: AtomicWaker::new(),
            _executor: PhantomData,
        }
    }

    /// Claim the table for one supervisor; `false` when one claimed it.
    pub(super) fn claim(&self) -> bool {
        !self.claimed.swap(true, Ordering::AcqRel)
    }

    /// The next free seat, for one channel of executor `C`; `None` once all
    /// `N` are issued.
    pub(super) fn issue(&'static self) -> Option<Watch<C>> {
        let index = self
            .issued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |issued| {
                (usize::try_from(issued).ok()? < N).then(|| issued.wrapping_add(1))
            })
            .ok()?;
        let seat = self.seats.get(usize::try_from(index).ok()?)?;
        Some(Watch {
            seat,
            changed: &self.changed,
            supervisor: &self.supervisor,
            channel: index,
            _executor: PhantomData,
        })
    }

    /// Register the polling task of `C`, proven by `_in`, to be woken by
    /// the next write.
    pub(super) fn register(&self, cx: &Context<'_>, _in: &ContextToken<C::Context>) {
        self.supervisor.register(cx.waker());
    }

    /// Whether a seat was written since the last call.
    pub(super) fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    /// The seats' state at `now`: the first seat whose deadline has come,
    /// otherwise the earliest pending deadline. Seats read mid-write are
    /// skipped; their writer wakes the supervisor once it finished.
    pub(super) fn scan(&self, now: Instant) -> Scan {
        let issued = usize::try_from(self.issued.load(Ordering::Acquire)).unwrap_or(N);
        let mut earliest: Option<Instant> = None;
        for (channel, seat) in (0_u32..).zip(self.seats.iter().take(issued)) {
            if !seat.posted.load(Ordering::Acquire) {
                continue;
            }
            let Some(published) = seat.deadline.read() else {
                continue;
            };
            let deadline = Instant::from_micros(published.value);
            if now >= deadline {
                return Scan::Overdue { channel, deadline };
            }
            earliest = Some(earliest.map_or(deadline, |earliest| earliest.min(deadline)));
        }
        Scan::Pending(earliest)
    }

    /// Whether a supervisor's waker is registered now.
    #[cfg(test)]
    pub(super) fn registered(&self) -> bool {
        match self.supervisor.take() {
            Some(waker) => {
                self.supervisor.register(&waker);
                true
            }
            None => false,
        }
    }
}

impl<C: SignalExecutor, const N: usize> Default for Obligations<C, N> {
    fn default() -> Self {
        Self::new()
    }
}

/// What a scan of the seats found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Scan {
    /// A channel's request reached its deadline unanswered.
    Overdue { channel: u32, deadline: Instant },
    /// No seat is due; the earliest pending deadline, if any.
    Pending(Option<Instant>),
}

/// One channel's seat, held by its client on executor `C`.
#[must_use = "dropping a seat leaves its channel unable to post"]
pub struct Watch<C: SignalExecutor> {
    seat: &'static Seat,
    changed: &'static AtomicBool,
    supervisor: &'static AtomicWaker,
    channel: u32,
    _executor: PhantomData<fn() -> C>,
}

impl<C: SignalExecutor> Watch<C> {
    /// The seat's number in its table, which a report names.
    pub const fn channel(&self) -> u32 {
        self.channel
    }

    /// Publish the posted request's `deadline`, unchanged, then wake the
    /// supervisor; only in executor `C`'s context, proven by `_in`.
    pub(super) fn oblige(&self, deadline: Instant, _in: &ContextToken<C::Context>) {
        self.seat.deadline.publish(deadline.as_micros());
        self.seat.posted.store(true, Ordering::Release);
        self.notify();
    }

    /// Withdraw the obligation of an answer taken before its deadline; only
    /// in executor `C`'s context, proven by `_in`.
    pub(super) fn discharge(&self, _in: &ContextToken<C::Context>) {
        self.seat.posted.store(false, Ordering::Release);
        self.notify();
    }

    fn notify(&self) {
        self.changed.store(true, Ordering::Release);
        self.supervisor.wake();
    }
}
