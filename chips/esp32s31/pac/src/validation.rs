//! Owner acquisition for isolated probe images (`cfg(oer_probe)`).
//!
//! Vendor ABI adaptation and named hardware operations belong to the HAL.
//! This module only constructs the same finite PAC owners production code
//! uses, from a root a probe image claims for its one closed comparison; it
//! implements no semantic operation and no comparison verdict.

use crate::{
    MacInterruptRegisters, MacInterruptSetup, MacPowerInterruptRegisters, RadioPartitions,
    SharedRadioParts, SharedRadioRegisters, WifiRadioRegisters, WifiRxDmaRegisters,
};

/// The Wi-Fi MAC register set of one probe image.
#[inline(always)]
pub fn wifi_radio_registers() -> WifiRadioRegisters {
    WifiRadioRegisters::new(RadioPartitions::for_probe().wifi_mac)
}

/// The Wi-Fi RX descriptor walker's registers of one probe image.
#[inline(always)]
pub fn wifi_rx_dma_registers() -> WifiRxDmaRegisters {
    RadioPartitions::for_probe().wifi_rx_dma
}

/// The shared radio owner of one probe image.
#[inline(always)]
pub fn shared_radio_registers() -> SharedRadioRegisters {
    let RadioPartitions {
        radio_phy,
        coexistence,
        shared_radio,
        ..
    } = RadioPartitions::for_probe();
    SharedRadioRegisters::new(SharedRadioParts {
        radio_phy,
        coexistence,
        shared_radio,
    })
}

/// The task-side MAC interrupt setup of one probe image.
#[inline(always)]
pub fn mac_interrupt_setup() -> MacInterruptSetup {
    RadioPartitions::for_probe().wifi_interrupts
}

/// The hard-MAC interrupt registers of one probe image.
#[inline(always)]
pub fn mac_interrupt_registers() -> MacInterruptRegisters {
    MacInterruptRegisters::from_peripheral_for_probe(mac_interrupt_setup().peripheral)
}

/// The WDEVPWR interrupt registers of one probe image.
#[inline(always)]
pub fn mac_power_interrupt_registers() -> MacPowerInterruptRegisters {
    MacPowerInterruptRegisters::from_peripheral_for_probe(mac_interrupt_setup().power_peripheral)
}

mod transactions;
