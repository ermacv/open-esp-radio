#![no_std]
use oer_esp32s31_hal::layouts::DmaWindowAddress;
use oer_esp32s31_pac::ieee802154::Ieee802154MacTask;

pub fn publish(mac: &mut Ieee802154MacTask, address: DmaWindowAddress, raw: u32) {
    mac.write_tx_dma_address(address, |_| {});
    mac.write_rx_dma_address(address, |_| {});
    #[cfg(ui_reject)]
    mac.write_tx_dma_address(raw, |_| {}); // ui-error: E0308
    #[cfg(ui_reject)]
    mac.write_rx_dma_address(raw, |_| {}); // ui-error: E0308
    let _ = raw;
}
