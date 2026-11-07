#![no_std]
use oer_chips_esp32s31_hal::{system::root::Root, peripherals::aes::AesParts};

pub fn aes(root: Root) -> Result<AesParts, oer_chips_family_boot::adoption::InitError> {
    let runtime = oer_chips_family_boot::adoption::initialize(root.into_runtime())?.hardware;
    let aes = runtime.aes;
    #[cfg(ui_reject)]
    let _again = runtime.aes; // ui-error: E0382
    Ok(aes)
}
