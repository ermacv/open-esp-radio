//! Optional AP epoch observations shared by exclusive and paired roles.

use super::*;
#[cfg(feature = "diagnostics")]
use core::cell::RefCell;
#[cfg(feature = "diagnostics")]
use embassy_sync::blocking_mutex::Mutex;
#[cfg(feature = "diagnostics")]
use oer_esp32s31_ieee80211_runtime::diagnostics::access_point::{
    AccessPointTerminalObservation, AccessPointTerminalObserver,
};

#[cfg(feature = "diagnostics")]
struct ProductionAccessPointTerminalObserver;

#[cfg(feature = "diagnostics")]
static ACCESS_POINT_TERMINAL_OBSERVER: ProductionAccessPointTerminalObserver =
    ProductionAccessPointTerminalObserver;
#[cfg(feature = "diagnostics")]
static ACCESS_POINT_TERMINAL_OBSERVATION: Mutex<
    CriticalSectionRawMutex,
    RefCell<Option<AccessPointTerminalObservation>>,
> = Mutex::new(RefCell::new(None));
#[cfg(feature = "diagnostics")]
static ACCESS_POINT_RX_HARDWARE_OBSERVATION: Mutex<
    CriticalSectionRawMutex,
    RefCell<crate::DiagnosticRxStatistics>,
> = Mutex::new(RefCell::new(crate::DiagnosticRxStatistics {
    mpdu_count: 0,
    data_success: 0,
    fcs_error: 0,
    abort: 0,
    abort_fcs_pass: 0,
    power_drop_error: 0,
    he_sig_b_error: 0,
    same_bm_error: 0,
    signal_field: 0,
    end: 0,
    other_unicast: 0,
    buffer_full: 0,
    fifo_overflow: 0,
    tkip_error: 0,
    bt_block_error: 0,
    frequency_hop_error: 0,
    last_unmatched_error: 0,
    ack_interrupt: 0,
    rts_interrupt: 0,
    brx_agc_error: 0,
    brx_error: 0,
    nrx_error: 0,
    nrx_abort: 0,
    nrx_agc_exit: 0,
    nrx_baseband_off: 0,
    nrx_fdm_watchdog: 0,
    nrx_restart: 0,
    nrx_service: 0,
    nrx_tx_over: 0,
    nrx_unsupported: 0,
    nrx_he_format: 0,
    nrx_ht_sig: 0,
    nrx_he_unsupported: 0,
    nrx_he_sig_a_crc: 0,
    rx_hang: 0,
    tx_hang: 0,
    rx_tx_hang: 0,
    rx_tx_panic: 0,
}));

#[cfg(feature = "diagnostics")]
impl AccessPointTerminalObserver for ProductionAccessPointTerminalObserver {
    fn observe(&self, observation: AccessPointTerminalObservation) {
        ACCESS_POINT_TERMINAL_OBSERVATION.lock(|slot| {
            slot.replace(Some(observation));
        });
    }
}

#[cfg(feature = "diagnostics")]
pub(super) fn begin_access_point_observation() -> &'static dyn AccessPointTerminalObserver {
    ACCESS_POINT_TERMINAL_OBSERVATION.lock(|slot| {
        slot.replace(None);
    });
    ACCESS_POINT_RX_HARDWARE_OBSERVATION.lock(|slot| {
        slot.replace(crate::DiagnosticRxStatistics::default());
    });
    &ACCESS_POINT_TERMINAL_OBSERVER
}

#[cfg(feature = "diagnostics")]
pub(super) fn store_access_point_rx_hardware_observation(
    observation: crate::DiagnosticRxStatistics,
) {
    ACCESS_POINT_RX_HARDWARE_OBSERVATION.lock(|slot| {
        slot.replace(observation);
    });
}

#[cfg(feature = "diagnostics")]
fn access_point_rx_hardware_observation() -> crate::DiagnosticRxStatistics {
    ACCESS_POINT_RX_HARDWARE_OBSERVATION.lock(|slot| *slot.borrow())
}

#[cfg(feature = "diagnostics")]
fn take_access_point_observation() -> Option<AccessPointTerminalObservation> {
    ACCESS_POINT_TERMINAL_OBSERVATION.lock(|slot| slot.take())
}

#[cfg(feature = "diagnostics")]
#[inline(never)]
pub(super) fn publish_access_point_observation(
    hook: fn(crate::AccessPointObservation),
    channel: WifiChannel,
    observation: &AccessPointTerminalObservation,
    rx_hardware: crate::DiagnosticRxStatistics,
) {
    let control = &observation.control;
    let mac = &observation.mac;
    let engine = &observation.engine;
    hook(crate::AccessPointObservation {
        channel: channel.primary(),
        bandwidth_mhz: channel.bandwidth_mhz(),
        beacons_transmitted: mac.beacons_transmitted,
        missed_beacon_intervals: control.missed_beacon_intervals,
        maximum_beacon_lateness_micros: control.maximum_beacon_lateness_micros,
        tx_interrupt_wakes: control.tx_interrupt_wakes,
        tx_deadline_wakes: control.tx_deadline_wakes,
        maximum_tx_pending_micros: control.maximum_tx_pending_micros,
        maximum_network_tx_pending_micros: control.maximum_network_tx_pending_micros,
        network_tx_attempts_at_maximum_pending: control.network_tx_attempts_at_maximum_pending,
        maximum_rx_service_micros: control.maximum_rx_service_micros,
        maximum_rx_dma_service_micros: control.maximum_rx_dma_service_micros,
        total_rx_dma_service_micros: control.total_rx_dma_service_micros,
        rx_dma_service_calls: control.rx_dma_service_calls,
        maximum_rx_protocol_service_micros: control.maximum_rx_protocol_service_micros,
        maximum_rx_protected_data_service_micros: control.maximum_rx_protected_data_service_micros,
        total_rx_protected_data_service_micros: control.total_rx_protected_data_service_micros,
        maximum_rx_management_service_micros: control.maximum_rx_management_service_micros,
        maximum_rx_eapol_service_micros: control.maximum_rx_eapol_service_micros,
        maximum_network_backpressure_micros: control.maximum_network_backpressure_micros,
        authentication_responses: mac.authentication_responses_transmitted,
        association_responses: mac.association_responses_transmitted,
        authorized_peers: engine.authorized_peers,
        maximum_associated_peers: engine.maximum_associated_peers,
        maximum_authorized_peers: engine.maximum_authorized_peers,
        peer_removals: engine.peer_removals,
        authentication_timeouts: engine.authentication_timeouts,
        wpa2_response_windows: engine.wpa2_response_windows,
        wpa2_pending_on_stop: engine.wpa2_pending_on_stop,
        wpa2_retransmissions: engine.wpa2_retransmissions,
        wpa2_handshake_failures: engine.wpa2_handshake_failures,
        wpa2_handshake_timeouts: engine.wpa2_handshake_timeouts,
        inactivity_timeouts: engine.inactivity_timeouts,
        disassociations_prepared: engine.disassociations_prepared,
        disassociations_published: mac.disassociations_published,
        disassociations_acknowledged: mac.disassociations_acknowledged,
        deauthentications_prepared: engine.deauthentications_prepared,
        deauthentications_published: mac.deauthentications_published,
        deauthentications_acknowledged: mac.deauthentications_acknowledged,
        tx_block_ack_requests_prepared: engine.tx_block_ack_requests_prepared,
        tx_block_ack_responses_observed: engine.tx_block_ack_responses_observed,
        tx_block_ack_agreements_operational: engine.tx_block_ack_agreements_operational,
        tx_block_ack_responses_rejected: engine.tx_block_ack_responses_rejected,
        tx_block_ack_negotiation_timeouts: engine.tx_block_ack_negotiation_timeouts,
        rx_block_ack_responses_transmitted: mac.rx_block_ack_responses_transmitted,
        rx_hardware,
        retained_rx_descriptors: control.retained_rx_descriptors,
        ignored_rx_frames: control.ignored_rx_frames,
        rx_mic_failures: control.rx_mic_failures,
        rx_quarantined_frames: control.rx_quarantined_frames,
        rx_view_rejected: control.rx_view_rejected,
        control_frames_staged: control.control_frames_staged,
        control_frames_dropped_while_busy: control.control_frames_dropped_while_busy,
        ethernet_frames_staged: control.ethernet_frames_staged,
        ethernet_arp_requests_staged: control.ethernet_arp_requests_staged,
        ethernet_tcp_frames_staged: control.ethernet_tcp_frames_staged,
        network_tx_frames_observed: control.network_tx_frames_observed,
        network_tx_arp_requests: control.network_tx_arp_requests,
        network_tx_arp_replies: control.network_tx_arp_replies,
        network_tx_rejected_no_peer: control.network_tx_rejected_no_peer,
        network_tx_rejected_destination: control.network_tx_rejected_destination,
        network_tx_frames_rejected: control.network_tx_frames_rejected,
        rx_ht_data_frames: control.rx_ht_data_frames,
        rx_ht_mpdus_with_aggregation_bit: control.rx_ht_mpdus_with_aggregation_bit,
        rx_rssi_samples: control.rx_rssi_samples,
        rx_rssi_sum_dbm: control.rx_rssi_sum_dbm,
        rx_rssi_min_dbm: control.rx_rssi_min_dbm,
        rx_rssi_max_dbm: control.rx_rssi_max_dbm,
        rx_ht40_mcs_frames: control.rx_ht40_mcs_frames,
        rx_ht40_long_gi_frames: control.rx_ht40_long_gi_frames,
        rx_ht40_short_gi_frames: control.rx_ht40_short_gi_frames,
        rx_ht40_mcs32_frames: control.rx_ht40_mcs32_frames,
        rx_ht_mcs32_width_mismatches: control.rx_ht_mcs32_width_mismatches,
        tx_ht_aggregates: control.tx_ht_aggregates,
        tx_ht40_mcs7_aggregates: control.tx_ht40_mcs7_aggregates,
        tx_aggregate_fill: control.tx_aggregate_fill,
        data_frames_transmitted: mac.data_frames_transmitted,
        ht_duplicate_tx_requests: mac.ht_duplicate_tx_requests,
        ht_duplicate_tx_selection: mac.ht_duplicate_tx_selection,
        data_tx_attempts: mac.data_tx.attempts,
        data_tx_retried_frames: mac.data_tx.retried_frames,
        data_tx_maximum_attempts: mac.data_tx.maximum_attempts,
        data_tx_minimum_final_rate_kbps: mac.data_tx.minimum_final_rate_kbps,
        data_tx_ack_snr_samples: mac.data_tx.ack_snr_samples,
        data_tx_minimum_ack_snr_db: mac.data_tx.minimum_ack_snr_db,
        data_tx_maximum_ack_snr_db: mac.data_tx.maximum_ack_snr_db,
        tx_ack_timeout_retries: mac.data_tx.ack_timeout_retries,
        tx_cts_timeout_retries: mac.data_tx.cts_timeout_retries,
        tx_collision_retries: mac.data_tx.collision_retries,
        tx_probe_ack_timeouts: mac.tx_failures.probe_ack_timeouts,
        tx_hardware_failures: mac.tx_failures.hardware_failures,
        tx_hardware_timeouts: mac.tx_failures.hardware_timeouts,
        tx_collision_limits: mac.tx_failures.collision_limits,
        tx_last_hardware_status: mac.tx_failures.last_hardware_status,
        protected_data_frames: control.protected_data_frames,
        protected_data_unauthorized: control.protected_data_unauthorized,
        protected_data_foreign: control.protected_data_foreign,
        protected_data_duplicates: control.protected_data_duplicates,
        rx_reorder_buffered_mpdus: control.rx_reorder_buffered_mpdus,
        rx_reorder_dispatched_mpdus: control.rx_reorder_dispatched_mpdus,
        rx_reorder_hardware_window_resets: control.rx_reorder_hardware_window_resets,
        rx_reorder_gap_timeouts: control.rx_reorder_gap_timeouts,
        rx_block_ack_requests: control.rx_block_ack_requests,
        rx_reorder_bar_released_mpdus: control.rx_reorder_bar_released_mpdus,
        rx_reorder_bar_missing_sequences: control.rx_reorder_bar_missing_sequences,
        protected_data_radio_rejected: control.protected_data_radio_rejected,
        protected_data_protocol_rejected: control.protected_data_protocol_rejected,
        first_rx_protocol_rejection: control.first_rx_protocol_rejection,
        security_mode_mismatches: control.security_mode_mismatches,
    });
}

#[cfg(feature = "diagnostics")]
#[inline(never)]
pub(super) fn publish_stored_access_point_observation(
    hook: fn(crate::AccessPointObservation),
    channel: WifiChannel,
) {
    let observation = take_access_point_observation()
        .expect("successful AP teardown emits one terminal observation");
    publish_access_point_observation(
        hook,
        channel,
        &observation,
        access_point_rx_hardware_observation(),
    );
}
