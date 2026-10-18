//! ESP32-S31 FTM admission and reviewed-source frontier.
//!
//! The recovered PHY enable bit is necessary but not sufficient for FTM. This
//! module never converts that leaf into a capability bit, frame publication or
//! distance result. Connected operation remains rejected until the timestamp
//! and calibration ownership listed by [`station_ftm_hardware_frontier`] is
//! implemented from reviewed evidence.

use oer_esp32s31_hal::{owner::SharedPhyAccess, phy::agc as phy_agc};

use oer_ieee80211_mac::ftm::{FtmRequest, FtmTrigger};

use oer_ieee80211_sta::ftm::FtmRequestTransmission;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationFtmFrontierStatus {
    ProductionSource,
    ReversibleSourceTransaction,
    MissingReviewedSemantics,
    IntentionallyDisabled,
}

/// Static source-only FTM frontier; reading it performs no MMIO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationFtmHardwareFrontier {
    pub action_codec: StationFtmFrontierStatus,
    pub requester_session: StationFtmFrontierStatus,
    pub phy_enable_leaf: StationFtmFrontierStatus,
    pub runtime_phy_owner_binding: StationFtmFrontierStatus,
    pub ftm_preamble_rx_timestamp: StationFtmFrontierStatus,
    pub ack_departure_tx_timestamp: StationFtmFrontierStatus,
    pub timestamp_clock_contract: StationFtmFrontierStatus,
    pub antenna_delay_calibration: StationFtmFrontierStatus,
    pub action_publication_completion: StationFtmFrontierStatus,
    pub advertised_initiator_capability: StationFtmFrontierStatus,
    pub distance_estimate: StationFtmFrontierStatus,
}

pub const fn station_ftm_hardware_frontier() -> StationFtmHardwareFrontier {
    StationFtmHardwareFrontier {
        action_codec: StationFtmFrontierStatus::ProductionSource,
        requester_session: StationFtmFrontierStatus::ProductionSource,
        phy_enable_leaf: StationFtmFrontierStatus::ReversibleSourceTransaction,
        runtime_phy_owner_binding: StationFtmFrontierStatus::MissingReviewedSemantics,
        ftm_preamble_rx_timestamp: StationFtmFrontierStatus::MissingReviewedSemantics,
        ack_departure_tx_timestamp: StationFtmFrontierStatus::MissingReviewedSemantics,
        timestamp_clock_contract: StationFtmFrontierStatus::MissingReviewedSemantics,
        antenna_delay_calibration: StationFtmFrontierStatus::MissingReviewedSemantics,
        action_publication_completion: StationFtmFrontierStatus::MissingReviewedSemantics,
        advertised_initiator_capability: StationFtmFrontierStatus::IntentionallyDisabled,
        distance_estimate: StationFtmFrontierStatus::IntentionallyDisabled,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationFtmHardwareStage {
    None,
    PortableInitialRequestValidated,
    PhyEnableLeafRestored { previous_enabled: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationFtmUnsupportedStage {
    RuntimePhyOwnerBinding,
    FtmPreambleRxTimestampCapture,
    AckDepartureTxTimestampCapture,
    TimestampClockContract,
    AntennaDelayCalibration,
    ActionPublicationCompletion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationFtmHardwareError {
    InvalidPortableRequest,
    PhyEnableReadbackMismatch,
    PhyEnableRestoreMismatch,
    Unsupported {
        reached: StationFtmHardwareStage,
        missing: StationFtmUnsupportedStage,
    },
}

/// Validate the exact body produced by the portable requester, then stop at
/// the first missing connected-runtime owner. No sequence number, DMA buffer,
/// PHY bit or capability field is changed.
pub fn station_ftm_request_frontier(
    transmission: &FtmRequestTransmission,
) -> StationFtmHardwareError {
    let Ok(request) = FtmRequest::decode_body(transmission.body()) else {
        return StationFtmHardwareError::InvalidPortableRequest;
    };
    if request.trigger != FtmTrigger::StartOrContinue || request.parameters.is_none() {
        return StationFtmHardwareError::InvalidPortableRequest;
    }
    StationFtmHardwareError::Unsupported {
        reached: StationFtmHardwareStage::PortableInitialRequestValidated,
        missing: StationFtmUnsupportedStage::RuntimePhyOwnerBinding,
    }
}

/// Exercise and exactly restore the recovered `phy_set_ftm_en` leaf.
///
/// This is an explicit source-frontier probe, not connected FTM admission. A
/// successful round trip still returns `Unsupported` before any Action frame
/// can be published because the local antenna-point timestamp capture is not
/// mapped in reviewed ESP32-S31 evidence.
pub fn probe_station_ftm_phy_enable(
    registers: &mut impl SharedPhyAccess,
) -> Result<(), StationFtmHardwareError> {
    let restore = phy_agc::prepare_ftm_enabled(registers, true);
    let previous_enabled = restore.previous();
    if !phy_agc::ftm_enabled(registers) {
        restore.restore(registers);
        if phy_agc::ftm_enabled(registers) != previous_enabled {
            return Err(StationFtmHardwareError::PhyEnableRestoreMismatch);
        }
        return Err(StationFtmHardwareError::PhyEnableReadbackMismatch);
    }
    restore.restore(registers);
    if phy_agc::ftm_enabled(registers) != previous_enabled {
        return Err(StationFtmHardwareError::PhyEnableRestoreMismatch);
    }
    Err(StationFtmHardwareError::Unsupported {
        reached: StationFtmHardwareStage::PhyEnableLeafRestored { previous_enabled },
        missing: StationFtmUnsupportedStage::FtmPreambleRxTimestampCapture,
    })
}

#[cfg(test)]
mod tests;
