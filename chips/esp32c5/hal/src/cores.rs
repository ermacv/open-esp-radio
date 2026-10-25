//! The chip's one HP core (hart): the fatal path's proof ([`Fatal`]) and the
//! chip's software reset ([`software_reset`]).
//!
//! No other HP hart exists, so a fatal path that masked this hart's
//! interrupts runs alone on the HP side: the stop of every other hart is
//! empty. The core-control word, UART0's function clock, has one writer,
//! the software reset; a reset inside a fatal reset finds the outer one
//! never resuming.

use core::marker::PhantomData;

use oer_esp32c5_pac::platform::{CoreControlHandoff, CoreControlRegisters};
use oer_mmio::Placement;

#[cfg(test)]
mod tests;

/// Proof that this hart runs a fatal path: its interrupts masked, and it
/// never returns to the code it interrupted. Only [`fatal`] makes one, and
/// the proof does not leave the closure it is lent to.
pub struct Fatal<'a> {
    _path: PhantomData<fn(&'a ()) -> &'a ()>,
    _hart: PhantomData<*const ()>,
}

/// Lend `fatal_path` the proof that this hart runs a fatal path.
///
/// # Safety
/// Call only from a fatal path that masked the hart's interrupts (the CLIC
/// threshold at its maximum, then `mstatus.MIE` clear) and that, after
/// `fatal_path` returns, halts the hart or resets the chip without unmasking
/// them or returning to the code it interrupted.
#[allow(unsafe_code, reason = "the fatal-path contract of the token")]
pub unsafe fn fatal<T>(fatal_path: impl for<'a> FnOnce(Fatal<'a>) -> T) -> T {
    fatal_path(Fatal {
        _path: PhantomData,
        _hart: PhantomData,
    })
}

/// The word's owner, for the reset.
#[allow(
    unsafe_code,
    reason = "the software reset captures the word it alone writes"
)]
fn capture() -> CoreControlRegisters {
    // SAFETY: only the software reset builds this owner, from a fatal path
    // (`Fatal`) on the chip's one HP hart. A reset that a fault inside it
    // starts again never returns to the first one, whose owner never
    // accesses the word again. esp-hal writes the word only from a UART
    // driver no image makes (`registers/shared-words.toml`).
    let handoff = unsafe { CoreControlHandoff::assume_exclusive() };
    CoreControlRegisters::from_handoff(handoff)
}

/// Prepare the chip's reset: UART0's clock for the boot ROM.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/system.rs` (`software_reset`);
/// ESP-IDF 4d59230d `components/esp_system/port/soc/esp32c5/system_internal.c`.
fn prepare_reset<P: Placement>(words: &mut CoreControlRegisters<P>) {
    words.restore_uart0_clock();
}

/// The ROM routine this module calls.
mod rom {
    #[cfg(target_arch = "riscv32")]
    unsafe extern "C" {
        /// SOURCE: the pinned ESP32-C5 rev1.0 ROM (`esp32c5_rev100_rom.elf`,
        /// `chips/esp32c5/vendor/artifacts.toml`): Blobray's function record
        /// of `software_reset` sets `LP_AON.SYS_CFG`'s `HPSYS_SW_RESET`
        /// (0x600b1034 bit 31), which resets the HP system.
        fn software_reset() -> !;
    }

    /// Reset the HP system through the ROM.
    #[cfg(target_arch = "riscv32")]
    #[allow(unsafe_code, reason = "a ROM routine")]
    pub(super) fn system_reset() -> ! {
        // SAFETY: the routine requests the system reset and does not return.
        unsafe { software_reset() }
    }

    /// The host has no ROM.
    #[cfg(not(target_arch = "riscv32"))]
    #[expect(clippy::panic, reason = "no host code resets the chip")]
    pub(super) fn system_reset() -> ! {
        panic!("the system reset is the chip ROM's")
    }
}

/// Reset the chip from `fatal`'s path, as the boot ROM expects it: UART0's
/// clock restored, then the ROM's system reset.
pub fn software_reset(_fatal: Fatal<'_>) -> ! {
    prepare_reset(&mut capture());
    oer_mmio::fence();
    rom::system_reset()
}
