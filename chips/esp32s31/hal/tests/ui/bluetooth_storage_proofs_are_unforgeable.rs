#![no_std]
use oer_esp32s31_hal::bluetooth::{
    BluetoothDeviceTableEntryCount, ControllerHal, DeviceTablePublication, RxChainHead,
    RxMemoryListPublished,
};
use oer_esp32s31_hal::types::BluetoothControllerSramAddress;

// The memory crate's codec vouches for the storage, in an unsafe block.
pub fn vouched(
    first: BluetoothControllerSramAddress,
    count: BluetoothDeviceTableEntryCount,
) -> DeviceTablePublication {
    // SAFETY: the case only checks that the constructor needs `unsafe`.
    unsafe { DeviceTablePublication::new(first, count) }
}

// Safe code does not vouch for storage.
#[cfg(ui_reject)]
pub fn unvouched(
    first: BluetoothControllerSramAddress,
    count: BluetoothDeviceTableEntryCount,
) -> DeviceTablePublication {
    DeviceTablePublication::new(first, count) // ui-error: E0133
}

// A chain head is not filled in outside its codec.
#[cfg(ui_reject)]
pub fn forged_head() -> RxChainHead {
    RxChainHead { scanning: true } // ui-error: cannot construct `RxChainHead` with struct literal syntax due to private fields
}

// The scanner starts only with a published list.
#[cfg(ui_reject)]
pub fn start_without_a_list(controller: &mut ControllerHal<'_>) {
    let _ = controller.publish_scan_start(); // ui-error: E0061
}

pub fn start(controller: &mut ControllerHal<'_>, scanning: &RxMemoryListPublished) {
    let _ = controller.publish_scan_start(scanning);
}
