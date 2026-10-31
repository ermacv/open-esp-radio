//! The read of the system timer's counter, which every context makes: strobe
//! the latch of unit zero, wait the short settle until the latched value is
//! valid, then read LO, HI, LO until the two LO reads agree with the latch
//! still valid.
//!
//! Every context strobes the same latch, without a lock: a strobe from a
//! context that preempts a read relatches a newer count, and the reader that
//! was preempted then sees its two LO reads differ, or the latch invalid,
//! and reads again. A verified read therefore returns one count the counter
//! held after the read began, and a read that starts after another returned
//! returns a count no smaller.
//!
//! The value registers change only when a copy completes, and copies
//! complete in the order of their strobes, so each register read sees a
//! word of the newest completed copy. When preempting strobes keep a read
//! from agreeing for all its rounds, the read is unverified and names a
//! floor instead: a count between two copies it saw, which no earlier read
//! exceeds and no later read undercuts.
//!
//! Two harts read the latch at once: their strobes and reads interleave
//! access by access, not as whole reads. The argument above holds for that
//! too, since it uses only these properties of the latch: copies complete
//! in strobe order with counts that never decrease, each value-register read
//! sees a word of the newest completed copy, VALID reads false while a
//! hart's own strobe has not completed, and one hart's strobe reaches the
//! latch before its next read of it. These are the same model's assumptions
//! as for one hart; no stand run has measured concurrent strobes. A
//! verified read returns one whole copy that completed during the read,
//! whichever hart strobed it; a read that starts after another returned, on
//! either hart, returns no less. What
//! changes is the round bound: another hart can strobe between any two
//! accesses, without preempting, so a read can end unverified under
//! cross-hart contention alone; its floor keeps time monotonic. The host
//! tests run both harts' reads under seeded and alternating schedules, with
//! queued and absorbed strobes.
//!
//! SOURCE: ESP-IDF 4d59230d `esp_hal_systimer/systimer_hal.c`
//! (`systimer_hal_get_counter_value`: update, wait for valid, LO/HI/LO) and
//! esp-hal 1f646c5 `timer/systimer.rs` (`Unit::read_count`).

/// The counter's latch never became valid within its settle bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LatchStuck;

/// A read exhausted its rounds; its floor preserves monotonic time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unverified {
    /// A count no earlier read exceeds and no later read undercuts.
    pub floor: u64,
}

/// The counter latch of system timer unit zero, shared by every context.
pub trait CounterLatch {
    /// Strobe the latch: copy the count into the value registers.
    fn update(&self);
    /// Whether the latched value is synchronized and valid.
    fn valid(&self) -> bool;
    /// The latched count's high bits.
    fn high(&self) -> u32;
    /// The latched count's low 32 bits.
    fn low(&self) -> u32;
}

/// The polls of the valid flag one latch may take: the copy crosses into
/// the 16 MHz counter domain in a few of its cycles, well inside the 20 µs a
/// named settle may last at any CPU clock.
pub const LATCH_SETTLE_POLLS: u32 = 64;

/// The LO/HI/LO rounds one read may take: each further round needs a strobe
/// from a preempting context between two of its reads.
pub const READ_ROUNDS: u32 = 16;
const _: () = {
    if READ_ROUNDS == 0 {
        panic!("a read takes at least one round");
    }
};

/// Wait the latch's settle: whether its value turned valid within the bound.
fn settle(latch: &impl CounterLatch) -> bool {
    (0..LATCH_SETTLE_POLLS).any(|_| latch.valid())
}

/// Strobe the latch once and check that it settles within its bound: the
/// check the time driver makes when it takes the system timer, before any
/// context reads the time.
///
/// # Errors
///
/// [`LatchStuck`] when the latch never turns valid.
pub fn check(latch: &impl CounterLatch) -> Result<(), LatchStuck> {
    latch.update();
    if settle(latch) {
        Ok(())
    } else {
        Err(LatchStuck)
    }
}

fn join(high: u32, low: u32) -> u64 {
    u64::from(high) << 32 | u64::from(low)
}

/// The counter's current value, in ticks, read whole: one copy's LO, HI, LO
/// with the two LO reads equal, after a settle that saw the latch valid and
/// with the latch still valid after them. It never waits longer than its
/// bounds: the latch settles within [`LATCH_SETTLE_POLLS`] ([`check`]
/// proved it when the system timer was taken), and each further round needs
/// a strobe from a preempting context.
///
/// # Errors
///
/// [`Unverified`], with its floor, when no round of [`READ_ROUNDS`]
/// verified a count.
pub fn read(latch: &impl CounterLatch) -> Result<u64, Unverified> {
    latch.update();
    // Once a settle saw the latch valid, every later copy belongs to a strobe
    // made after this read began.
    let mut fresh = settle(latch);
    let mut low = latch.low();
    let mut round = (low, 0, low);
    for _ in 0..READ_ROUNDS {
        let high = latch.high();
        let again = latch.low();
        if fresh && again == low && latch.valid() {
            return Ok(join(high, low));
        }
        round = (low, high, again);
        // A preempting context relatched the counter meanwhile: wait for
        // its copy and read the newer count.
        fresh |= settle(latch);
        low = latch.low();
    }
    let (first, high, last) = round;
    // The two LO reads came from copies less than 2^32 ticks apart: an LO
    // no smaller is the same HI, so the first copy is whole; a smaller one
    // crossed a 2^32 boundary, which the HI read after it names.
    let floor = if last >= first {
        join(high, first)
    } else {
        join(latch.high(), 0)
    };
    Err(Unverified { floor })
}

#[cfg(test)]
mod tests;

/// Bootstrap time over an owned latch, before the common boot deadline exists.
pub struct Monotonic<L, const TICKS_PER_MICROSECOND: u64>(L);

impl<L: CounterLatch, const TICKS_PER_MICROSECOND: u64> Monotonic<L, TICKS_PER_MICROSECOND> {
    /// The initial bounded latch check, before a `BootDeadline` can be started.
    /// The owner calls this after opening the timer clock.
    ///
    /// # Errors
    /// [`LatchStuck`] when all settle polls see an invalid latch.
    pub fn start_initial_latch_check(latch: L) -> Result<Self, LatchStuck> {
        const {
            if TICKS_PER_MICROSECOND == 0 {
                panic!("a timer has nonzero tick frequency");
            }
        }
        check(&latch)?;
        Ok(Self(latch))
    }

    /// Transfer the same latch to the next time owner, without another check.
    pub fn into_counter(self) -> L {
        self.0
    }
}

impl<L: CounterLatch, const TICKS_PER_MICROSECOND: u64> oer_time::Clock
    for Monotonic<L, TICKS_PER_MICROSECOND>
{
    fn now(&self) -> oer_time::Instant {
        let divisor = const {
            match core::num::NonZeroU64::new(TICKS_PER_MICROSECOND) {
                Some(divisor) => divisor,
                None => panic!("a timer has nonzero tick frequency"),
            }
        };
        let ticks = read(&self.0).unwrap_or_else(|unverified| unverified.floor);
        oer_time::Instant::from_micros(ticks / divisor)
    }
}
