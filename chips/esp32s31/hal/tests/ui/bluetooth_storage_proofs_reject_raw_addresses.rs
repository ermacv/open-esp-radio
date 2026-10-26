// ui-requires: bluetooth-engine
#![no_std]
use oer_esp32s31_hal::bluetooth::engine::{ArenaMemory, Engine};

// The owning engine remains the public publication path.
pub fn publish(engine: &mut Engine<ArenaMemory<'_, '_, 0, 1, 2, 1, 4, 4, 8>>) {
    let _ = engine.publish_device_table();
}

#[cfg(ui_reject)]
pub fn forge_from_address(address: u32) {
    use oer_esp32s31_hal::bluetooth::BluetoothMemoryListSelector;
    let _ = oer_esp32s31_hal::bluetooth::RxChainHead::new( // ui-error: E0603
        BluetoothMemoryListSelector::One, address, false); // ui-error: E0308
    let _ = oer_esp32s31_hal::bluetooth::DeviceTablePublication::new( // ui-error: E0603
        address, oer_mmio::Index::constant::<0>()); // ui-error: E0308
    let _ = oer_esp32s31_hal::bluetooth::DirectionFindingDescriptor::bind(address); // ui-error: E0603
    // ui-error@-1: E0308
    let _ = oer_esp32s31_hal::bluetooth::BlePhyHardwareObjects::bind(address); // ui-error: E0603
    // ui-error@-1: E0308
}

#[cfg(ui_reject)]
pub fn forge_count(table: oer_esp32s31_hal::layouts::DmaRef<oer_esp32s31_hal::layouts::BleDeviceTable>) {
    let _ = oer_esp32s31_hal::bluetooth::DeviceTablePublication::new( // ui-error: E0603
        table, u8::MAX); // ui-error: E0308
}
