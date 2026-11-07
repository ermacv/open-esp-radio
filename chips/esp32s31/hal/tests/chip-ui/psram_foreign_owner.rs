#![no_std]
use oer_chips_esp32s31_hal::system::boot::{FlashRegisters, PsramRegisters};
use oer_chips_family_boot::psram::MappedPsram;

pub fn loan(foreign: MappedPsram<FlashRegisters>, own: MappedPsram<PsramRegisters>) {
    #[cfg(ui_reject)]
    let _region = PsramRegisters::into_region(foreign); // ui-error: E0308
    let _region = PsramRegisters::into_region(own);
    let _ = foreign;
}
