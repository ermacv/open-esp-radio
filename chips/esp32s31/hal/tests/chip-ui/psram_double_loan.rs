#![no_std]
use oer_chips_esp32s31_hal::system::boot::PsramRegisters;
use oer_chips_family_boot::psram::MappedPsram;

pub fn loan(mapped: MappedPsram<PsramRegisters>) {
    let _region = PsramRegisters::into_region(mapped);
    #[cfg(ui_reject)]
    let _second = PsramRegisters::into_region(mapped); // ui-error: E0382
}
