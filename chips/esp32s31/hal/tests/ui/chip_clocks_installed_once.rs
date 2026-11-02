#![no_std]

use oer_chips_esp32s31_hal::{power::ChipClocks, root::Root};

pub fn install(root: Root) -> Result<ChipClocks, oer_chips_family_boot::adoption::InitError> {
    let clocks = oer_chips_family_boot::adoption::initialize(root.into_runtime())?.hardware.clocks;
    #[cfg(ui_reject)]
    let _again = root.into_runtime(); // ui-error: E0382
    Ok(clocks)
}
