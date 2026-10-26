#![no_std]
use oer_esp32s31_hal::platform::{core_control::CoreControlRegisters, SecondaryHart};

pub fn hold(registers: &mut CoreControlRegisters, secondary: SecondaryHart) {
    registers.hold(secondary);
    #[cfg(ui_reject)]
    registers.hold(oer_esp32s31_hal::platform::BOOT_HART); // ui-error: E0308
}
