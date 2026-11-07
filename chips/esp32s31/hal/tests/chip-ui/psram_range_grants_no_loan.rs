#![no_std]
use oer_chips_esp32s31_hal::system::boot::PsramRegisters;
use oer_chips_family_boot::psram::MappedPsram;

pub fn loan(mapped: MappedPsram<PsramRegisters>) {
    let observed = mapped.mapped();
    let _copy = observed;
    #[cfg(ui_reject)]
    let _region = PsramRegisters::into_region(observed); // ui-error: E0308
    let _region = PsramRegisters::into_region(mapped);
}
