#![no_std]
use oer_chips_esp32s31_hal::radio::bluetooth::ControllerPublicAddress;
pub fn identity(address: ControllerPublicAddress) -> [u8; 6] { address.canonical_bytes() }
#[cfg(ui_reject)]
use oer_chips_esp32s31_hal::radio::bluetooth::DeviceTablePublication; // ui-error: E0603
#[cfg(ui_reject)]
use oer_chips_esp32s31_hal::radio::bluetooth::RxChainHead; // ui-error: E0603
#[cfg(ui_reject)]
use oer_chips_esp32s31_hal::radio::bluetooth::BlePhyHardwareObjects; // ui-error: E0603
#[cfg(ui_reject)]
use oer_chips_esp32s31_hal::radio::bluetooth::DirectionFindingDescriptor; // ui-error: E0603
#[cfg(ui_reject)]
use oer_chips_esp32s31_hal::radio::bluetooth::BluetoothControllerSramAddress; // ui-error: E0432
#[cfg(ui_reject)]
use oer_chips_esp32s31_hal::radio::bluetooth::registers::BluetoothTaskRegisters; // ui-error: E0603
#[cfg(ui_reject)]
use oer_chips_esp32s31_hal::radio::bluetooth::registers::BluetoothPhyRegisterInitInputs; // ui-error: E0603
#[cfg(ui_reject)]
use oer_chips_esp32s31_hal::radio::bluetooth::BluetoothSchedulerSoftwareListRemovalJoin; // ui-error: E0603
#[cfg(ui_reject)]
use oer_chips_esp32s31_hal::radio::bluetooth::BluetoothSchedulerSoftwareListRemovalReady; // ui-error: E0432
