#![no_std]

use oer_chips_esp32s31_hal::system::root::{RadioHardware, Root};

pub fn install(root: Root) -> Result<RadioHardware, oer_chips_family_boot::adoption::InitError> {
    let radio = oer_chips_family_boot::adoption::initialize(root.into_runtime())?.hardware.radio;
    #[cfg(ui_reject)]
    let _again = root.into_runtime(); // ui-error: E0382
    Ok(radio)
}
