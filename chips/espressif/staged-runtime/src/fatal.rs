//! The image's fatal paths: a hart that ends ([`fatal_halt`]) and the chip's
//! reset ([`reset`]).
//!
//! Each masks the hart's interrupts for good (the CLIC threshold at its
//! maximum, then `mstatus.MIE` clear) before anything else, so no handler of
//! the hart runs again, and lends the chip HAL's [`Fatal`] proof to what
//! follows: with it a path may stop the other hart before the reset
//! (`cores::stop_the_world`, which gives no right to another owner's
//! block), and the chip resets through the HAL (`cores::software_reset`),
//! not esp-hal.

#[cfg(feature = "esp32s31")]
use oer_esp32s31_hal::cores;

pub use cores::Fatal;

/// End this hart in a fatal path: mask its interrupts for good, lend
/// `fatal_path` the proof, then wait for good. The other hart keeps running
/// unless `fatal_path` stopped it.
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "lends the chip HAL's fatal-path proof")]
pub fn fatal_halt(fatal_path: impl for<'a> FnOnce(Fatal<'a>)) -> ! {
    crate::critical::hart::mask_for_good();
    // SAFETY: the hart's interrupts are masked above, the threshold first,
    // and `halt` below waits for good without unmasking them or returning
    // to the code that called this.
    unsafe { cores::fatal(fatal_path) };
    crate::halt()
}

/// Reset the chip from a fatal path: mask this hart's interrupts for good,
/// then the chip HAL's software reset, which restores what the boot ROM
/// needs and resets through the ROM. A path that cannot hold the
/// core-control words (`cores::AwaitWatchdog`) halts its masked hart until
/// the RTC watchdog the image armed when it took its watchdogs
/// (`watchdog::Watchdogs`) resets the chip.
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "lends the chip HAL's fatal-path proof")]
pub fn reset() -> ! {
    crate::critical::hart::mask_for_good();
    // SAFETY: the hart's interrupts are masked above, the threshold first,
    // and the software reset does not return.
    unsafe {
        cores::fatal(|fatal| {
            let cores::AwaitWatchdog { reason: _ } = cores::software_reset(fatal);
        })
    };
    // Reached only when the reset could not be prepared: the masked hart
    // waits for an armed watchdog's reset.
    crate::halt()
}
