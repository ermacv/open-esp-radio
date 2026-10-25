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

pub use cores::{Fatal, StopError, StopTheWorld};

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

/// Reset the chip from a fatal path that records its state first: mask this
/// hart's interrupts for good, stop the other hart (`cores::stop_the_world`)
/// and give `record` the stop, or why there is none, then reset as
/// [`reset`] does. Under the stop the image's router writes no more, so a
/// record reads the interrupt controller as the hardware holds it.
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "lends the chip HAL's fatal-path proof")]
pub fn reset_recording(
    record: impl for<'a, 'b> FnOnce(Result<&'b cores::StopTheWorld<'a>, cores::StopError>),
) -> ! {
    crate::critical::hart::mask_for_good();
    // SAFETY: the hart's interrupts are masked above, the threshold first,
    // and the software reset does not return; a path that cannot prepare it
    // halts below without unmasking them.
    unsafe {
        cores::fatal(|fatal| {
            let stop = cores::stop_the_world(&fatal);
            record(stop.as_ref().map_err(|error| *error));
            drop(stop);
            let cores::AwaitWatchdog { reason: _ } = cores::software_reset(fatal);
        })
    };
    crate::halt()
}

/// Reset the chip from a fatal path that records nothing
/// ([`reset_recording`]): mask this hart's interrupts for good, stop the
/// other hart, then the chip HAL's software reset, which restores what the
/// boot ROM needs and resets through the ROM. A path that cannot hold the
/// core-control words (`cores::AwaitWatchdog`) halts its masked hart until
/// the RTC watchdog the image armed when it took its watchdogs
/// (`watchdog::Watchdogs`) resets the chip.
#[cfg(target_arch = "riscv32")]
pub fn reset() -> ! {
    reset_recording(|_stop| ())
}
