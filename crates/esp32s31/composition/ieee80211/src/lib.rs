#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! Concrete ESP32-S31 Embassy Wi-Fi composition on the shared radio.
//!
//! On ESP32-S31, `new` brings Wi-Fi up as a client of the application's
//! [`SharedRadio`] and returns the Wi-Fi application capabilities with the
//! sole owner-holding runner. Board firmware owns credentials, IP policy and
//! sockets; it does not assemble PAC, DMA, ISR or role transactions. The
//! [`resources`] profile is also available on the host for product resource
//! and ownership validation.
//!
//! Every shared-PHY transaction (bring-up, channel changes, release) takes the
//! radio arbiter lease for that one transaction; the shared radio's periodic
//! tracking is the only PHY maintenance. Lifecycle failures use the
//! shared-PHY terminal policy: an unconfirmed MAC/RX stop, an ambiguous RF
//! close/wake or registration, and failed initial tracking or channel
//! execution request system reset while retaining the failed frontier. The
//! composition selects that response through the non-radio SoC adapter;
//! neither PHY nor MAC/DMA drivers depend on a reset/watchdog mechanism.
//! Rejected requests and ownership faults at an already-quiesced frontier
//! retain their non-runnable owners without escalation.
//! [`RadioConfig`] additionally requires a stable caller-owned [`WatchdogConfig`]
//! and explicit startup and shutdown budgets. Its SoC TIMG1 lease starts
//! before the transition and remains armed until it completes. Cancellation
//! leaves the timer armed; no periodic feed or default exists. This does not
//! qualify a worst-case reset-to-RF-off bound.

#[cfg(target_arch = "riscv32")]
mod lifecycle_policy;
#[cfg(target_arch = "riscv32")]
mod watchdog;
#[cfg(target_arch = "riscv32")]
pub use watchdog::WatchdogConfig;

// Inputs of `new`, `RadioConfig` and `WatchdogConfig`, and the shared radio
// they join, so an application can construct the composition through this
// crate (or the `oer` facade) alone.
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_hal::root::{ConcurrentPartitions, RadioHardware, WifiPartition};
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_ieee80211_esp_hal::EspHalWifiPlatform;
pub use oer_esp32s31_ieee80211_mac::tx::protection::RtsLengthThreshold;
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_radio_esp_hal::{EspHalRadioClocks, EspHalRadioPlatform};
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_radio_runtime::{RadioPhyError, RadioPhyPrepared};

/// The shared ESP32-S31 radio Wi-Fi joins: the arbiter with the esp-hal
/// platform and clock sources. The application creates it once and runs its
/// periodic PHY tracking
/// ([`RadioSystem::run_tracking`](oer_esp32s31_radio_runtime::RadioSystem::run_tracking)).
#[cfg(target_arch = "riscv32")]
pub type SharedRadio = oer_esp32s31_radio_runtime::RadioSystem<
    EspHalRadioPlatform,
    EspHalRadioClocks,
    oer_time_embassy::EmbassyClock,
>;
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_soc_esp_hal::watchdog::{DeadlineBudget, DeadlineWatchdog};
pub use oer_ieee80211_runtime::await_stack_boundary;
mod network_diagnostics;
pub mod resources;

pub use network_diagnostics::NetworkInterface;
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::{
    AccessPointAirtimeConfiguration, AccessPointAirtimeSelection,
};

#[cfg(not(feature = "owned-network"))]
#[cfg(target_arch = "riscv32")]
compile_error!("owned Xarxa is the only network integration: enable the owned-network feature");

#[cfg(feature = "diagnostics")]
#[cfg(target_arch = "riscv32")]
macro_rules! diagnostics_event {
    ($($argument:tt)*) => { log::info!($($argument)*) };
}

#[cfg(feature = "diagnostics")]
#[cfg(target_arch = "riscv32")]
macro_rules! diagnostics_debug {
    ($($argument:tt)*) => { log::debug!($($argument)*) };
}

#[cfg(not(feature = "diagnostics"))]
#[cfg(target_arch = "riscv32")]
macro_rules! diagnostics_event {
    ($($argument:tt)*) => {{
        if false {
            let _ = core::format_args!($($argument)*);
        }
    }};
}

#[cfg(not(feature = "diagnostics"))]
#[cfg(target_arch = "riscv32")]
macro_rules! diagnostics_debug {
    ($($argument:tt)*) => {{
        if false {
            let _ = core::format_args!($($argument)*);
        }
    }};
}

#[cfg(target_arch = "riscv32")]
mod composition;
#[cfg(feature = "diagnostics")]
#[cfg(target_arch = "riscv32")]
mod diagnostics;
#[cfg(target_arch = "riscv32")]
mod efuse;
#[cfg(target_arch = "riscv32")]
mod esp_now;
pub mod exit_evidence;
#[cfg(target_arch = "riscv32")]
mod facade;
#[cfg(target_arch = "riscv32")]
mod interrupts;
#[cfg(target_arch = "riscv32")]
mod monitor;
#[cfg(target_arch = "riscv32")]
mod radio_resources;
#[cfg(target_arch = "riscv32")]
mod status;
#[cfg(target_arch = "riscv32")]
mod supervisor;

/// Zero-copy RX origin shared by the station and AP endpoints, for its
/// adoption and fallback counters.
#[cfg(all(target_arch = "riscv32", feature = "owned-network"))]
pub fn rx_zero_copy_origin()
-> &'static oer_esp32s31_ieee80211_runtime::datapath::owned::RxZeroCopyOrigin {
    supervisor::rx_zero_copy_origin()
}
#[cfg(target_arch = "riscv32")]
mod wifi_network;

#[cfg(feature = "diagnostics")]
#[cfg(target_arch = "riscv32")]
pub use diagnostics::{
    ConnectedRxObservation, ConnectedRxObserver, DecodedRxPhyObservation, HeSuRxObservation,
    HtRxObservation, ReceiveEvidence,
};
#[cfg(target_arch = "riscv32")]
pub use efuse::EfuseMacError;
#[cfg(target_arch = "riscv32")]
pub use esp_now::{
    ESP_NOW_CCMP_HEADER_LEN, ESP_NOW_CCMP_MIC_LEN, ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY,
    ESP_NOW_DEFAULT_PEER_CAPACITY, ESP_NOW_KEY_LEN, ESP_NOW_RX_REPLAY_WINDOW_BITS,
    ESP_NOW_V1_MAX_PAYLOAD_LEN, ESP_NOW_V1_MAX_PROTECTED_MPDU_LEN,
    ESP_NOW_V1_MIN_PROTECTED_MPDU_LEN, ESP_NOW_V2_ACTION_PREFIX_LEN, ESP_NOW_V2_MAX_ACTION_LEN,
    ESP_NOW_V2_MAX_ELEMENT_COUNT, ESP_NOW_V2_MAX_ELEMENT_PAYLOAD_LEN, ESP_NOW_V2_MAX_MPDU_LEN,
    ESP_NOW_V2_MAX_PAYLOAD_LEN, ESP_NOW_V2_MAX_VENDOR_CONTENT_LEN, ESP_NOW_V2_VERSION,
    ESP32S31_DEFAULT_ESP_NOW_RX_QUEUE_DEPTH, ESP32S31_DEFAULT_ESP_NOW_TX_QUEUE_DEPTH,
    EspNowCcmpPacketNumber, EspNowCcmpPacketNumberError, EspNowConfig, EspNowConfigError,
    EspNowConnectedControl, EspNowConnectedControlConfigError, EspNowConnectedControlError,
    EspNowConnectedControlShutdown, EspNowCryptoDiagnostics, EspNowCryptoError, EspNowDestination,
    EspNowEncryptedPeerConfig, EspNowEncryptedPeerDiagnostics, EspNowEncryptedPeerError,
    EspNowEncryptedPeerId, EspNowEncryptedPeerMutationFailure, EspNowEncryptedPeerReplacement,
    EspNowEncryptedPeerRestoreFailure, EspNowEncryptedPeerTable, EspNowEncryptedPeerView,
    EspNowEncryptedProtocol, EspNowEncryptedReceiveError, EspNowEncryptedRxCandidate,
    EspNowEncryptedSendError, EspNowEncryptedV1Unavailable, EspNowKeyOwner, EspNowKeySlot,
    EspNowLmk, EspNowLongRangeMissing, EspNowLongRangeReached, EspNowLongRangeUnsupported,
    EspNowOffChannelFailureStage, EspNowOwnedV1Tx, EspNowPeerCapability, EspNowPeerChannelPolicy,
    EspNowPeerConfig, EspNowPeerId, EspNowPeerSecurity, EspNowPeerTableError, EspNowPhyMode,
    EspNowPhySupport, EspNowPmk, EspNowPmkError, EspNowPmkId, EspNowPmkMutationFailure,
    EspNowPmkOwner, EspNowPreparedEncryptedV1Tx, EspNowPreparedV2Tx, EspNowProtectedV1Envelope,
    EspNowProtectedV1WireError, EspNowProtocol, EspNowRandomValue, EspNowReceivePublisher,
    EspNowReceiveReceiver, EspNowReceivedV2, EspNowRemovedEncryptedPeer, EspNowRxMailboxEpochError,
    EspNowRxMailboxResources, EspNowRxMailboxShutdown, EspNowRxMetadata, EspNowRxPublishOutcome,
    EspNowRxPublisher, EspNowRxRateNormalization, EspNowRxReceiver, EspNowRxReplayCandidate,
    EspNowRxResources, EspNowTransmitHandle, EspNowTransmitMailboxOwner, EspNowTxBackpressure,
    EspNowTxBinding, EspNowTxCancelReason, EspNowTxCompletion, EspNowTxConfig, EspNowTxConfigError,
    EspNowTxError, EspNowTxMailboxEpochError, EspNowTxMailboxInvariantError,
    EspNowTxMailboxShutdown, EspNowTxResources, EspNowTxRuntimeFailure, EspNowTxTerminal,
    EspNowTxTicket, EspNowTxTrySendError, EspNowUnicastAddress, EspNowV1WireError, EspNowV2Action,
    EspNowV2Element, EspNowV2Elements, EspNowV2Frame, EspNowV2Payload, EspNowV2Reassembly,
    EspNowV2ReceiveError, EspNowV2RxEvent, EspNowV2RxMailboxError, EspNowV2RxOutcome,
    EspNowV2SendError, EspNowV2TxTrySendError, EspNowV2WireError, EspNowVersionError,
    EspNowWireVersion, EspressifLongRangeRate, StandaloneEspNowBinding,
    StandaloneEspNowBindingError, StandaloneEspNowChannelControl,
    StandaloneEspNowOffChannelRunError, StandaloneEspNowOffChannelRunFailure,
    StandaloneEspNowPeerError, StandaloneEspNowPhyChannelControl, StandaloneEspNowPrepareFailure,
    StandaloneEspNowReceive, StandaloneEspNowRequest, StandaloneEspNowRunError,
    StandaloneEspNowRunFailure, StandaloneEspNowRunReport, StandaloneEspNowRx,
    StandaloneEspNowRxProgress, StandaloneEspNowService, StandaloneEspNowStopError,
    StandaloneEspNowStopped, WifiStandaloneEspNowPlan, attach_esp_now_tx,
    encrypted_peer_destination, esp_now_encrypted_v1_codec_status, esp_now_wire_version,
    esp32s31_esp_now_phy_support, normalize_esp_now_rx_metadata,
    prepare_esp32s31_standalone_esp_now,
};
#[cfg(target_arch = "riscv32")]
pub use facade::{
    NewError, RadioError, WifiControl, WifiInitialization, WifiParts, WifiStarted, WifiSystem,
};
#[cfg(feature = "mac-irq-diagnostics")]
#[cfg(target_arch = "riscv32")]
pub use interrupts::MacIrqObservation;
#[cfg(target_arch = "riscv32")]
pub use interrupts::{mac_interrupt, power_interrupt};
#[cfg(target_arch = "riscv32")]
pub use monitor::{
    ESP32S31_MONITOR_CAPTURE_CAPACITY, Esp32s31MonitorBasebandFormat, Esp32s31MonitorPhyInfo,
    MONITOR_CHANNEL_SEQUENCE_CAPACITY, MonitorCapturePolicy, MonitorCaptureStatistics,
    MonitorChannelPolicy, MonitorChannelSequence, MonitorChannelSequenceError, MonitorFrames,
    MonitorRequest, ReceivedMonitorFrame,
};
#[cfg(feature = "core0-rx-coarse-telemetry")]
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_ieee80211_runtime::datapath::{
    TX_PERFORMANCE, TxPerformanceSnapshot, configure_adaptive_recycled_rx_probe_for_diagnostics,
    configure_recycled_rx_probe_delay_for_diagnostics,
    rx::dma::configure_interrupt_driven_recycled_append_for_diagnostics,
};
#[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_ieee80211_runtime::diagnostics::core0_rx_performance::{
    CORE0_PERFORMANCE, Core0PerformanceSample, Core0PerformanceSnapshot,
};
#[cfg(feature = "diagnostics")]
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_ieee80211_runtime::diagnostics::network::{
    RxNetworkDeliveryEvent, RxNetworkDeliveryObserver, RxObservedEthernetFrame,
    RxQosSequenceObservation,
};
#[cfg(feature = "task-poll-telemetry")]
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_ieee80211_runtime::diagnostics::{
    core0_ap_rx_cycles::{CORE0_AP_RX_CYCLES, Core0ApRxCycleSnapshot},
    core0_rx_cycles::{CORE0_RX_CYCLES, Core0RxCycleSnapshot, cycle_count},
    core0_rx_reorder_cycles::{CORE0_REORDER_CYCLES, Core0ReorderSnapshot},
    core0_rx_service_histogram::{
        CORE0_RX_SERVICE_HISTOGRAM, CORE0_RX_SERVICE_HISTOGRAM_BINS, Core0RxServiceBinSnapshot,
        Core0RxServiceHistogramSnapshot,
    },
};
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason;
#[cfg(target_arch = "riscv32")]
pub use radio_resources::WifiStackResources;
#[cfg(feature = "tx-psram-dma-probe")]
#[cfg(target_arch = "riscv32")]
pub use radio_resources::configure_direct_psram_tx_dma_probe;
#[cfg(feature = "tx-psram-dma-probe")]
#[cfg(target_arch = "riscv32")]
pub use radio_resources::{
    DirectPsramTxDmaProbeObservation, direct_psram_tx_dma_probe_observation,
};
#[cfg(target_arch = "riscv32")]
pub use radio_resources::{WifiDevice, WifiDevices, WifiNetworkDevice};
#[cfg(target_arch = "riscv32")]
pub use status::{
    AccessPointStatus, AccessPointStatusSnapshot, StationLinkSecurity, StationLinkState,
    StationStatus, StationStatusSnapshot,
};
#[cfg(feature = "diagnostics")]
#[cfg(target_arch = "riscv32")]
pub use supervisor::station::{DiagnosticRxStatistics, DiagnosticSnapshot, DiagnosticTxVector};
#[cfg(target_arch = "riscv32")]
pub use supervisor::{SystemRunner, new};
#[cfg(target_arch = "riscv32")]
pub use wifi_network::WifiNetworkRunner;

/// One low-overhead batch of Core0 connected-DATAPATH poll residence.
///
/// The diagnostic image supplies the CPU frequency used to convert `mcycle`
/// deltas. Batching keeps atomic/reporting work out of the per-poll hot path.
#[cfg(feature = "connected-datapath-cycle-telemetry")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub struct ConnectedDatapathPollBatch {
    pub polls: u32,
    pub poll_micros: u32,
    pub maximum_poll_micros: u32,
    pub over_100_micros: u32,
    pub over_500_micros: u32,
    pub over_1_000_micros: u32,
    pub over_5_000_micros: u32,
}

/// Value-only sink for connected-DATAPATH executor residence.
///
/// This hook exists only in the dedicated diagnostic build. It cannot access
/// the runner or alter wake/ownership semantics.
#[cfg(feature = "connected-datapath-cycle-telemetry")]
#[derive(Clone, Copy)]
#[cfg(target_arch = "riscv32")]
pub struct ConnectedDatapathPollObserver {
    cycles_per_micro: u32,
    record: fn(ConnectedDatapathPollBatch),
}

#[cfg(feature = "connected-datapath-cycle-telemetry")]
#[cfg(target_arch = "riscv32")]
impl ConnectedDatapathPollObserver {
    pub const fn new(cycles_per_micro: u32, record: fn(ConnectedDatapathPollBatch)) -> Self {
        assert!(cycles_per_micro != 0, "CPU clock must be non-zero");
        Self {
            cycles_per_micro,
            record,
        }
    }

    pub(crate) const fn cycles_per_micro(self) -> u32 {
        self.cycles_per_micro
    }

    pub(crate) fn record(self, batch: ConnectedDatapathPollBatch) {
        (self.record)(batch);
    }
}

/// Board-derived Wi-Fi identity. Reading eFuse remains an application
/// responsibility; credentials are supplied separately to `start_station`.
/// The PHY calibration identity and retained calibration cache belong to the
/// shared radio ([`SharedRadio`]).
#[cfg(target_arch = "riscv32")]
// CAPABILITY: phy-calibration-state-and-tracking-connected-observation-driven-service
pub struct RadioConfig {
    #[cfg(feature = "rx-ownership-observation")]
    pub(crate) rx_ownership_observer:
        Option<&'static dyn oer_esp32s31_ieee80211_dma::rx_observation::RxOwnershipObserver>,
    pub(crate) watchdog: &'static WatchdogConfig,
    pub(crate) access_point_airtime: Option<
        oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::AccessPointAirtimeConfiguration,
    >,
    pub(crate) station_mac: oer_radio::wifi::WifiMacAddress,
    pub(crate) access_point_mac: oer_radio::wifi::WifiMacAddress,
    pub(crate) initial_channel: oer_ieee80211_mac::channel::WifiChannel,
    pub(crate) maximum_tx_power_quarter_dbm: Option<i8>,
    pub(crate) rts_length_threshold: Option<RtsLengthThreshold>,
    #[cfg(feature = "connected-datapath-cycle-telemetry")]
    pub(crate) connected_datapath_poll_observer: Option<ConnectedDatapathPollObserver>,
    #[cfg(feature = "diagnostics")]
    pub(crate) diagnostics: Option<DiagnosticObservers>,
}

#[cfg(target_arch = "riscv32")]
impl RadioConfig {
    pub const fn new(
        watchdog: &'static WatchdogConfig,
        station_mac: oer_radio::wifi::WifiMacAddress,
        access_point_mac: oer_radio::wifi::WifiMacAddress,
        initial_channel: oer_ieee80211_mac::channel::WifiChannel,
    ) -> Self {
        Self {
            watchdog,
            #[cfg(feature = "rx-ownership-observation")]
            rx_ownership_observer: None,
            station_mac,
            access_point_airtime: None,
            access_point_mac,
            initial_channel,
            maximum_tx_power_quarter_dbm: None,
            rts_length_threshold: Some(RtsLengthThreshold::VENDOR_DEFAULT),
            #[cfg(feature = "connected-datapath-cycle-telemetry")]
            connected_datapath_poll_observer: None,
            #[cfg(feature = "diagnostics")]
            diagnostics: None,
        }
    }

    /// Observe physical RX allocation lifetimes using a caller-owned sink.
    /// Timestamping and recorder storage remain outside the radio.
    #[cfg(feature = "rx-ownership-observation")]
    pub fn with_rx_ownership_observer(
        mut self,
        observer: &'static dyn oer_esp32s31_ieee80211_dma::rx_observation::RxOwnershipObserver,
    ) -> Self {
        self.rx_ownership_observer = Some(observer);
        self
    }

    /// Attach an explicit airtime model to standalone AP epochs. The board
    /// retains the ledger through faults. Simultaneous STA+AP is outside this policy.
    pub fn with_access_point_airtime(
        mut self,
        configuration: oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::AccessPointAirtimeConfiguration,
    ) -> Self {
        self.access_point_airtime = Some(configuration);
        self
    }

    /// Apply the board/regulatory TX ceiling to the calibrated power profile.
    pub const fn with_maximum_tx_power_quarter_dbm(mut self, maximum: i8) -> Self {
        self.maximum_tx_power_quarter_dbm = Some(maximum);
        self
    }

    /// Set the local dot11RTSThreshold: every individually addressed PSDU
    /// longer than `threshold` bytes is preceded by RTS/CTS. `None` disables
    /// the length rule; BSS-required protection is unaffected. The default is
    /// the vendor's 2346 bytes.
    pub const fn with_rts_length_threshold(
        mut self,
        threshold: Option<RtsLengthThreshold>,
    ) -> Self {
        self.rts_length_threshold = threshold;
        self
    }

    /// Attach the dedicated Core0 connected-DATAPATH residence sink.
    #[cfg(feature = "connected-datapath-cycle-telemetry")]
    pub const fn with_connected_datapath_poll_observer(
        mut self,
        observer: ConnectedDatapathPollObserver,
    ) -> Self {
        self.connected_datapath_poll_observer = Some(observer);
        self
    }

    /// Attach value-only, non-blocking diagnostics observers. This API does
    /// not exist in production builds and grants no register or owner access.
    #[cfg(feature = "diagnostics")]
    pub const fn with_diagnostic_observers(mut self, hooks: DiagnosticObservers) -> Self {
        self.diagnostics = Some(hooks);
        self
    }
}

/// Optional value-only observers compiled only into diagnostics firmware.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy)]
#[cfg(target_arch = "riscv32")]
pub struct DiagnosticObservers {
    /// Intrusive per-stage timing/counter observer. Correctness images leave
    /// this unset; only dedicated pipeline diagnostics may charge the RX hot
    /// path for these observations.
    pub rx_pipeline: Option<
        &'static dyn oer_esp32s31_ieee80211_runtime::diagnostics::rx_pipeline::RxPipelineObserver,
    >,
    /// Low-frequency typed BlockAck agreement observer used by correctness
    /// images without attaching the per-frame pipeline profiler.
    pub rx_reorder: Option<
        &'static dyn oer_esp32s31_ieee80211_runtime::diagnostics::rx_pipeline::RxReorderAgreementObserver,
    >,
    pub aggregate_tx: &'static dyn oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::AggregateTxObserver,
    pub connected_rx: &'static dyn ConnectedRxObserver,
    pub rx_delivery: Option<
        &'static dyn oer_esp32s31_ieee80211_runtime::diagnostics::network::RxNetworkDeliveryObserver,
    >,
    #[cfg(feature = "mac-irq-diagnostics")]
    pub mac_irq: fn(MacIrqObservation),
    pub station_attempt: fn(StationAttemptObservation),
    pub access_point: fn(AccessPointObservation),
}

#[cfg(all(feature = "diagnostics", target_arch = "riscv32"))]
pub use oer_esp32s31_ieee80211_runtime::roles::access_point::AccessPointAggregateFill;

/// Per-association A-MPDU fill of one AP epoch, indexed by association ID
/// minus one.
#[cfg(all(feature = "diagnostics", target_arch = "riscv32"))]
pub type AccessPointAggregateFills = [AccessPointAggregateFill; oer_ieee80211_ap::AP_MAX_CLIENTS];

/// Value-only terminal AP epoch evidence emitted after TX, RX and IRQ have
/// quiesced but before their typed owners return to role-neutral Wi-Fi.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub struct AccessPointObservation {
    pub channel: u8,
    pub bandwidth_mhz: u16,
    pub beacons_transmitted: u32,
    pub missed_beacon_intervals: u32,
    pub maximum_beacon_lateness_micros: u32,
    pub tx_interrupt_wakes: u32,
    pub tx_deadline_wakes: u32,
    pub maximum_tx_pending_micros: u32,
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
    pub maximum_associated_peers: u8,
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
    pub disassociations_published: u32,
    pub disassociations_acknowledged: u32,
    pub deauthentications_prepared: u32,
    pub deauthentications_published: u32,
    pub deauthentications_acknowledged: u32,
    pub tx_block_ack_requests_prepared: u32,
    pub tx_block_ack_responses_observed: u32,
    pub tx_block_ack_agreements_operational: u32,
    pub tx_block_ack_responses_rejected: u32,
    pub tx_block_ack_negotiation_timeouts: u32,
    /// Peer-originated RX ADDBA responses that reached terminal TX success.
    pub rx_block_ack_responses_transmitted: u32,
    /// Hardware MAC/baseband counter increments across the complete AP epoch.
    pub rx_hardware: DiagnosticRxStatistics,
    pub retained_rx_descriptors: u32,
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
    pub rx_ht_data_frames: u32,
    /// MPDUs whose copied HT-SIG metadata has the Aggregation bit set.
    /// This is not an aggregate-PPDU count and does not imply aggregate depth.
    pub rx_ht_mpdus_with_aggregation_bit: u32,
    pub rx_rssi_samples: u32,
    pub rx_rssi_sum_dbm: i32,
    pub rx_rssi_min_dbm: i8,
    pub rx_rssi_max_dbm: i8,
    pub rx_ht40_mcs_frames: [u32; 8],
    pub rx_ht40_long_gi_frames: u32,
    pub rx_ht40_short_gi_frames: u32,
    pub rx_ht40_mcs32_frames: u32,
    pub rx_ht_mcs32_width_mismatches: u32,
    pub tx_ht_aggregates: u32,
    pub tx_ht40_mcs7_aggregates: u32,
    /// Fill of the network A-MPDUs started to each association, indexed by
    /// association ID minus one.
    pub tx_aggregate_fill: AccessPointAggregateFills,
    pub data_frames_transmitted: u32,
    pub ht_duplicate_tx_requests: u32,
    pub ht_duplicate_tx_selection: oer_esp32s31_ieee80211_mac::tx::HtDuplicateTxSelection,
    pub data_tx_attempts: u32,
    pub data_tx_retried_frames: u32,
    pub data_tx_maximum_attempts: u8,
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
    /// Client BlockAckReq frames addressed to this AP.
    pub rx_block_ack_requests: u32,
    /// Retained MPDUs released by client BlockAckReq window moves.
    pub rx_reorder_bar_released_mpdus: u32,
    /// Sequence numbers those moves passed without an MPDU.
    pub rx_reorder_bar_missing_sequences: u32,
    pub protected_data_radio_rejected: u32,
    pub protected_data_protocol_rejected: u32,
    pub first_rx_protocol_rejection: Option<AccessPointRxRejection>,
    /// Data MPDUs whose Protected bit contradicted the requested AP mode.
    pub security_mode_mismatches: u32,
}

/// Value-only failed-attempt detail emitted to diagnostics firmware.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum StationAttemptObservation {
    AttemptFailed {
        attempt: u16,
        stage: oer_ieee80211_sta::station::StaLifecycleStage,
    },
    RetryExhausted {
        attempts: u16,
        stage: oer_ieee80211_sta::station::StaLifecycleStage,
    },
}

pub use oer_esp32s31_ieee80211_runtime::roles::access_point::{
    AccessPointRxRejection, AccessPointRxRejectionReason,
};
