//! The read of the system timer's counter, which every context makes: strobe
//! the latch of unit zero, wait the short settle until the latched value is
//! valid, then read LO, HI, LO until the two LO reads agree with the latch
//! still valid.
//!
//! Every context strobes the same latch, without a lock: a strobe from a
//! context that preempts a read relatches a newer count, and the reader that
//! was preempted then sees its two LO reads differ, or the latch invalid,
//! and reads again. Each read therefore returns one count the counter held
//! after the read began, and a read that starts after another returned
//! returns a count no smaller.
//!
//! SOURCE: ESP-IDF 4d59230d `esp_hal_systimer/systimer_hal.c`
//! (`systimer_hal_get_counter_value`: update, wait for valid, LO/HI/LO) and
//! esp-hal 1f646c5 `timer/systimer.rs` (`Unit::read_count`).

/// The counter latch of system timer unit zero, shared by every context.
pub(crate) trait CounterLatch {
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
pub(crate) const LATCH_SETTLE_POLLS: u32 = 64;

/// The LO/HI/LO rounds one read may take: each further round needs a strobe
/// from a preempting context between two of its reads.
pub(crate) const READ_ROUNDS: u32 = 16;

/// The latch did not settle within its bound: a fault of the system timer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LatchStuck;

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
pub(crate) fn check(latch: &impl CounterLatch) -> Result<(), LatchStuck> {
    latch.update();
    if settle(latch) {
        Ok(())
    } else {
        Err(LatchStuck)
    }
}

/// The counter's current value, in ticks. It never fails and never waits
/// longer than its bounds: the latch settles within [`LATCH_SETTLE_POLLS`]
/// ([`check`] proved it when the system timer was taken), and a read that
/// preempting strobes keep from agreeing for [`READ_ROUNDS`] rounds returns
/// the last round's HI with the LO read after it.
pub(crate) fn read(latch: &impl CounterLatch) -> u64 {
    latch.update();
    let _ = settle(latch);
    let mut low = latch.low();
    let mut high = latch.high();
    for _ in 0..READ_ROUNDS {
        let again = latch.low();
        if again == low && latch.valid() {
            break;
        }
        // A preempting context relatched the counter meanwhile: wait for
        // its copy and read the newer count.
        let _ = settle(latch);
        low = latch.low();
        high = latch.high();
    }
    u64::from(high) << 32 | u64::from(low)
}

#[cfg(test)]
mod tests;
