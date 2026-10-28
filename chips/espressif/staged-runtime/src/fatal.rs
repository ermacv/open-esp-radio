//! The image's fatal paths: a hart that ends ([`fatal_halt`]) and the chip's
//! reset ([`reset`]), over the image's platform `P`
//! ([`Platform`](oer_espressif_platform_port::Platform): the chip HAL's port).
//!
//! The platform masks the hart's interrupts (the CLIC threshold at its
//! maximum, then `mstatus.MIE` clear) before lending its proof
//! (`Platform::Fatal`) to what follows: with it a path may stop the other
//! hart before the reset (`Platform::stop_the_world`, which gives no right
//! to another owner's block), and the chip resets through the platform
//! (`Platform::software_reset`).

#[cfg(target_arch = "riscv32")]
use oer_espressif_platform_port::{AwaitWatchdog, InvalidHart, Platform};

/// End this hart in a fatal path: mask its interrupts for good, lend
/// `fatal_path` the proof, then wait for good. The other hart keeps running
/// unless `fatal_path` stopped it. A path on a hart outside the chip's
/// geometry records the invalid hart and resets instead.
#[cfg(target_arch = "riscv32")]
pub fn fatal_halt<P: Platform>(fatal_path: impl for<'a> FnOnce(P::Fatal<'a>)) -> ! {
    P::fatal(|fatal| {
        if P::fatal_hart(&fatal).is_err() {
            reset_recording_with::<P>(fatal, |_| ());
        } else {
            fatal_path(fatal);
        }
    })
}

/// Reset the chip from a fatal path that records its state first: mask this
/// hart's interrupts for good, stop the other hart
/// (`Platform::stop_the_world`) and give `record` the stop, or why there is
/// none, then reset as [`reset`] does. Under the stop the image's router
/// writes no more, so a record reads the interrupt controller as the hardware
/// holds it.
#[cfg(target_arch = "riscv32")]
pub fn reset_recording<P: Platform>(
    record: impl for<'a, 'b> FnOnce(Result<&'b P::Stop<'a>, P::StopError>),
) -> ! {
    P::fatal(|fatal| reset_recording_with::<P>(fatal, record))
}

#[cfg(target_arch = "riscv32")]
fn reset_recording_with<P: Platform>(
    fatal: P::Fatal<'_>,
    record: impl for<'a, 'b> FnOnce(Result<&'b P::Stop<'a>, P::StopError>),
) {
    let hart = P::fatal_hart(&fatal);
    let reset = || {
        let stop = P::stop_the_world(&fatal);
        record(stop.as_ref().map_err(|error| *error));
        drop(stop);
        let AwaitWatchdog { reason: _ } = P::software_reset(fatal);
    };
    match hart {
        Ok(_) => reset(),
        Err(InvalidHart { raw }) => crate::panic::with_invalid_hart_record(raw, reset),
    }
}

/// Reset the chip from a fatal path that records nothing
/// ([`reset_recording`]): mask this hart's interrupts for good, stop the
/// other hart, then the platform's software reset, which restores what the
/// boot ROM needs and triggers the system reset through the owned PAC words.
/// A path that cannot hold the core-control words (`AwaitWatchdog`) halts
/// its masked hart until the RTC watchdog the image armed when it took its
/// watchdogs resets the chip.
#[cfg(target_arch = "riscv32")]
pub fn reset<P: Platform>() -> ! {
    reset_recording::<P>(|_stop| ())
}
