//! Role-neutral A-MPDU publication configuration.
//!
//! Descriptor retention, delimiter formatting and completion live in the MAC
//! crate. This layer makes the remaining role boundary explicit: only the
//! selected virtual interface and hardware key slot differ between an AP and
//! a station publication.

use crate::mac::tx::{HtAmpduTxConfig, HtProtectionSpacing, HtRate, TxControlFrame};
use oer_esp32s31_hal::types::MacInterface;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduTxRoleAdapter {
    pub interface: MacInterface,
    pub hardware_key_selector: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtAmpduPublicationInputs {
    pub rate: HtRate,
    pub aggregate_length: u16,
    pub subframes: u8,
    pub protection_spacing: HtProtectionSpacing,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub control: TxControlFrame,
    pub aifsn: u8,
    pub contention_window: u16,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
}

pub fn ht_ampdu_publication_config(
    role: AmpduTxRoleAdapter,
    inputs: HtAmpduPublicationInputs,
) -> Option<HtAmpduTxConfig> {
    let mut config = HtAmpduTxConfig::new(inputs.rate, inputs.aggregate_length, inputs.subframes)?;
    config.protection_spacing = inputs.protection_spacing;
    config.data_power_primary = inputs.data_power_primary;
    config.data_power_alternate = inputs.data_power_alternate;
    config.control = inputs.control;
    config.aifsn = inputs.aifsn;
    config.contention_window = inputs.contention_window;
    config.interface = role.interface;
    config.scheduler_priority = inputs.scheduler_priority;
    config.pti = inputs.packet_priority;
    config.pti_count = 1;
    config.hardware_key_selector = role.hardware_key_selector;
    Some(config)
}

#[cfg(test)]
mod tests;
