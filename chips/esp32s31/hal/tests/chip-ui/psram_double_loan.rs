#![no_std]
use oer_esp32s31_hal::boot::PsramRegisters;
use oer_espressif_boot::psram::MappedPsram;

pub fn loan(mapped: MappedPsram<PsramRegisters>) {
    let _region = PsramRegisters::into_region(mapped);
    #[cfg(ui_reject)]
    let _second = PsramRegisters::into_region(mapped); // ui-error: E0382
}
