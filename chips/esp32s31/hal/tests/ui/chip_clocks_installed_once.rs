#![no_std]

use oer_esp32s31_hal::{power::ChipClocks, root::Root};

pub fn install(root: Root) -> Result<ChipClocks, oer_esp32s31_hal::root::InitError> {
    let clocks = root.into_runtime()?.clocks;
    #[cfg(ui_reject)]
    let _again = root.into_runtime(); // ui-error: E0382
    Ok(clocks)
}
