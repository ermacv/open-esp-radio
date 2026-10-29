#![no_std]
use oer_esp32s31_hal::boot::PsramRegisters;
use oer_espressif_boot::psram::MappedPsram;

pub fn loan(mapped: MappedPsram<PsramRegisters>) {
    let _region = PsramRegisters::into_region(mapped);
    #[cfg(ui_reject)]
    let (_remap_owner, _) = mapped.into_parts(); // ui-error: E0382
}
