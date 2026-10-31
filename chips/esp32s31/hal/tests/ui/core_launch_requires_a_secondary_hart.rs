#![no_std]
use oer_esp32s31_hal::{
    cores::SecondaryEntry,
    platform::{core_control::CoreControlRegisters, SecondaryHart},
};

pub fn hold(registers: &mut CoreControlRegisters, secondary: SecondaryHart) {
    registers.hold(secondary);
    #[cfg(ui_reject)]
    registers.hold(oer_esp32s31_hal::platform::BOOT_HART); // ui-error: E0308
}

#[allow(unsafe_code)]
// SAFETY: a type-level case; no hart enters this address.
pub const SECONDARY: SecondaryEntry = unsafe { SecondaryEntry::new::<1>(0x2f00_0000) };

#[cfg(ui_reject)]
#[allow(unsafe_code)]
// SAFETY: a type-level case; no hart enters this address.
pub const BOOT: SecondaryEntry = unsafe { SecondaryEntry::new::<0>(0x2f00_0000) }; // ui-error@outside: E0080: evaluation panicked: a secondary entry names a secondary hart
