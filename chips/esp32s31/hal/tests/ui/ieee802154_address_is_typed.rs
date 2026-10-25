#![no_std]
use oer_esp32s31_hal::layouts::DmaWindowAddress;
use oer_esp32s31_pac::ieee802154_mac::Ieee802154MacTask;

pub fn publish(mac: &mut Ieee802154MacTask, address: DmaWindowAddress, raw: u32) {
    mac.write_tx_dma_address(address);
    mac.write_rx_dma_address(address);
    #[cfg(ui_reject)]
    mac.write_tx_dma_address(raw); // ui-error: E0308
    #[cfg(ui_reject)]
    mac.write_rx_dma_address(raw); // ui-error: E0308
    let _ = raw;
}
