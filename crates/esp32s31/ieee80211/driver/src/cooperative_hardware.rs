//! Cooperative register access for one task-owned ESP32-S31 radio.
//!
//! A TX future must not retain a mutable PAC borrow while it waits for a
//! completion interrupt. The vendor FIQ services RX-success before
//! TX-complete, so a single-task Rust runtime needs to let its RX bottom half
//! borrow the same register owner between finite TX hardware transactions.

use crate::station_tsf::StationTsfWrite;
use oer_esp32s31_hal::{
    ieee80211::{
        arena::{PublishedRadioOwner, RadioAccess, RadioOwnerArenaError, ReclaimedRadioOwner},
        mac::WifiMacHal,
    },
    owner::RadioRuntimeOwner,
    types::{
        MacAssociationId, MacHe20PeerConfig, MacHe20PeerError, MacHeBeamformingReportProfile,
        MacHeErSuAckRateProfile, MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit,
        MacHeTid, MacHeTriggerTxQueueSnapshot, MacHeTxProgram, MacHtAmpduCompletionObservation,
        MacHtTxProgram, MacKeyInstallOutcome, MacLegacyTxProgram, MacMinimumMpduStartSpacing,
        MacRxDmaSnapshot, MacStaReceivePolicySnapshot, MacTxCompletionObservation,
        MacTxDetachOutcome, MacTxDetachReason, MacTxQueueDetached,
    },
};
use oer_ieee80211_mac::sequence::SequenceNumber;

use oer_esp32s31_ieee80211_mac::{
    ap_policy::ApRxPolicyHardware,
    ap_tsf::ApTsfHardware,
    crypto::{
        CcmpKeyHardware, CryptoKeyError, StaGroupCcmpKeyMaterial, StaGroupCcmpReplaceError,
        StaGroupCcmpSlot, replace_sta_group_ccmp, replace_sta_group_ccmp_with_rollback,
    },
    he::He20PeerHardware,
    init::{
        MacRuntimeStopHardware, MacSnifferHardware, StaLinkRxPolicyHardware, StaNoiseFloorHardware,
    },
    rate::{
        control::BeamformingReportHardware,
        low::{MacLowRateGateProbe, MacLowRateTransitionError},
    },
    rx::{
        RxDma, RxDmaBinding, RxDmaCursorObservation, RxDmaReloadSettled, RxDmaWalkerEnabled,
        RxDmaWalkerStopped,
        hardware::{self as rx_ampdu_hw, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    },
    sta_ap_registers::StaApRegisterHardware,
    tx::{HardwareOwnedTxDma, PreparedTxDma, TxHardware, ampdu::HtAmpduHardware},
};

/// Short-lived radio facade over the task's sole opaque runtime owner.
///
/// Every trait method borrows the register owner only for one synchronous
/// hardware transaction. Consequently an async TX operation may keep this
/// facade across `.await` without keeping the PAC mutably borrowed; the same
/// task can service a pending RX-success edge through the original
/// [`RefCell`](core::cell::RefCell).
///
/// A dynamic borrow failure identifies an accidental overlap of two
/// synchronous register transactions. This type neither steals peripherals
/// nor creates another runtime owner.
///
/// SOURCE: complete `libpp[wdev.o]::wDev_ProcessFiq` handles
/// RX-success bit `0x4000` before TX-complete bit `0x80`.
pub struct CooperativeRadioHardware<'arena> {
    registers: PublishedRadioOwner<'arena>,
}

impl<'arena> CooperativeRadioHardware<'arena> {
    /// Bind the unique published register lease to cooperative radio access.
    pub const fn new(registers: PublishedRadioOwner<'arena>) -> Self {
        Self { registers }
    }

    /// Copyable handle used by executor tasks for bounded HAL transactions.
    ///
    /// Returning the handle does not expose the PAC owner: every operation is
    /// dynamically serialized by the same HAL arena, and lifecycle code must
    /// stop those tasks before consuming this hardware facade.
    pub const fn register_access(&self) -> RadioAccess<'arena> {
        self.registers.access()
    }

    fn station_wake_hal(
        &self,
    ) -> oer_esp32s31_hal::ieee80211::station_wake::StationWakeHal<'arena> {
        self.registers.access().try_station_wake_hal().expect(
            "a synchronous station wake transaction must not overlap another MMIO transaction",
        )
    }

    fn wifi_mac_hal(&self) -> WifiMacHal<'arena> {
        self.registers
            .access()
            .try_wifi_mac_hal()
            .expect("a synchronous Wi-Fi MAC transaction must not overlap another MMIO transaction")
    }

    /// Read the associated-STA RX policy through the same serialized PAC
    /// owner used by the live service.
    pub fn sta_receive_policy_snapshot(&self) -> MacStaReceivePolicySnapshot {
        self.registers
            .access()
            .try_station_receive_policy_snapshot()
            .expect("a synchronous STA policy snapshot must not overlap another MMIO transaction")
    }

    /// Revoke station RX admission at the outer descriptor-epoch boundary.
    pub fn disable_station_receive_policy(&mut self) {
        self.wifi_mac_hal().disable_station_receive_policy();
    }

    /// Read the live MAC RX walker frontier through the serialized PAC owner.
    pub fn mac_rx_dma_snapshot(&self) -> MacRxDmaSnapshot {
        self.registers
            .access()
            .try_receive_dma_snapshot()
            .expect("a synchronous RX DMA snapshot must not overlap another MMIO transaction")
    }

    /// Read the value-only MAC RX counters through the same serialized owner.
    /// This grants no register access and is used only to delimit one typed
    /// qualification epoch around a live cooperative service.
    pub fn receive_statistics_snapshot(
        &self,
    ) -> oer_esp32s31_hal::ieee80211::mac::MacRxStatisticsSnapshot {
        self.registers
            .access()
            .try_receive_statistics_snapshot()
            .expect("an RX statistics snapshot must not overlap another MMIO transaction")
    }

    /// Recover the exact PAC owner after every child task and synchronous
    /// register transaction has returned.
    pub fn try_into_registers(self) -> Result<RadioRuntimeOwner, (Self, RadioOwnerArenaError)> {
        match self.registers.try_reclaim() {
            Ok(registers) => Ok(registers),
            Err((registers, error)) => Err((Self { registers }, error)),
        }
    }

    /// Recover the PAC owner and its exact task-stable arena binding.
    pub fn try_into_reclaimed_registers(
        self,
    ) -> Result<ReclaimedRadioOwner<'arena>, (Self, RadioOwnerArenaError)> {
        match self.registers.try_reclaim_with_republish() {
            Ok(reclaimed) => Ok(reclaimed),
            Err((registers, error)) => Err((Self { registers }, error)),
        }
    }
}

impl CcmpKeyHardware for CooperativeRadioHardware<'_> {
    fn configure_ccmp_association(
        &mut self,
        interface: oer_esp32s31_hal::types::MacInterface,
        mode: oer_esp32s31_hal::types::MacSppMode,
    ) {
        self.wifi_mac_hal()
            .configure_ccmp_association(interface, mode);
    }
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: oer_esp32s31_hal::types::MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        self.registers
            .access()
            .try_install_station_ccmp_entry(index, identity, temporal_key)
            .expect("CCMP installation must not overlap another MMIO transaction")
    }

    fn install_ap_ccmp_entry(
        &mut self,
        index: u8,
        identity: oer_esp32s31_hal::types::MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        self.registers
            .access()
            .try_install_access_point_ccmp_entry(index, identity, temporal_key)
            .expect("AP CCMP installation must not overlap another MMIO transaction")
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        self.registers
            .access()
            .try_clear_ccmp_entry(index)
            .expect("CCMP clearing must not overlap another MMIO transaction");
    }

    fn ccmp_entry_is_valid(&self, index: u8) -> Option<bool> {
        self.registers
            .access()
            .try_ccmp_entry_is_valid(index)
            .expect("CCMP observation must not overlap another MMIO transaction")
    }
}

impl ApRxPolicyHardware for CooperativeRadioHardware<'_> {
    fn apply_ap_link_policy(&mut self, access_point: [u8; 6]) {
        self.wifi_mac_hal()
            .configure_access_point_receive_policy(access_point);
    }

    fn disable_ap_link_policy(&mut self) {
        self.wifi_mac_hal().disable_access_point_receive_policy();
    }
}

impl MacSnifferHardware for CooperativeRadioHardware<'_> {
    fn configure_open_promiscuous_receive(&mut self) {
        self.wifi_mac_hal().configure_open_promiscuous_receive();
    }

    fn disable_open_promiscuous_receive(&mut self) {
        self.wifi_mac_hal().disable_open_mac_promiscuous_receive();
    }
}

impl MacRuntimeStopHardware for CooperativeRadioHardware<'_> {
    fn request_mac_runtime_stop(&mut self) {
        self.wifi_mac_hal().request_channel_stop();
    }

    fn mac_runtime_active_state(&mut self) -> u8 {
        self.wifi_mac_hal().channel_active_state()
    }

    fn resume_mac_runtime(&mut self) {
        let _ = self.wifi_mac_hal().restart_after_channel_switch();
    }
}

impl ApTsfHardware for CooperativeRadioHardware<'_> {
    fn reset_and_start_access_point_tsf(
        &mut self,
        _: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsfWrite,
    ) {
        self.wifi_mac_hal().reset_and_start_access_point_tsf();
    }

    fn stop_access_point_tsf(
        &mut self,
        _: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsfWrite,
    ) {
        self.wifi_mac_hal().stop_access_point_tsf();
    }
}

impl StaApRegisterHardware for CooperativeRadioHardware<'_> {
    fn apply_sta_ap_receive_registers(
        &mut self,
        plan: oer_esp32s31_hal::ieee80211::mac::MacStaApReceivePlan,
    ) {
        self.wifi_mac_hal().configure_sta_ap_receive_plan(plan);
    }

    fn disable_station_receive_registers(&mut self) {
        self.wifi_mac_hal().disable_station_receive_policy();
    }

    fn disable_access_point_receive_registers(&mut self) {
        self.wifi_mac_hal().disable_access_point_receive_policy();
    }

    fn disable_all_role_receive_registers(&mut self) {
        self.wifi_mac_hal().disable_all_role_receive_policies();
    }
}

impl He20PeerHardware for CooperativeRadioHardware<'_> {
    fn program_he20_peer(
        &mut self,
        config: MacHe20PeerConfig,
        rts_threshold: Option<u16>,
    ) -> Result<(), MacHe20PeerError> {
        He20PeerHardware::program_he20_peer(&mut self.wifi_mac_hal(), config, rts_threshold)
    }

    fn program_he20_association(
        &mut self,
        association_id: MacAssociationId,
        minimum_mpdu_start_spacing: MacMinimumMpduStartSpacing,
        bssid_index: u8,
    ) {
        He20PeerHardware::program_he20_association(
            &mut self.wifi_mac_hal(),
            association_id,
            minimum_mpdu_start_spacing,
            bssid_index,
        )
    }

    fn initialize_he_buffer_status_report(&mut self) {
        He20PeerHardware::initialize_he_buffer_status_report(&mut self.wifi_mac_hal());
    }
}

impl BeamformingReportHardware for CooperativeRadioHardware<'_> {
    fn set_he_beamforming_report_profile(&mut self, profile: MacHeBeamformingReportProfile) {
        BeamformingReportHardware::set_he_beamforming_report_profile(
            &mut self.wifi_mac_hal(),
            profile,
        );
    }

    fn set_he_ersu_ack_rate_profile(&mut self, profile: MacHeErSuAckRateProfile) {
        BeamformingReportHardware::set_he_ersu_ack_rate_profile(&mut self.wifi_mac_hal(), profile);
    }
}

impl StaLinkRxPolicyHardware for CooperativeRadioHardware<'_> {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]) {
        self.registers
            .access()
            .try_configure_station_receive_policy(bssid)
            .expect("STA policy configuration must not overlap another MMIO transaction");
    }
}

impl oer_esp32s31_ieee80211_mac::init::StaEspNowRxPolicyHardware for CooperativeRadioHardware<'_> {
    fn apply_sta_esp_now_policy(&mut self, bssid: [u8; 6]) {
        self.registers
            .access()
            .try_configure_station_esp_now_receive_policy(bssid)
            .expect("ESP-NOW STA policy configuration must not overlap another MMIO transaction");
    }
}

impl StaNoiseFloorHardware for CooperativeRadioHardware<'_> {
    fn read_noise_floor_dbm(&self) -> i8 {
        self.registers
            .access()
            .try_noise_floor_dbm()
            .expect("noise-floor sampling must not overlap another MMIO transaction")
    }
}

impl TxHardware for CooperativeRadioHardware<'_> {
    fn probe_phy_low_rate_gate(
        &mut self,
    ) -> Result<MacLowRateGateProbe, MacLowRateTransitionError> {
        TxHardware::probe_phy_low_rate_gate(&mut self.wifi_mac_hal())
    }

    fn prepare_bound_legacy_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        TxHardware::prepare_bound_legacy_tx(&mut self.wifi_mac_hal(), dma, queue, program)
    }

    fn start_bound_legacy_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8) {
        TxHardware::start_bound_legacy_tx(&mut self.wifi_mac_hal(), dma, queue);
    }

    fn prepare_bound_ht_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        TxHardware::prepare_bound_ht_tx(&mut self.wifi_mac_hal(), dma, queue, program)
    }

    fn start_bound_ht_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8) {
        TxHardware::start_bound_ht_tx(&mut self.wifi_mac_hal(), dma, queue);
    }

    fn prepare_bound_he_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHeTxProgram,
    ) -> bool {
        TxHardware::prepare_bound_he_tx(&mut self.wifi_mac_hal(), dma, queue, program)
    }

    fn start_bound_he_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8) {
        TxHardware::start_bound_he_tx(&mut self.wifi_mac_hal(), dma, queue);
    }

    fn ordinary_tx_queue_snapshot(
        &mut self,
        queue: u8,
    ) -> Option<oer_esp32s31_hal::types::MacOrdinaryTxQueueSnapshot> {
        Some(self.wifi_mac_hal().ordinary_tx_queue_snapshot(queue))
    }

    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionObservation> {
        TxHardware::take_tx_completion(&mut self.wifi_mac_hal(), queue)
    }

    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        TxHardware::begin_tx_timeout_abort(&mut self.wifi_mac_hal(), queue)
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        TxHardware::with_tx_queue_detached(
            &mut self.wifi_mac_hal(),
            queue,
            expected_descriptor_head,
            reason,
            detached,
        )
    }

    fn take_block_ack_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionObservation> {
        TxHardware::take_block_ack_completion(&mut self.wifi_mac_hal(), queue)
    }
}

impl HtAmpduHardware for CooperativeRadioHardware<'_> {
    fn prepare_he_trigger_based_queue(
        &mut self,
        policy: MacHeTbTidLimit,
        reservation: MacHeTbLinkReservation,
        tid: MacHeTid,
        mpdu_lengths: &[u16],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        HtAmpduHardware::prepare_he_trigger_based_queue(
            &mut self.wifi_mac_hal(),
            policy,
            reservation,
            tid,
            mpdu_lengths,
            queued_msdu_bytes,
        )
    }

    fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        HtAmpduHardware::clear_he_trigger_based_queue(&mut self.wifi_mac_hal(), reservation);
    }

    fn he_trigger_based_queue_snapshot(
        &self,
        reservation: MacHeTbLinkReservation,
    ) -> Option<MacHeTriggerTxQueueSnapshot> {
        HtAmpduHardware::he_trigger_based_queue_snapshot(&self.wifi_mac_hal(), reservation)
    }
}

impl RxDma for CooperativeRadioHardware<'_> {
    fn buffer_full_count(&mut self) -> Option<u16> {
        RxDma::buffer_full_count(&mut self.wifi_mac_hal())
    }

    fn last_descriptor_low(&mut self) -> u32 {
        RxDma::last_descriptor_low(&mut self.wifi_mac_hal())
    }

    fn next_descriptor_low(&mut self) -> u32 {
        RxDma::next_descriptor_low(&mut self.wifi_mac_hal())
    }

    fn next_descriptor(&mut self) -> oer_esp32s31_ieee80211_dma::rx_dma::RxDmaNextDescriptor {
        RxDma::next_descriptor(&mut self.wifi_mac_hal())
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R {
        RxDma::with_ordered_cursor(&mut self.wifi_mac_hal(), observed)
    }

    fn walker_enabled(&mut self) -> bool {
        RxDma::walker_enabled(&mut self.wifi_mac_hal())
    }

    fn reload_pending(&mut self) -> bool {
        RxDma::reload_pending(&mut self.wifi_mac_hal())
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_reload_settled(&mut self.wifi_mac_hal(), settled)
    }

    fn configure_descriptor_window(&mut self, binding: &RxDmaBinding) {
        RxDma::configure_descriptor_window(&mut self.wifi_mac_hal(), binding);
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding, address: u32) {
        RxDma::write_descriptor_base(&mut self.wifi_mac_hal(), binding, address);
    }

    fn publish_walker_enable(&mut self, binding: &RxDmaBinding) {
        RxDma::publish_walker_enable(&mut self.wifi_mac_hal(), binding);
    }

    fn request_reload(&mut self, binding: &RxDmaBinding) {
        RxDma::request_reload(&mut self.wifi_mac_hal(), binding);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        binding: &RxDmaBinding,
        enabled: impl for<'confirmation> FnOnce(RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_walker_enabled(&mut self.wifi_mac_hal(), binding, enabled)
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_walker_stopped(&mut self.wifi_mac_hal(), stopped)
    }

    fn fence(&mut self) {
        RxDma::fence(&mut self.wifi_mac_hal());
    }
}

/// The facade's only station TSF write, which [`StationTsf`] alone can call
/// ([`StationTsfWrite`]).
///
/// [`StationTsf`]: crate::station_tsf::StationTsf
impl crate::station_tsf::StationTsfHardware for CooperativeRadioHardware<'_> {
    fn station_tsf(&mut self) -> u64 {
        CooperativeRadioHardware::station_tsf(self)
    }

    fn set_station_tsf(&mut self, _: StationTsfWrite, value: u64) {
        self.registers.set_station_tsf(value).expect(
            "a synchronous Wi-Fi MAC transaction must not overlap another MMIO transaction",
        );
    }
}

impl crate::lower_mac::StationTbttHardware for CooperativeRadioHardware<'_> {
    fn start_station_tbtt(&mut self, schedule: oer_esp32s31_hal::types::StaTbttSchedule) {
        CooperativeRadioHardware::start_station_tbtt(self, schedule);
    }

    fn stop_station_tbtt(&mut self) {
        CooperativeRadioHardware::stop_station_tbtt(self);
    }
}

impl crate::lower_mac::RxBeaconPriorityHardware for CooperativeRadioHardware<'_> {
    fn set_rx_beacon_pti(&mut self, pti: oer_esp32s31_hal::types::MacPti) {
        CooperativeRadioHardware::set_rx_beacon_pti(self, pti);
    }

    fn clear_rx_beacon_pti(&mut self) {
        CooperativeRadioHardware::clear_rx_beacon_pti(self);
    }
}

impl crate::lower_mac::TxGateHardware for CooperativeRadioHardware<'_> {
    fn set_power_save_tx_block(&mut self, blocked: bool) {
        CooperativeRadioHardware::set_power_save_tx_block(self, blocked);
    }
}

impl CooperativeRadioHardware<'_> {
    pub fn station_tsf(&mut self) -> u64 {
        self.wifi_mac_hal().station_tsf()
    }

    /// The Wi-Fi MAC local time, the counter of receive timestamps.
    pub fn mac_local_time(&mut self) -> u32 {
        self.wifi_mac_hal().mac_local_time()
    }

    /// Start the station TSF.
    pub fn enable_station_tsf(&mut self) {
        self.wifi_mac_hal().enable_station_tsf();
    }

    /// Program the station TBTT schedule of power management.
    pub fn start_station_tbtt(&mut self, schedule: oer_esp32s31_hal::types::StaTbttSchedule) {
        self.station_wake_hal().start_station_tbtt(schedule);
    }

    /// Replace the running station TBTT interval.
    pub fn set_station_tbtt_interval(
        &mut self,
        interval_micros: u32,
    ) -> Result<(), oer_esp32s31_hal::types::StaTbttScheduleError> {
        self.station_wake_hal()
            .set_station_tbtt_interval(interval_micros)
    }

    /// Replace the station TBTT lead and the wake lead beside it.
    pub fn set_station_tbtt_ahead(&mut self, ahead_micros: u16, wake_ahead_micros: u16) {
        self.station_wake_hal()
            .set_station_tbtt_ahead(ahead_micros, wake_ahead_micros);
    }

    /// Stop the station TBTT schedule.
    pub fn stop_station_tbtt(&mut self) {
        self.station_wake_hal().stop_station_tbtt();
    }

    /// Block or unblock the TX queues for station power management.
    pub fn set_power_save_tx_block(&mut self, blocked: bool) {
        self.wifi_mac_hal().set_power_save_tx_block(blocked);
    }

    /// Publish the beacon receive priority.
    pub fn set_rx_beacon_pti(&mut self, pti: oer_esp32s31_hal::types::MacPti) {
        // The vendor passes one priority as both the beacon and the shared
        // priority (`pm_coex.o::pm_coex_update_rx_beacon_pti`).
        self.wifi_mac_hal().set_rx_beacon_pti(pti, pti);
    }

    /// Clear the beacon receive priority request.
    pub fn clear_rx_beacon_pti(&mut self) {
        self.wifi_mac_hal().clear_rx_beacon_pti();
    }

    /// Publish the hardware beacon receive window and time.
    pub fn set_rx_beacon_time(&mut self, window_micros: u16, time_micros: u32) {
        self.wifi_mac_hal()
            .set_rx_beacon_time(window_micros, time_micros);
    }

    pub fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        rx_ampdu_hw::program(&mut self.wifi_mac_hal(), agreement)
    }

    pub fn clear_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        rx_ampdu_hw::clear(&mut self.wifi_mac_hal(), hardware_index)
    }

    pub fn reset_rx_block_ack_window(
        &mut self,
        hardware_index: u8,
        tid: u8,
        starting_sequence: SequenceNumber,
        window: u16,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        rx_ampdu_hw::reset_window(
            &mut self.wifi_mac_hal(),
            hardware_index,
            tid,
            starting_sequence,
            window,
        )
    }

    pub fn program_extra_softap_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        rx_ampdu_hw::program_extra_softap(&mut self.wifi_mac_hal(), agreement)
    }

    pub fn clear_extra_softap_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        rx_ampdu_hw::clear_extra_softap(&mut self.wifi_mac_hal(), hardware_index)
    }

    pub fn reset_extra_softap_rx_block_ack_window(
        &mut self,
        hardware_index: u8,
        starting_sequence: SequenceNumber,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        rx_ampdu_hw::reset_extra_softap_window(
            &mut self.wifi_mac_hal(),
            hardware_index,
            starting_sequence,
        )
    }

    pub fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        let tid = MacHeTid::new(tid).ok_or(S31RxBlockAckAgreementError::Tid(tid))?;
        self.wifi_mac_hal()
            .set_he_trigger_based_tid_enabled(tid, enabled);
        Ok(())
    }

    pub fn replace_sta_group_ccmp(
        &mut self,
        slot: &mut StaGroupCcmpSlot,
        key_id: oer_ieee80211_mac::ccmp::GroupKeyId,
        temporal_key: &[u8; 16],
    ) -> Result<(), CryptoKeyError> {
        replace_sta_group_ccmp(&mut self.wifi_mac_hal(), slot, key_id, temporal_key)
    }

    pub fn replace_sta_group_ccmp_with_rollback(
        &mut self,
        slot: &mut StaGroupCcmpSlot,
        current: &StaGroupCcmpKeyMaterial,
        replacement: &StaGroupCcmpKeyMaterial,
    ) -> Result<(), StaGroupCcmpReplaceError> {
        replace_sta_group_ccmp_with_rollback(&mut self.wifi_mac_hal(), slot, current, replacement)
    }
}

impl oer_esp32s31_ieee80211_mac::rx::hardware::RxBlockAckHardware for CooperativeRadioHardware<'_> {
    fn diagnostic_rx_block_ack_entry_snapshot(
        &mut self,
        hardware_index: u8,
    ) -> Option<oer_esp32s31_hal::ieee80211::mac::RxBlockAckEntrySnapshot> {
        self.wifi_mac_hal().rx_block_ack_entry_snapshot(
            oer_esp32s31_hal::types::MacRxBlockAckEntryIndex::new(u32::from(hardware_index))?,
        )
    }

    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        CooperativeRadioHardware::program_rx_block_ack(self, agreement)
    }

    fn clear_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        CooperativeRadioHardware::clear_rx_block_ack(self, hardware_index)
    }

    fn reset_rx_block_ack_window(
        &mut self,
        hardware_index: u8,
        tid: u8,
        starting_sequence: SequenceNumber,
        window: u16,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        CooperativeRadioHardware::reset_rx_block_ack_window(
            self,
            hardware_index,
            tid,
            starting_sequence,
            window,
        )
    }

    fn program_extra_softap_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        CooperativeRadioHardware::program_extra_softap_rx_block_ack(self, agreement)
    }

    fn clear_extra_softap_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        CooperativeRadioHardware::clear_extra_softap_rx_block_ack(self, hardware_index)
    }

    fn reset_extra_softap_rx_block_ack_window(
        &mut self,
        hardware_index: u8,
        starting_sequence: SequenceNumber,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        CooperativeRadioHardware::reset_extra_softap_rx_block_ack_window(
            self,
            hardware_index,
            starting_sequence,
        )
    }
}
