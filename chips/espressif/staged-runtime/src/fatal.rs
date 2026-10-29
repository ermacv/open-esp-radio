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
/// `fatal_path` the proof, then wait for good. Boot failures use
/// [`boot_failure`] to record and explicitly reset. The other hart keeps running
/// unless `fatal_path` stopped it. A path on a hart outside the chip's
/// geometry records the invalid hart and resets instead.
#[cfg(target_arch = "riscv32")]
pub fn fatal_halt<P: Platform>(
    fatal_path: impl for<'a> FnOnce(&crate::retained_fault::Claim<'_>, P::Fatal<'a>),
) -> ! {
    P::fatal(|fatal| {
        let Ok(claim) = crate::retained_fault::claim() else {
            reset_platform_once::<P>(fatal);
            return;
        };
        if P::fatal_hart(&fatal).is_err() {
            reset_claimed_with::<P>(fatal, &claim, |_, _| ());
        } else {
            fatal_path(&claim, fatal);
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
    record: impl for<'a, 'b> FnOnce(
        &crate::retained_fault::Claim<'_>,
        Result<&'b P::Stop<'a>, P::StopError>,
    ),
) -> ! {
    P::fatal(|fatal| reset_recording_with::<P>(fatal, record))
}

#[cfg(target_arch = "riscv32")]
fn reset_recording_with<P: Platform>(
    fatal: P::Fatal<'_>,
    record: impl for<'a, 'b> FnOnce(
        &crate::retained_fault::Claim<'_>,
        Result<&'b P::Stop<'a>, P::StopError>,
    ),
) {
    let Ok(claim) = crate::retained_fault::claim() else {
        reset_platform_once::<P>(fatal);
        return;
    };
    reset_claimed_with::<P>(fatal, &claim, record);
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn reset_claimed<P: Platform>(
    claim: &crate::retained_fault::Claim<'_>,
    record: impl for<'a, 'b> FnOnce(
        &crate::retained_fault::Claim<'_>,
        Result<&'b P::Stop<'a>, P::StopError>,
    ),
) -> ! {
    P::fatal(|fatal| reset_claimed_with::<P>(fatal, claim, record))
}

#[cfg(target_arch = "riscv32")]
fn reset_claimed_with<P: Platform>(
    fatal: P::Fatal<'_>,
    claim: &crate::retained_fault::Claim<'_>,
    record: impl for<'a, 'b> FnOnce(
        &crate::retained_fault::Claim<'_>,
        Result<&'b P::Stop<'a>, P::StopError>,
    ),
) {
    let hart = P::fatal_hart(&fatal);
    let reset = || {
        let stop = P::stop_the_world(&fatal);
        record(claim, stop.as_ref().map_err(|error| *error));
        drop(stop);
        reset_platform_once::<P>(fatal);
    };
    match hart {
        Ok(_) => reset(),
        Err(InvalidHart { raw }) => crate::panic::with_invalid_hart_record(claim, raw, reset),
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
    reset_recording::<P>(|_, _stop| ())
}

/// Record a boot/admission failure before console output or peer stop, then
/// request an explicit reset. The platform's reset preserves the armed watchdog;
/// an unavailable reset owner leaves that hardware protection in charge.
#[cfg(target_arch = "riscv32")]
pub fn boot_failure<P: Platform>(reason: &str, report: impl FnOnce()) -> ! {
    P::fatal(|fatal| {
        let Ok(claim) = crate::retained_fault::claim() else {
            reset_platform_once::<P>(fatal);
            return;
        };
        if let Ok(record) = encode_boot_failure(reason, oer_riscv::hart_id() as u32) {
            let _committed = claim.commit(&crate::panic::SLOT, record);
        }
        report();
        reset_claimed::<P>(&claim, |_, _| ());
    })
}

const BOOT_FAILURE: u32 = 1;

fn encode_boot_failure(
    reason: &str,
    hart: u32,
) -> Result<oer_diagnostics::fault::Record, oer_diagnostics::fault::Error> {
    use oer_diagnostics::fault::{Class, Domain, Record};
    Record::new(
        Class::ConfigurationBoot,
        Domain::Boot,
        BOOT_FAILURE,
        hart,
        0,
        reason.as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn boot_failure_keeps_the_reason_without_a_retry_counter() {
        let record = super::encode_boot_failure("interrupt-vectors", 0).unwrap();
        let view = record.peek().unwrap();
        assert_eq!(view.class, oer_diagnostics::fault::Class::ConfigurationBoot);
        assert_eq!(view.payload, b"interrupt-vectors");
    }
}

// A fault during stop/the hook may enter a second exception. Only its first
// reset attempt can touch the platform. If that attempt faults, the third
// exception's fatal closure returns to the platform's architectural halt.
// With intact SRAM/entry/mask code there is no fourth exception frame.
#[cfg(target_arch = "riscv32")]
pub(crate) fn reset_platform_once<P: Platform>(fatal: P::Fatal<'_>) {
    if crate::retained_fault::begin_reset() {
        let AwaitWatchdog { reason: _ } = P::software_reset(fatal);
    }
}
