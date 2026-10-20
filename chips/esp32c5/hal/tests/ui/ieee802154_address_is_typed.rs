#![no_std]
use oer_esp32c5_pac::Ieee802154RegisterLease;
use oer_esp32c5_hal::layouts::DmaWindowAddress;

pub fn publish(lease: &mut Ieee802154RegisterLease<'_>, address: DmaWindowAddress, raw: u32) {
    lease.publish_transmit_dma_address(address);
    lease.publish_receive_dma_address(address);
    #[cfg(ui_reject)]
    lease.publish_transmit_dma_address(raw); // ui-error: E0308
    #[cfg(ui_reject)]
    lease.publish_receive_dma_address(raw); // ui-error: E0308
    let _ = raw;
}
