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
//! SOURCE: ESP-IDF 4d59230d `esp_hal_systimer/systimer_hal.c`
//! (`systimer_hal_get_counter_value`: update, wait for valid, LO/HI/LO) and
//! esp-hal 1f646c5 `timer/systimer.rs` (`Unit::read_count`).

use oer_espressif_platform_port::{LatchStuck, Unverified};

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

impl<P: oer_mmio::Placement> CounterLatch for super::SystimerCounter<P> {
    #[inline(always)]
    fn update(&self) {
        Self::update(self);
    }

    #[inline(always)]
    fn valid(&self) -> bool {
        Self::valid(self)
    }

    #[inline(always)]
    fn high(&self) -> u32 {
        Self::high(self)
    }

    #[inline(always)]
    fn low(&self) -> u32 {
        Self::low(self)
    }
}

/// The polls of the valid flag one latch may take: the copy crosses into
/// the 16 MHz counter domain in a few of its cycles, well inside the 20 µs a
/// named settle may last at any CPU clock.
pub const LATCH_SETTLE_POLLS: u32 = 64;

/// The LO/HI/LO rounds one read may take: each further round needs a strobe
/// from a preempting context between two of its reads.
pub const READ_ROUNDS: u32 = 16;
const _: () = assert!(READ_ROUNDS > 0, "a read takes at least one round");

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
