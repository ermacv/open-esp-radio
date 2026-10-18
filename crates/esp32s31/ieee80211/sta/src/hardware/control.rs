//! ESP32-S31 hardware contract used by connected-station control.
//!
//! The protocol state machine depends on this narrow interface rather than
//! the PAC owner directly.  The contract belongs to the chip composition: it
//! describes TSF, RX BlockAck and HE-TID operations, but no executor wakeups
//! or Embassy task lifecycle.

use oer_esp32s31_hal::types::MacStaReceivePolicySnapshot;

use oer_esp32s31_ieee80211::{
    cooperative_hardware::CooperativeRadioHardware, station_tsf::StationTsfHardware,
};

use oer_esp32s31_ieee80211_mac::{
    crypto::{StaGroupCcmpKeyMaterial, StaGroupCcmpReplaceError, StaGroupCcmpSlot},
    init::MacRuntimeStopHardware,
    rx::hardware::{RxBlockAckHardware, S31RxBlockAckAgreementError},
    tx::TxHardware,
};

use oer_ieee80211_sta::twt::{IndividualTwtAgreement, IndividualTwtProposal};

use oer_esp32s31_hal::types::{MacPti, StaTbttSchedule, StaTbttScheduleError};

/// Deepest source-proven stage of one S31 individual-TWT handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationIndividualTwtHardwareStage {
    None,
}

/// First hardware semantic missing from reviewed S31 source/SVD evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationIndividualTwtUnsupportedStage {
    /// The MAC exposes an ITWT/PTI leaf, but reviewed evidence does not map
    /// individual flow IDs and accepted TSF schedules onto that register or
    /// prove coexistence admission ordering.
    ItwtCoexistenceAdmission,
    /// No reviewed source binds an accepted TWT service period to a station
    /// TSF wake compare plus RF/PHY/BB retention transaction.
    WakeScheduleProgramming,
    /// No successful installation exists whose exact register image can be
    /// restored while preserving another flow.
    WakeScheduleRestore,
}

/// Typed fail-closed S31 boundary; no variant implies that RF sleep occurred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationIndividualTwtHardwareError {
    Unsupported {
        reached: StationIndividualTwtHardwareStage,
        missing: StationIndividualTwtUnsupportedStage,
    },
}

/// ESP32-S31 register operations required by connected BlockAck control.
///
/// The station TSF is written only through the connection's
/// [`StationTsf`](oer_esp32s31_ieee80211::station_tsf::StationTsf) owner.
pub trait ConnectedControlHardware:
    TxHardware + RxBlockAckHardware + MacRuntimeStopHardware + StationTsfHardware
{
    /// The Wi-Fi MAC local time: the free-running microsecond counter whose
    /// readings receive timestamps carry, wrapping in 32 bits.
    fn mac_local_time(&mut self) -> u32;

    /// Close interface-zero RX admission while its descriptor ring is still
    /// live. This is a lifecycle edge, not control cleanup: callers must
    /// perform it before interrupt quiescence or DMA walker stop.
    fn disable_station_receive_policy(&mut self);

    /// Read the reviewed interface-zero RX identity/filter projection used by
    /// automatic beacon-monitor admission.
    ///
    /// The default is deliberately absent rather than a fabricated zero
    /// image. Hardware-independent test doubles and ports without this exact
    /// PAC readback therefore retain the software monitor and fail closed at
    /// `MissingAssociationReadback`.
    fn station_beacon_monitor_readback(&mut self) -> Option<MacStaReceivePolicySnapshot> {
        None
    }

    fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError>;

    /// Program the station TBTT schedule of power management.
    fn start_station_tbtt(&mut self, schedule: StaTbttSchedule);

    /// Replace the running station TBTT interval.
    fn set_station_tbtt_interval(
        &mut self,
        interval_micros: u32,
    ) -> Result<(), StaTbttScheduleError>;

    /// Replace the station TBTT lead and the wake lead beside it.
    fn set_station_tbtt_ahead(&mut self, ahead_micros: u16, wake_ahead_micros: u16);

    /// Stop the station TBTT schedule.
    fn stop_station_tbtt(&mut self);

    /// Block or unblock the TX queues for station power management.
    fn set_power_save_tx_block(&mut self, blocked: bool);

    /// Publish the beacon receive priority.
    fn set_rx_beacon_pti(&mut self, pti: MacPti);

    /// Clear the beacon receive priority request.
    fn clear_rx_beacon_pti(&mut self);

    /// Publish the hardware beacon receive window and time.
    fn set_rx_beacon_time(&mut self, window_micros: u16, time_micros: u32);

    /// Prove that this exact proposal can be represented before the shared TX
    /// owner is allowed to publish a TWT Setup request. An error must not
    /// change MAC, coexistence, clock, RF, PHY or BB state.
    fn admit_station_individual_twt(
        &mut self,
        _proposal: &IndividualTwtProposal,
    ) -> Result<(), StationIndividualTwtHardwareError> {
        Err(StationIndividualTwtHardwareError::Unsupported {
            reached: StationIndividualTwtHardwareStage::None,
            missing: StationIndividualTwtUnsupportedStage::ItwtCoexistenceAdmission,
        })
    }

    /// Install one peer-accepted agreement atomically. Failure must retain the
    /// awake pre-call state so connected control can send a teardown.
    fn install_station_individual_twt(
        &mut self,
        _agreement: &IndividualTwtAgreement,
    ) -> Result<(), StationIndividualTwtHardwareError> {
        Err(StationIndividualTwtHardwareError::Unsupported {
            reached: StationIndividualTwtHardwareStage::None,
            missing: StationIndividualTwtUnsupportedStage::WakeScheduleProgramming,
        })
    }

    /// Remove an installed agreement before local teardown, stop or
    /// reconnect. Failure requires the outer owner to quarantine the epoch.
    fn remove_station_individual_twt(
        &mut self,
        _agreement: &IndividualTwtAgreement,
    ) -> Result<(), StationIndividualTwtHardwareError> {
        Err(StationIndividualTwtHardwareError::Unsupported {
            reached: StationIndividualTwtHardwareStage::None,
            missing: StationIndividualTwtUnsupportedStage::WakeScheduleRestore,
        })
    }

    /// Atomically consume the current association's authority to replace its
    /// single active group-key entry. Hardware test doubles which do not
    /// exercise WPA2 rekey may retain the fail-closed default.
    fn replace_sta_group_ccmp(
        &mut self,
        _slot: &mut StaGroupCcmpSlot,
        _current: &StaGroupCcmpKeyMaterial,
        _replacement: &StaGroupCcmpKeyMaterial,
    ) -> Result<(), StaGroupCcmpReplaceError> {
        Err(StaGroupCcmpReplaceError::InvalidReplacement(
            oer_esp32s31_ieee80211_mac::crypto::CryptoKeyError::HardwareRejected,
        ))
    }
}

impl ConnectedControlHardware for CooperativeRadioHardware<'_> {
    fn mac_local_time(&mut self) -> u32 {
        CooperativeRadioHardware::mac_local_time(self)
    }

    fn disable_station_receive_policy(&mut self) {
        CooperativeRadioHardware::disable_station_receive_policy(self);
    }

    fn station_beacon_monitor_readback(&mut self) -> Option<MacStaReceivePolicySnapshot> {
        Some(self.sta_receive_policy_snapshot())
    }

    fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        CooperativeRadioHardware::set_he_tid_enabled(self, tid, enabled)
    }

    fn start_station_tbtt(&mut self, schedule: StaTbttSchedule) {
        CooperativeRadioHardware::start_station_tbtt(self, schedule);
    }

    fn set_station_tbtt_interval(
        &mut self,
        interval_micros: u32,
    ) -> Result<(), StaTbttScheduleError> {
        CooperativeRadioHardware::set_station_tbtt_interval(self, interval_micros)
    }

    fn set_station_tbtt_ahead(&mut self, ahead_micros: u16, wake_ahead_micros: u16) {
        CooperativeRadioHardware::set_station_tbtt_ahead(self, ahead_micros, wake_ahead_micros);
    }

    fn stop_station_tbtt(&mut self) {
        CooperativeRadioHardware::stop_station_tbtt(self);
    }

    fn set_power_save_tx_block(&mut self, blocked: bool) {
        CooperativeRadioHardware::set_power_save_tx_block(self, blocked);
    }

    fn set_rx_beacon_pti(&mut self, pti: MacPti) {
        CooperativeRadioHardware::set_rx_beacon_pti(self, pti);
    }

    fn clear_rx_beacon_pti(&mut self) {
        CooperativeRadioHardware::clear_rx_beacon_pti(self);
    }

    fn set_rx_beacon_time(&mut self, window_micros: u16, time_micros: u32) {
        CooperativeRadioHardware::set_rx_beacon_time(self, window_micros, time_micros);
    }

    fn admit_station_individual_twt(
        &mut self,
        _proposal: &IndividualTwtProposal,
    ) -> Result<(), StationIndividualTwtHardwareError> {
        // The reviewed SVD names MAC_COEX ITWT/PTI registers but does not
        // define their per-flow encoding, ownership or relation to the STA
        // TSF wake transaction. Do not probe or guess those bits.
        Err(StationIndividualTwtHardwareError::Unsupported {
            reached: StationIndividualTwtHardwareStage::None,
            missing: StationIndividualTwtUnsupportedStage::ItwtCoexistenceAdmission,
        })
    }

    fn replace_sta_group_ccmp(
        &mut self,
        slot: &mut StaGroupCcmpSlot,
        current: &StaGroupCcmpKeyMaterial,
        replacement: &StaGroupCcmpKeyMaterial,
    ) -> Result<(), StaGroupCcmpReplaceError> {
        CooperativeRadioHardware::replace_sta_group_ccmp_with_rollback(
            self,
            slot,
            current,
            replacement,
        )
    }
}
