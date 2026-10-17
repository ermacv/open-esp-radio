//! Capability acquisition for isolated compiled probe images.
//!
//! Vendor ABI adaptation and named hardware operations belong to the HAL.
//! This module only constructs the same finite PAC owners used by production
//! code; it does not implement semantic operations or comparison verdicts.

#![deny(unsafe_code)]

use crate::{
    BluetoothInterruptRegisters, BluetoothMemoryListPointerImage, BluetoothMemoryListSelector,
    BluetoothMemoryListSlot, MacInterruptRegisters, MacInterruptSetup, MacPowerInterruptRegisters,
    RadioPartitions,
};

/// Construct the ordinary task-owned register set for one probe image.
#[inline(always)]
pub fn wifi_radio_registers() -> crate::WifiRadioRegisters {
    crate::WifiRadioRegisters::new(RadioPartitions::for_validation().wifi_mac)
}

/// Construct the shared radio owner for one probe image.
#[inline(always)]
pub fn shared_radio_registers() -> crate::SharedRadioRegisters {
    let RadioPartitions {
        radio_phy,
        coexistence,
        shared_radio,
        ..
    } = RadioPartitions::for_validation();
    crate::SharedRadioRegisters::new(crate::SharedRadioParts {
        radio_phy,
        coexistence,
        shared_radio,
    })
}

/// Construct the task-side interrupt setup owner used by the production
/// connected-STA preparation transaction.
#[inline(always)]
pub fn mac_interrupt_setup() -> MacInterruptSetup {
    RadioPartitions::for_validation().wifi_interrupts
}

/// Construct the disjoint hard-MAC interrupt capability for one probe image.
#[inline(always)]
pub fn mac_interrupt_registers() -> MacInterruptRegisters {
    MacInterruptRegisters::from_peripheral_for_validation(mac_interrupt_setup().peripheral)
}

/// Construct the disjoint power-interrupt capability for one probe image.
#[inline(always)]
pub fn mac_power_interrupt_registers() -> MacPowerInterruptRegisters {
    MacPowerInterruptRegisters::from_peripheral_for_validation(
        mac_interrupt_setup().power_peripheral,
    )
}

/// Construct the active Bluetooth interrupt-bank capability for one isolated
/// compiled probe image.
///
/// Ordinary production code cannot call this constructor: the validation
/// module is absent unless `validation-probes` is enabled. Production can
/// reach the same register owner only by consuming inactive ownership through
/// baseline prepare and explicit shared-ISR staging; this isolated probe
/// deliberately bypasses that lifecycle to compare one bounded transaction.
#[inline(always)]
pub fn bluetooth_interrupt_registers() -> BluetoothInterruptRegisters {
    BluetoothInterruptRegisters {
        peripherals: RadioPartitions::for_validation()
            .bluetooth_interrupts
            .peripherals,
    }
}

/// Construct the Bluetooth task register set and its inactive interrupt bank.
#[inline(always)]
fn bluetooth_task() -> (
    crate::BluetoothTaskRegisters,
    crate::BluetoothInterruptSetup,
) {
    let RadioPartitions {
        bluetooth,
        bluetooth_interrupts,
        ..
    } = RadioPartitions::for_validation();
    (
        crate::BluetoothTaskRegisters::new(bluetooth),
        bluetooth_interrupts,
    )
}

/// Execute the exact bounded MMIO transaction recovered for
/// `bt_bb_v2_init_cmplx(1)` inside one isolated comparison image.
///
/// The bridge calls the same hidden PAC SPI used by the production
/// post-common-PHY typestate. It bypasses lifecycle ownership only inside this
/// isolated validation image, so the comparison exercises the shipping MMIO
/// implementation rather than a shadow model.
///
/// # Safety
///
/// The caller must provide an isolated validation image whose modeled state
/// satisfies the common-PHY prerequisite of
/// `SharedRadioRegisters::initialize_btbb_v2_arg_one`. No other radio
/// owner may be used after this function returns: the powered task and
/// interrupt partitions are deliberately retained because verified Bluetooth
/// teardown is not implemented yet.
#[allow(
    unsafe_code,
    reason = "the validation bridge explicitly exposes the modeled common-PHY prerequisite"
)]
#[inline(always)]
pub unsafe fn initialize_bluetooth_baseband_v2(gain_parameter: u8) {
    let (task, interrupts) = bluetooth_task();
    let mut shared = shared_radio_registers();
    shared.initialize_btbb_v2_arg_one(gain_parameter);
    let _powered_owners = (task, shared, interrupts);
}

/// Execute one exact controller memory-list pointer publication inside an
/// isolated comparison image.
///
/// The bridge calls the shipping hidden PAC SPI and retains both Bluetooth
/// owners locally. It does not manufacture an active lifecycle state or
/// reconstruct cold ownership after the transaction.
///
/// # Safety
///
/// The caller must provide an isolated validation image whose modeled state
/// permits the selected controller list to be changed. For an address image,
/// the caller must also model correctly initialized, exclusively serialized
/// storage that remains alive for the rest of the image execution.
#[allow(
    unsafe_code,
    reason = "the validation bridge preserves controller-lifecycle and pointed-storage prerequisites"
)]
#[inline(always)]
pub unsafe fn program_bluetooth_memory_list_pointer(
    selector: BluetoothMemoryListSelector,
    slot: BluetoothMemoryListSlot,
    image: BluetoothMemoryListPointerImage,
) {
    let (mut task, interrupts) = bluetooth_task();
    // SAFETY: the caller upholds this bridge's modeled list-change and
    // pointed-storage contract; `task` is the sole validation owner.
    unsafe {
        task.program_memory_list_pointer(selector, slot, image);
    }
    let _powered_owners = (task, interrupts);
}

#[cfg(feature = "validation-probes")]
pub(crate) mod transactions;
