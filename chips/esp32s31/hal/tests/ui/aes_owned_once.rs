#![no_std]
use oer_esp32s31_hal::{root::Root, peripherals::aes::AesParts};

pub fn aes(root: Root) -> Result<AesParts, oer_espressif_boot::adoption::InitError> {
    let runtime = oer_espressif_boot::adoption::initialize(root.into_runtime())?.hardware;
    let aes = runtime.aes;
    #[cfg(ui_reject)]
    let _again = runtime.aes; // ui-error: E0382
    Ok(aes)
}
