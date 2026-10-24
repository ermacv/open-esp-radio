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

/// Wait the latch's settle: `Ok` once its value is valid.
fn settle(latch: &impl CounterLatch) -> Result<(), LatchStuck> {
    for _ in 0..LATCH_SETTLE_POLLS {
        if latch.valid() {
            return Ok(());
        }
    }
    Err(LatchStuck)
}

/// The counter's current value, in ticks.
///
/// # Errors
///
/// [`LatchStuck`] when the latch never turns valid or preempting strobes
/// keep the reads from agreeing for [`READ_ROUNDS`] rounds.
pub(crate) fn read(latch: &impl CounterLatch) -> Result<u64, LatchStuck> {
    latch.update();
    settle(latch)?;
    let mut low = latch.low();
    for _ in 0..READ_ROUNDS {
        let high = latch.high();
        let again = latch.low();
        if again == low && latch.valid() {
            return Ok(u64::from(high) << 32 | u64::from(low));
        }
        // A preempting context relatched the counter meanwhile: wait for
        // its copy and read the newer count.
        settle(latch)?;
        low = latch.low();
    }
    Err(LatchStuck)
}

#[cfg(test)]
mod tests;
