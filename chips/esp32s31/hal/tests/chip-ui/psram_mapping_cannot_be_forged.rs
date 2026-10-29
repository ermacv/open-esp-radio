#![no_std]
use oer_esp32s31_hal::boot::PsramRegisters;
use oer_espressif_boot::{mmu::MappedRange, psram::MappedPsram};

pub fn loan(owner: PsramRegisters, observed: MappedRange) {
    #[cfg(ui_reject)]
    let _mapped = MappedPsram { registers: owner, mapped: observed }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let _ = (owner, observed);
}
