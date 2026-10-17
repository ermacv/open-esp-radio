//! Wi-Fi-specific MAC operations reached from the vendor PHY ABI.
//!
//! These leaves are deliberately separate from the shared PHY modules: their
//! physical registers belong to the 802.11 MAC and are not reusable by
//! Bluetooth, BLE or IEEE 802.15.4 PHY paths.

use core::cell::RefMut;

use crate::ieee80211::MacBorrow;

pub use crate::types::{
    ExtraSoftApRxBlockAckEntrySnapshot, MacApReceivePolicySnapshot, MacCoexPrioritySnapshot,
    MacInterface, MacItwtClearIndex, MacKeyInstallOutcome, MacPti, MacRoleReceivePolicy,
    MacRxDecodeErrorStatistics, MacRxDecodeErrorStatisticsDelta, MacRxDmaSnapshot,
    MacRxHangStatistics, MacRxHangStatisticsDelta, MacRxPrimaryStatistics,
    MacRxPrimaryStatisticsDelta, MacRxStatisticsSnapshot, MacStaApReceivePlan, MacStaPolicyMode,
    MacStaReceivePolicySnapshot, MacTxPowerTable, MacTxPtiCount, MacTxPtiProgram, MacTxQueueIndex,
    MacTxStatisticsSnapshot, RxBlockAckEntrySnapshot,
};

use crate::types::{
    MacAssociationId, MacExtraSoftApRxBlockAckEntryIndex, MacHe20PeerConfig, MacHe20PeerError,
    MacHeBeamformingReportProfile, MacHeErSuAckRateProfile, MacHeTbLinkReservation,
    MacHeTbProgramError, MacHeTbTidLimit, MacHeTid, MacHeTriggerRxDiagnostics,
    MacHeTriggerTxQueueSnapshot, MacHeTxProgram, MacHtAmpduCompletionObservation, MacHtTxProgram,
    MacKeyEntryIndex, MacLegacyTxProgram, MacMinimumMpduStartSpacing, MacOrdinaryTxQueueSnapshot,
    MacRxBlockAckEntryIndex, MacRxBlockAckStartingSequence, MacRxBlockAckTid, MacRxBlockAckWindow,
    MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason, MacTxQueueDetached,
};

use oer_memory::{HardwareOwnedTxDma, PreparedTxDma, StableDmaRange};

use oer_esp32s31_pac::{
    CoexistenceLowPowerClockObservation, MacInterruptSetup as PacMacInterruptSetup,
    SharedRadioRegisters, WifiRadioRegisters,
};

/// Complete identity of one hardware MAC interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacInterfaceIdentity {
    pub interface: MacInterface,
    pub address: [u8; 6],
    pub bssid: [u8; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdHandshakeOutcome {
    /// Number of not-ready observations before the ready edge.
    pub samples: u32,
    /// Total hardware observations, including the final ready edge.
    pub observations: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdHandshakeTimeout {
    /// Number of not-ready observations consumed from the finite budget.
    pub samples: u32,
    /// Caller-provided finite not-ready observation limit.
    pub sample_limit: u32,
}

trait MacColdHandshakeBackend {
    fn request_cold_start(&mut self);
    fn sample_cold_start_ready(&mut self) -> bool;
    fn mask_mac_interrupts(&mut self);
    fn clear_mac_interrupts(&mut self);
}

impl MacColdHandshakeBackend for ColdRegisters<'_> {
    fn request_cold_start(&mut self) {
        self.parts().0.request_mac_cold_start();
    }

    fn sample_cold_start_ready(&mut self) -> bool {
        self.parts().0.sample_mac_cold_start_ready()
    }

    fn mask_mac_interrupts(&mut self) {
        self.parts().2.mask_all_mac_interrupts();
    }

    fn clear_mac_interrupts(&mut self) {
        self.parts().2.clear_all_mac_interrupts();
    }
}

fn execute_cold_mac_handshake(
    backend: &mut impl MacColdHandshakeBackend,
    sample_limit: u32,
) -> Result<MacColdHandshakeOutcome, MacColdHandshakeTimeout> {
    backend.request_cold_start();

    let mut samples = 0;
    loop {
        if backend.sample_cold_start_ready() {
            break;
        }
        samples += 1;
        if samples >= sample_limit {
            return Err(MacColdHandshakeTimeout {
                samples,
                sample_limit,
            });
        }
    }

    backend.mask_mac_interrupts();
    backend.clear_mac_interrupts();
    Ok(MacColdHandshakeOutcome {
        samples,
        observations: samples + 1,
    })
}

/// Execute the finite cold-MAC handshake and interrupt cleanup sequence.
///
/// SOURCE: complete pinned `libpp[hal_mac.o]::hal_init`, offsets
/// `0x00..0x3a`. The vendor waits forever; this HAL operation owns the finite
/// sample limit, polling, and multi-register cleanup order. One initial sample
/// always occurs. Each not-ready observation increments the sample count, then
/// reaches timeout when that count is greater than or equal to `sample_limit`;
/// a zero limit therefore still performs exactly one observation.
fn begin_cold_mac_handshake(
    registers: &mut ColdRegisters<'_>,
    sample_limit: u32,
) -> Result<MacColdHandshakeOutcome, MacColdHandshakeTimeout> {
    execute_cold_mac_handshake(registers, sample_limit)
}

/// The owners one cold MAC transaction borrows: a clocked Wi-Fi client's MAC
/// and interrupt setup, with the shared radio owner borrowed from the arbiter
/// lease.
struct ColdRegisters<'registers> {
    mac: &'registers mut WifiRadioRegisters,
    interrupts: &'registers mut PacMacInterruptSetup,
    shared: &'registers mut SharedRadioRegisters,
}

impl ColdRegisters<'_> {
    fn parts(
        &mut self,
    ) -> (
        &mut WifiRadioRegisters,
        &mut SharedRadioRegisters,
        &mut PacMacInterruptSetup,
    ) {
        (self.mac, self.shared, self.interrupts)
    }
}

/// Closed HAL authority for the one-way cold Wi-Fi MAC transition.
///
/// The capability is borrowed from a clocked Wi-Fi client with the arbiter
/// lease, and cannot be stored beyond them. It exposes reviewed cold transactions only;
/// neither the PAC owner nor arbitrary register access is available.
pub struct WifiMacColdHal<'registers> {
    registers: ColdRegisters<'registers>,
}

impl<'registers> WifiMacColdHal<'registers> {
    pub(crate) fn from_leased(
        mac: &'registers mut WifiRadioRegisters,
        interrupts: &'registers mut PacMacInterruptSetup,
        shared: &'registers mut SharedRadioRegisters,
    ) -> Self {
        Self {
            registers: ColdRegisters {
                mac,
                interrupts,
                shared,
            },
        }
    }

    fn mac(&mut self) -> &mut WifiRadioRegisters {
        self.registers.parts().0
    }

    pub fn set_wifi_mac_reset(&mut self, asserted: bool) {
        self.registers
            .parts()
            .1
            .radio_phy_mut()
            .set_wifi_mac_reset(asserted);
    }

    pub fn initialize_antenna(&mut self) {
        self.mac().initialize_mac_antenna();
    }

    pub fn initialize_coex(
        &mut self,
        rx_ack: u8,
        wifi_default: u8,
        tb: [u8; 7],
        beamforming: [u8; 3],
        multi_target: [u8; 2],
    ) {
        self.mac()
            .initialize_mac_coex(rx_ack, wifi_default, tb, beamforming, multi_target);
    }

    pub fn initialize_crypto_bypass(&mut self) {
        self.mac().initialize_mac_crypto_bypass();
    }

    pub fn enable_interrupts(&mut self, event_mask: crate::types::MacInterruptMask) {
        let (mac, _, interrupts) = self.registers.parts();
        mac.enable_mac_with_interrupt_mask(interrupts, event_mask);
    }

    pub fn initialize_hal_tail(
        &mut self,
        event_mask: crate::types::MacInterruptMask,
        slow_clock_calibration: crate::types::MacRtcCalibration,
    ) {
        let (mac, _, interrupts) = self.registers.parts();
        mac.initialize_mac_hal_tail(interrupts, event_mask, slow_clock_calibration)
    }

    pub fn begin_handshake(
        &mut self,
        sample_limit: u32,
    ) -> Result<MacColdHandshakeOutcome, MacColdHandshakeTimeout> {
        begin_cold_mac_handshake(&mut self.registers, sample_limit)
    }

    pub fn initialize_he_prefix(&mut self) {
        let (mac, shared, _) = self.registers.parts();
        mac.initialize_mac_he_prefix(shared);
    }

    pub fn initialize_tx_power(&mut self, table: &MacTxPowerTable) {
        self.mac().initialize_mac_tx_power(table);
    }

    pub fn initialize_he_suffix(&mut self) {
        let (mac, shared, _) = self.registers.parts();
        mac.initialize_mac_he_suffix(shared);
    }

    pub fn initialize_last_rx_buffer_table(&mut self) {
        self.mac().initialize_mac_last_rx_buffer();
    }

    pub fn initialize_rx_buffer_prefix(&mut self) {
        self.mac().initialize_mac_rx_buffer_prefix();
    }

    pub fn initialize_receive_policy(&mut self) {
        self.mac().initialize_cold_receive_policy();
    }

    pub fn initialize_txrx_prefix(&mut self) {
        self.mac().initialize_mac_txrx_prefix();
    }

    pub fn initialize_txrx_callbacks(&mut self, delay_slot: crate::types::MacDelaySlot) {
        self.mac().initialize_mac_txrx_callbacks(delay_slot)
    }

    pub fn initialize_txrx_suffix(&mut self) {
        let (mac, shared, _) = self.registers.parts();
        mac.initialize_mac_txrx_suffix(shared);
    }

    pub fn program_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        self.mac()
            .program_receive_interface_address(interface, address);
    }

    pub fn disable_phy_low_rate(&mut self) {
        self.mac().configure_phy_low_rate(false);
    }

    pub fn configure_open_promiscuous_receive(&mut self) {
        self.mac().configure_open_mac_promiscuous_receive();
    }

    pub fn disable_open_mac_promiscuous_receive(&mut self) {
        self.mac().disable_open_mac_promiscuous_receive();
    }
}

impl WifiMacColdHal<'_> {
    /// Sample the shared coexistence low-power clock selection once.
    ///
    /// Each call performs fresh reads; `None` reports an unreviewed encoding.
    pub fn sample_coexistence_low_power_clock(
        &mut self,
    ) -> Option<CoexistenceLowPowerClockObservation> {
        self.registers
            .parts()
            .1
            .sample_coexistence_low_power_clock()
    }
}

/// Closed HAL authority for reviewed runtime Wi-Fi MAC transactions.
///
/// It borrows the Wi-Fi MAC registers alone and exposes neither them nor
/// `Deref`. LMAC code can request only the finite operations defined here;
/// shared radio registers are reached through the arbiter lease instead.
pub struct WifiMacHal<'registers> {
    registers: MacBorrow<'registers>,
}

impl<'registers> WifiMacHal<'registers> {
    pub(crate) fn from_mac(registers: &'registers mut WifiRadioRegisters) -> Self {
        Self {
            registers: MacBorrow::Owned(registers),
        }
    }

    pub(crate) fn from_published(registers: RefMut<'registers, WifiRadioRegisters>) -> Self {
        Self {
            registers: MacBorrow::Published(registers),
        }
    }

    fn pac(&self) -> &WifiRadioRegisters {
        &self.registers
    }

    fn pac_mut(&mut self) -> &mut WifiRadioRegisters {
        &mut self.registers
    }

    pub fn configure_open_promiscuous_receive(&mut self) {
        self.pac_mut().configure_open_mac_promiscuous_receive();
    }

    pub fn disable_open_mac_promiscuous_receive(&mut self) {
        self.pac_mut().disable_open_mac_promiscuous_receive();
    }

    /// Read the calibrated baseband noise-floor observation used by link
    /// policy. This exposes the measured value, not the underlying register
    /// owner or encoding.
    pub fn read_noise_floor_dbm(&self) -> i8 {
        self.pac().read_noise_floor_dbm()
    }

    /// Read the reviewed ROM low-rate enable status while the runtime MAC
    /// register authority is exclusively borrowed.
    pub fn phy_low_rate_enabled(&self) -> bool {
        self.pac().phy_low_rate_enabled()
    }

    /// Apply the complete three-RMW ROM low-rate gate transaction.
    pub fn configure_phy_low_rate(&mut self, enabled: bool) {
        self.pac_mut().configure_phy_low_rate(enabled);
    }

    /// Begin the reviewed no-power-save MAC quiesce sequence before PHY
    /// retuning. Sequencing belongs to the HAL; the PAC method is only the
    /// register-local RMW transaction.
    pub fn request_channel_stop(&mut self) {
        self.pac_mut().request_mac_channel_stop_without_power_save();
    }

    /// Sample the hardware activity field used by the bounded HAL poll.
    pub fn channel_active_state(&self) -> u8 {
        self.pac().mac_channel_active_state()
    }

    /// Finish a channel switch and return the selected REGDMA link for
    /// diagnostics. Delays and polling remain outside the PAC.
    pub fn restart_after_channel_switch(&mut self) -> u8 {
        // SOURCE: complete `libpp[if_hwctrl.o]::ic_mac_init -> hal_mac_init ->
        // pwr_hal_select_wifimac_regdma_link` in
        // `BLOB_LIBPP_MAC_CHANNEL_SWITCH`. This is intentionally a HAL
        // sequence: the PAC exposes only the two register-local operations.
        self.pac_mut().resume_mac_channel_without_power_save();
        self.pac_mut().select_wifi_no_power_save_regdma_link();
        self.pac_mut().wifi_mac_regdma_link()
    }

    /// Publish one interface receive address through the complete register
    /// transaction. The typed interface selector prevents accidental bank
    /// aliasing while keeping register encoding private to the PAC.
    pub fn program_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        self.pac_mut()
            .program_receive_interface_address(interface, address);
    }

    /// Publish one interface BSSID through the complete register transaction.
    pub fn program_interface_bssid(&mut self, interface: MacInterface, bssid: [u8; 6]) {
        self.pac_mut().program_interface_bssid(interface, bssid);
    }

    /// Publish the receive address and BSSID using two complete vendor leaves.
    pub fn program_interface_identity(&mut self, identity: MacInterfaceIdentity) {
        self.program_interface_address(identity.interface, identity.address);
        self.program_interface_bssid(identity.interface, identity.bssid);
    }

    /// Apply one exact reviewed role-policy transaction.
    pub fn configure_role_receive_policy(&mut self, policy: MacRoleReceivePolicy) {
        self.pac_mut().apply_role_receive_policy(policy);
    }

    pub fn configure_station_receive_policy(&mut self, bssid: [u8; 6]) {
        self.pac_mut().apply_sta_link_receive_policy(bssid);
    }

    /// Apply only the exact vendor policy-six register transaction.
    /// Connected-STA entry normally uses [`Self::configure_station_receive_policy`]
    /// so the open scan/sniffer frontier is closed first.
    pub fn configure_station_policy_six(&mut self, bssid: [u8; 6], mode: MacStaPolicyMode) {
        self.configure_role_receive_policy(MacRoleReceivePolicy::Station { bssid, mode });
    }

    pub fn configure_access_point_receive_policy(&mut self, address: [u8; 6]) {
        self.configure_role_receive_policy(MacRoleReceivePolicy::AccessPoint { address });
    }

    pub fn disable_access_point_receive_policy(&mut self) {
        self.configure_role_receive_policy(MacRoleReceivePolicy::AccessPointDisabled);
    }

    pub fn disable_station_receive_policy(&mut self) {
        self.configure_role_receive_policy(MacRoleReceivePolicy::StationDisabled);
    }

    /// Disable both receive contexts in the order of the role-neutral suffix
    /// of vendor `wifi_set_rx_policy(0)`.
    pub fn disable_all_role_receive_policies(&mut self) {
        self.pac_mut().disable_all_role_receive_policies();
    }

    /// Program both reviewed receive contexts as one register composition.
    ///
    /// This does not claim simultaneous runtime ownership or select a channel.
    pub fn configure_sta_ap_receive_plan(&mut self, plan: MacStaApReceivePlan) {
        self.pac_mut().apply_sta_ap_receive_plan(plan);
    }

    pub fn station_receive_policy_snapshot(&self) -> MacStaReceivePolicySnapshot {
        self.pac().sta_receive_policy_snapshot()
    }

    pub fn access_point_receive_policy_snapshot(&self) -> MacApReceivePolicySnapshot {
        self.pac().ap_receive_policy_snapshot()
    }

    pub fn receive_statistics_snapshot(&self) -> MacRxStatisticsSnapshot {
        self.pac().rx_statistics_snapshot()
    }

    pub fn transmit_statistics_snapshot(&self) -> MacTxStatisticsSnapshot {
        self.pac().tx_statistics_snapshot()
    }

    pub fn coex_priority_snapshot(&self) -> MacCoexPrioritySnapshot {
        self.pac().mac_coex_priority_snapshot()
    }

    /// Read the bounded hardware-owned RX walker projection without exposing
    /// the PAC owner to diagnostics or runtime integration code.
    pub fn receive_dma_snapshot(&self) -> MacRxDmaSnapshot {
        self.pac().mac_rx_dma_snapshot()
    }

    /// Apply the interface's association mode before its first key installation.
    pub fn configure_ccmp_association(
        &mut self,
        interface: crate::types::MacInterface,
        mode: crate::types::MacSppMode,
    ) {
        self.pac_mut()
            .configure_mac_ccmp_association(interface, mode);
    }

    /// Install one reviewed semantic station CCMP key. Slot selection and key
    /// lifetime remain driver policy; register encoding is owned by the PAC.
    pub fn install_station_ccmp_entry(
        &mut self,
        index: u8,
        identity: crate::types::MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        let Some(index) = MacKeyEntryIndex::new(u32::from(index)) else {
            return MacKeyInstallOutcome::Rejected;
        };
        self.pac_mut()
            .install_sta_ccmp_key_entry(index, identity, temporal_key)
    }

    /// Install one reviewed semantic access-point CCMP key. Association/key-ID
    /// mapping remains driver policy; register encoding is owned by the PAC.
    pub fn install_access_point_ccmp_entry(
        &mut self,
        index: u8,
        identity: crate::types::MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        let Some(index) = MacKeyEntryIndex::new(u32::from(index)) else {
            return MacKeyInstallOutcome::Rejected;
        };
        self.pac_mut()
            .install_ap_ccmp_key_entry(index, identity, temporal_key)
    }

    /// Clear one key-table entry when its generated index is valid.
    pub fn clear_ccmp_entry(&mut self, index: u8) {
        if let Some(index) = MacKeyEntryIndex::new(u32::from(index)) {
            self.pac_mut().clear_mac_key_entry(index);
        }
    }

    /// Observe key-table validity without exposing the table owner.
    pub fn ccmp_entry_is_valid(&self, index: u8) -> Option<bool> {
        MacKeyEntryIndex::new(u32::from(index))
            .map(|index| self.pac().mac_key_entry_is_valid(index))
    }

    /// Stop the access-point TSF using the complete reviewed vendor leaf.
    pub fn stop_access_point_tsf(&mut self) {
        self.pac_mut().stop_softap_tsf();
    }

    /// Start a new access-point TSF epoch through the reviewed selector-zero
    /// transaction. No raw timestamp word or register image crosses the HAL.
    pub fn reset_and_start_access_point_tsf(&mut self) {
        self.pac_mut().reset_and_start_softap_tsf();
    }

    /// Publish the complete two-edge receive-beacon PTI transaction.
    pub fn set_rx_beacon_pti(&mut self, beacon: MacPti, shared: MacPti) {
        self.pac_mut().set_rx_beacon_pti(beacon, shared);
    }

    /// Publish the complete receive-beacon PTI clear edge.
    pub fn clear_rx_beacon_pti(&mut self) {
        self.pac_mut().clear_rx_beacon_pti();
    }

    /// Publish the hardware beacon receive window and time.
    pub fn set_rx_beacon_time(&mut self, window_micros: u16, time_micros: u32) {
        self.pac_mut()
            .set_rx_beacon_time(window_micros, time_micros);
    }

    /// Block or unblock the TX queues for station power management.
    pub fn set_power_save_tx_block(&mut self, blocked: bool) {
        self.pac_mut().set_power_save_tx_block(blocked);
    }

    /// Publish the complete two-edge individual-TWT PTI transaction.
    pub fn set_itwt_pti(&mut self, argument_is_zero: bool, shared: MacPti) {
        self.pac_mut().set_itwt_pti(argument_is_zero, shared);
    }

    /// Publish one bounded individual-TWT clear request.
    pub fn clear_itwt_pti(&mut self, index: MacItwtClearIndex) {
        self.pac_mut().clear_itwt_pti(index);
    }

    /// Publish the complete scheduler and queue-vector PTI transaction.
    pub fn set_tx_pti(&mut self, queue: MacTxQueueIndex, program: MacTxPtiProgram) {
        self.pac_mut().set_tx_pti(queue, program);
    }

    /// Program the reviewed HE20 peer register image.
    pub fn program_he20_peer(
        &mut self,
        config: MacHe20PeerConfig,
        rts_threshold: Option<u16>,
    ) -> Result<(), MacHe20PeerError> {
        self.pac_mut().program_he20_peer(config, rts_threshold)
    }

    /// Program the reviewed HE20 association fields.
    pub fn program_he20_association(
        &mut self,
        association_id: MacAssociationId,
        minimum_mpdu_start_spacing: MacMinimumMpduStartSpacing,
        bssid_index: u8,
    ) {
        self.pac_mut().program_he20_association(
            association_id,
            minimum_mpdu_start_spacing,
            bssid_index,
        )
    }

    /// Initialize the reviewed HE buffer-status-report register state.
    pub fn initialize_he_buffer_status_report(&mut self) {
        self.pac_mut().initialize_he_buffer_status_report();
    }

    /// Publish the reviewed HE beamforming report-rate register image.
    pub fn set_he_beamforming_report_profile(&mut self, profile: MacHeBeamformingReportProfile) {
        self.pac_mut().set_he_beamforming_report_profile(profile);
    }

    /// Publish the matching reviewed ER-SU ACK-rate register image.
    pub fn set_he_ersu_ack_rate_profile(&mut self, profile: MacHeErSuAckRateProfile) {
        self.pac_mut().set_he_ersu_ack_rate_profile(profile);
    }

    pub fn prepare_bound_legacy_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        self.pac_mut()
            .prepare_bound_legacy_mac_tx(dma, queue, program)
    }

    pub fn start_bound_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8) {
        self.pac_mut().start_bound_mac_tx(dma, queue);
    }

    pub fn prepare_bound_ht_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        self.pac_mut().prepare_bound_ht_mac_tx(dma, queue, program)
    }

    pub fn prepare_bound_he_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHeTxProgram,
    ) -> bool {
        self.pac_mut().prepare_bound_he_mac_tx(dma, queue, program)
    }

    pub fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionObservation> {
        self.pac_mut().take_mac_tx_completion(queue)
    }

    pub fn ordinary_tx_queue_snapshot(&self, queue: u8) -> MacOrdinaryTxQueueSnapshot {
        self.pac().ordinary_tx_queue_snapshot(queue)
    }

    pub fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        self.pac_mut().begin_mac_tx_timeout_abort(queue)
    }

    pub fn with_tx_queue_detached<R>(
        &mut self,
        queue: u8,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        self.pac_mut().with_detached_mac_tx(queue, reason, detached)
    }

    pub fn take_block_ack_completion(
        &mut self,
        queue: u8,
    ) -> Option<MacHtAmpduCompletionObservation> {
        self.pac_mut().take_mac_block_ack_completion(queue)
    }

    pub fn prepare_he_trigger_based_queue(
        &mut self,
        policy: MacHeTbTidLimit,
        reservation: MacHeTbLinkReservation,
        tid: MacHeTid,
        mpdu_lengths: &[u16],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        self.pac_mut().prepare_he_trigger_based_queue(
            policy,
            reservation,
            tid,
            mpdu_lengths,
            queued_msdu_bytes,
        )
    }

    pub fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        self.pac_mut().clear_he_trigger_based_queue(reservation);
    }

    pub fn he_trigger_based_queue_snapshot(
        &self,
        reservation: MacHeTbLinkReservation,
    ) -> MacHeTriggerTxQueueSnapshot {
        self.pac().he_trigger_based_queue_snapshot(reservation)
    }

    /// Observe the non-latched RX diagnostic word decoded by the reviewed PAC.
    pub fn he_trigger_receive_diagnostics(&self) -> MacHeTriggerRxDiagnostics {
        self.pac().he_trigger_receive_diagnostics()
    }

    pub fn rx_buffer_full_count(&mut self) -> u16 {
        self.pac_mut().mac_rx_buffer_full_count()
    }

    pub fn rx_last_descriptor_low(&mut self) -> u32 {
        self.pac().mac_rx_last_descriptor_low()
    }

    pub fn rx_next_descriptor_low(&mut self) -> u32 {
        self.pac().mac_rx_next_descriptor().address_low()
    }

    pub fn rx_next_descriptor(&self) -> oer_esp32s31_pac::MacRxNextDescriptorObservation {
        self.pac().mac_rx_next_descriptor()
    }

    pub fn rx_walker_enabled(&mut self) -> bool {
        self.pac_mut().mac_rx_walker_enabled()
    }

    pub fn rx_reload_pending(&mut self) -> bool {
        self.pac_mut().mac_rx_reload_pending()
    }

    pub fn configure_rx_descriptor_window(&mut self, range: &StableDmaRange<'_>) {
        self.pac_mut().configure_mac_rx_descriptor_window(range);
    }

    pub fn write_rx_descriptor_base(&mut self, range: &StableDmaRange<'_>, address: u32) {
        self.pac_mut().write_mac_rx_descriptor_base(range, address);
    }

    pub fn publish_rx_walker_enable(&mut self, range: &StableDmaRange<'_>) {
        self.pac_mut().publish_mac_rx_walker_enable(range);
    }

    pub fn request_rx_descriptor_reload(&mut self, range: &StableDmaRange<'_>) {
        self.pac_mut().request_mac_rx_descriptor_reload(range);
    }

    pub fn try_enable_rx_walker(&mut self, range: &StableDmaRange<'_>) -> bool {
        self.pac_mut().try_enable_mac_rx_walker(range)
    }

    pub fn try_disable_rx_walker(&mut self) -> bool {
        self.pac_mut().try_disable_mac_rx_walker()
    }

    pub fn order_device_accesses(&mut self) {
        self.pac_mut().order_device_accesses();
    }

    pub fn station_tsf(&mut self) -> u64 {
        self.pac_mut().station_tsf()
    }

    /// Snapshot the AP timer through the same exclusive owner as STA TSF.
    /// The complete select/read/release transaction ends before returning.
    pub fn access_point_tsf(&mut self) -> u64 {
        self.pac_mut().access_point_tsf()
    }

    /// Replace the station TSF, as the vendor does from an access point's
    /// timestamp. Outside the HAL only the unique register owner writes it
    /// ([`PublishedRadioOwner::set_station_tsf`](crate::ieee80211::arena::PublishedRadioOwner::set_station_tsf)).
    pub(crate) fn set_station_tsf(&mut self, value: u64) {
        self.pac_mut().set_station_tsf(value);
    }

    /// The Wi-Fi MAC local time: the free-running microsecond counter whose
    /// readings receive timestamps carry.
    pub fn mac_local_time(&self) -> u32 {
        self.pac().mac_local_time().now()
    }

    /// Start the station TSF, as the vendor does when the station starts.
    pub fn enable_station_tsf(&mut self) {
        self.pac_mut().enable_station_tsf();
    }

    pub fn program_rx_block_ack_entry(
        &mut self,
        index: MacRxBlockAckEntryIndex,
        interface: MacInterface,
        peer: [u8; 6],
        tid: MacRxBlockAckTid,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) {
        self.pac_mut().program_rx_block_ack_entry(
            index,
            interface,
            peer,
            tid,
            starting_sequence,
            window,
        );
    }

    pub fn delete_rx_block_ack_entry(&mut self, index: MacRxBlockAckEntryIndex) {
        self.pac_mut().delete_rx_block_ack_entry(index);
    }

    pub fn rx_block_ack_entry_snapshot(
        &self,
        index: MacRxBlockAckEntryIndex,
    ) -> Option<RxBlockAckEntrySnapshot> {
        self.pac().rx_block_ack_entry_snapshot(index)
    }

    pub fn reset_rx_block_ack_window(
        &mut self,
        index: MacRxBlockAckEntryIndex,
        tid: MacRxBlockAckTid,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) {
        self.pac_mut()
            .reset_rx_block_ack_window(index, tid, starting_sequence, window);
    }

    pub fn program_extra_softap_rx_block_ack_entry(
        &mut self,
        index: MacExtraSoftApRxBlockAckEntryIndex,
        interface: MacInterface,
        peer: [u8; 6],
        tid: MacRxBlockAckTid,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) -> bool {
        self.pac_mut().program_extra_softap_rx_block_ack_entry(
            index,
            interface,
            peer,
            tid,
            starting_sequence,
            window,
        )
    }

    pub fn delete_extra_softap_rx_block_ack_entry(
        &mut self,
        index: MacExtraSoftApRxBlockAckEntryIndex,
    ) {
        self.pac_mut().delete_extra_softap_rx_block_ack_entry(index);
    }

    pub fn reset_extra_softap_rx_block_ack_window(
        &mut self,
        index: MacExtraSoftApRxBlockAckEntryIndex,
        starting_sequence: MacRxBlockAckStartingSequence,
        window: MacRxBlockAckWindow,
    ) {
        self.pac_mut()
            .reset_extra_softap_rx_block_ack_window(index, starting_sequence, window);
    }

    pub fn extra_softap_rx_block_ack_entry_snapshot(
        &mut self,
        index: MacExtraSoftApRxBlockAckEntryIndex,
    ) -> ExtraSoftApRxBlockAckEntrySnapshot {
        self.pac_mut()
            .extra_softap_rx_block_ack_entry_snapshot(index)
    }

    pub fn set_he_trigger_based_tid_enabled(&mut self, tid: MacHeTid, enabled: bool) {
        self.pac_mut()
            .set_he_trigger_based_tid_enabled(tid, enabled);
    }
}

/// Execute the exact role-policy HAL transaction in an isolated validation
/// image without exposing the PAC owner to the probe crate.
///
/// Only singleton acquisition is replaced. Register ownership and the named
/// production HAL operation are unchanged.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_configure_role_receive_policy(policy: MacRoleReceivePolicy) {
    crate::owner::RadioRuntimeOwner::claim_for_validation()
        .wifi_mac_hal()
        .configure_role_receive_policy(policy);
}

/// Apply complete rev0 ROM `phy_enable_cca` or `phy_disable_cca`.
#[cfg(all(feature = "validation-probes", target_arch = "riscv32"))]
fn set_cca_enabled(registers: &mut WifiRadioRegisters, enabled: bool) {
    registers.set_phy_wifi_cca_enabled(enabled);
}

/// Apply complete rev0 ROM `phy_sifs_reg_init`.
#[cfg(all(feature = "validation-probes", target_arch = "riscv32"))]
fn initialize_sifs(registers: &mut WifiRadioRegisters) {
    registers.initialize_phy_wifi_sifs();
}

#[cfg(all(feature = "validation-probes", target_arch = "riscv32"))]
#[doc(hidden)]
pub fn validation_set_cca_enabled(enabled: bool) {
    let mut owner = crate::owner::RadioRuntimeOwner::claim_for_validation();
    set_cca_enabled(owner.pac_mut(), enabled);
}

#[cfg(all(feature = "validation-probes", target_arch = "riscv32"))]
#[doc(hidden)]
pub fn validation_initialize_sifs() {
    let mut owner = crate::owner::RadioRuntimeOwner::claim_for_validation();
    initialize_sifs(owner.pac_mut());
}

#[cfg(test)]
mod tests;
