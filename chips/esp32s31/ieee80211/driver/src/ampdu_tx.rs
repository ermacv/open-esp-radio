//! Role-neutral A-MPDU publication configuration.
//!
//! Descriptor retention, delimiter formatting and completion live in the MAC
//! crate. This layer makes the remaining role boundary explicit: only the
//! selected virtual interface and hardware key slot differ between an AP and
//! a station publication.

use oer_esp32s31_hal::types::MacInterface;
use oer_esp32s31_ieee80211_mac::tx::ampdu::TX_BLOCK_ACK_MAX_WINDOW;
use oer_esp32s31_ieee80211_mac::tx::{
    HtAmpduTxConfig, HtProtectionSpacing, HtRate, TxControlFrame,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduTxRoleAdapter {
    pub interface: MacInterface,
    pub hardware_key_selector: u8,
}

/// Complete role-owned HT policy for one fresh aggregate.
///
/// Peer lookup and frame encoding remain in AP/STA code. Once admitted, the
/// interface, key, PHY rate and bounded BlockAck window move together so the
/// scheduler cannot independently recompute one of them for standby
/// publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtAmpduTxRolePolicy {
    role: AmpduTxRoleAdapter,
    rate: HtRate,
    frame_limit: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtAmpduTxRolePolicyError {
    EmptyBlockAckWindow,
    BlockAckWindowTooWide(u16),
    EmptyConfiguredLimit,
    ConfiguredLimitTooWide(u8),
    InvalidArenaCapacity(usize),
}

impl HtAmpduTxRolePolicy {
    pub fn new(
        role: AmpduTxRoleAdapter,
        rate: HtRate,
        negotiated_window: u16,
        configured_limit: u8,
        arena_capacity: usize,
    ) -> Result<Self, HtAmpduTxRolePolicyError> {
        if negotiated_window == 0 {
            return Err(HtAmpduTxRolePolicyError::EmptyBlockAckWindow);
        }
        if negotiated_window > TX_BLOCK_ACK_MAX_WINDOW {
            return Err(HtAmpduTxRolePolicyError::BlockAckWindowTooWide(
                negotiated_window,
            ));
        }
        if configured_limit == 0 {
            return Err(HtAmpduTxRolePolicyError::EmptyConfiguredLimit);
        }
        if arena_capacity == 0 || arena_capacity > usize::from(TX_BLOCK_ACK_MAX_WINDOW) {
            return Err(HtAmpduTxRolePolicyError::InvalidArenaCapacity(
                arena_capacity,
            ));
        }
        if u16::from(configured_limit) > TX_BLOCK_ACK_MAX_WINDOW {
            return Err(HtAmpduTxRolePolicyError::ConfiguredLimitTooWide(
                configured_limit,
            ));
        }
        let frame_limit = usize::from(configured_limit)
            .min(usize::from(negotiated_window))
            .min(arena_capacity) as u8;
        Ok(Self {
            role,
            rate,
            frame_limit,
        })
    }

    pub const fn role(self) -> AmpduTxRoleAdapter {
        self.role
    }

    pub const fn rate(self) -> HtRate {
        self.rate
    }

    pub const fn frame_limit(self) -> u8 {
        self.frame_limit
    }
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
