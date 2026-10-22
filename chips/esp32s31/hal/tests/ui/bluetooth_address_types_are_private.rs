#![no_std]
use oer_esp32s31_hal::bluetooth::ControllerPublicAddress;
pub fn identity(address: ControllerPublicAddress) -> [u8; 6] { address.canonical_bytes() }
#[cfg(ui_reject)]
use oer_esp32s31_hal::bluetooth::DeviceTablePublication; // ui-error: E0603
#[cfg(ui_reject)]
use oer_esp32s31_hal::bluetooth::RxChainHead; // ui-error: E0603
#[cfg(ui_reject)]
use oer_esp32s31_hal::bluetooth::BlePhyHardwareObjects; // ui-error: E0603
#[cfg(ui_reject)]
use oer_esp32s31_hal::bluetooth::DirectionFindingDescriptor; // ui-error: E0603
#[cfg(ui_reject)]
use oer_esp32s31_hal::bluetooth::BluetoothControllerSramAddress; // ui-error: E0432
#[cfg(ui_reject)]
use oer_esp32s31_hal::types::BluetoothControllerSramAddress as RawControllerAddress; // ui-error: E0603
#[cfg(ui_reject)]
use oer_esp32s31_hal::types::BluetoothPhyRegisterInitInputs; // ui-error: E0603
#[cfg(ui_reject)]
use oer_esp32s31_hal::bluetooth::BluetoothSchedulerSoftwareListRemovalJoin; // ui-error: E0603
#[cfg(ui_reject)]
use oer_esp32s31_hal::bluetooth::BluetoothSchedulerSoftwareListRemovalReady; // ui-error: E0432
