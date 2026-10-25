//! A fake latch over a counter that advances on every register access, with
//! a hook that runs a preempting read at a chosen access.

use core::cell::Cell;

use super::{CounterLatch, LATCH_SETTLE_POLLS, LatchStuck, READ_ROUNDS, Unverified, check, read};

/// A read the test expects to verify.
fn verified(latch: &impl CounterLatch) -> u64 {
    read(latch).expect("the read verifies")
}

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
            let value = verified(self);
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
    let value = verified(&latch);
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
        let value = verified(&latch);
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
        let before = verified(&latch);
        let start = latch.accesses.get();
        latch.preempt_at.set(Some(start + preempt));
        let during = verified(&latch);
        let inner = latch.preempted.get();
        let after = verified(&latch);
        assert!(latch.held(during), "{preempt}: {during:#x}");
        assert!(before <= during, "{preempt}: {before:#x} > {during:#x}");
        assert!(during <= after, "{preempt}: {during:#x} > {after:#x}");
        if let Some(inner) = inner {
            assert!(before <= inner && inner <= after, "{preempt}: {inner:#x}");
        }
    }
}

/// A latch that a completed preempting read relatches before each of the
/// first `left` HI reads, one tick later each time.
struct PreemptEveryHigh {
    latched: Cell<u64>,
    left: Cell<u32>,
}

impl CounterLatch for PreemptEveryHigh {
    fn update(&self) {}
    fn valid(&self) -> bool {
        true
    }
    fn high(&self) -> u32 {
        if self.left.get() > 0 {
            self.left.set(self.left.get() - 1);
            self.latched.set(self.latched.get() + 1);
        }
        (self.latched.get() >> 32) as u32
    }
    fn low(&self) -> u32 {
        self.latched.get() as u32
    }
}

#[test]
fn a_read_that_preemptions_exhaust_names_a_monotonic_floor() {
    // More preempting reads than a read has rounds, with the last rounds
    // before, at and after LO's rollover: the read is unverified, and its
    // floor lies between the counts the latch held during it, so the clock
    // never runs backward.
    for start in 0xffff_ffe0..=0xffff_fff8 {
        for preemptions in READ_ROUNDS + 1..=READ_ROUNDS + 4 {
            let latch = PreemptEveryHigh {
                latched: Cell::new(start),
                left: Cell::new(preemptions),
            };
            assert_eq!(check(&latch), Ok(()));
            let Err(Unverified { floor }) = read(&latch) else {
                panic!("{start:#x}/{preemptions}: an exhausted read is unverified");
            };
            assert!(
                start <= floor && floor <= latch.latched.get(),
                "{start:#x}/{preemptions}: {floor:#x}"
            );
            let next = read(&latch).unwrap_or_else(|unverified| unverified.floor);
            assert!(
                floor <= next,
                "{start:#x}/{preemptions}: {floor:#x} > {next:#x}"
            );
        }
    }
}

/// A latch whose every strobe after the first stays in flight: VALID never
/// turns true again.
struct NeverValidAgain {
    latched: u64,
    ready: Cell<bool>,
    first: Cell<bool>,
}

impl CounterLatch for NeverValidAgain {
    fn update(&self) {
        if !self.first.replace(false) {
            self.ready.set(false);
        }
    }
    fn valid(&self) -> bool {
        self.ready.get()
    }
    fn high(&self) -> u32 {
        (self.latched >> 32) as u32
    }
    fn low(&self) -> u32 {
        self.latched as u32
    }
}

#[test]
fn a_latch_that_stays_invalid_never_certifies_its_old_count() {
    let latch = NeverValidAgain {
        latched: 42,
        ready: Cell::new(true),
        first: Cell::new(true),
    };
    assert_eq!(check(&latch), Ok(()));
    // The count is the copy before this read's strobe: a floor, never a
    // verified reading.
    assert_eq!(read(&latch), Err(Unverified { floor: 42 }));
}
