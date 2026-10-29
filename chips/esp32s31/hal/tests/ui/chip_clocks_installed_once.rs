#![no_std]

use oer_esp32s31_hal::{power::ChipClocks, root::Root};

pub fn install(root: Root) -> Result<ChipClocks, oer_espressif_boot::adoption::InitError> {
    let clocks = oer_espressif_boot::adoption::initialize(root.into_runtime())?.hardware.clocks;
    #[cfg(ui_reject)]
    let _again = root.into_runtime(); // ui-error: E0382
    Ok(clocks)
}
