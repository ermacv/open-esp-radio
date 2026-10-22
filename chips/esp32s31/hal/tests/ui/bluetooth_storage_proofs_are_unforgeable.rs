// ui-requires: bluetooth-engine
#![no_std]
use oer_esp32s31_hal::bluetooth::{ControllerHal, RxMemoryListPublished};
use oer_esp32s31_hal::bluetooth::engine::{LeDeviceTable, LeRxChain, BlePhyEngineCpuOwned,
    DirectionFindingWorkspaceCpuOwned, ControllerDmaSpace};

pub fn publish(table: &LeDeviceTable, receive: &LeRxChain<2>, controller: &mut ControllerHal<'_>) {
    table.publish(controller);
    let _ = receive.publish_initial_head(controller);
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
pub fn unbound_phy(phy: &BlePhyEngineCpuOwned) {
    let _ = phy.hardware_objects(); // ui-error: E0624
}
#[cfg(ui_reject)]
pub fn unbound_df(df: &DirectionFindingWorkspaceCpuOwned) {
    let _ = df.disabled_cte_descriptor(); // ui-error: E0624
}
#[cfg(ui_reject)]
pub fn raw_publish(controller: &mut ControllerHal<'_>) {
    let _ = controller.publish_scheduler_hardware_list_head( // ui-error: E0624
        oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListIndex::ZERO, None);
}
#[cfg(ui_reject)]
pub fn raw_base<S: ControllerDmaSpace>() {
    let _ = S::base(&0); // ui-error: E0599
}
