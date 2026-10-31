//! The supervised obligations of one executor's peer clients: one static
//! seat per obligated channel, reserved when the channel is made, which the
//! liveness supervisor of the same executor watches independently of
//! whether anyone still waits for the answer.
//!
//! A seat holds only metadata, the deadline of the channel's posted request
//! (none while the channel is idle), in a single-writer [`DeadlineCell`]:
//! the channel's client is its one writer, the supervisor its one reader.
//! The client publishes the deadline before the request reaches the server
//! and wakes the supervisor; it withdraws it only when it took the answer
//! before the deadline. An expiry, an answer taken late, a wait cancelled
//! for good or never started all leave the deadline in the seat, which the
//! supervisor reports once its own clock reaches it.
//!
//! The client and the supervisor are tasks of one executor `C` (the types
//! say so), so they never run at once and the waker stays on that
//! executor's hart. A read that overlaps a write is skipped, never waited
//! for; the writer wakes the supervisor after every write, so it reads
//! again.

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    task::Context,
};

use atomic_waker::AtomicWaker;
use oer_time::Instant;

use crate::deadline_cell::DeadlineCell;

/// A seat's value while its channel has no posted request.
const NONE: u64 = u64::MAX;

/// The obligation seats of one supervisor, `N` of them.
pub struct Obligations<const N: usize> {
    seats: [DeadlineCell; N],
    /// Seats handed to channels so far.
    issued: AtomicU32,
    /// Whether a supervisor claimed this table.
    claimed: AtomicBool,
    /// Set by every write, taken by the supervisor's scan.
    changed: AtomicBool,
    supervisor: AtomicWaker,
}

impl<const N: usize> Obligations<N> {
    pub const fn new() -> Self {
        const {
            if N > u32::MAX as usize {
                panic!("seats are numbered in u32");
            }
        };
        Self {
            seats: [const { DeadlineCell::new(NONE) }; N],
            issued: AtomicU32::new(0),
            claimed: AtomicBool::new(false),
            changed: AtomicBool::new(false),
            supervisor: AtomicWaker::new(),
        }
    }

    /// Claim the table for one supervisor; `false` when one claimed it.
    pub(super) fn claim(&self) -> bool {
        !self.claimed.swap(true, Ordering::AcqRel)
    }

    /// The next free seat, for one channel of executor `C`; `None` once all
    /// `N` are issued.
    pub(super) fn issue<C>(&'static self) -> Option<Watch<C>> {
        let index = self
            .issued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |issued| {
                (usize::try_from(issued).ok()? < N).then(|| issued.wrapping_add(1))
            })
            .ok()?;
        let cell = self.seats.get(usize::try_from(index).ok()?)?;
        Some(Watch {
            cell,
            changed: &self.changed,
            supervisor: &self.supervisor,
            channel: index,
            _executor: PhantomData,
        })
    }

    /// Register the supervisor's task to be woken by the next write.
    pub(super) fn register(&self, cx: &Context<'_>) {
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
        for (channel, cell) in (0_u32..).zip(self.seats.iter().take(issued)) {
            let Some(published) = cell.read() else {
                continue;
            };
            if published.value == NONE {
                continue;
            }
            let deadline = Instant::from_micros(published.value);
            if now >= deadline {
                return Scan::Overdue { channel, deadline };
            }
            earliest = Some(earliest.map_or(deadline, |earliest| earliest.min(deadline)));
        }
        Scan::Pending(earliest)
    }
}

impl<const N: usize> Default for Obligations<N> {
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
pub struct Watch<C> {
    cell: &'static DeadlineCell,
    changed: &'static AtomicBool,
    supervisor: &'static AtomicWaker,
    channel: u32,
    _executor: PhantomData<fn() -> C>,
}

impl<C> Watch<C> {
    /// The seat's number in its table, which a report names.
    pub const fn channel(&self) -> u32 {
        self.channel
    }

    /// Publish the posted request's `deadline`, then wake the supervisor.
    pub(super) fn oblige(&self, deadline: Instant) {
        // The largest instant is the seat's empty mark; one microsecond
        // earlier changes no real deadline.
        self.write(deadline.as_micros().min(NONE.wrapping_sub(1)));
    }

    /// Withdraw the obligation of an answer taken before its deadline.
    pub(super) fn discharge(&self) {
        self.write(NONE);
    }

    fn write(&self, value: u64) {
        self.cell.publish(value);
        self.changed.store(true, Ordering::Release);
        self.supervisor.wake();
    }
}
