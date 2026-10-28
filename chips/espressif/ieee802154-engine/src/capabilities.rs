//! The portable capabilities the engine implements, as ESP-IDF's OpenThread
//! port reports them for the radio, and the capability table a chip
//! publishes from its own transmit-power level set.
//!
//! The CCA threshold, CSL, enhanced-ACK capacities and the other values are
//! those of an IEEE 802.15.4 radio role. The operation set is that role's
//! without `CSMA_CA` and `TRANSMIT_RETRIES`, which belong above the port. No
//! port backend in this repository publishes them.
//!
//! The validated-level adapter is from ESP-IDF
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`,
//! `components/ieee802154/driver/esp_ieee802154_pib.c` (Copyright Espressif
//! Systems (Shanghai) Co Ltd, Apache-2.0), ported and modified by Vladislav
//! Ermachenkov; see the root `NOTICE`.

use oer_espressif_ieee802154_mac_values::Ieee802154MultipanIndex;
use oer_ieee802154_mac::{
    CcaThresholdCapability, CslCapability, Ieee802154Capabilities, RadioCapabilities,
    TransmitPowerCapability, TransmitPowerLevels,
};

use crate::engine::RECEIVE_SENSITIVITY_DBM;
use crate::pib::CCA_THRESHOLD_RANGE_DBM;

/// Convert a validated chip level set into the portable port's vocabulary.
pub const fn portable_tx_power_levels(
    levels: oer_espressif_ieee802154_mac_values::Ieee802154TxPowerLevels<'static>,
) -> TransmitPowerLevels {
    // Both validations accept a non-empty, non-decreasing sequence.
    match TransmitPowerLevels::new(levels.levels_dbm()) {
        Ok(levels) => levels,
        Err(_) => unreachable!(),
    }
}

/// The portable operations a radio over this engine implements.
///
/// Source matching without an interface configures the frame-pending table
/// of interface zero, as ESP-IDF's single-interface API does. An engine
/// built with multi-PAN adds [`RadioCapabilities::MULTI_PAN`]
/// ([`MULTIPAN_CAPABILITIES`]). The engine transmits each frame once:
/// CSMA-CA backoffs and transmit retries belong above the port, so neither
/// is listed.
pub const RADIO_CAPABILITIES: RadioCapabilities = RadioCapabilities::NONE
    .union(RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT)
    .union(RadioCapabilities::SECURITY_OFFLOAD)
    .union(RadioCapabilities::ENERGY_SCAN)
    .union(RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT)
    .union(RadioCapabilities::SCHEDULED_TRANSMIT)
    .union(RadioCapabilities::SCHEDULED_RECEIVE)
    .union(RadioCapabilities::PROMISCUOUS)
    .union(RadioCapabilities::RECEIVE_TIMESTAMP)
    .union(RadioCapabilities::AUTOMATIC_ACKNOWLEDGEMENT)
    .union(RadioCapabilities::SOURCE_MATCH)
    .union(RadioCapabilities::TIME_SYNC);

/// [`RADIO_CAPABILITIES`] of a multi-PAN engine.
pub const MULTIPAN_CAPABILITIES: RadioCapabilities =
    RADIO_CAPABILITIES.union(RadioCapabilities::MULTI_PAN);

/// The CSL timing ESP-IDF's OpenThread port reports for the radio: a drift
/// of ± 50 ppm (`CONFIG_OPENTHREAD_CSL_ACCURACY` default) and an uncertainty
/// of 50 × 10 microseconds (`CONFIG_OPENTHREAD_CSL_UNCERTAIN` default).
pub const CSL_CAPABILITY: CslCapability = CslCapability {
    accuracy_ppm: 50,
    uncertainty_10us: 50,
};

/// Header IE bytes an enhanced ACK carries at most (OpenThread
/// `OT_ACK_IE_MAX_SIZE`).
pub const ENHANCED_ACK_IE_CAPACITY: usize = 16;

/// Probing initiators the enhanced-ACK generator serves at most:
/// `OPENTHREAD_CONFIG_MLE_LINK_METRICS_MAX_SERIES_SUPPORTED`, which is
/// OpenThread's `OPENTHREAD_CONFIG_MLE_MAX_CHILDREN`, ten in ESP-IDF's
/// default configuration (`CONFIG_OPENTHREAD_MLE_MAX_CHILDREN`).
pub const ENH_ACK_PROBING_CAPACITY: usize = 10;

/// The CCA threshold the radio starts with: the PIB default
/// (`CONFIG_IEEE802154_CCA_THRESHOLD`,
/// [`Ieee802154PibDefaults::default`](crate::pib::Ieee802154PibDefaults)).
pub const CCA_RESET_DBM: i8 = -75;

/// The capabilities of a radio over this engine whose transmit power radiates
/// `levels` (a chip's level set, [`portable_tx_power_levels`]): every
/// channel starts at the highest level, the CCA threshold at
/// [`CCA_RESET_DBM`] within [`CCA_THRESHOLD_RANGE_DBM`]. A multi-PAN engine
/// addresses [`Ieee802154MultipanIndex::COUNT`] interfaces, any other one.
pub const fn capabilities(levels: TransmitPowerLevels, multipan: bool) -> Ieee802154Capabilities {
    let (operations, interfaces) = if multipan {
        (MULTIPAN_CAPABILITIES, Ieee802154MultipanIndex::COUNT)
    } else {
        (RADIO_CAPABILITIES, 1)
    };
    Ieee802154Capabilities {
        operations,
        interfaces,
        transmit_power: Some(TransmitPowerCapability {
            levels,
            reset_dbm: levels.highest_dbm(),
        }),
        cca_threshold: Some(CcaThresholdCapability {
            lowest_dbm: *CCA_THRESHOLD_RANGE_DBM.start(),
            highest_dbm: *CCA_THRESHOLD_RANGE_DBM.end(),
            reset_dbm: CCA_RESET_DBM,
        }),
        receive_sensitivity_dbm: RECEIVE_SENSITIVITY_DBM,
        csl: Some(CSL_CAPABILITY),
        enhanced_ack_header_ies: ENHANCED_ACK_IE_CAPACITY as u8,
    }
}

#[cfg(test)]
mod tests;
