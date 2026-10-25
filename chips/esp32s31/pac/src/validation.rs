//! Owner acquisition for isolated probe images (`cfg(oer_probe)`).
//!
//! Vendor ABI adaptation and named hardware operations belong to the HAL.
//! This module only hands out the same partitions production code takes,
//! from a root a probe image claims for its one closed comparison; it
//! implements no semantic operation and no comparison verdict.

use crate::{
    RadioPartitions, SharedRadioParts, SharedRadioRegisters, WifiInterruptPartition,
    WifiMacPartition, wifi_mac_rx_dma::WifiMacRxDma,
};

/// The Wi-Fi MAC partition of one probe image.
#[inline(always)]
pub fn wifi_mac_partition() -> WifiMacPartition {
    RadioPartitions::for_probe().wifi_mac
}

/// The Wi-Fi RX descriptor walker's registers of one probe image.
#[inline(always)]
pub fn wifi_rx_dma() -> WifiMacRxDma {
    RadioPartitions::for_probe().wifi_rx_dma
}

/// The Wi-Fi interrupt banks of one probe image.
#[inline(always)]
pub fn wifi_interrupts() -> WifiInterruptPartition {
    RadioPartitions::for_probe().wifi_interrupts
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
