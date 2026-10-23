// ui-requires: bluetooth-engine
#![no_std]
use oer_esp32s31_hal::bluetooth::{ControllerHal, RxMemoryListPublished};
use oer_esp32s31_hal::bluetooth::engine::{LeDeviceTable, LeRxChain, ControllerDmaSpace, Engine, ArenaMemory};

pub fn publish(engine: &mut Engine<ArenaMemory<'_, '_, 0, 1, 2, 1, 4, 4, 8>>) {
    let _ = engine.publish_device_table();
}
#[cfg(ui_reject)]
pub fn standalone_table(table: &LeDeviceTable, controller: &mut ControllerHal<'_>) {
    table.publish(controller); // ui-error: E0624
}
#[cfg(ui_reject)]
pub fn standalone_receive(receive: &LeRxChain<2>, controller: &mut ControllerHal<'_>) {
    let _ = receive.publish_initial_head(controller); // ui-error: E0624
}
pub fn start(controller: &mut ControllerHal<'_>, scanning: &RxMemoryListPublished) {
    let _ = controller.publish_scan_start(scanning);
}
#[cfg(ui_reject)]
pub fn unbound_table(table: &LeDeviceTable) {
    let _ = table.publication(); // ui-error: E0624
}
#[cfg(ui_reject)]
pub fn unbound_receive(receive: &LeRxChain<2>) {
    let _ = receive.publication_head(); // ui-error: E0624
}
#[cfg(ui_reject)]
pub fn address(scanning: &RxMemoryListPublished) {
    let _ = scanning.head(); // ui-error: E0624
}
#[cfg(ui_reject)]
use oer_esp32s31_hal::bluetooth::engine::BlePhyEngineCpuOwned; // ui-error: E0603
#[cfg(ui_reject)]
use oer_esp32s31_hal::bluetooth::engine::DirectionFindingWorkspaceCpuOwned; // ui-error: E0603
#[cfg(ui_reject)]
pub fn raw_publish(controller: &mut ControllerHal<'_>) {
    let _ = controller.publish_scheduler_hardware_list_head( // ui-error: E0624
        oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListIndex::ZERO, None);
}
#[cfg(ui_reject)]
pub fn raw_base<S: ControllerDmaSpace>() {
    let _ = S::base(&0); // ui-error: E0599
}
