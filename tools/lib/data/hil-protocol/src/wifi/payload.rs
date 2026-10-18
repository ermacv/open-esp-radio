//! Wi-Fi role requests, ownership transitions and observations.

use super::{
    NetworkCredentials, NetworkCredentialsError, NetworkIpv4Configuration,
    WIFI_MONITOR_FRAME_CHUNK_MAX_LEN,
};
use core::fmt;
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Target-observed ownership edges for one requested station epoch cycle.
///
/// This is deliberately semantic rather than target-specific: the target
/// adapter may implement the individual operations differently, but it may
/// publish completion only after every owned resource crossed these four
/// finite boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationEpochEvidence {
    pub runner_stopped: bool,
    pub scan_owners_returned: bool,
    pub join_completed: bool,
    pub wdev_runner_started: bool,
}

/// Wi-Fi role represented by the target-side application owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum WifiRole {
    Idle,
    Station,
    Monitor,
    AccessPoint,
    StationAccessPoint,
}

/// Logical network endpoint backed by the shared physical Wi-Fi owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum WifiNetworkInterface {
    Station,
    AccessPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum WifiRoleOperation {
    Start,
    Stop,
    Restart,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum WifiRoleFailureReason {
    Rejected,
    HardwareFault,
    GenerationMismatch,
}

/// Terminal correlated failure for a role command. This prevents the host
/// from turning a target-side ownership fault into an opaque timeout.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiRoleFailureEvidence {
    pub role: WifiRole,
    pub operation: WifiRoleOperation,
    pub reason: WifiRoleFailureReason,
}

/// Complete target-neutral configuration for the first AP role.
///
/// Beacon interval (100 TU) and DTIM period (2) are driver guarantees. Channel
/// width and client admission remain explicit test inputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum WifiChannelWidth {
    Mhz20,
    Mhz40Above,
    Mhz40Below,
}

impl WifiChannelWidth {
    pub const fn bandwidth_mhz(self) -> u16 {
        match self {
            Self::Mhz20 => 20,
            Self::Mhz40Above | Self::Mhz40Below => 40,
        }
    }

    pub const fn admits_primary(self, channel: u8) -> bool {
        match self {
            Self::Mhz20 => channel >= 1 && channel <= 13,
            Self::Mhz40Above => channel >= 1 && channel <= 9,
            Self::Mhz40Below => channel >= 5 && channel <= 13,
        }
    }
}

/// Security mode selected for one explicitly started access-point epoch.
///
/// Keeping the choice on the wire lets one firmware image perform causal
/// Open/WPA2 comparisons instead of comparing feature-dependent ELF layouts.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum WifiAccessPointSecurity {
    Open,
    Wpa2Personal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiAccessPointRequest {
    pub credentials: NetworkCredentials,
    pub security: WifiAccessPointSecurity,
    pub channel: u8,
    pub channel_width: WifiChannelWidth,
    pub client_limit: u8,
    /// IP configuration owned by the HIL application while the AP role is
    /// active. This is deliberately outside the radio-driver request.
    pub ipv4: NetworkIpv4Configuration,
}

/// One upstream station plus one same-channel downstream SoftAP request.
///
/// The AP channel is explicit and the production driver rejects the complete
/// request unless the upstream association negotiates exactly that channel
/// and width. The HIL layer does not add a channel-switching fallback.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiStationAccessPointRequest {
    pub station_credentials: NetworkCredentials,
    pub access_point: WifiAccessPointRequest,
}

impl WifiStationAccessPointRequest {
    pub fn validate(&self) -> Result<(), WifiAccessPointRequestError> {
        self.station_credentials
            .validate()
            .map_err(WifiAccessPointRequestError::Credentials)?;
        self.access_point.validate()
    }
}

impl WifiAccessPointRequest {
    pub fn validate(&self) -> Result<(), WifiAccessPointRequestError> {
        self.credentials
            .validate()
            .map_err(WifiAccessPointRequestError::Credentials)?;
        if !self.channel_width.admits_primary(self.channel) {
            return Err(WifiAccessPointRequestError::Channel);
        }
        if !(1..=15).contains(&self.client_limit) {
            return Err(WifiAccessPointRequestError::ClientLimit);
        }
        if !matches!(
            self.ipv4,
            NetworkIpv4Configuration::Static { gateway: None, .. }
        ) || !self.ipv4.validate()
        {
            return Err(WifiAccessPointRequestError::Ipv4);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiAccessPointRequestError {
    Credentials(NetworkCredentialsError),
    Channel,
    ClientLimit,
    Ipv4,
}

impl fmt::Display for WifiAccessPointRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credentials(error) => error.fmt(formatter),
            Self::Channel => formatter
                .write_str("AP primary channel and secondary-channel geometry are inconsistent"),
            Self::ClientLimit => formatter.write_str("AP client limit must be in 1..=15"),
            Self::Ipv4 => formatter
                .write_str("AP mode requires a valid gateway-free static IPv4 configuration"),
        }
    }
}

/// Compact, target-neutral standalone scan request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiScanRequest {
    /// Bit zero selects channel 1 and bit twelve selects channel 13.
    pub channel_mask_2_4_ghz: u16,
    pub dwell_millis: u16,
}

/// Compact, target-neutral standalone monitor request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiMonitorRequest {
    pub channel: u8,
    /// Zero retains the complete frame; a nonzero value truncates captures.
    pub snapshot_length: u16,
}

/// Finite typed monitor export request. Duration is owned by the target so a
/// congested serial link cannot postpone the role stop indefinitely.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiMonitorCaptureRequest {
    pub channel: u8,
    pub snapshot_length: u16,
    pub duration_millis: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum WifiMonitorEvidenceSource {
    Hardware,
    Protocol,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiMonitorObserved<T> {
    pub source: WifiMonitorEvidenceSource,
    pub value: T,
}

/// Typed transport form of the staged chip's receive vector. The raw hardware
/// rate code remains explicitly scoped to its decoded PHY format.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiMonitorPhyEvidence {
    pub format: WifiMonitorPhyFormat,
    pub hardware_rate_code: u8,
    pub he_siga1: u32,
    pub he_siga2: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum WifiMonitorPhyFormat {
    Dot11b,
    Ofdm,
    Ht,
    Vht,
    HeSu,
    HeMu,
    HeExtendedRangeSu,
    HeTriggerBased,
    VhtMu,
    Unknown(u8),
}

/// One independently checksummed piece of one captured normalized MPDU.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiMonitorFrameChunk {
    pub generation: u32,
    pub frame_sequence: u32,
    pub dequeued_at_micros: u64,
    pub captured_length: u16,
    pub logical_length: u16,
    pub offset: u16,
    pub channel: Option<WifiMonitorObserved<u8>>,
    pub rssi_dbm: Option<WifiMonitorObserved<i8>>,
    pub rate: Option<WifiMonitorObserved<WifiMonitorPhyEvidence>>,
    bytes: heapless::Vec<u8, WIFI_MONITOR_FRAME_CHUNK_MAX_LEN>,
}

impl WifiMonitorFrameChunk {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        generation: u32,
        frame_sequence: u32,
        dequeued_at_micros: u64,
        captured_length: u16,
        logical_length: u16,
        offset: u16,
        channel: Option<WifiMonitorObserved<u8>>,
        rssi_dbm: Option<WifiMonitorObserved<i8>>,
        rate: Option<WifiMonitorObserved<WifiMonitorPhyEvidence>>,
        bytes: &[u8],
    ) -> Result<Self, WifiMonitorFrameChunkError> {
        if captured_length == 0 || bytes.is_empty() {
            return Err(WifiMonitorFrameChunkError::Empty);
        }
        let end = usize::from(offset)
            .checked_add(bytes.len())
            .ok_or(WifiMonitorFrameChunkError::Range)?;
        if end > usize::from(captured_length) {
            return Err(WifiMonitorFrameChunkError::Range);
        }
        let mut body = heapless::Vec::new();
        body.extend_from_slice(bytes)
            .map_err(|_| WifiMonitorFrameChunkError::TooLarge)?;
        Ok(Self {
            generation,
            frame_sequence,
            dequeued_at_micros,
            captured_length,
            logical_length,
            offset,
            channel,
            rssi_dbm,
            rate,
            bytes: body,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    pub fn is_final(&self) -> bool {
        usize::from(self.offset) + self.bytes.len() == usize::from(self.captured_length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiMonitorFrameChunkError {
    Empty,
    TooLarge,
    Range,
}

/// Completion of one explicit role ownership transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiRoleTransitionEvidence {
    pub previous: WifiRole,
    pub current: WifiRole,
    pub generation: u32,
}

/// What the shared radio did while Wi-Fi left and rejoined it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum WifiRadioRestartRf {
    /// Wi-Fi was the last PHY client: RF was closed and woken again.
    ClosedAndWoken,
    /// Another PHY client kept RF open; Wi-Fi only left and rejoined.
    KeptOpen,
}

/// Completion of an idle Wi-Fi restart on the shared radio.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiRadioRestartEvidence {
    pub generation: u32,
    pub rf: WifiRadioRestartRf,
}

/// Bounded scan evidence. The complete BSS table stays in the driver API and
/// is intentionally not expanded to fit the UART protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiScanEvidence {
    pub generation: u32,
    pub elapsed_micros: u64,
    pub observed_frames: u32,
    pub unique_bss: u8,
    pub dropped_unique_bss: u32,
    pub configured_ssid_found: bool,
    pub configured_ssid_channel: u8,
    pub configured_ssid_rssi_dbm: i8,
}

/// Observations retained by the HIL consumer during one monitor epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiMonitorEvidence {
    pub generation: u32,
    pub elapsed_micros: u64,
    pub channel: u8,
    pub captured_frames: u32,
    pub captured_bytes: u64,
    pub generation_mismatches: u32,
    /// Frames whose RX metadata explicitly named another channel.
    pub channel_mismatches: u32,
    /// Frames for which the backend did not expose per-frame channel data.
    pub channel_unavailable: u32,
    /// Most recent explicit hardware/protocol channel, or zero if unavailable.
    pub last_observed_channel: u8,
    /// Capture-pool publication counters for this exact generation.
    pub published_frames: u32,
    pub full_drops: u32,
    pub oversized_drops: u32,
    pub discarded_frames: u32,
    /// Frames whose complete ordered chunk set was admitted to the protocol
    /// queue before this terminal evidence.
    pub exported_frames: u32,
}

/// Complete interval delta of the MAC/baseband RX statistics made available
/// by the target. Counter names whose hardware meaning is not yet established
/// remain explicitly vendor-shaped instead of being assigned a false cause.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiMacRxHardwareEvidence {
    pub mpdu_count: u16,
    pub data_success: u16,
    pub fcs_error: u16,
    pub abort: u16,
    pub abort_fcs_pass: u16,
    pub power_drop_error: u16,
    pub he_sig_b_error: u16,
    pub same_bm_error: u16,
    pub signal_field: u16,
    pub end: u16,
    pub other_unicast: u16,
    pub buffer_full: u16,
    pub fifo_overflow: u16,
    pub tkip_error: u16,
    pub bluetooth_block_error: u16,
    pub frequency_hop_error: u16,
    /// Vendor-named terminal RX state counter. Its exact semantic meaning is
    /// intentionally not inferred by the target-neutral HIL protocol.
    pub last_unmatched_error: u16,
    pub ack_interrupt: u16,
    pub rts_interrupt: u16,
    pub brx_agc_error: u16,
    pub brx_error: u16,
    pub nrx_error: u16,
    pub nrx_abort: u16,
    pub nrx_agc_exit: u16,
    pub nrx_baseband_off: u16,
    pub nrx_fdm_watchdog: u16,
    pub nrx_restart: u16,
    pub nrx_service: u16,
    pub nrx_tx_over: u16,
    pub nrx_unsupported: u16,
    pub nrx_he_format: u16,
    pub nrx_ht_sig: u16,
    pub nrx_he_unsupported: u16,
    pub nrx_he_sig_a_crc: u16,
    pub rx_hang: u8,
    pub tx_hang: u8,
    pub rx_tx_hang: u32,
    pub rx_tx_panic: u32,
}

/// Fill of the network A-MPDUs one access point started to one association,
/// reported after the AP stops, one record per association that received
/// any. The AP diagnostics observer records it; an observer-free image sends
/// none.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiApAggregateFill {
    pub generation: u32,
    pub association_id: u16,
    pub aggregates: u32,
    /// Sum of the subframes of every started A-MPDU.
    pub subframes: u32,
    pub maximum_subframes: u8,
    /// A-MPDUs of 1, 2..=7, 8..=15, 16..=31 and 32 or more subframes.
    pub histogram: [u32; 5],
}

/// Bounded evidence retained for one access-point ownership epoch.
///
/// These counters describe MAC/runtime work only. IP services and host-side
/// client observations belong to the qualification report, not the driver.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiAccessPointEvidence {
    pub generation: u32,
    pub channel: u8,
    pub bandwidth_mhz: u16,
    pub beacons_transmitted: u32,
    pub missed_beacon_intervals: u32,
    pub maximum_beacon_lateness_micros: u32,
    pub tx_interrupt_wakes: u32,
    pub tx_deadline_wakes: u32,
    pub maximum_tx_pending_micros: u32,
    /// Longest network data transaction; excludes chained AP control frames.
    pub maximum_network_tx_pending_micros: u32,
    pub network_tx_attempts_at_maximum_pending: u8,
    pub maximum_rx_service_micros: u32,
    pub maximum_rx_dma_service_micros: u32,
    pub total_rx_dma_service_micros: u32,
    pub rx_dma_service_calls: u32,
    pub maximum_rx_protocol_service_micros: u32,
    pub maximum_rx_protected_data_service_micros: u32,
    pub total_rx_protected_data_service_micros: u32,
    pub maximum_rx_management_service_micros: u32,
    pub maximum_rx_eapol_service_micros: u32,
    pub maximum_network_backpressure_micros: u32,
    pub authentication_responses: u32,
    pub association_responses: u32,
    /// Successful controlled-port openings, including re-authorizations.
    pub authorized_peers: u32,
    /// Maximum number of peers admitted at the same time.
    pub maximum_associated_peers: u8,
    /// Maximum number of controlled ports open at the same time.
    pub maximum_authorized_peers: u8,
    pub peer_removals: u32,
    pub authentication_timeouts: u32,
    pub wpa2_response_windows: u32,
    pub wpa2_pending_on_stop: u32,
    pub wpa2_retransmissions: u32,
    pub wpa2_handshake_failures: u32,
    pub wpa2_handshake_timeouts: u32,
    pub inactivity_timeouts: u32,
    pub disassociations_prepared: u32,
    /// Disconnect frames accepted by the target hardware TX owner.
    pub disassociations_published: u32,
    /// Published disconnect frames whose terminal completion reported an ACK.
    pub disassociations_acknowledged: u32,
    pub deauthentications_prepared: u32,
    pub deauthentications_published: u32,
    pub deauthentications_acknowledged: u32,
    pub tx_block_ack_requests_prepared: u32,
    pub tx_block_ack_responses_observed: u32,
    pub tx_block_ack_agreements_operational: u32,
    pub tx_block_ack_responses_rejected: u32,
    pub tx_block_ack_negotiation_timeouts: u32,
    /// Peer-originated RX ADDBA responses completed by the AP hardware owner.
    pub rx_block_ack_responses_transmitted: u32,
    /// Complete vendor-shaped RX units made visible to the AP protocol path.
    pub completed_rx_units: u32,
    pub completed_rx_descriptors: u32,
    /// Descriptors safely rearmed and returned to DMA during the live epoch.
    pub recycled_rx_descriptors: u32,
    /// Hardware MAC/baseband counter increments across the complete AP epoch.
    pub rx_hardware: WifiMacRxHardwareEvidence,
    /// Completed descriptors retained until the walker is stopped because a
    /// later completion was still serving as their generation guard.
    pub retained_rx_descriptors: u32,
    /// Complete units intentionally discarded after their payload was observed.
    pub discarded_rx_units: u32,
    /// Bulk protected units discarded after upper-copy saturation while DMA
    /// ownership was recycled immediately.
    pub rx_overload_discarded_units: u32,
    pub rx_critical_reserve_admissions: u32,
    pub rx_critical_admission_blocked: u32,
    pub ignored_rx_frames: u32,
    pub rx_mic_failures: u32,
    pub rx_quarantined_frames: u32,
    pub rx_view_rejected: u32,
    pub control_frames_staged: u32,
    pub control_frames_dropped_while_busy: u32,
    pub ethernet_frames_staged: u32,
    pub ethernet_arp_requests_staged: u32,
    pub ethernet_tcp_frames_staged: u32,
    pub network_tx_frames_observed: u32,
    pub network_tx_arp_requests: u32,
    pub network_tx_arp_replies: u32,
    pub network_tx_rejected_no_peer: u32,
    pub network_tx_rejected_destination: u32,
    pub network_tx_frames_rejected: u32,
    /// Protected HT data MPDUs observed by the target RX boundary.
    pub rx_ht_data_frames: u32,
    /// Protected HT MPDUs whose copied HT-SIG Aggregation bit is set.
    /// The descriptor contract does not expose PPDU boundaries or depth.
    pub rx_ht_mpdus_with_aggregation_bit: u32,
    pub rx_rssi_samples: u32,
    pub rx_rssi_sum_dbm: i32,
    pub rx_rssi_min_dbm: i8,
    pub rx_rssi_max_dbm: i8,
    /// Protected HT40 data MPDUs grouped by hardware-observed MCS0..MCS7.
    pub rx_ht40_mcs_frames: [u32; 8],
    /// Protected HT40 data MPDUs observed with the 800 ns guard interval.
    pub rx_ht40_long_gi_frames: u32,
    /// Protected HT40 data MPDUs observed with the 400 ns guard interval.
    pub rx_ht40_short_gi_frames: u32,
    /// AP network A-MPDU transactions started with a typed HT vector.
    pub tx_ht_aggregates: u32,
    /// AP network A-MPDU transactions started with HT40 MCS7.
    pub tx_ht40_mcs7_aggregates: u32,
    pub data_frames_transmitted: u32,
    /// Total hardware publications for data MPDUs, including retries.
    pub data_tx_attempts: u32,
    /// Data MPDUs which required more than one hardware publication.
    pub data_tx_retried_frames: u32,
    pub data_tx_maximum_attempts: u8,
    /// Lowest terminal legacy/HT rate observed after any retry ladder.
    pub data_tx_minimum_final_rate_kbps: u32,
    pub data_tx_ack_snr_samples: u32,
    pub data_tx_minimum_ack_snr_db: i8,
    pub data_tx_maximum_ack_snr_db: i8,
    pub tx_ack_timeout_retries: u32,
    pub tx_cts_timeout_retries: u32,
    pub tx_collision_retries: u32,
    /// Subset of tx_hardware_failures: one-attempt probe responses without ACK.
    pub tx_probe_ack_timeouts: u8,
    pub tx_hardware_failures: u8,
    pub tx_hardware_timeouts: u8,
    pub tx_collision_limits: u8,
    pub tx_last_hardware_status: u8,
    pub protected_data_frames: u32,
    pub protected_data_unauthorized: u32,
    pub protected_data_foreign: u32,
    pub protected_data_duplicates: u32,
    pub rx_reorder_buffered_mpdus: u32,
    pub rx_reorder_dispatched_mpdus: u32,
    pub rx_reorder_hardware_window_resets: u32,
    pub rx_reorder_gap_timeouts: u32,
    /// Client BlockAckReq frames addressed to the AP.
    pub rx_block_ack_requests: u32,
    /// Retained MPDUs released by those requests' window moves.
    pub rx_reorder_bar_released_mpdus: u32,
    /// Sequence numbers those moves passed without an MPDU: frames the
    /// client gave up on and the AP never received.
    pub rx_reorder_bar_missing_sequences: u32,
    pub protected_data_radio_rejected: u32,
    pub protected_data_protocol_rejected: u32,
    #[serde(default)]
    pub first_rx_protocol_rejection: Option<crate::wifi::WifiRxRejection>,
    /// None when the image has no driver observer. Counts span the AP epoch,
    /// independently of individual traffic measurement windows.
    #[serde(default)]
    pub tx_retention: Option<crate::wifi::WifiTxRetentionEvidence>,
}

/// Terminal evidence for one explicit same-channel STA+AP stop transaction.
///
/// The role transition proves that every shared physical owner returned to
/// idle; the AP report preserves beacon and fairness-relevant observations
/// from that exact paired generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiStationAccessPointStopEvidence {
    pub transition: WifiRoleTransitionEvidence,
    pub access_point: WifiAccessPointEvidence,
}

impl StationEpochEvidence {
    pub const COMPLETE: Self = Self {
        runner_stopped: true,
        scan_owners_returned: true,
        join_completed: true,
        wdev_runner_started: true,
    };

    pub const fn is_complete(self) -> bool {
        self.runner_stopped
            && self.scan_owners_returned
            && self.join_completed
            && self.wdev_runner_started
    }
}

/// Why a connected station generation returned to candidate selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationDisconnectReason {
    /// The connected beacon monitor proved that the selected AP disappeared.
    BeaconLoss,
    /// The AP sent an IEEE 802.11 deauthentication frame.
    PeerDeauthentication { reason_code: u16 },
    /// The AP sent an IEEE 802.11 disassociation frame.
    PeerDisassociation { reason_code: u16 },
    /// Restoring active power-management state failed at the peer-visible TX edge.
    ActiveStateRestoreFailed,
    /// Connected-state WPA2 Group Key Handshake failed closed.
    GroupKeyHandshakeFailed,
    /// Another connected link policy returned the peer owner without claiming
    /// a beacon deadline; this must not qualify an AP-loss test.
    LinkPolicy,
    /// The host/application requested a healthy connected-epoch cycle.
    ReconnectRequested,
    /// The bounded connected-control mailbox overflowed, so the event stream
    /// can no longer be processed as complete.
    ///
    /// Kept at the end so the discriminants of the existing wire vocabulary
    /// remain stable.
    ControlMailboxOverflow,
    /// The access point did not answer the SA Query an unprotected
    /// disconnect started under management frame protection.
    SaQueryTimeout,
    /// The join beacon's TSF could not be advanced to the start of power
    /// management without crossing 2^64.
    JoinTsfUnrepresentable,
}

/// Stable station stage vocabulary used by HIL lifecycle evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationFailureStage {
    CandidateSelection,
    Authentication,
    Association,
    Security,
    Connected,
    Hardware,
}

/// Target-independent classification of a failed station attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationAttemptFailureReason {
    /// A complete candidate scan did not find the configured network.
    NoCandidate,
    /// A finite peer protocol exchange failed or timed out.
    PeerProtocol,
    /// Hardware ownership or a bounded hardware transaction failed.
    Hardware,
    /// The adapter observed an impossible production ownership contract.
    ContractViolation,
}

/// Station phase whose transition faulted: the portable
/// `oer_ieee80211_sta::station::StaFaultPhase`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationFaultPhase {
    /// The link had not connected in this attempt.
    Attempt,
    /// Bringing up the connected epoch.
    Start,
    /// The running connected epoch.
    Connected,
    /// Returning the connected epoch's owners.
    Teardown,
}

/// What a faulted station transition could not prove: the portable
/// `oer_ieee80211_sta::station::StaFaultCause`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationFaultCause {
    Hardware,
    Security,
    Configuration,
    Resources,
    RfState,
    /// The lower-MAC port the station is a client of was lost.
    PortLost,
}

/// Reliable target-observed station lifecycle edge.
///
/// Generation zero is the initial connection. The outer lifecycle increments
/// the generation only after a connected epoch returns its owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationLifecycleEvent {
    Connected {
        generation: u32,
        /// Association geometry from the production connected report, not the
        /// requested TOML channel or a last-packet PHY sample.
        association_bandwidth_mhz: Option<u16>,
        /// Installed connected security owner, if the image can observe it.
        security: Option<StationLinkSecurity>,
    },
    Disconnected {
        generation: u32,
        reason: StationDisconnectReason,
    },
    /// One complete attempt returned every owner and was classified for retry.
    AttemptFailed {
        generation: u32,
        attempt: u16,
        stage: StationFailureStage,
        reason: StationAttemptFailureReason,
    },
    /// The bounded reconnect policy returned the final owner without another
    /// hidden attempt or backoff.
    RetryExhausted {
        generation: u32,
        attempts: u16,
        stage: StationFailureStage,
        reason: StationAttemptFailureReason,
    },
    /// The station role faulted and retains a non-reusable owner: no further
    /// lifecycle edge follows, and later Wi-Fi commands fail.
    Faulted {
        generation: u32,
        phase: StationFaultPhase,
        cause: StationFaultCause,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationLinkSecurity {
    Open,
    Wpa2Personal {
        /// The association protects its robust management frames.
        management_protection: bool,
    },
    /// SAE authenticated the association.
    Wpa3Personal,
}
