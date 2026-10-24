//! A fake latch over a counter that advances on every register access, with
//! a hook that runs a preempting read at a chosen access.

use core::cell::Cell;

use super::{CounterLatch, LATCH_SETTLE_POLLS, LatchStuck, check, read};

struct Fake {
    /// The free-running count; it advances by `step` on every access.
    count: Cell<u64>,
    step: u64,
    latched: Cell<u64>,
    /// Accesses left until the latch turns valid after a strobe.
    pending: Cell<u32>,
    settle: u32,
    accesses: Cell<u32>,
    /// The access at which a preempting context reads, and its results.
    preempt_at: Cell<Option<u32>>,
    preempted: Cell<Option<u64>>,
    strobes: Cell<u32>,
    /// Every count the latch held, in order.
    history: [Cell<u64>; 64],
    latches: Cell<usize>,
}

impl Fake {
    fn new(start: u64, step: u64, settle: u32) -> Self {
        Self {
            count: Cell::new(start),
            step,
            latched: Cell::new(0),
            pending: Cell::new(u32::MAX),
            settle,
            accesses: Cell::new(0),
            preempt_at: Cell::new(None),
            preempted: Cell::new(None),
            strobes: Cell::new(0),
            history: [const { Cell::new(0) }; 64],
            latches: Cell::new(0),
        }
    }

    /// Whether the latch held `value` at some point.
    fn held(&self, value: u64) -> bool {
        self.history[..self.latches.get()]
            .iter()
            .any(|held| held.get() == value)
    }

    /// One register access: the clock runs, a pending copy completes, and a
    /// preempting context may run its own whole read here.
    fn access(&self) {
        self.count.set(self.count.get() + self.step);
        let accesses = self.accesses.get() + 1;
        self.accesses.set(accesses);
        if self.pending.get() != u32::MAX {
            if self.pending.get() == 0 {
                self.latched.set(self.count.get());
                self.history[self.latches.get()].set(self.count.get());
                self.latches.set(self.latches.get() + 1);
                self.pending.set(u32::MAX);
            } else {
                self.pending.set(self.pending.get() - 1);
            }
        }
        if self.preempt_at.get() == Some(accesses) {
            self.preempt_at.set(None);
            let value = read(self);
            self.preempted.set(Some(value));
        }
    }
}

impl CounterLatch for Fake {
    fn update(&self) {
        self.strobes.set(self.strobes.get() + 1);
        self.pending.set(self.settle);
        self.access();
    }
    fn valid(&self) -> bool {
        self.access();
        self.pending.get() == u32::MAX
    }
    fn high(&self) -> u32 {
        self.access();
        (self.latched.get() >> 32) as u32
    }
    fn low(&self) -> u32 {
        self.access();
        self.latched.get() as u32
    }
}

#[test]
fn a_read_returns_the_latched_count_after_a_late_valid() {
    let latch = Fake::new(0x1234_0000_0000, 1, 5);
    let value = read(&latch);
    assert!(latch.held(value), "{value:#x}");
    assert!(value > 0x1234_0000_0000);
    assert_eq!(latch.strobes.get(), 1);
}

#[test]
fn a_latch_that_never_settles_is_a_fault() {
    let latch = Fake::new(0, 1, LATCH_SETTLE_POLLS + 10);
    assert_eq!(check(&latch), Err(LatchStuck));
    let latch = Fake::new(0, 1, 5);
    assert_eq!(check(&latch), Ok(()));
}

#[test]
fn a_low_word_that_wraps_between_reads_is_read_again() {
    // The count crosses a 32-bit boundary while the reader reads HI: a
    // preempting strobe relatches the newer count, so HI and the second LO
    // belong to it and the first LO does not.
    for preempt in 1..=12 {
        let latch = Fake::new(0xffff_fff0, 3, 1);
        latch.preempt_at.set(Some(preempt));
        let value = read(&latch);
        // Each returned count is one the latch held: its HI and LO belong
        // to one latch.
        assert!(latch.held(value), "{preempt}: {value:#x}");
        if let Some(inner) = latch.preempted.get() {
            assert!(latch.held(inner), "{preempt}: {inner:#x}");
        }
    }
}

#[test]
fn interleaved_reads_from_two_contexts_are_monotonic() {
    // A task reads, a handler preempts it at every possible access, and the
    // task reads again: every value is a count the latch held, and a read
    // that starts after another returned never returns less.
    for preempt in 1..=14 {
        let latch = Fake::new(0x7_ffff_ff00, 7, 2);
        let before = read(&latch);
        let start = latch.accesses.get();
        latch.preempt_at.set(Some(start + preempt));
        let during = read(&latch);
        let inner = latch.preempted.get();
        let after = read(&latch);
        assert!(latch.held(during), "{preempt}: {during:#x}");
        assert!(before <= during, "{preempt}: {before:#x} > {during:#x}");
        assert!(during <= after, "{preempt}: {during:#x} > {after:#x}");
        if let Some(inner) = inner {
            assert!(before <= inner && inner <= after, "{preempt}: {inner:#x}");
        }
    }
}
