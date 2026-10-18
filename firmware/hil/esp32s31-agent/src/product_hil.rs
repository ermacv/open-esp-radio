//! Product-level HIL composition.
//!
//! This module is deliberately an application of the public driver API. PAC,
//! DMA, ISR and station internals stay in `oer-esp32s31-ieee80211-system`.

use core::{
    num::NonZeroU16,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use embassy_executor::{SendSpawner, Spawner};

use embassy_futures::select::{Either, select};

use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};

use embassy_time::{Instant, Timer};

use esp_hal::{
    efuse::{self, InterfaceMacAddress},
    rng::Trng,
};

use network::{Iface, Resources as NetworkResources, Runner as NetworkRunner};
#[cfg(feature = "phy-tracking-hold")]
mod phy_tracking_hold;
#[cfg(feature = "station-exit-evidence")]
use oer::wifi::StaLifecycleStage;

use oer::ieee80211::mac::channel::Channel as MacChannel;
use oer::wifi::{
    AccessPointClientLimit, AccessPointRequest, AccessPointSecurity, MacRxEvidence,
    MonitorCapturePolicy, MonitorRequest, Pmk, Preference, StaReconnectPolicy,
    StationAccessPointRequest, StationListenInterval, StationPowerMode, StationRequest,
    StationScanChannels, StationScanPolicy, StationSecurity, WifiChannel,
    WifiChannelWidth as DriverWifiChannelWidth, WifiMacAddress, WifiMonitorConfig,
    WifiRoleStartFailure, WifiRoleStopFailure, WifiScanRequest as DriverWifiScanRequest, WifiSsid,
};
#[cfg(feature = "mac-irq-telemetry")]
use oer_esp32s31_ieee80211_system::MacIrqObservation;

#[cfg(feature = "driver-observation")]
use oer_esp32s31_ieee80211_system::DiagnosticSnapshot;
#[cfg(feature = "station-exit-evidence")]
use oer_esp32s31_ieee80211_system::{
    AccessPointObservation, DiagnosticObservers, StationAttemptObservation,
};
use oer_esp32s31_ieee80211_system::{
    AccessPointStatus, ConnectedDisconnectReason, Esp32s31MonitorBasebandFormat,
    Esp32s31MonitorPhyInfo, MonitorFrames, RadioConfig, ReceivedMonitorFrame, StationLinkState,
    StationStatus, SystemRunner, WifiControl, WifiDevice, WifiParts, WifiStarted,
};
#[cfg(feature = "connected-datapath-poll-telemetry")]
use oer_esp32s31_ieee80211_system::{ConnectedDatapathPollBatch, ConnectedDatapathPollObserver};

use oer_esp32s31_phy::PhyCalibrationPath;

#[cfg(all(
    feature = "station-exit-evidence",
    not(any(
        feature = "mac-irq-telemetry",
        feature = "rx-delivery-telemetry",
        feature = "core0-rx-cycle-telemetry"
    ))
))]
use oer_hil_esp32s31_telemetry::rx_pipeline::RxCorrectnessObserver;

use oer_hil_esp32s31_telemetry::{
    aggregate_tx::AggregateTxCounters, mac_irq::MacIrqClassificationCounters,
    rx_pipeline::RxPipelineCounters, task_poll::TaskPollSet,
};

use oer_tool_data_hil_protocol::{
    phy::StartupArtifactDisposition, wifi::NetworkCredentials, wifi::NetworkIpv4Configuration,
    wifi::StationDisconnectReason, wifi::StationEpochEvidence, wifi::StationFaultCause,
    wifi::StationFaultPhase, wifi::WIFI_MONITOR_FRAME_CHUNK_MAX_LEN, wifi::WifiAccessPointEvidence,
    wifi::WifiAccessPointSecurity as HilWifiAccessPointSecurity,
    wifi::WifiChannelWidth as HilWifiChannelWidth, wifi::WifiDataPlanePlacement,
    wifi::WifiMonitorCaptureRequest, wifi::WifiMonitorEvidence, wifi::WifiMonitorEvidenceSource,
    wifi::WifiMonitorFrameChunk, wifi::WifiMonitorObserved, wifi::WifiMonitorPhyEvidence,
    wifi::WifiMonitorPhyFormat, wifi::WifiNetworkInterface, wifi::WifiRadioRestartEvidence,
    wifi::WifiRadioRestartRf, wifi::WifiRole, wifi::WifiRoleFailureEvidence,
    wifi::WifiRoleFailureReason, wifi::WifiRoleOperation, wifi::WifiRoleTransitionEvidence,
    wifi::WifiScanEvidence, wifi::WifiStationAccessPointStopEvidence,
};
#[cfg(feature = "station-exit-evidence")]
use oer_tool_data_hil_protocol::{wifi::StationAttemptFailureReason, wifi::StationFailureStage};

use crate::console::{
    PreInitializationRequest, WifiControlRequest, complete_access_point_start,
    complete_access_point_stop, complete_initialization, complete_monitor_capture,
    complete_monitor_start, complete_monitor_stop, complete_station_access_point_stop,
    complete_station_epoch_cycle, complete_wifi_radio_restart, complete_wifi_role_failure,
    complete_wifi_role_transition, complete_wifi_scan, publish_event_reliably,
    publish_monitor_frame, publish_startup_artifact, publish_station_lifecycle,
    receive_wifi_control_request, runtime_log, set_wifi_role,
};

use oer_adapters_esp_hal_soc::L1CachePerformanceCounters;

use oer_tool_data_hil_protocol::wifi::StationLifecycleEvent;

use oer_ieee80211_runtime::await_stack_boundary;

use static_cell::{ConstStaticCell, StaticCell};

mod ap_scheduler;
mod ieee802154;
#[cfg(feature = "i-route-probe")]
pub(crate) use ieee802154::route_probe_interrupt;
mod network;
#[cfg(feature = "rx-ownership-telemetry")]
mod rx_ownership;
mod rx_qualification;
mod rx_rejection;
mod traffic;

#[cfg(not(feature = "station-exit-evidence"))]
use oer_tool_data_hil_protocol::wifi::WifiMacRxHardwareEvidence;
#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
use traffic::observe_open_radio_core0_task_polls;

use traffic::{
    configure_multi_flow_burst_datagrams, observe_open_radio_task_polls, start_connected_traffic,
    start_traffic_dispatcher,
};

const SCAN_DWELL_MS: u16 = 200;
const MAXIMUM_TX_POWER_QUARTER_DBM: i8 = 80;

pub(crate) use crate::limits::OPEN_RADIO_TCP_CHUNK_CAPACITY;

pub(crate) const OPEN_RADIO_TASK_POLL_TELEMETRY: bool =
    cfg!(feature = "connected-datapath-poll-telemetry");
pub(crate) const OPEN_RADIO_DRIVER_OBSERVATION: bool = cfg!(feature = "driver-observation");

struct AppNetworkStart {
    station_device: WifiDevice,
    access_point_device: WifiDevice,
    station_ipv4: NetworkIpv4Configuration,
    rx_checksum: oer_tool_data_hil_protocol::wifi::WifiRxChecksumPolicy,
    tx_udp_checksum: oer_tool_data_hil_protocol::wifi::WifiTxUdpChecksumPolicy,
    seed: u64,
    l1_cache: &'static L1CachePerformanceCounters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NetworkConfigCommand {
    Set(NetworkIpv4Configuration),
    Clear,
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) enum QualificationRequester {
    UdpRx,
    UdpTx,
    UdpTxBegin,
    Tcp,
    TcpBegin,
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct QualificationSample {
    pub rx_primary: Option<ObservedRxStatistics>,
    pub rx_interrupt_posts: u32,
    pub tx_vector: Option<ObservedTxVector>,
    pub aggregate_tx: Option<oer_hil_esp32s31_telemetry::aggregate_tx::AggregateTxCounterSnapshot>,
}

mod rx_statistics;

use rx_statistics::ObservedRxStatistics;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::product_hil) struct ObservedTxVector {
    pub bandwidth_mhz: u16,
    pub aggregate_rate_kbps: u32,
}

#[unsafe(link_section = ".critical.data.panic_stage")]
static DIAGNOSTIC_STAGE: AtomicU32 = AtomicU32::new(0);
static STATION_NETWORK_RESOURCES: StaticCell<NetworkResources> = StaticCell::new();
static ACCESS_POINT_NETWORK_RESOURCES: StaticCell<NetworkResources> = StaticCell::new();
static APP_NETWORK_START: Channel<CriticalSectionRawMutex, AppNetworkStart, 1> = Channel::new();
static PRIMARY_NETWORK_START: Channel<CriticalSectionRawMutex, AppNetworkStart, 1> = Channel::new();
static APP_NETWORK_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static STATION_NETWORK_CONFIG_REQUESTS: Channel<CriticalSectionRawMutex, NetworkConfigCommand, 1> =
    Channel::new();
static ACCESS_POINT_NETWORK_CONFIG_REQUESTS: Channel<
    CriticalSectionRawMutex,
    NetworkConfigCommand,
    1,
> = Channel::new();
static STATION_NETWORK_CONFIG_APPLIED: Channel<CriticalSectionRawMutex, (), 1> = Channel::new();
static ACCESS_POINT_NETWORK_CONFIG_APPLIED: Channel<CriticalSectionRawMutex, (), 1> =
    Channel::new();
#[cfg(feature = "driver-observation")]
static QUALIFICATION_REQUESTS: Channel<CriticalSectionRawMutex, QualificationRequester, 3> =
    Channel::new();
#[cfg(feature = "driver-observation")]
static UDP_RX_QUALIFICATION: Channel<CriticalSectionRawMutex, QualificationSample, 1> =
    Channel::new();
#[cfg(feature = "driver-observation")]
static UDP_TX_QUALIFICATION: Channel<CriticalSectionRawMutex, QualificationSample, 1> =
    Channel::new();
#[cfg(feature = "driver-observation")]
static TCP_QUALIFICATION: Channel<CriticalSectionRawMutex, QualificationSample, 1> = Channel::new();
#[cfg(feature = "station-exit-evidence")]
static CONNECTED_RX_OBSERVER: ConstStaticCell<rx_qualification::HilConnectedRxObserver> =
    ConstStaticCell::new(rx_qualification::HilConnectedRxObserver::new(4_323));
static PHY_CALIBRATION_ARTIFACT: ConstStaticCell<
    [u8; crate::phy_calibration_artifact::MAX_ENCODED_LEN],
> = ConstStaticCell::new([0; crate::phy_calibration_artifact::MAX_ENCODED_LEN]);
static STATION_LIFECYCLE: Channel<CriticalSectionRawMutex, StationLinkEdge, 16> = Channel::new();
static STATION_TX_BLOCK_ACK_OPERATIONAL_TIDS: embassy_sync::watch::Watch<
    CriticalSectionRawMutex,
    u32,
    2,
> = embassy_sync::watch::Watch::new_with(0);
static AP_TX_BLOCK_ACK_OPERATIONAL_PEERS: AtomicU32 = AtomicU32::new(0);
static AP_TX_BLOCK_ACK_SMALLEST_WINDOW: AtomicU32 = AtomicU32::new(0);
static AP_TX_BLOCK_ACK_LARGEST_WINDOW: AtomicU32 = AtomicU32::new(0);
static L1_CACHE_COUNTERS_ENABLED: AtomicBool = AtomicBool::new(false);
static AP_CHANNEL: AtomicU32 = AtomicU32::new(0);
static AP_BANDWIDTH_MHZ: AtomicU32 = AtomicU32::new(0);
static AP_BEACONS: AtomicU32 = AtomicU32::new(0);
static AP_MISSED_BEACON_INTERVALS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_BEACON_LATENESS_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_TX_INTERRUPT_WAKES: AtomicU32 = AtomicU32::new(0);
static AP_TX_DEADLINE_WAKES: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_TX_PENDING_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_NETWORK_TX_PENDING_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_ATTEMPTS_AT_MAXIMUM_PENDING: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_RX_SERVICE_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_RX_DMA_SERVICE_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_TOTAL_RX_DMA_SERVICE_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_RX_DMA_SERVICE_CALLS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_RX_PROTOCOL_SERVICE_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_RX_PROTECTED_DATA_SERVICE_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_TOTAL_RX_PROTECTED_DATA_SERVICE_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_RX_MANAGEMENT_SERVICE_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_RX_EAPOL_SERVICE_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_NETWORK_BACKPRESSURE_MICROS: AtomicU32 = AtomicU32::new(0);
static AP_AUTHENTICATIONS: AtomicU32 = AtomicU32::new(0);
static AP_ASSOCIATIONS: AtomicU32 = AtomicU32::new(0);
static AP_AUTHORIZATIONS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_ASSOCIATED_PEERS: AtomicU32 = AtomicU32::new(0);
static AP_MAXIMUM_AUTHORIZED_PEERS: AtomicU32 = AtomicU32::new(0);
static AP_REMOVALS: AtomicU32 = AtomicU32::new(0);
static AP_AUTHENTICATION_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static AP_WPA2_RESPONSE_WINDOWS: AtomicU32 = AtomicU32::new(0);
static AP_WPA2_PENDING_ON_STOP: AtomicU32 = AtomicU32::new(0);
static AP_WPA2_RETRANSMISSIONS: AtomicU32 = AtomicU32::new(0);
static AP_WPA2_HANDSHAKE_FAILURES: AtomicU32 = AtomicU32::new(0);
static AP_WPA2_HANDSHAKE_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static AP_INACTIVITY_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static AP_DISASSOCIATIONS_PREPARED: AtomicU32 = AtomicU32::new(0);
static AP_DISASSOCIATIONS_PUBLISHED: AtomicU32 = AtomicU32::new(0);
static AP_DISASSOCIATIONS_ACKNOWLEDGED: AtomicU32 = AtomicU32::new(0);
static AP_DEAUTHENTICATIONS_PREPARED: AtomicU32 = AtomicU32::new(0);
static AP_DEAUTHENTICATIONS_PUBLISHED: AtomicU32 = AtomicU32::new(0);
static AP_DEAUTHENTICATIONS_ACKNOWLEDGED: AtomicU32 = AtomicU32::new(0);
static AP_TX_BLOCK_ACK_REQUESTS_PREPARED: AtomicU32 = AtomicU32::new(0);
static AP_TX_BLOCK_ACK_RESPONSES_OBSERVED: AtomicU32 = AtomicU32::new(0);
static AP_TX_BLOCK_ACK_AGREEMENTS_OPERATIONAL: AtomicU32 = AtomicU32::new(0);
static AP_TX_BLOCK_ACK_RESPONSES_REJECTED: AtomicU32 = AtomicU32::new(0);
static AP_TX_BLOCK_ACK_NEGOTIATION_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static AP_RX_BLOCK_ACK_RESPONSES_TRANSMITTED: AtomicU32 = AtomicU32::new(0);
static AP_RX_COMPLETED_UNITS_BASELINE: AtomicU32 = AtomicU32::new(0);
static AP_RX_COMPLETED_DESCRIPTORS_BASELINE: AtomicU32 = AtomicU32::new(0);
static AP_RX_RECYCLED_DESCRIPTORS_BASELINE: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "station-exit-evidence")]
static AP_RX_HARDWARE: Signal<CriticalSectionRawMutex, ObservedRxStatistics> = Signal::new();
static AP_RETAINED_RX_DESCRIPTORS: AtomicU32 = AtomicU32::new(0);
static AP_RX_DISCARDED_UNITS_BASELINE: AtomicU32 = AtomicU32::new(0);
static AP_RX_OVERLOAD_DISCARDED_UNITS_BASELINE: AtomicU32 = AtomicU32::new(0);
static AP_RX_CRITICAL_RESERVE_ADMISSIONS_BASELINE: AtomicU32 = AtomicU32::new(0);
static AP_RX_CRITICAL_ADMISSION_BLOCKED_BASELINE: AtomicU32 = AtomicU32::new(0);
static AP_IGNORED_RX_FRAMES: AtomicU32 = AtomicU32::new(0);
static AP_RX_MIC_FAILURES: AtomicU32 = AtomicU32::new(0);
static AP_RX_QUARANTINED_FRAMES: AtomicU32 = AtomicU32::new(0);
static AP_RX_VIEW_REJECTED: AtomicU32 = AtomicU32::new(0);
static AP_CONTROL_FRAMES_STAGED: AtomicU32 = AtomicU32::new(0);
static AP_CONTROL_FRAMES_DROPPED_WHILE_BUSY: AtomicU32 = AtomicU32::new(0);
static AP_ETHERNET_FRAMES_STAGED: AtomicU32 = AtomicU32::new(0);
static AP_ETHERNET_ARP_REQUESTS_STAGED: AtomicU32 = AtomicU32::new(0);
static AP_ETHERNET_TCP_FRAMES_STAGED: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_FRAMES_OBSERVED: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_ARP_REQUESTS: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_ARP_REPLIES: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_REJECTED_NO_PEER: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_REJECTED_DESTINATION: AtomicU32 = AtomicU32::new(0);
static AP_NETWORK_TX_FRAMES_REJECTED: AtomicU32 = AtomicU32::new(0);
static AP_RX_HT_DATA_FRAMES: AtomicU32 = AtomicU32::new(0);
static AP_RX_HT_MPDUS_WITH_AGGREGATION_BIT: AtomicU32 = AtomicU32::new(0);
static AP_RX_RSSI_SAMPLES: AtomicU32 = AtomicU32::new(0);
static AP_RX_RSSI_SUM_DBM: AtomicU32 = AtomicU32::new(0);
static AP_RX_RSSI_MIN_DBM: AtomicU32 = AtomicU32::new(0);
static AP_RX_RSSI_MAX_DBM: AtomicU32 = AtomicU32::new(0);
static AP_RX_HT40_MCS_FRAMES: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
static AP_RX_HT40_LONG_GI_FRAMES: AtomicU32 = AtomicU32::new(0);
static AP_RX_HT40_SHORT_GI_FRAMES: AtomicU32 = AtomicU32::new(0);
static AP_TX_HT_AGGREGATES: AtomicU32 = AtomicU32::new(0);
static AP_TX_HT40_MCS7_AGGREGATES: AtomicU32 = AtomicU32::new(0);
static AP_DATA_FRAMES_TRANSMITTED: AtomicU32 = AtomicU32::new(0);
static AP_DATA_TX_ATTEMPTS: AtomicU32 = AtomicU32::new(0);
static AP_DATA_TX_RETRIED_FRAMES: AtomicU32 = AtomicU32::new(0);
static AP_DATA_TX_MAXIMUM_ATTEMPTS: AtomicU32 = AtomicU32::new(0);
static AP_DATA_TX_MINIMUM_FINAL_RATE_KBPS: AtomicU32 = AtomicU32::new(0);
static AP_DATA_TX_ACK_SNR_SAMPLES: AtomicU32 = AtomicU32::new(0);
static AP_DATA_TX_MINIMUM_ACK_SNR_DB: AtomicU32 = AtomicU32::new(0);
static AP_DATA_TX_MAXIMUM_ACK_SNR_DB: AtomicU32 = AtomicU32::new(0);
static AP_TX_ACK_TIMEOUT_RETRIES: AtomicU32 = AtomicU32::new(0);
static AP_TX_CTS_TIMEOUT_RETRIES: AtomicU32 = AtomicU32::new(0);
static AP_TX_COLLISION_RETRIES: AtomicU32 = AtomicU32::new(0);
static AP_TX_PROBE_ACK_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static AP_TX_FAILURES: AtomicU32 = AtomicU32::new(0);
static AP_PROTECTED_DATA_FRAMES: AtomicU32 = AtomicU32::new(0);
static AP_PROTECTED_DATA_UNAUTHORIZED: AtomicU32 = AtomicU32::new(0);
static AP_PROTECTED_DATA_FOREIGN: AtomicU32 = AtomicU32::new(0);
static AP_PROTECTED_DATA_DUPLICATES: AtomicU32 = AtomicU32::new(0);
static AP_RX_REORDER_BUFFERED_MPDUS: AtomicU32 = AtomicU32::new(0);
static AP_RX_REORDER_DISPATCHED_MPDUS: AtomicU32 = AtomicU32::new(0);
static AP_RX_REORDER_HARDWARE_WINDOW_RESETS: AtomicU32 = AtomicU32::new(0);
static AP_RX_REORDER_GAP_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static AP_RX_BLOCK_ACK_REQUESTS: AtomicU32 = AtomicU32::new(0);
static AP_RX_REORDER_BAR_RELEASED_MPDUS: AtomicU32 = AtomicU32::new(0);
static AP_RX_REORDER_BAR_MISSING_SEQUENCES: AtomicU32 = AtomicU32::new(0);
static AP_PROTECTED_DATA_RADIO_REJECTED: AtomicU32 = AtomicU32::new(0);
static AP_PROTECTED_DATA_PROTOCOL_REJECTED: AtomicU32 = AtomicU32::new(0);

#[cfg(feature = "station-exit-evidence")]
static AP_TX_AGGREGATE_FILL: embassy_sync::blocking_mutex::Mutex<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    core::cell::Cell<Option<oer_esp32s31_ieee80211_system::AccessPointAggregateFills>>,
> = embassy_sync::blocking_mutex::Mutex::new(core::cell::Cell::new(None));

/// Publish one fill record per association that received an A-MPDU in this
/// AP epoch, before the correlated stop completion.
async fn report_access_point_aggregate_fill(request_id: u32, generation: u32) {
    #[cfg(feature = "station-exit-evidence")]
    for fill in AP_TX_AGGREGATE_FILL
        .lock(core::cell::Cell::get)
        .into_iter()
        .flatten()
    {
        if fill.aggregates == 0 {
            continue;
        }
        crate::console::publish_event_reliably(
            0,
            request_id,
            oer_tool_data_hil_protocol::wifi::AccessPointAggregateFill(
                oer_tool_data_hil_protocol::wifi::WifiApAggregateFill {
                    generation,
                    association_id: fill.association_id,
                    aggregates: fill.aggregates,
                    subframes: fill.subframes,
                    maximum_subframes: fill.maximum_subframes,
                    histogram: fill.histogram,
                },
            ),
        )
        .await;
    }
    #[cfg(not(feature = "station-exit-evidence"))]
    let _ = (request_id, generation);
}

#[cfg(feature = "station-exit-evidence")]
fn observe_access_point(observation: AccessPointObservation) {
    AP_TX_AGGREGATE_FILL.lock(|fill| fill.set(Some(observation.tx_aggregate_fill)));
    AP_CHANNEL.store(u32::from(observation.channel), Ordering::Release);
    AP_BANDWIDTH_MHZ.store(u32::from(observation.bandwidth_mhz), Ordering::Release);
    AP_BEACONS.store(observation.beacons_transmitted, Ordering::Release);
    AP_MISSED_BEACON_INTERVALS.store(observation.missed_beacon_intervals, Ordering::Release);
    AP_MAXIMUM_BEACON_LATENESS_MICROS.store(
        observation.maximum_beacon_lateness_micros,
        Ordering::Release,
    );
    AP_TX_INTERRUPT_WAKES.store(observation.tx_interrupt_wakes, Ordering::Release);
    AP_TX_DEADLINE_WAKES.store(observation.tx_deadline_wakes, Ordering::Release);
    AP_MAXIMUM_TX_PENDING_MICROS.store(observation.maximum_tx_pending_micros, Ordering::Release);
    AP_MAXIMUM_NETWORK_TX_PENDING_MICROS.store(
        observation.maximum_network_tx_pending_micros,
        Ordering::Release,
    );
    AP_NETWORK_TX_ATTEMPTS_AT_MAXIMUM_PENDING.store(
        u32::from(observation.network_tx_attempts_at_maximum_pending),
        Ordering::Release,
    );
    AP_MAXIMUM_RX_SERVICE_MICROS.store(observation.maximum_rx_service_micros, Ordering::Release);
    AP_MAXIMUM_RX_DMA_SERVICE_MICROS
        .store(observation.maximum_rx_dma_service_micros, Ordering::Release);
    AP_TOTAL_RX_DMA_SERVICE_MICROS
        .store(observation.total_rx_dma_service_micros, Ordering::Release);
    AP_RX_DMA_SERVICE_CALLS.store(observation.rx_dma_service_calls, Ordering::Release);
    AP_MAXIMUM_RX_PROTOCOL_SERVICE_MICROS.store(
        observation.maximum_rx_protocol_service_micros,
        Ordering::Release,
    );
    AP_MAXIMUM_RX_PROTECTED_DATA_SERVICE_MICROS.store(
        observation.maximum_rx_protected_data_service_micros,
        Ordering::Release,
    );
    AP_TOTAL_RX_PROTECTED_DATA_SERVICE_MICROS.store(
        observation.total_rx_protected_data_service_micros,
        Ordering::Release,
    );
    AP_MAXIMUM_RX_MANAGEMENT_SERVICE_MICROS.store(
        observation.maximum_rx_management_service_micros,
        Ordering::Release,
    );
    AP_MAXIMUM_RX_EAPOL_SERVICE_MICROS.store(
        observation.maximum_rx_eapol_service_micros,
        Ordering::Release,
    );
    AP_MAXIMUM_NETWORK_BACKPRESSURE_MICROS.store(
        observation.maximum_network_backpressure_micros,
        Ordering::Release,
    );
    AP_AUTHENTICATIONS.store(observation.authentication_responses, Ordering::Release);
    AP_ASSOCIATIONS.store(observation.association_responses, Ordering::Release);
    AP_AUTHORIZATIONS.store(observation.authorized_peers, Ordering::Release);
    AP_MAXIMUM_ASSOCIATED_PEERS.store(
        u32::from(observation.maximum_associated_peers),
        Ordering::Release,
    );
    AP_MAXIMUM_AUTHORIZED_PEERS.store(
        u32::from(observation.maximum_authorized_peers),
        Ordering::Release,
    );
    AP_REMOVALS.store(observation.peer_removals, Ordering::Release);
    AP_AUTHENTICATION_TIMEOUTS.store(observation.authentication_timeouts, Ordering::Release);
    AP_WPA2_RESPONSE_WINDOWS.store(observation.wpa2_response_windows, Ordering::Release);
    AP_WPA2_PENDING_ON_STOP.store(observation.wpa2_pending_on_stop, Ordering::Release);
    AP_WPA2_RETRANSMISSIONS.store(observation.wpa2_retransmissions, Ordering::Release);
    AP_WPA2_HANDSHAKE_FAILURES.store(observation.wpa2_handshake_failures, Ordering::Release);
    AP_WPA2_HANDSHAKE_TIMEOUTS.store(observation.wpa2_handshake_timeouts, Ordering::Release);
    AP_INACTIVITY_TIMEOUTS.store(observation.inactivity_timeouts, Ordering::Release);
    AP_DISASSOCIATIONS_PREPARED.store(observation.disassociations_prepared, Ordering::Release);
    AP_DISASSOCIATIONS_PUBLISHED.store(observation.disassociations_published, Ordering::Release);
    AP_DISASSOCIATIONS_ACKNOWLEDGED
        .store(observation.disassociations_acknowledged, Ordering::Release);
    AP_DEAUTHENTICATIONS_PREPARED.store(observation.deauthentications_prepared, Ordering::Release);
    AP_DEAUTHENTICATIONS_PUBLISHED
        .store(observation.deauthentications_published, Ordering::Release);
    AP_DEAUTHENTICATIONS_ACKNOWLEDGED.store(
        observation.deauthentications_acknowledged,
        Ordering::Release,
    );
    AP_TX_BLOCK_ACK_REQUESTS_PREPARED.store(
        observation.tx_block_ack_requests_prepared,
        Ordering::Release,
    );
    AP_TX_BLOCK_ACK_RESPONSES_OBSERVED.store(
        observation.tx_block_ack_responses_observed,
        Ordering::Release,
    );
    AP_TX_BLOCK_ACK_AGREEMENTS_OPERATIONAL.store(
        observation.tx_block_ack_agreements_operational,
        Ordering::Release,
    );
    AP_TX_BLOCK_ACK_RESPONSES_REJECTED.store(
        observation.tx_block_ack_responses_rejected,
        Ordering::Release,
    );
    AP_TX_BLOCK_ACK_NEGOTIATION_TIMEOUTS.store(
        observation.tx_block_ack_negotiation_timeouts,
        Ordering::Release,
    );
    AP_RX_BLOCK_ACK_RESPONSES_TRANSMITTED.store(
        observation.rx_block_ack_responses_transmitted,
        Ordering::Release,
    );
    AP_RX_HARDWARE.signal(observation.rx_hardware.into());
    AP_RETAINED_RX_DESCRIPTORS.store(observation.retained_rx_descriptors, Ordering::Release);
    AP_IGNORED_RX_FRAMES.store(observation.ignored_rx_frames, Ordering::Release);
    AP_RX_MIC_FAILURES.store(observation.rx_mic_failures, Ordering::Release);
    AP_RX_QUARANTINED_FRAMES.store(observation.rx_quarantined_frames, Ordering::Release);
    AP_RX_VIEW_REJECTED.store(observation.rx_view_rejected, Ordering::Release);
    AP_CONTROL_FRAMES_STAGED.store(observation.control_frames_staged, Ordering::Release);
    AP_CONTROL_FRAMES_DROPPED_WHILE_BUSY.store(
        observation.control_frames_dropped_while_busy,
        Ordering::Release,
    );
    AP_ETHERNET_FRAMES_STAGED.store(observation.ethernet_frames_staged, Ordering::Release);
    AP_ETHERNET_ARP_REQUESTS_STAGED
        .store(observation.ethernet_arp_requests_staged, Ordering::Release);
    AP_ETHERNET_TCP_FRAMES_STAGED.store(observation.ethernet_tcp_frames_staged, Ordering::Release);
    AP_NETWORK_TX_FRAMES_OBSERVED.store(observation.network_tx_frames_observed, Ordering::Release);
    AP_NETWORK_TX_ARP_REQUESTS.store(observation.network_tx_arp_requests, Ordering::Release);
    AP_NETWORK_TX_ARP_REPLIES.store(observation.network_tx_arp_replies, Ordering::Release);
    AP_NETWORK_TX_REJECTED_NO_PEER
        .store(observation.network_tx_rejected_no_peer, Ordering::Release);
    AP_NETWORK_TX_REJECTED_DESTINATION.store(
        observation.network_tx_rejected_destination,
        Ordering::Release,
    );
    AP_NETWORK_TX_FRAMES_REJECTED.store(observation.network_tx_frames_rejected, Ordering::Release);
    AP_RX_HT_DATA_FRAMES.store(observation.rx_ht_data_frames, Ordering::Release);
    AP_RX_HT_MPDUS_WITH_AGGREGATION_BIT.store(
        observation.rx_ht_mpdus_with_aggregation_bit,
        Ordering::Release,
    );
    AP_RX_RSSI_SAMPLES.store(observation.rx_rssi_samples, Ordering::Release);
    AP_RX_RSSI_SUM_DBM.store(observation.rx_rssi_sum_dbm as u32, Ordering::Release);
    AP_RX_RSSI_MIN_DBM.store(observation.rx_rssi_min_dbm as u8 as u32, Ordering::Release);
    AP_RX_RSSI_MAX_DBM.store(observation.rx_rssi_max_dbm as u8 as u32, Ordering::Release);
    for (target, observed) in AP_RX_HT40_MCS_FRAMES
        .iter()
        .zip(observation.rx_ht40_mcs_frames)
    {
        target.store(observed, Ordering::Release);
    }
    AP_RX_HT40_LONG_GI_FRAMES.store(observation.rx_ht40_long_gi_frames, Ordering::Release);
    AP_RX_HT40_SHORT_GI_FRAMES.store(observation.rx_ht40_short_gi_frames, Ordering::Release);
    AP_TX_HT_AGGREGATES.store(observation.tx_ht_aggregates, Ordering::Release);
    AP_TX_HT40_MCS7_AGGREGATES.store(observation.tx_ht40_mcs7_aggregates, Ordering::Release);
    AP_DATA_FRAMES_TRANSMITTED.store(observation.data_frames_transmitted, Ordering::Release);
    AP_DATA_TX_ATTEMPTS.store(observation.data_tx_attempts, Ordering::Release);
    AP_DATA_TX_RETRIED_FRAMES.store(observation.data_tx_retried_frames, Ordering::Release);
    AP_DATA_TX_MAXIMUM_ATTEMPTS.store(
        u32::from(observation.data_tx_maximum_attempts),
        Ordering::Release,
    );
    AP_DATA_TX_MINIMUM_FINAL_RATE_KBPS.store(
        observation.data_tx_minimum_final_rate_kbps,
        Ordering::Release,
    );
    AP_DATA_TX_ACK_SNR_SAMPLES.store(observation.data_tx_ack_snr_samples, Ordering::Release);
    AP_DATA_TX_MINIMUM_ACK_SNR_DB.store(
        u32::from(observation.data_tx_minimum_ack_snr_db as u8),
        Ordering::Release,
    );
    AP_DATA_TX_MAXIMUM_ACK_SNR_DB.store(
        u32::from(observation.data_tx_maximum_ack_snr_db as u8),
        Ordering::Release,
    );
    AP_TX_ACK_TIMEOUT_RETRIES.store(observation.tx_ack_timeout_retries, Ordering::Release);
    AP_TX_CTS_TIMEOUT_RETRIES.store(observation.tx_cts_timeout_retries, Ordering::Release);
    AP_TX_COLLISION_RETRIES.store(observation.tx_collision_retries, Ordering::Release);
    AP_TX_PROBE_ACK_TIMEOUTS.store(
        u32::from(observation.tx_probe_ack_timeouts),
        Ordering::Release,
    );
    AP_TX_FAILURES.store(
        u32::from(observation.tx_hardware_failures)
            | (u32::from(observation.tx_hardware_timeouts) << 8)
            | (u32::from(observation.tx_collision_limits) << 16)
            | (u32::from(observation.tx_last_hardware_status) << 24),
        Ordering::Release,
    );
    AP_PROTECTED_DATA_FRAMES.store(observation.protected_data_frames, Ordering::Release);
    AP_PROTECTED_DATA_UNAUTHORIZED
        .store(observation.protected_data_unauthorized, Ordering::Release);
    AP_PROTECTED_DATA_FOREIGN.store(observation.protected_data_foreign, Ordering::Release);
    AP_PROTECTED_DATA_DUPLICATES.store(observation.protected_data_duplicates, Ordering::Release);
    AP_RX_REORDER_BUFFERED_MPDUS.store(observation.rx_reorder_buffered_mpdus, Ordering::Release);
    AP_RX_REORDER_DISPATCHED_MPDUS
        .store(observation.rx_reorder_dispatched_mpdus, Ordering::Release);
    AP_RX_REORDER_HARDWARE_WINDOW_RESETS.store(
        observation.rx_reorder_hardware_window_resets,
        Ordering::Release,
    );
    AP_RX_REORDER_GAP_TIMEOUTS.store(observation.rx_reorder_gap_timeouts, Ordering::Release);
    AP_RX_BLOCK_ACK_REQUESTS.store(observation.rx_block_ack_requests, Ordering::Release);
    AP_RX_REORDER_BAR_RELEASED_MPDUS
        .store(observation.rx_reorder_bar_released_mpdus, Ordering::Release);
    AP_RX_REORDER_BAR_MISSING_SEQUENCES.store(
        observation.rx_reorder_bar_missing_sequences,
        Ordering::Release,
    );
    AP_PROTECTED_DATA_RADIO_REJECTED
        .store(observation.protected_data_radio_rejected, Ordering::Release);
    rx_rejection::observe(observation.first_rx_protocol_rejection);
    AP_PROTECTED_DATA_PROTOCOL_REJECTED.store(
        observation.protected_data_protocol_rejected,
        Ordering::Release,
    );
}

fn reset_access_point_evidence() {
    #[cfg(feature = "station-exit-evidence")]
    {
        rx_rejection::reset();
        AGGREGATE_TX.tx_retention.reset();
        observe_access_point(AccessPointObservation::default());
        let rx = RX_PIPELINE.snapshot();
        AP_RX_COMPLETED_UNITS_BASELINE.store(rx.completed_units, Ordering::Release);
        AP_RX_COMPLETED_DESCRIPTORS_BASELINE.store(rx.completed_descriptors, Ordering::Release);
        AP_RX_RECYCLED_DESCRIPTORS_BASELINE.store(rx.recycled_descriptors, Ordering::Release);
        AP_RX_DISCARDED_UNITS_BASELINE.store(rx.discarded_units, Ordering::Release);
        AP_RX_OVERLOAD_DISCARDED_UNITS_BASELINE
            .store(rx.overload_discarded_units, Ordering::Release);
        AP_RX_CRITICAL_RESERVE_ADMISSIONS_BASELINE
            .store(rx.critical_reserve_admissions, Ordering::Release);
        AP_RX_CRITICAL_ADMISSION_BLOCKED_BASELINE
            .store(rx.critical_admission_blocked_services, Ordering::Release);
    }
}

fn access_point_evidence(
    generation: u32,
    requested_channel: u8,
    requested_bandwidth_mhz: u16,
) -> WifiAccessPointEvidence {
    let observed_channel = AP_CHANNEL.load(Ordering::Acquire) as u8;
    let observed_bandwidth_mhz = AP_BANDWIDTH_MHZ.load(Ordering::Acquire) as u16;
    let tx_failures = AP_TX_FAILURES.load(Ordering::Acquire);
    let rx = RX_PIPELINE.snapshot();
    #[cfg(feature = "station-exit-evidence")]
    let rx_hardware = AP_RX_HARDWARE.try_take().unwrap_or_default().into();
    #[cfg(not(feature = "station-exit-evidence"))]
    let rx_hardware = WifiMacRxHardwareEvidence::default();
    WifiAccessPointEvidence {
        tx_retention: cfg!(feature = "station-exit-evidence")
            .then(|| AGGREGATE_TX.tx_retention.snapshot()),
        generation,
        channel: if observed_channel == 0 {
            requested_channel
        } else {
            observed_channel
        },
        bandwidth_mhz: if observed_bandwidth_mhz == 0 {
            requested_bandwidth_mhz
        } else {
            observed_bandwidth_mhz
        },
        beacons_transmitted: AP_BEACONS.load(Ordering::Acquire),
        missed_beacon_intervals: AP_MISSED_BEACON_INTERVALS.load(Ordering::Acquire),
        maximum_beacon_lateness_micros: AP_MAXIMUM_BEACON_LATENESS_MICROS.load(Ordering::Acquire),
        tx_interrupt_wakes: AP_TX_INTERRUPT_WAKES.load(Ordering::Acquire),
        tx_deadline_wakes: AP_TX_DEADLINE_WAKES.load(Ordering::Acquire),
        maximum_tx_pending_micros: AP_MAXIMUM_TX_PENDING_MICROS.load(Ordering::Acquire),
        maximum_network_tx_pending_micros: AP_MAXIMUM_NETWORK_TX_PENDING_MICROS
            .load(Ordering::Acquire),
        network_tx_attempts_at_maximum_pending: AP_NETWORK_TX_ATTEMPTS_AT_MAXIMUM_PENDING
            .load(Ordering::Acquire) as u8,
        maximum_rx_service_micros: AP_MAXIMUM_RX_SERVICE_MICROS.load(Ordering::Acquire),
        maximum_rx_dma_service_micros: AP_MAXIMUM_RX_DMA_SERVICE_MICROS.load(Ordering::Acquire),
        total_rx_dma_service_micros: AP_TOTAL_RX_DMA_SERVICE_MICROS.load(Ordering::Acquire),
        rx_dma_service_calls: AP_RX_DMA_SERVICE_CALLS.load(Ordering::Acquire),
        maximum_rx_protocol_service_micros: AP_MAXIMUM_RX_PROTOCOL_SERVICE_MICROS
            .load(Ordering::Acquire),
        maximum_rx_protected_data_service_micros: AP_MAXIMUM_RX_PROTECTED_DATA_SERVICE_MICROS
            .load(Ordering::Acquire),
        total_rx_protected_data_service_micros: AP_TOTAL_RX_PROTECTED_DATA_SERVICE_MICROS
            .load(Ordering::Acquire),
        maximum_rx_management_service_micros: AP_MAXIMUM_RX_MANAGEMENT_SERVICE_MICROS
            .load(Ordering::Acquire),
        maximum_rx_eapol_service_micros: AP_MAXIMUM_RX_EAPOL_SERVICE_MICROS.load(Ordering::Acquire),
        maximum_network_backpressure_micros: AP_MAXIMUM_NETWORK_BACKPRESSURE_MICROS
            .load(Ordering::Acquire),
        authentication_responses: AP_AUTHENTICATIONS.load(Ordering::Acquire),
        association_responses: AP_ASSOCIATIONS.load(Ordering::Acquire),
        authorized_peers: AP_AUTHORIZATIONS.load(Ordering::Acquire),
        maximum_associated_peers: AP_MAXIMUM_ASSOCIATED_PEERS.load(Ordering::Acquire) as u8,
        maximum_authorized_peers: AP_MAXIMUM_AUTHORIZED_PEERS.load(Ordering::Acquire) as u8,
        peer_removals: AP_REMOVALS.load(Ordering::Acquire),
        authentication_timeouts: AP_AUTHENTICATION_TIMEOUTS.load(Ordering::Acquire),
        wpa2_response_windows: AP_WPA2_RESPONSE_WINDOWS.load(Ordering::Acquire),
        wpa2_pending_on_stop: AP_WPA2_PENDING_ON_STOP.load(Ordering::Acquire),
        wpa2_retransmissions: AP_WPA2_RETRANSMISSIONS.load(Ordering::Acquire),
        wpa2_handshake_failures: AP_WPA2_HANDSHAKE_FAILURES.load(Ordering::Acquire),
        wpa2_handshake_timeouts: AP_WPA2_HANDSHAKE_TIMEOUTS.load(Ordering::Acquire),
        inactivity_timeouts: AP_INACTIVITY_TIMEOUTS.load(Ordering::Acquire),
        disassociations_prepared: AP_DISASSOCIATIONS_PREPARED.load(Ordering::Acquire),
        disassociations_published: AP_DISASSOCIATIONS_PUBLISHED.load(Ordering::Acquire),
        disassociations_acknowledged: AP_DISASSOCIATIONS_ACKNOWLEDGED.load(Ordering::Acquire),
        deauthentications_prepared: AP_DEAUTHENTICATIONS_PREPARED.load(Ordering::Acquire),
        deauthentications_published: AP_DEAUTHENTICATIONS_PUBLISHED.load(Ordering::Acquire),
        deauthentications_acknowledged: AP_DEAUTHENTICATIONS_ACKNOWLEDGED.load(Ordering::Acquire),
        tx_block_ack_requests_prepared: AP_TX_BLOCK_ACK_REQUESTS_PREPARED.load(Ordering::Acquire),
        tx_block_ack_responses_observed: AP_TX_BLOCK_ACK_RESPONSES_OBSERVED.load(Ordering::Acquire),
        tx_block_ack_agreements_operational: AP_TX_BLOCK_ACK_AGREEMENTS_OPERATIONAL
            .load(Ordering::Acquire),
        tx_block_ack_responses_rejected: AP_TX_BLOCK_ACK_RESPONSES_REJECTED.load(Ordering::Acquire),
        tx_block_ack_negotiation_timeouts: AP_TX_BLOCK_ACK_NEGOTIATION_TIMEOUTS
            .load(Ordering::Acquire),
        rx_block_ack_responses_transmitted: AP_RX_BLOCK_ACK_RESPONSES_TRANSMITTED
            .load(Ordering::Acquire),
        completed_rx_units: rx
            .completed_units
            .wrapping_sub(AP_RX_COMPLETED_UNITS_BASELINE.load(Ordering::Acquire)),
        completed_rx_descriptors: rx
            .completed_descriptors
            .wrapping_sub(AP_RX_COMPLETED_DESCRIPTORS_BASELINE.load(Ordering::Acquire)),
        recycled_rx_descriptors: rx
            .recycled_descriptors
            .wrapping_sub(AP_RX_RECYCLED_DESCRIPTORS_BASELINE.load(Ordering::Acquire)),
        rx_hardware,
        retained_rx_descriptors: AP_RETAINED_RX_DESCRIPTORS.load(Ordering::Acquire),
        discarded_rx_units: rx
            .discarded_units
            .wrapping_sub(AP_RX_DISCARDED_UNITS_BASELINE.load(Ordering::Acquire)),
        rx_overload_discarded_units: rx
            .overload_discarded_units
            .wrapping_sub(AP_RX_OVERLOAD_DISCARDED_UNITS_BASELINE.load(Ordering::Acquire)),
        rx_critical_reserve_admissions: rx
            .critical_reserve_admissions
            .wrapping_sub(AP_RX_CRITICAL_RESERVE_ADMISSIONS_BASELINE.load(Ordering::Acquire)),
        rx_critical_admission_blocked: rx
            .critical_admission_blocked_services
            .wrapping_sub(AP_RX_CRITICAL_ADMISSION_BLOCKED_BASELINE.load(Ordering::Acquire)),
        ignored_rx_frames: AP_IGNORED_RX_FRAMES.load(Ordering::Acquire),
        rx_mic_failures: AP_RX_MIC_FAILURES.load(Ordering::Acquire),
        rx_quarantined_frames: AP_RX_QUARANTINED_FRAMES.load(Ordering::Acquire),
        rx_view_rejected: AP_RX_VIEW_REJECTED.load(Ordering::Acquire),
        control_frames_staged: AP_CONTROL_FRAMES_STAGED.load(Ordering::Acquire),
        control_frames_dropped_while_busy: AP_CONTROL_FRAMES_DROPPED_WHILE_BUSY
            .load(Ordering::Acquire),
        ethernet_frames_staged: AP_ETHERNET_FRAMES_STAGED.load(Ordering::Acquire),
        ethernet_arp_requests_staged: AP_ETHERNET_ARP_REQUESTS_STAGED.load(Ordering::Acquire),
        ethernet_tcp_frames_staged: AP_ETHERNET_TCP_FRAMES_STAGED.load(Ordering::Acquire),
        network_tx_frames_observed: AP_NETWORK_TX_FRAMES_OBSERVED.load(Ordering::Acquire),
        network_tx_arp_requests: AP_NETWORK_TX_ARP_REQUESTS.load(Ordering::Acquire),
        network_tx_arp_replies: AP_NETWORK_TX_ARP_REPLIES.load(Ordering::Acquire),
        network_tx_rejected_no_peer: AP_NETWORK_TX_REJECTED_NO_PEER.load(Ordering::Acquire),
        network_tx_rejected_destination: AP_NETWORK_TX_REJECTED_DESTINATION.load(Ordering::Acquire),
        network_tx_frames_rejected: AP_NETWORK_TX_FRAMES_REJECTED.load(Ordering::Acquire),
        rx_ht_data_frames: AP_RX_HT_DATA_FRAMES.load(Ordering::Acquire),
        rx_ht_mpdus_with_aggregation_bit: AP_RX_HT_MPDUS_WITH_AGGREGATION_BIT
            .load(Ordering::Acquire),
        rx_rssi_samples: AP_RX_RSSI_SAMPLES.load(Ordering::Acquire),
        rx_rssi_sum_dbm: AP_RX_RSSI_SUM_DBM.load(Ordering::Acquire) as i32,
        rx_rssi_min_dbm: AP_RX_RSSI_MIN_DBM.load(Ordering::Acquire) as u8 as i8,
        rx_rssi_max_dbm: AP_RX_RSSI_MAX_DBM.load(Ordering::Acquire) as u8 as i8,
        rx_ht40_mcs_frames: core::array::from_fn(|index| {
            AP_RX_HT40_MCS_FRAMES[index].load(Ordering::Acquire)
        }),
        rx_ht40_long_gi_frames: AP_RX_HT40_LONG_GI_FRAMES.load(Ordering::Acquire),
        rx_ht40_short_gi_frames: AP_RX_HT40_SHORT_GI_FRAMES.load(Ordering::Acquire),
        tx_ht_aggregates: AP_TX_HT_AGGREGATES.load(Ordering::Acquire),
        tx_ht40_mcs7_aggregates: AP_TX_HT40_MCS7_AGGREGATES.load(Ordering::Acquire),
        data_frames_transmitted: AP_DATA_FRAMES_TRANSMITTED.load(Ordering::Acquire),
        data_tx_attempts: AP_DATA_TX_ATTEMPTS.load(Ordering::Acquire),
        data_tx_retried_frames: AP_DATA_TX_RETRIED_FRAMES.load(Ordering::Acquire),
        data_tx_maximum_attempts: AP_DATA_TX_MAXIMUM_ATTEMPTS.load(Ordering::Acquire) as u8,
        data_tx_minimum_final_rate_kbps: AP_DATA_TX_MINIMUM_FINAL_RATE_KBPS.load(Ordering::Acquire),
        data_tx_ack_snr_samples: AP_DATA_TX_ACK_SNR_SAMPLES.load(Ordering::Acquire),
        data_tx_minimum_ack_snr_db: AP_DATA_TX_MINIMUM_ACK_SNR_DB.load(Ordering::Acquire) as u8
            as i8,
        data_tx_maximum_ack_snr_db: AP_DATA_TX_MAXIMUM_ACK_SNR_DB.load(Ordering::Acquire) as u8
            as i8,
        tx_ack_timeout_retries: AP_TX_ACK_TIMEOUT_RETRIES.load(Ordering::Acquire),
        tx_cts_timeout_retries: AP_TX_CTS_TIMEOUT_RETRIES.load(Ordering::Acquire),
        tx_collision_retries: AP_TX_COLLISION_RETRIES.load(Ordering::Acquire),
        tx_probe_ack_timeouts: AP_TX_PROBE_ACK_TIMEOUTS.load(Ordering::Acquire) as u8,
        tx_hardware_failures: tx_failures as u8,
        tx_hardware_timeouts: (tx_failures >> 8) as u8,
        tx_collision_limits: (tx_failures >> 16) as u8,
        tx_last_hardware_status: (tx_failures >> 24) as u8,
        protected_data_frames: AP_PROTECTED_DATA_FRAMES.load(Ordering::Acquire),
        protected_data_unauthorized: AP_PROTECTED_DATA_UNAUTHORIZED.load(Ordering::Acquire),
        protected_data_foreign: AP_PROTECTED_DATA_FOREIGN.load(Ordering::Acquire),
        protected_data_duplicates: AP_PROTECTED_DATA_DUPLICATES.load(Ordering::Acquire),
        rx_reorder_buffered_mpdus: AP_RX_REORDER_BUFFERED_MPDUS.load(Ordering::Acquire),
        rx_reorder_dispatched_mpdus: AP_RX_REORDER_DISPATCHED_MPDUS.load(Ordering::Acquire),
        rx_reorder_hardware_window_resets: AP_RX_REORDER_HARDWARE_WINDOW_RESETS
            .load(Ordering::Acquire),
        rx_reorder_gap_timeouts: AP_RX_REORDER_GAP_TIMEOUTS.load(Ordering::Acquire),
        rx_block_ack_requests: AP_RX_BLOCK_ACK_REQUESTS.load(Ordering::Acquire),
        rx_reorder_bar_released_mpdus: AP_RX_REORDER_BAR_RELEASED_MPDUS.load(Ordering::Acquire),
        rx_reorder_bar_missing_sequences: AP_RX_REORDER_BAR_MISSING_SEQUENCES
            .load(Ordering::Acquire),
        protected_data_radio_rejected: AP_PROTECTED_DATA_RADIO_REJECTED.load(Ordering::Acquire),
        first_rx_protocol_rejection: rx_rejection::snapshot(),
        protected_data_protocol_rejected: AP_PROTECTED_DATA_PROTOCOL_REJECTED
            .load(Ordering::Acquire),
    }
}

#[derive(Clone, Copy, Debug)]
enum StationLinkEdge {
    Connected,
    Disconnected(StationDisconnectReason),
    Faulted(oer::wifi::StaFaultReason),
    #[cfg(feature = "station-exit-evidence")]
    AttemptFailed {
        attempt: u16,
        stage: StaLifecycleStage,
    },
    #[cfg(feature = "station-exit-evidence")]
    RetryExhausted {
        attempts: u16,
        stage: StaLifecycleStage,
    },
}

/// Report the MAC's receive counters at a connected epoch's exit.
fn log_connected_exit_evidence() {
    if let Some(rx) = oer_esp32s31_ieee80211_system::exit_evidence::take_connected_exit_rx() {
        runtime_log(format_args!(
            "OEXR mpdu={} data={} other_unicast={} fcs={} abort={} buffer_full={} fifo_overflow={}",
            rx.mpdu,
            rx.data_success,
            rx.other_unicast,
            rx.fcs_error,
            rx.abort,
            rx.buffer_full,
            rx.fifo_overflow,
        ));
    }
}

#[cfg(feature = "station-exit-evidence")]
fn observe_station_attempt(observation: StationAttemptObservation) {
    log_station_rx_frontier(observation);
    let edge = match observation {
        StationAttemptObservation::AttemptFailed { attempt, stage } => {
            StationLinkEdge::AttemptFailed { attempt, stage }
        }
        StationAttemptObservation::RetryExhausted { attempts, stage } => {
            StationLinkEdge::RetryExhausted { attempts, stage }
        }
    };
    STATION_LIFECYCLE
        .try_send(edge)
        .expect("qualification station lifecycle queue must not overflow");
}

#[cfg(feature = "station-exit-evidence")]
fn log_station_rx_frontier(observation: StationAttemptObservation) {
    let pipeline = RX_PIPELINE.snapshot();
    let irq = MAC_IRQ.snapshot();
    runtime_log(format_args!(
        "ORLC event={observation:?} irq_epochs={} irq_rx_only={} irq_rx_mixed={} \
         service_calls={} frontier={} admitted={} protocol_frames={} data_frames={} \
         network_enqueued={} network_dropped={} buffer_full={}",
        pipeline.rx_irq_epochs,
        irq.rx_only_entries,
        irq.rx_mixed_entries,
        pipeline.service_calls,
        pipeline.completion_frontier_frames,
        pipeline.admitted_frames,
        pipeline.protocol_frames,
        pipeline.protocol_data_frames,
        pipeline.network_enqueued,
        pipeline.network_dropped,
        pipeline.dma_buffer_full_increments,
    ));
}

// Driver observers execute on the RX/TX hot paths. Their atomics stay
// in internal SRAM so measuring a production image does not introduce PSRAM
// cache traffic into the path being measured.
#[unsafe(link_section = ".critical.data.open_radio_rx_telemetry")]
pub(crate) static RX_PIPELINE: RxPipelineCounters = RxPipelineCounters::new(now_micros);
#[cfg(all(
    feature = "station-exit-evidence",
    not(any(
        feature = "mac-irq-telemetry",
        feature = "rx-delivery-telemetry",
        feature = "core0-rx-cycle-telemetry"
    ))
))]
#[unsafe(link_section = ".critical.data.open_radio_rx_telemetry")]
static RX_CORRECTNESS: RxCorrectnessObserver = RxCorrectnessObserver::new(&RX_PIPELINE);
// This observer contains the non-zero `now_micros` function pointer. It must
// be copied into SRAM as initialized data; placing it in a NOLOAD/BSS section
// zeroes that pointer and traps on the first TX interrupt observation.
#[unsafe(link_section = ".critical.data.open_radio_tx_telemetry")]
pub(crate) static AGGREGATE_TX: AggregateTxCounters = AggregateTxCounters::with_clock(now_micros);
#[unsafe(link_section = ".critical.data.open_radio_rx_telemetry")]
pub(crate) static MAC_IRQ: MacIrqClassificationCounters = MacIrqClassificationCounters::new();
#[unsafe(link_section = ".critical.data.open_radio_task_poll_telemetry")]
pub(crate) static TASK_POLLS: TaskPollSet = TaskPollSet::new();

#[cfg(feature = "connected-datapath-poll-telemetry")]
fn record_connected_datapath_poll_batch(batch: ConnectedDatapathPollBatch) {
    oer_hil_esp32s31_telemetry::task_poll::TaskPollCounters::record_batch(
        TASK_POLLS.radio(),
        oer_hil_esp32s31_telemetry::task_poll::TaskPollSnapshot {
            polls: batch.polls,
            poll_micros: batch.poll_micros,
            lifetime_max_micros: batch.maximum_poll_micros,
            over_100_micros: batch.over_100_micros,
            over_500_micros: batch.over_500_micros,
            over_1_000_micros: batch.over_1_000_micros,
            over_5_000_micros: batch.over_5_000_micros,
        },
    );
}
fn now_micros() -> u64 {
    Instant::now().as_micros()
}

fn protocol_observed<T>(evidence: MacRxEvidence<T>) -> Option<WifiMonitorObserved<T>> {
    match evidence {
        MacRxEvidence::HardwareObserved(value) => Some(WifiMonitorObserved {
            source: WifiMonitorEvidenceSource::Hardware,
            value,
        }),
        MacRxEvidence::ProtocolValidated(value) => Some(WifiMonitorObserved {
            source: WifiMonitorEvidenceSource::Protocol,
            value,
        }),
        MacRxEvidence::Unavailable => None,
    }
}

fn protocol_rate(
    evidence: MacRxEvidence<Esp32s31MonitorPhyInfo>,
) -> Option<WifiMonitorObserved<WifiMonitorPhyEvidence>> {
    protocol_observed(evidence).map(|observed| WifiMonitorObserved {
        source: observed.source,
        value: WifiMonitorPhyEvidence {
            format: match observed.value.baseband_format() {
                Esp32s31MonitorBasebandFormat::Dot11b => WifiMonitorPhyFormat::Dot11b,
                Esp32s31MonitorBasebandFormat::Ofdm => WifiMonitorPhyFormat::Ofdm,
                Esp32s31MonitorBasebandFormat::Ht => WifiMonitorPhyFormat::Ht,
                Esp32s31MonitorBasebandFormat::Vht => WifiMonitorPhyFormat::Vht,
                Esp32s31MonitorBasebandFormat::HeSu => WifiMonitorPhyFormat::HeSu,
                Esp32s31MonitorBasebandFormat::HeMu => WifiMonitorPhyFormat::HeMu,
                Esp32s31MonitorBasebandFormat::HeExtendedRangeSu => {
                    WifiMonitorPhyFormat::HeExtendedRangeSu
                }
                Esp32s31MonitorBasebandFormat::HeTriggerBased => {
                    WifiMonitorPhyFormat::HeTriggerBased
                }
                Esp32s31MonitorBasebandFormat::VhtMu => WifiMonitorPhyFormat::VhtMu,
                Esp32s31MonitorBasebandFormat::Unknown(raw) => WifiMonitorPhyFormat::Unknown(raw),
            },
            hardware_rate_code: observed.value.rate,
            he_siga1: observed.value.he_siga1,
            he_siga2: observed.value.he_siga2,
        },
    })
}

struct ExportedMonitorFrame {
    captured_bytes: u64,
    generation_mismatch: bool,
    channel_mismatch: bool,
    channel_unavailable: bool,
    last_observed_channel: Option<MacChannel>,
}

async fn export_monitor_frame(
    request_id: u32,
    generation: u32,
    frame_sequence: u32,
    requested_channel: MacChannel,
    frame: &ReceivedMonitorFrame,
) -> ExportedMonitorFrame {
    let channel = protocol_observed(frame.metadata().rx.channel);
    let (channel_mismatch, channel_unavailable, last_observed_channel) = match channel {
        Some(observed) => (
            observed.value != requested_channel,
            false,
            Some(observed.value),
        ),
        None => (false, true, None),
    };
    // The HIL protocol carries the primary channel number.
    let channel = channel.map(|observed| WifiMonitorObserved {
        source: observed.source,
        value: observed.value.number(),
    });
    let rssi_dbm = protocol_observed(frame.metadata().rx.rssi_dbm);
    let rate = protocol_rate(frame.metadata().rx.rate);
    let captured_length = u16::try_from(frame.captured_length())
        .expect("monitor frame fits the configured capture slot");
    let logical_length = u16::try_from(frame.metadata().logical_length.min(u16::MAX as usize))
        .expect("bounded logical length fits u16");
    let dequeued_at_micros = now_micros();
    for (index, bytes) in frame
        .bytes()
        .chunks(WIFI_MONITOR_FRAME_CHUNK_MAX_LEN)
        .enumerate()
    {
        let offset = u16::try_from(index * WIFI_MONITOR_FRAME_CHUNK_MAX_LEN)
            .expect("capture offset fits u16");
        let chunk = WifiMonitorFrameChunk::try_new(
            generation,
            frame_sequence,
            dequeued_at_micros,
            captured_length,
            logical_length,
            offset,
            channel,
            rssi_dbm,
            rate,
            bytes,
        )
        .expect("bounded monitor chunk fits the HIL protocol");
        publish_monitor_frame(request_id, chunk).await;
    }
    ExportedMonitorFrame {
        captured_bytes: u64::from(captured_length),
        generation_mismatch: frame.metadata().generation != generation,
        channel_mismatch,
        channel_unavailable,
        last_observed_channel,
    }
}

async fn run_finite_monitor_capture(
    idle: WifiControl,
    monitor_frames: &MonitorFrames,
    request_id: u32,
    request: WifiMonitorCaptureRequest,
) -> WifiControl {
    let requested_channel =
        WifiChannel::mhz20(request.channel).expect("console validates the monitor channel");
    let mut monitor_request =
        MonitorRequest::new(requested_channel, WifiMonitorConfig::normalized());
    if let Some(snapshot_length) = NonZeroU16::new(request.snapshot_length) {
        monitor_request =
            monitor_request.with_capture_policy(MonitorCapturePolicy::truncate_at(snapshot_length));
    }
    let owner = idle
        .start_monitor(monitor_request)
        .await
        .unwrap_or_else(|error| panic!("production finite monitor start failed: {error:?}"));
    let generation = owner.generation().value();
    let capture_started = Instant::now();
    set_wifi_role(WifiRole::Monitor);
    complete_monitor_start(
        request_id,
        WifiRoleTransitionEvidence {
            previous: WifiRole::Idle,
            current: WifiRole::Monitor,
            generation,
        },
    )
    .await;

    let deadline = Timer::after_millis(u64::from(request.duration_millis));
    let mut deadline = core::pin::pin!(deadline);
    let mut captured_frames = 0_u32;
    let mut captured_bytes = 0_u64;
    let mut generation_mismatches = 0_u32;
    let mut channel_mismatches = 0_u32;
    let mut channel_unavailable = 0_u32;
    let mut last_observed_channel = None;
    loop {
        match select(deadline.as_mut(), monitor_frames.receive()).await {
            Either::First(()) => break,
            Either::Second(frame) => {
                let observation = export_monitor_frame(
                    request_id,
                    generation,
                    captured_frames,
                    MacChannel::from(requested_channel),
                    &frame,
                )
                .await;
                captured_frames = captured_frames.saturating_add(1);
                captured_bytes = captured_bytes.saturating_add(observation.captured_bytes);
                generation_mismatches = generation_mismatches
                    .saturating_add(u32::from(observation.generation_mismatch));
                channel_mismatches =
                    channel_mismatches.saturating_add(u32::from(observation.channel_mismatch));
                channel_unavailable =
                    channel_unavailable.saturating_add(u32::from(observation.channel_unavailable));
                if let Some(observed) = observation.last_observed_channel {
                    last_observed_channel = Some(observed);
                }
            }
        }
    }
    let elapsed_micros = capture_started.elapsed().as_micros();
    let idle = owner
        .stop()
        .await
        .unwrap_or_else(|error| panic!("production finite monitor stop failed: {error:?}"));
    let statistics = monitor_frames.statistics();
    set_wifi_role(WifiRole::Idle);
    complete_monitor_capture(
        request_id,
        WifiMonitorEvidence {
            generation,
            elapsed_micros,
            channel: request.channel,
            captured_frames,
            captured_bytes,
            generation_mismatches,
            channel_mismatches,
            channel_unavailable,
            last_observed_channel: last_observed_channel.map_or(0, MacChannel::number),
            published_frames: statistics.published_frames,
            full_drops: statistics.full_drops,
            oversized_drops: statistics.oversized_drops,
            discarded_frames: statistics.discarded_frames,
            exported_frames: captured_frames,
        },
    )
    .await;
    idle
}

#[cfg(feature = "mac-irq-telemetry")]
fn observe_mac_irq(observation: MacIrqObservation) {
    match observation {
        MacIrqObservation::RxEpoch => RX_PIPELINE.record_rx_irq_epoch(),
        MacIrqObservation::TxEpoch => AGGREGATE_TX.record_tx_irq_epoch(now_micros),
        MacIrqObservation::Entry {
            had_status,
            posted_events,
            had_auxiliary_event,
            had_unhandled_event,
        } => MAC_IRQ.record(
            had_status,
            posted_events,
            had_auxiliary_event,
            had_unhandled_event,
        ),
    }
}

#[inline(always)]
pub fn diagnostic_snapshot() -> (u32, u32) {
    (DIAGNOSTIC_STAGE.load(Ordering::Acquire), 0)
}

/// ESP-IDF's periodic `phy_track_pll` timer for the shared radio.
/// The timer stops between ticks while a HIL experiment suspends it.
#[embassy_executor::task]
async fn phy_tracking_task(radio: &'static SharedRadio) {
    #[cfg(feature = "phy-tracking-hold")]
    let _server = crate::phy_tracking::HoldServer::register();
    loop {
        while crate::phy_tracking::suspended() {
            #[cfg(feature = "phy-tracking-hold")]
            {
                use embassy_futures::select::{Either, select};
                if let Either::Second(request) = select(
                    crate::phy_tracking::changed(),
                    crate::phy_tracking::hold_requested(),
                )
                .await
                {
                    let evidence = phy_tracking_hold::measure(radio, request).await;
                    crate::phy_tracking::hold_measured(evidence);
                }
            }
            #[cfg(not(feature = "phy-tracking-hold"))]
            crate::phy_tracking::changed().await;
        }
        if let Err(error) = radio
            .run_tracking_until(
                crate::phy_tracking::until_suspended(),
                crate::phy_tracking::record,
            )
            .await
        {
            panic!("shared PHY tracking failed: {error:?}");
        }
    }
}

#[embassy_executor::task]
#[allow(
    large_assignments,
    reason = "the unique production radio runner is moved once into its static Embassy task arena; the linked-image stack audit remains authoritative"
)]
async fn radio_runner_task(spawner: Spawner, runner: SystemRunner) {
    #[cfg(any(
        feature = "core0-rx-cycle-telemetry",
        feature = "core0-rx-coarse-telemetry"
    ))]
    observe_open_radio_core0_task_polls(runner.run(spawner), TASK_POLLS.radio()).await;
    #[cfg(not(any(
        feature = "core0-rx-cycle-telemetry",
        feature = "core0-rx-coarse-telemetry"
    )))]
    observe_open_radio_task_polls(
        runner.run(spawner),
        TASK_POLLS.radio(),
        OPEN_RADIO_TASK_POLL_TELEMETRY,
    )
    .await
}

#[cfg(feature = "driver-observation")]
#[embassy_executor::task]
async fn qualification_snapshot_task(snapshot: DiagnosticSnapshot) {
    loop {
        let requester = QUALIFICATION_REQUESTS.receive().await;
        // Aggregate lifecycle observations execute on this same cooperative
        // radio executor. Sample here, between polls, rather than reading a
        // partially updated observation from the network core.
        if matches!(
            requester,
            QualificationRequester::UdpTxBegin | QualificationRequester::TcpBegin
        ) {
            AGGREGATE_TX.begin_interval();
        }
        let sample = QualificationSample {
            aggregate_tx: Some(AGGREGATE_TX.snapshot()),
            rx_primary: snapshot.rx_statistics().map(ObservedRxStatistics::from),
            rx_interrupt_posts: snapshot.rx_interrupt_posts(),
            tx_vector: snapshot.tx_vector().map(|vector| ObservedTxVector {
                bandwidth_mhz: vector.bandwidth_mhz,
                aggregate_rate_kbps: vector.aggregate_rate_kbps,
            }),
        };
        match requester {
            QualificationRequester::UdpRx => UDP_RX_QUALIFICATION.send(sample).await,
            QualificationRequester::UdpTx | QualificationRequester::UdpTxBegin => {
                UDP_TX_QUALIFICATION.send(sample).await
            }
            QualificationRequester::Tcp | QualificationRequester::TcpBegin => {
                TCP_QUALIFICATION.send(sample).await
            }
        }
    }
}

// The qualification snapshot task answers these requests; it exists only
// with driver observation.
#[cfg(feature = "driver-observation")]
pub(in crate::product_hil) async fn qualification_sample(
    requester: QualificationRequester,
) -> QualificationSample {
    QUALIFICATION_REQUESTS.send(requester).await;
    match requester {
        QualificationRequester::UdpRx => UDP_RX_QUALIFICATION.receive().await,
        QualificationRequester::UdpTx | QualificationRequester::UdpTxBegin => {
            UDP_TX_QUALIFICATION.receive().await
        }
        QualificationRequester::Tcp | QualificationRequester::TcpBegin => {
            TCP_QUALIFICATION.receive().await
        }
    }
}

#[cfg(not(feature = "driver-observation"))]
pub(in crate::product_hil) async fn qualification_sample(
    _requester: QualificationRequester,
) -> QualificationSample {
    QualificationSample {
        rx_primary: None,
        rx_interrupt_posts: 0,
        tx_vector: None,
        aggregate_tx: None,
    }
}

#[embassy_executor::task(pool_size = 2)]
async fn network_runner_task(mut runner: NetworkRunner<'static>, _role: WifiNetworkInterface) {
    let network = runner.run();
    #[cfg(feature = "task-poll-telemetry")]
    let network = network::progress::observe(network, network::observation::counters(_role));
    observe_open_radio_task_polls(
        network,
        TASK_POLLS.network(),
        OPEN_RADIO_TASK_POLL_TELEMETRY,
    )
    .await
}

#[embassy_executor::task(pool_size = 2)]
async fn network_config_task(iface: Iface<'static>, network_interface: WifiNetworkInterface) {
    let (requests, applied) = network_config_channels(network_interface);
    loop {
        match requests.receive().await {
            NetworkConfigCommand::Set(configuration) => {
                network::configure(iface, Some(configuration));
            }
            NetworkConfigCommand::Clear => network::configure(iface, None),
        }
        applied.send(()).await;
    }
}

/// CPU1-local network composition used by the production split topology.
#[embassy_executor::task]
pub(crate) async fn secondary_network_task(spawner: Spawner) {
    spawner.spawn(
        crate::hang_watchdog::network_heartbeat_task()
            .expect("network core heartbeat must allocate once"),
    );
    run_network_composition(spawner, APP_NETWORK_START.receive().await).await
}

/// CPU0-local network composition retained only for explicit placement diagnostics.
#[embassy_executor::task]
async fn primary_network_task(spawner: Spawner) {
    run_network_composition(spawner, PRIMARY_NETWORK_START.receive().await).await
}

async fn run_network_composition(spawner: Spawner, start: AppNetworkStart) -> ! {
    start_traffic_dispatcher(spawner);
    start_network_endpoint(
        spawner,
        start.station_device,
        STATION_NETWORK_RESOURCES.init(NetworkResources::new()),
        network::Settings {
            ipv4: Some(start.station_ipv4),
            seed: start.seed,
            rx_checksum: start.rx_checksum,
            tx_udp_checksum: start.tx_udp_checksum,
        },
        WifiNetworkInterface::Station,
        start.l1_cache,
    );
    start_network_endpoint(
        spawner,
        start.access_point_device,
        ACCESS_POINT_NETWORK_RESOURCES.init(NetworkResources::new()),
        network::Settings {
            ipv4: None,
            seed: start.seed ^ (1_u64 << 63),
            rx_checksum: start.rx_checksum,
            tx_udp_checksum: start.tx_udp_checksum,
        },
        WifiNetworkInterface::AccessPoint,
        start.l1_cache,
    );
    APP_NETWORK_READY.signal(());
    core::future::pending().await
}

fn start_network_endpoint(
    spawner: Spawner,
    device: WifiDevice,
    resources: &'static mut NetworkResources,
    settings: network::Settings,
    network_interface: WifiNetworkInterface,
    l1_cache: &'static L1CachePerformanceCounters,
) {
    let (iface, network_runner) = network::new(device, resources, settings, network_interface);
    spawner.spawn(
        network_runner_task(network_runner, network_interface)
            .expect("network runner task pool must fit both roles"),
    );
    spawner.spawn(
        network_config_task(iface, network_interface)
            .expect("network config task pool must fit both roles"),
    );
    spawner.spawn(
        network_services_start_task(spawner, iface, network_interface, l1_cache)
            .expect("network service gate task pool must fit both roles"),
    );
}

/// One configuration waiter per interface hands reporting to its permanent task
/// before starting the socket owners, avoiding competing state-waker registrations.
#[embassy_executor::task(pool_size = 2)]
async fn network_services_start_task(
    spawner: Spawner,
    iface: Iface<'static>,
    network_interface: WifiNetworkInterface,
    l1_cache: &'static L1CachePerformanceCounters,
) {
    iface.wait_config_v4_up().await;
    spawner.spawn(
        network_report_task(iface, network_interface)
            .expect("network report task pool must fit both roles"),
    );
    start_connected_traffic(spawner, iface.stack(), network_interface, l1_cache);
}

fn network_config_channels(
    network_interface: WifiNetworkInterface,
) -> (
    &'static Channel<CriticalSectionRawMutex, NetworkConfigCommand, 1>,
    &'static Channel<CriticalSectionRawMutex, (), 1>,
) {
    match network_interface {
        WifiNetworkInterface::Station => (
            &STATION_NETWORK_CONFIG_REQUESTS,
            &STATION_NETWORK_CONFIG_APPLIED,
        ),
        WifiNetworkInterface::AccessPoint => (
            &ACCESS_POINT_NETWORK_CONFIG_REQUESTS,
            &ACCESS_POINT_NETWORK_CONFIG_APPLIED,
        ),
    }
}

async fn apply_network_config(
    network_interface: WifiNetworkInterface,
    command: NetworkConfigCommand,
) {
    let (requests, applied) = network_config_channels(network_interface);
    requests.send(command).await;
    applied.receive().await;
}

#[embassy_executor::task(pool_size = 2)]
async fn network_report_task(iface: Iface<'static>, network_interface: WifiNetworkInterface) {
    report_network(iface, network_interface).await
}

#[embassy_executor::task]
async fn station_lifecycle_task(mut status: StationStatus) {
    let mut generation = 0_u32;
    let mut connected = false;
    loop {
        let (edge, negotiated) = match select(status.changed(), STATION_LIFECYCLE.receive()).await {
            Either::First(snapshot) => {
                STATION_TX_BLOCK_ACK_OPERATIONAL_TIDS
                    .sender()
                    .send(u32::from(snapshot.tx_block_ack_operational_tids));
                (station_status_edge(snapshot.state), Some(snapshot))
            }
            Either::Second(edge) => (Some(edge), None),
        };
        let Some(edge) = edge else {
            continue;
        };
        match edge {
            StationLinkEdge::Connected => {
                if !connected {
                    let link = negotiated
                        .filter(|snapshot| matches!(snapshot.state, StationLinkState::Connected));
                    let security =
                        link.and_then(|snapshot| snapshot.link_security)
                            .map(|security| {
                                match security {
                            oer_esp32s31_ieee80211_system::StationLinkSecurity::Open => {
                                oer_tool_data_hil_protocol::wifi::StationLinkSecurity::Open
                            }
                            oer_esp32s31_ieee80211_system::StationLinkSecurity::Wpa2Personal {
                                management_protection,
                            } => {
                                oer_tool_data_hil_protocol::wifi::StationLinkSecurity::Wpa2Personal {
                                    management_protection,
                                }
                            }
                            oer_esp32s31_ieee80211_system::StationLinkSecurity::Wpa3Personal => {
                                oer_tool_data_hil_protocol::wifi::StationLinkSecurity::Wpa3Personal
                            }
                        }
                            });
                    publish_station_lifecycle(StationLifecycleEvent::Connected {
                        generation,
                        association_bandwidth_mhz: link
                            .and_then(|snapshot| snapshot.association_bandwidth_mhz),
                        security,
                    })
                    .await;
                    connected = true;
                }
            }
            StationLinkEdge::Disconnected(reason) => {
                if connected {
                    log_connected_exit_evidence();
                    publish_station_lifecycle(StationLifecycleEvent::Disconnected {
                        generation,
                        reason,
                    })
                    .await;
                    generation = generation.wrapping_add(1);
                    connected = false;
                }
            }
            StationLinkEdge::Faulted(reason) => {
                log_connected_exit_evidence();
                publish_station_lifecycle(StationLifecycleEvent::Faulted {
                    generation,
                    phase: hil_fault_phase(reason.phase),
                    cause: hil_fault_cause(reason.cause),
                })
                .await;
                connected = false;
            }
            #[cfg(feature = "station-exit-evidence")]
            StationLinkEdge::AttemptFailed { attempt, stage } => {
                publish_station_lifecycle(StationLifecycleEvent::AttemptFailed {
                    generation,
                    attempt,
                    stage: hil_failure_stage(stage),
                    reason: hil_failure_reason(stage),
                })
                .await;
            }
            #[cfg(feature = "station-exit-evidence")]
            StationLinkEdge::RetryExhausted { attempts, stage } => {
                publish_station_lifecycle(StationLifecycleEvent::RetryExhausted {
                    generation,
                    attempts,
                    stage: hil_failure_stage(stage),
                    reason: hil_failure_reason(stage),
                })
                .await;
            }
        }
    }
}

#[embassy_executor::task]
async fn access_point_status_task(mut status: AccessPointStatus) {
    loop {
        let snapshot = status.changed().await;
        let mut peers = 0_u32;
        let mut smallest = u32::MAX;
        let mut largest = 0_u32;
        for agreement in snapshot
            .peers
            .iter()
            .flatten()
            .filter_map(|peer| peer.tx_block_ack)
        {
            let window = u32::from(agreement.window);
            peers = peers.saturating_add(1);
            smallest = smallest.min(window);
            largest = largest.max(window);
        }
        AP_TX_BLOCK_ACK_OPERATIONAL_PEERS.store(peers, Ordering::Release);
        AP_TX_BLOCK_ACK_SMALLEST_WINDOW
            .store(if peers == 0 { 0 } else { smallest }, Ordering::Release);
        AP_TX_BLOCK_ACK_LARGEST_WINDOW.store(largest, Ordering::Release);
    }
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
pub(crate) fn access_point_tx_block_ack_geometry() -> (u32, u32, u32) {
    (
        AP_TX_BLOCK_ACK_OPERATIONAL_PEERS.load(Ordering::Acquire),
        AP_TX_BLOCK_ACK_SMALLEST_WINDOW.load(Ordering::Acquire),
        AP_TX_BLOCK_ACK_LARGEST_WINDOW.load(Ordering::Acquire),
    )
}

fn station_status_edge(state: StationLinkState) -> Option<StationLinkEdge> {
    match state {
        StationLinkState::Connected => Some(StationLinkEdge::Connected),
        StationLinkState::Disconnected(Some(reason)) => {
            Some(StationLinkEdge::Disconnected(match reason {
                ConnectedDisconnectReason::BeaconLoss => StationDisconnectReason::BeaconLoss,
                ConnectedDisconnectReason::PeerDeauthentication { reason_code } => {
                    StationDisconnectReason::PeerDeauthentication { reason_code }
                }
                ConnectedDisconnectReason::PeerDisassociation { reason_code } => {
                    StationDisconnectReason::PeerDisassociation { reason_code }
                }
                ConnectedDisconnectReason::ActiveStateRestoreFailed => {
                    StationDisconnectReason::ActiveStateRestoreFailed
                }
                ConnectedDisconnectReason::GroupKeyHandshakeFailed => {
                    StationDisconnectReason::GroupKeyHandshakeFailed
                }
                ConnectedDisconnectReason::ControlMailboxOverflow => {
                    StationDisconnectReason::ControlMailboxOverflow
                }
                ConnectedDisconnectReason::SaQueryTimeout => {
                    StationDisconnectReason::SaQueryTimeout
                }
                ConnectedDisconnectReason::JoinTsfUnrepresentable => {
                    StationDisconnectReason::JoinTsfUnrepresentable
                }
            }))
        }
        StationLinkState::Disconnected(None) => None,
        StationLinkState::Faulted(reason) => Some(StationLinkEdge::Faulted(reason)),
    }
}

const fn hil_fault_phase(phase: oer::wifi::StaFaultPhase) -> StationFaultPhase {
    use oer::wifi::StaFaultPhase as DriverPhase;
    match phase {
        DriverPhase::Attempt => StationFaultPhase::Attempt,
        DriverPhase::Start => StationFaultPhase::Start,
        DriverPhase::Connected => StationFaultPhase::Connected,
        DriverPhase::Teardown => StationFaultPhase::Teardown,
    }
}

const fn hil_fault_cause(cause: oer::wifi::StaFaultCause) -> StationFaultCause {
    use oer::wifi::StaFaultCause as DriverCause;
    match cause {
        DriverCause::Hardware => StationFaultCause::Hardware,
        DriverCause::Security => StationFaultCause::Security,
        DriverCause::Configuration => StationFaultCause::Configuration,
        DriverCause::Resources => StationFaultCause::Resources,
        DriverCause::RfState => StationFaultCause::RfState,
        DriverCause::PortLost => StationFaultCause::PortLost,
    }
}

#[cfg(feature = "station-exit-evidence")]
const fn hil_failure_stage(stage: StaLifecycleStage) -> StationFailureStage {
    use oer::wifi::StaLifecycleStage as DriverStage;
    match stage {
        DriverStage::CandidateSelection => StationFailureStage::CandidateSelection,
        DriverStage::Authentication => StationFailureStage::Authentication,
        DriverStage::Association => StationFailureStage::Association,
        DriverStage::Security => StationFailureStage::Security,
        DriverStage::Connected => StationFailureStage::Connected,
        DriverStage::Hardware => StationFailureStage::Hardware,
    }
}

#[cfg(feature = "station-exit-evidence")]
const fn hil_failure_reason(stage: StaLifecycleStage) -> StationAttemptFailureReason {
    use oer::wifi::StaLifecycleStage as DriverStage;
    match stage {
        DriverStage::CandidateSelection => StationAttemptFailureReason::NoCandidate,
        DriverStage::Authentication | DriverStage::Association | DriverStage::Security => {
            StationAttemptFailureReason::PeerProtocol
        }
        DriverStage::Connected | DriverStage::Hardware => StationAttemptFailureReason::Hardware,
    }
}

/// The product power mode of a requested station power save; `None` for a
/// zero listen interval.
pub(crate) fn station_power_mode(
    power_save: oer_tool_data_hil_protocol::wifi::WifiStationPowerSave,
) -> Option<StationPowerMode> {
    use oer_tool_data_hil_protocol::wifi::WifiStationPowerSave;
    Some(match power_save {
        WifiStationPowerSave::None => StationPowerMode::None,
        WifiStationPowerSave::MinModem => StationPowerMode::MinModem,
        WifiStationPowerSave::MaxModem { listen_interval } => {
            StationPowerMode::MaxModem(StationListenInterval::new(listen_interval)?)
        }
    })
}

fn station_request(ssid: &[u8], passphrase: &[u8]) -> StationRequest {
    station_request_with_preference(ssid, passphrase, Preference::PreferHe20)
}

fn paired_station_request(ssid: &[u8], passphrase: &[u8]) -> StationRequest {
    station_request_with_preference(ssid, passphrase, Preference::Automatic)
}

fn station_request_with_preference(
    ssid: &[u8],
    passphrase: &[u8],
    association_preference: Preference,
) -> StationRequest {
    let ssid = WifiSsid::new(ssid).expect("validated HIL SSID must fit the driver request");
    let security = StationSecurity::wpa2_personal(passphrase, ssid.as_bytes())
        .expect("validated HIL personal credentials must derive a PMK");
    StationRequest::new(
        ssid,
        security,
        StaReconnectPolicy::new(3, 100, 1_000, 100).expect("fixed HIL reconnect policy is valid"),
        StationScanPolicy::new(
            StationScanChannels::CHANNELS_1_TO_13,
            NonZeroU16::new(SCAN_DWELL_MS).expect("scan dwell is nonzero"),
            association_preference,
        ),
    )
}

fn access_point_request(
    request: &oer_tool_data_hil_protocol::wifi::WifiAccessPointRequest,
) -> AccessPointRequest {
    let ssid = WifiSsid::new(request.credentials.ssid())
        .expect("validated HIL AP SSID must fit the driver request");
    let security = match request.security {
        HilWifiAccessPointSecurity::Open => AccessPointSecurity::open(),
        HilWifiAccessPointSecurity::Wpa2Personal => {
            let pmk = Pmk::derive(request.credentials.passphrase(), ssid.as_bytes())
                .expect("validated HIL AP credentials must derive a PMK");
            AccessPointSecurity::wpa2_personal(pmk)
        }
    };
    let width = match request.channel_width {
        HilWifiChannelWidth::Mhz20 => DriverWifiChannelWidth::Mhz20,
        HilWifiChannelWidth::Mhz40Above => DriverWifiChannelWidth::Mhz40Above,
        HilWifiChannelWidth::Mhz40Below => DriverWifiChannelWidth::Mhz40Below,
    };
    AccessPointRequest::new(
        ssid,
        security,
        WifiChannel::new_2_4_ghz(request.channel, width)
            .expect("console validates the AP channel geometry"),
        AccessPointClientLimit::new(request.client_limit)
            .expect("console validates the AP client limit"),
    )
    .expect("validated HIL AP request satisfies the production AP contract")
}

async fn report_network(iface: Iface<'static>, network_interface: WifiNetworkInterface) -> ! {
    loop {
        iface.wait_config_v4_up().await;
        let info = network::info(iface, network_interface)
            .expect("wait_config_v4_up proves an IPv4 configuration");
        publish_event_reliably(0, 0, oer_tool_data_hil_protocol::network::Ready(info)).await;
        runtime_log(format_args!(
            "OPEN_RADIO_HIL result=PASS stage=network-ready interface={network_interface:?} address={:?} gateway={:?}",
            info.address, info.gateway,
        ));
        iface.wait_config_v4_down().await;
    }
}

/// The platform owners of the shared radio and of its Wi-Fi client.
pub(crate) struct RadioPlatforms {
    pub(crate) radio: oer_esp32s31_ieee80211_system::EspHalRadioPlatform,
    pub(crate) wifi: oer_esp32s31_ieee80211_system::EspHalWifiPlatform,
    /// The Bluetooth client's random source, which owns the SoC entropy
    /// source the Wi-Fi client also reads.
    #[cfg(feature = "wifi-ble-coex")]
    pub(crate) bluetooth_entropy: &'static oer_esp32s31_bluetooth_system::BluetoothEntropy<'static>,
    /// The Bluetooth client's interrupt routes.
    #[cfg(feature = "wifi-ble-coex")]
    pub(crate) bluetooth_interrupts: oer_adapters_esp_hal_radio::EspHalBluetoothInterruptRoutes,
}

type SharedRadio = oer_esp32s31_ieee80211_system::SharedRadio;

pub async fn run(
    spawner: Spawner,
    _secondary_core_spawner: SendSpawner,
    platforms: RadioPlatforms,
    trng: Trng,
    l1_cache: &'static L1CachePerformanceCounters,
    watchdog: &'static oer_adapters_esp_hal_soc::watchdog::DeadlineWatchdog,
) {
    DIAGNOSTIC_STAGE.store(10, Ordering::Release);
    #[cfg(not(feature = "i-diagnostic"))]
    let PreInitializationRequest::Startup(startup) =
        crate::console::receive_pre_initialization_request().await;
    #[cfg(feature = "i-diagnostic")]
    let startup = match crate::console::receive_pre_initialization_request().await {
        PreInitializationRequest::Startup(configuration) => configuration,
        #[cfg(feature = "i-event-status-probe")]
        PreInitializationRequest::Ieee802154EventStatus(probe) => {
            let evidence = ieee802154::run_event_status_probe(platforms.radio, probe.request);
            publish_event_reliably(
                0,
                probe.request_id,
                oer_tool_data_hil_protocol::ieee802154::EventStatusProbed(evidence),
            )
            .await;
            return;
        }
        #[cfg(feature = "i-route-probe")]
        PreInitializationRequest::Ieee802154Route(probe) => {
            let evidence = ieee802154::run_route_probe(platforms.radio, probe.request);
            publish_event_reliably(
                0,
                probe.request_id,
                oer_tool_data_hil_protocol::ieee802154::RouteProbed(evidence),
            )
            .await;
            return;
        }
        #[cfg(feature = "i-ed-event-probe")]
        PreInitializationRequest::Ieee802154EdEvent(probe) => {
            let evidence = ieee802154::run_ed_event_probe(platforms.radio, probe.request);
            publish_event_reliably(
                0,
                probe.request_id,
                oer_tool_data_hil_protocol::ieee802154::EdEventProbed(evidence),
            )
            .await;
            return;
        }
        #[cfg(feature = "i-radio")]
        PreInitializationRequest::Ieee802154AirCheck(check) => {
            // The check holds the composition's bring-up future; pin it in place.
            let air_check = core::pin::pin!(ieee802154::run_air_check(
                spawner,
                platforms.radio,
                check.request
            ));
            let evidence = air_check.await;
            publish_event_reliably(
                0,
                check.request_id,
                oer_tool_data_hil_protocol::ieee802154::AirCheckCompleted(evidence),
            )
            .await;
            return;
        }
        #[cfg(feature = "i-radio")]
        PreInitializationRequest::Ieee802154Session(start) => {
            // The session holds the composition's bring-up future; pin it.
            let session = core::pin::pin!(ieee802154::run_session(
                spawner,
                platforms.radio,
                start.request_id,
                start.config
            ));
            session.await;
            return;
        }
        #[cfg(feature = "i-thread")]
        PreInitializationRequest::Ieee802154Thread(start) => {
            // The session holds the composition's bring-up future; pin it.
            let thread = core::pin::pin!(ieee802154::run_thread(
                spawner,
                platforms.radio,
                start.request_id,
                start.request
            ));
            thread.await;
            return;
        }
    };
    let crate::console::StartupConfiguration {
        ap_scheduler,
        request_id: initialization_request_id,
        ipv4: startup_ipv4,
        data_plane,
        rx_checksum,
        tx_udp_checksum,
        tx_buffer,
        rx_continuation,
        l1_cache_counters,
        phy_calibration_artifact,
    } = startup;
    L1_CACHE_COUNTERS_ENABLED.store(l1_cache_counters, Ordering::Relaxed);
    configure_multi_flow_burst_datagrams(
        if matches!(
            tx_buffer,
            oer_tool_data_hil_protocol::wifi::WifiTxBufferPolicy::OwnedSramPromotionBurstDiagnostic
        ) {
            32
        } else {
            1
        },
    );
    #[cfg(feature = "tx-psram-dma-probe")]
    oer_esp32s31_ieee80211_system::configure_direct_psram_tx_dma_probe(matches!(
        tx_buffer,
        oer_tool_data_hil_protocol::wifi::WifiTxBufferPolicy::PsramDirectDmaDiagnostic
    ));
    #[cfg(feature = "core0-rx-coarse-telemetry")]
    oer_esp32s31_ieee80211_system::configure_interrupt_driven_recycled_append_for_diagnostics(
        matches!(
            rx_continuation,
            oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::LevelIrqDiagnostic
        ),
    );
    #[cfg(feature = "core0-rx-coarse-telemetry")]
    oer_esp32s31_ieee80211_system::configure_adaptive_recycled_rx_probe_for_diagnostics(matches!(
        rx_continuation,
        oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::AdaptiveProbeDiagnostic
    ));
    #[cfg(feature = "core0-rx-coarse-telemetry")]
    oer_esp32s31_ieee80211_system::configure_recycled_rx_probe_delay_for_diagnostics(
        match rx_continuation {
            oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::DelayedProbe64Diagnostic => 64,
            oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::DelayedProbe128Diagnostic => 128,
            oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::DelayedProbe256Diagnostic => 256,
            oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::DelayedProbe512Diagnostic => 512,
            oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::DelayedProbe1024Diagnostic => 1024,
            oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::ImmediateSoftwareProbe
            | oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::LevelIrqDiagnostic
            | oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::AdaptiveProbeDiagnostic => 0,
        },
    );
    let mut station_address = [0; 6];
    station_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::Station).as_bytes());
    let mut access_point_address = [0; 6];
    access_point_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::AccessPoint).as_bytes());
    let station_mac = WifiMacAddress::new(station_address)
        .expect("ESP32-S31 station eFuse address must be unicast");
    let access_point_mac = WifiMacAddress::new(access_point_address)
        .expect("ESP32-S31 AP eFuse address must be unicast");
    #[cfg(feature = "station-exit-evidence")]
    let connected_rx_observer = CONNECTED_RX_OBSERVER.take();
    let config = RadioConfig::new(
        crate::watchdog::wifi(watchdog),
        station_mac,
        access_point_mac,
        WifiChannel::mhz20(1).expect("initial channel is valid"),
    )
    .with_maximum_tx_power_quarter_dbm(MAXIMUM_TX_POWER_QUARTER_DBM);
    #[cfg(feature = "rx-ownership-telemetry")]
    let config = config.with_rx_ownership_observer(&rx_ownership::RECORDER);
    let config = if let Some(model) = ap_scheduler::configuration(ap_scheduler) {
        config.with_access_point_airtime(model)
    } else {
        config
    };
    runtime_log(format_args!("OPEN_RADIO_HIL ap_scheduler={ap_scheduler:?}"));
    #[cfg(feature = "connected-datapath-poll-telemetry")]
    let config = config.with_connected_datapath_poll_observer(ConnectedDatapathPollObserver::new(
        320,
        record_connected_datapath_poll_batch,
    ));
    #[cfg(feature = "station-exit-evidence")]
    let config = config.with_diagnostic_observers(DiagnosticObservers {
        rx_pipeline: {
            #[cfg(any(
                feature = "mac-irq-telemetry",
                feature = "rx-delivery-telemetry",
                feature = "core0-rx-cycle-telemetry"
            ))]
            {
                Some(&RX_PIPELINE)
            }
            #[cfg(not(any(
                feature = "mac-irq-telemetry",
                feature = "rx-delivery-telemetry",
                feature = "core0-rx-cycle-telemetry"
            )))]
            {
                None
            }
        },
        rx_reorder: {
            #[cfg(any(
                feature = "mac-irq-telemetry",
                feature = "rx-delivery-telemetry",
                feature = "core0-rx-cycle-telemetry"
            ))]
            {
                None
            }
            #[cfg(not(any(
                feature = "mac-irq-telemetry",
                feature = "rx-delivery-telemetry",
                feature = "core0-rx-cycle-telemetry"
            )))]
            {
                Some(&RX_CORRECTNESS)
            }
        },
        aggregate_tx: &AGGREGATE_TX,
        connected_rx: connected_rx_observer,
        rx_delivery: Some(connected_rx_observer),
        #[cfg(feature = "mac-irq-telemetry")]
        mac_irq: observe_mac_irq,
        station_attempt: observe_station_attempt,
        access_point: observe_access_point,
    });
    let artifact_was_supplied = phy_calibration_artifact.is_some();
    let calibration_cache = phy_calibration_artifact
        .as_ref()
        .and_then(|artifact| crate::phy_calibration_artifact::decode(artifact.bytes()));
    let RadioPlatforms {
        radio: radio_platform,
        wifi: wifi_platform,
        #[cfg(feature = "wifi-ble-coex")]
        bluetooth_entropy,
        #[cfg(feature = "wifi-ble-coex")]
        bluetooth_interrupts,
    } = platforms;
    // The radio start consumes the platform.
    #[cfg(feature = "wifi-ble-coex")]
    let bluetooth_address = radio_platform.bluetooth_public_address();
    // The image runs its own tracking task, which HIL experiments suspend.
    let start = oer_esp32s31_radio_system::RadioStart::new()
        .with_tracking(oer_esp32s31_radio_system::Tracking::Caller);
    let start = match calibration_cache {
        Some(cache) => start.with_calibration_cache(cache),
        None => start,
    };
    let (radio, partitions) = oer_esp32s31_radio_system::start(spawner, radio_platform, start)
        .expect("the shared radio must start once");
    phy_register_image::adopt(radio);
    #[cfg(feature = "rx-clock-probe")]
    spawner.spawn(
        mac_clock_pairs_task(partitions.wifi.mac_local_time())
            .expect("the MAC clock pair task must allocate once"),
    );
    spawner.spawn(phy_tracking_task(radio).expect("PHY tracking task must allocate once"));
    spawner.spawn(
        crate::hang_watchdog::protocol_heartbeat_task()
            .expect("protocol heartbeat must allocate once"),
    );
    #[cfg(feature = "wifi-ble-coex")]
    crate::bluetooth::shared::start(
        spawner,
        radio,
        partitions.bluetooth,
        partitions.bluetooth_coex,
        bluetooth_interrupts,
        bluetooth_address,
        bluetooth_entropy,
    );

    let started_at = Instant::now();
    let WifiStarted {
        wifi,
        initialization,
        runner,
    } = await_stack_boundary!(oer_esp32s31_ieee80211_system::new(
        radio,
        partitions.wifi,
        partitions.wifi_coex,
        wifi_platform,
        trng,
        config
    ))
    .unwrap_or_else(|error| panic!("production Wi-Fi initialization failed: {error:?}"));
    // The runner is an idle supervisor until a role command arrives. Spawn it
    // before the asynchronous artifact publication so its large unique owner
    // graph is moved into the task arena instead of retained in this future.
    spawner.spawn(
        radio_runner_task(spawner, runner)
            .expect("production radio runner task must allocate once"),
    );
    // Report the production outcome after hardware initialization, never from
    // inside a timing-sensitive calibration transition.
    crate::console::runtime_log_reliably(format_args!(
        "hil-phy: initial_tracking={} wifi={} bluetooth_ieee802154={} inhibited={}",
        initialization.start.wifi.initial_tracking.is_some(),
        initialization
            .start
            .wifi
            .initial_tracking
            .is_some_and(|outcome| outcome.clients.wifi()),
        initialization
            .start
            .wifi
            .initial_tracking
            .is_some_and(|outcome| outcome.clients.bluetooth_ieee802154()),
        initialization
            .start
            .wifi
            .initial_tracking
            .is_some_and(|outcome| outcome.tracking_inhibited),
    ))
    .await;
    // Wi-Fi is this image's only radio client, so its bring-up registers the
    // shared PHY domain and carries the registration's fresh cache.
    let oer_esp32s31_ieee80211_system::RadioPhyPrepared::Registered(registration) =
        initialization.phy
    else {
        panic!("Wi-Fi must register the shared PHY as this image's first radio client");
    };
    let outcome = registration.outcome();
    #[cfg(feature = "driver-observation")]
    phy_diagnostics::log(outcome.rf_calibration).await;
    if let Some(cache) = registration.into_calibration_cache() {
        let disposition = match outcome.calibration_path {
            PhyCalibrationPath::FullAfterRejectedCache => StartupArtifactDisposition::Replaced,
            PhyCalibrationPath::PartialFromCache => StartupArtifactDisposition::Restored,
            PhyCalibrationPath::FullForCache if artifact_was_supplied => {
                // Transport-valid bytes can still carry an unknown HIL wire
                // schema. They are untrusted input, so a decode rejection
                // falls back to full calibration and replaces the artifact.
                StartupArtifactDisposition::Replaced
            }
            PhyCalibrationPath::FullForCache | PhyCalibrationPath::FullUncached => {
                StartupArtifactDisposition::Created
            }
        };
        crate::phy_calibration_projection::record(&cache);
        let encoded =
            crate::phy_calibration_artifact::encode(&cache, PHY_CALIBRATION_ARTIFACT.take())
                .expect("typed PHY calibration artifact exceeds its explicit HIL storage budget");
        publish_startup_artifact(disposition, started_at.elapsed().as_micros(), encoded).await;
    }
    let WifiParts {
        control,
        station_device,
        access_point_device,
        monitor_frames,
        station_status,
        access_point_status,
        #[cfg(feature = "driver-observation")]
        diagnostics,
        // Only the qualification snapshot consumes the driver observation.
        #[cfg(all(feature = "station-exit-evidence", not(feature = "driver-observation")))]
            diagnostics: _,
    } = wifi.into_parts();
    spawner.spawn(
        station_lifecycle_task(station_status).expect("station lifecycle task must allocate once"),
    );
    spawner.spawn(
        access_point_status_task(access_point_status)
            .expect("access-point status task must allocate once"),
    );
    #[cfg(feature = "driver-observation")]
    spawner.spawn(
        qualification_snapshot_task(diagnostics)
            .expect("driver observation snapshot task must allocate once"),
    );
    let seed = u64::from_le_bytes([
        station_address[0],
        station_address[1],
        station_address[2],
        station_address[3],
        station_address[4],
        station_address[5],
        0xa5,
        0x31,
    ]);
    let network_start = AppNetworkStart {
        station_device,
        access_point_device,
        rx_checksum,
        tx_udp_checksum,
        station_ipv4: startup_ipv4,
        seed,
        l1_cache,
    };
    match data_plane {
        WifiDataPlanePlacement::SingleCore => {
            spawner.spawn(
                primary_network_task(spawner)
                    .expect("primary-core network task must allocate once"),
            );
            PRIMARY_NETWORK_START.send(network_start).await;
        }
        WifiDataPlanePlacement::SplitRadioNetwork => {
            APP_NETWORK_START.send(network_start).await;
        }
    }
    spawner.spawn(
        wifi_role_task(control, monitor_frames, initialization_request_id)
            .expect("Wi-Fi role owner task must allocate once"),
    );
    APP_NETWORK_READY.wait().await;
    let data_plane_core = match data_plane {
        WifiDataPlanePlacement::SingleCore => 0,
        WifiDataPlanePlacement::SplitRadioNetwork => 1,
    };
    // The startup fields are runtime selectors only in the coarse same-image
    // diagnostic. Ordinary images compile the qualified production policy
    // directly, so reporting the ignored selector default would describe a
    // path which is not executing.
    #[cfg(feature = "core0-rx-coarse-telemetry")]
    let effective_rx_continuation = match rx_continuation {
        oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::ImmediateSoftwareProbe => {
            "immediate-software-probe"
        }
        oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::LevelIrqDiagnostic => "level-irq",
        oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::DelayedProbe64Diagnostic => {
            "delayed-probe-64us"
        }
        oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::DelayedProbe128Diagnostic => {
            "delayed-probe-128us"
        }
        oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::DelayedProbe256Diagnostic => {
            "delayed-probe-256us"
        }
        oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::DelayedProbe512Diagnostic => {
            "delayed-probe-512us"
        }
        oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::DelayedProbe1024Diagnostic => {
            "delayed-probe-1024us"
        }
        oer_tool_data_hil_protocol::wifi::WifiRxContinuationPolicy::AdaptiveProbeDiagnostic => {
            "adaptive-probe"
        }
    };
    #[cfg(not(feature = "core0-rx-coarse-telemetry"))]
    let effective_rx_continuation = "adaptive-probe";
    #[cfg(not(feature = "core0-rx-coarse-telemetry"))]
    let _ = rx_continuation;
    runtime_log(format_args!(
        "OPEN_RADIO_HIL data_plane={data_plane:?} radio_core=0 \
         protocol_core=0 network_core={data_plane_core} rx_checksum={rx_checksum:?} \
         tx_udp_checksum={tx_udp_checksum:?} tx_buffer={tx_buffer:?} \
         rx_continuation={effective_rx_continuation} \
         l1_cache_counters={l1_cache_counters}",
    ));
}

enum ProductWifiRole<P> {
    Idle(oer::wifi::WifiIdle<P>),
    Station(oer::wifi::WifiStation<P>),
    AccessPoint {
        owner: oer::wifi::WifiAccessPoint<P>,
        channel: u8,
        bandwidth_mhz: u16,
    },
    StationAccessPoint {
        owner: oer::wifi::WifiStationAccessPoint<P>,
        channel: u8,
        bandwidth_mhz: u16,
    },
    Monitor {
        owner: oer::wifi::WifiMonitor<P>,
        channel: MacChannel,
        started_at_micros: u64,
        captured_frames: u32,
        captured_bytes: u64,
        generation_mismatches: u32,
        channel_mismatches: u32,
        channel_unavailable: u32,
        last_observed_channel: Option<MacChannel>,
    },
}

async fn start_station_access_point_role<P: oer::wifi::WifiSupervisorPort>(
    idle: oer::wifi::WifiIdle<P>,
    request_id: u32,
    request: oer_tool_data_hil_protocol::wifi::WifiStationAccessPointRequest,
) -> ProductWifiRole<P>
where
    P::Error: core::fmt::Debug,
{
    reset_access_point_evidence();
    let station = paired_station_request(
        request.station_credentials.ssid(),
        request.station_credentials.passphrase(),
    );
    let access_point = access_point_request(&request.access_point);
    let access_point_ipv4 = request.access_point.ipv4;
    let channel = request.access_point.channel;
    let bandwidth_mhz = request.access_point.channel_width.bandwidth_mhz();
    match await_stack_boundary!(
        idle.start_station_access_point(StationAccessPointRequest::new(station, access_point,))
    ) {
        Ok(owner) => {
            apply_network_config(
                WifiNetworkInterface::AccessPoint,
                NetworkConfigCommand::Set(access_point_ipv4),
            )
            .await;
            STATION_LIFECYCLE.send(StationLinkEdge::Connected).await;
            DIAGNOSTIC_STAGE.store(30, Ordering::Release);
            set_wifi_role(WifiRole::StationAccessPoint);
            complete_wifi_role_transition(
                request_id,
                WifiRoleTransitionEvidence {
                    previous: WifiRole::Idle,
                    current: WifiRole::StationAccessPoint,
                    generation: owner.generation().value(),
                },
            )
            .await;
            ProductWifiRole::StationAccessPoint {
                owner,
                channel,
                bandwidth_mhz,
            }
        }
        Err(WifiRoleStartFailure::Rejected {
            wifi,
            request: _,
            error: _,
        }) => {
            complete_wifi_role_failure(
                request_id,
                WifiRoleFailureEvidence {
                    role: WifiRole::StationAccessPoint,
                    operation: WifiRoleOperation::Start,
                    reason: WifiRoleFailureReason::Rejected,
                },
            )
            .await;
            ProductWifiRole::Idle(wifi)
        }
        Err(WifiRoleStartFailure::Faulted { error }) => {
            runtime_log(format_args!(
                "OPEN_RADIO_HIL result=FAIL stage=sta-ap-start error={error:?}"
            ));
            complete_wifi_role_failure(
                request_id,
                WifiRoleFailureEvidence {
                    role: WifiRole::StationAccessPoint,
                    operation: WifiRoleOperation::Start,
                    reason: WifiRoleFailureReason::HardwareFault,
                },
            )
            .await;
            core::future::pending().await
        }
    }
}

async fn stop_station_access_point_role<P: oer::wifi::WifiSupervisorPort>(
    owner: oer::wifi::WifiStationAccessPoint<P>,
    channel: u8,
    bandwidth_mhz: u16,
) -> ProductWifiRole<P> {
    let request_id = match receive_wifi_control_request().await {
        WifiControlRequest::StopStationAccessPoint { request_id } => request_id,
        _ => unreachable!("console admits only paired stop while STA+AP owns Wi-Fi"),
    };
    let generation = owner.generation().value();
    match await_stack_boundary!(owner.stop()) {
        Ok(idle) => {
            apply_network_config(
                WifiNetworkInterface::AccessPoint,
                NetworkConfigCommand::Clear,
            )
            .await;
            STATION_LIFECYCLE
                .send(StationLinkEdge::Disconnected(
                    StationDisconnectReason::LinkPolicy,
                ))
                .await;
            set_wifi_role(WifiRole::Idle);
            report_access_point_aggregate_fill(request_id, generation).await;
            complete_station_access_point_stop(
                request_id,
                WifiStationAccessPointStopEvidence {
                    transition: WifiRoleTransitionEvidence {
                        previous: WifiRole::StationAccessPoint,
                        current: WifiRole::Idle,
                        generation,
                    },
                    access_point: access_point_evidence(generation, channel, bandwidth_mhz),
                },
            )
            .await;
            ProductWifiRole::Idle(idle)
        }
        Err(error) => {
            let reason = match error {
                WifiRoleStopFailure::GenerationMismatch => {
                    WifiRoleFailureReason::GenerationMismatch
                }
                WifiRoleStopFailure::Faulted(_) => WifiRoleFailureReason::HardwareFault,
            };
            complete_wifi_role_failure(
                request_id,
                WifiRoleFailureEvidence {
                    role: WifiRole::StationAccessPoint,
                    operation: WifiRoleOperation::Stop,
                    reason,
                },
            )
            .await;
            core::future::pending().await
        }
    }
}

#[embassy_executor::task]
async fn wifi_role_task(
    control: WifiControl,
    monitor_frames: MonitorFrames,
    initialization_request_id: u32,
) -> ! {
    DIAGNOSTIC_STAGE.store(20, Ordering::Release);
    set_wifi_role(WifiRole::Idle);
    complete_initialization(initialization_request_id).await;
    let mut credentials = None::<NetworkCredentials>;
    let mut role = ProductWifiRole::Idle(control);
    loop {
        role = match role {
            ProductWifiRole::StationAccessPoint {
                owner,
                channel,
                bandwidth_mhz,
            } => stop_station_access_point_role(owner, channel, bandwidth_mhz).await,
            ProductWifiRole::AccessPoint {
                owner,
                channel,
                bandwidth_mhz,
            } => {
                let request_id = match receive_wifi_control_request().await {
                    WifiControlRequest::StopAccessPoint { request_id } => request_id,
                    _ => unreachable!(
                        "console admits only AP stop while the access point owns Wi-Fi"
                    ),
                };
                let generation = owner.generation().value();
                match await_stack_boundary!(owner.stop()) {
                    Ok(idle) => {
                        #[cfg(feature = "tx-psram-dma-probe")]
                        {
                            let observation =
                                oer_esp32s31_ieee80211_system::direct_psram_tx_dma_probe_observation();
                            runtime_log(format_args!(
                                "OPEN_RADIO_HIL psram_tx_dma prepares={} first_address={:#010x} last_address={:#010x}",
                                observation.prepares,
                                observation.first_address,
                                observation.last_address,
                            ));
                        }
                        apply_network_config(
                            WifiNetworkInterface::AccessPoint,
                            NetworkConfigCommand::Clear,
                        )
                        .await;
                        set_wifi_role(WifiRole::Idle);
                        ap_scheduler::report(request_id).await;
                        report_access_point_aggregate_fill(request_id, generation).await;
                        complete_access_point_stop(
                            request_id,
                            access_point_evidence(generation, channel, bandwidth_mhz),
                        )
                        .await;
                        ProductWifiRole::Idle(idle)
                    }
                    Err(error) => {
                        let reason = match error {
                            WifiRoleStopFailure::GenerationMismatch => {
                                WifiRoleFailureReason::GenerationMismatch
                            }
                            WifiRoleStopFailure::Faulted(_) => WifiRoleFailureReason::HardwareFault,
                        };
                        complete_wifi_role_failure(
                            request_id,
                            WifiRoleFailureEvidence {
                                role: WifiRole::AccessPoint,
                                operation: WifiRoleOperation::Stop,
                                reason,
                            },
                        )
                        .await;
                        core::future::pending().await
                    }
                }
            }
            ProductWifiRole::Station(station) => match receive_wifi_control_request().await {
                WifiControlRequest::Cycle { request_id } => {
                    let idle = await_stack_boundary!(station.stop()).unwrap_or_else(|error| {
                        panic!("production station stop failed: {error:?}")
                    });
                    STATION_LIFECYCLE
                        .send(StationLinkEdge::Disconnected(
                            StationDisconnectReason::ReconnectRequested,
                        ))
                        .await;
                    let station = await_stack_boundary!(
                        idle.start_station(station_request(
                            credentials
                                .as_ref()
                                .expect("station role retains its credentials")
                                .ssid(),
                            credentials
                                .as_ref()
                                .expect("station role retains its credentials")
                                .passphrase(),
                        ))
                    )
                    .unwrap_or_else(|error| panic!("production station restart failed: {error:?}"));
                    complete_station_epoch_cycle(request_id, StationEpochEvidence::COMPLETE).await;
                    ProductWifiRole::Station(station)
                }
                WifiControlRequest::StopStation { request_id } => {
                    let stopped_generation = station.generation().value();
                    let idle = await_stack_boundary!(station.stop()).unwrap_or_else(|error| {
                        panic!("production station stop failed: {error:?}")
                    });
                    STATION_LIFECYCLE
                        .send(StationLinkEdge::Disconnected(
                            StationDisconnectReason::LinkPolicy,
                        ))
                        .await;
                    set_wifi_role(WifiRole::Idle);
                    complete_wifi_role_transition(
                        request_id,
                        WifiRoleTransitionEvidence {
                            previous: WifiRole::Station,
                            current: WifiRole::Idle,
                            generation: stopped_generation,
                        },
                    )
                    .await;
                    ProductWifiRole::Idle(idle)
                }
                _ => unreachable!("console admits only station commands while station owns Wi-Fi"),
            },
            ProductWifiRole::Idle(idle) => match receive_wifi_control_request().await {
                WifiControlRequest::RestartRadio { request_id } => {
                    match await_stack_boundary!(idle.restart_radio()) {
                        Ok((idle, report)) => {
                            let generation = report.generation().value();
                            let rf = match report.rf() {
                                oer::wifi::WifiRadioRestartRf::ClosedAndWoken => {
                                    WifiRadioRestartRf::ClosedAndWoken
                                }
                                oer::wifi::WifiRadioRestartRf::KeptOpen => {
                                    WifiRadioRestartRf::KeptOpen
                                }
                            };
                            complete_wifi_radio_restart(
                                request_id,
                                WifiRadioRestartEvidence { generation, rf },
                            )
                            .await;
                            runtime_log(format_args!(
                                "OPEN_RADIO_HIL radio restart generation={generation} rf={rf:?}",
                            ));
                            ProductWifiRole::Idle(idle)
                        }
                        Err(_) => {
                            complete_wifi_role_failure(
                                request_id,
                                WifiRoleFailureEvidence {
                                    role: WifiRole::Idle,
                                    operation: WifiRoleOperation::Restart,
                                    reason: WifiRoleFailureReason::HardwareFault,
                                },
                            )
                            .await;
                            core::future::pending().await
                        }
                    }
                }
                WifiControlRequest::StartStationAccessPoint {
                    request_id,
                    request,
                } => start_station_access_point_role(idle, request_id, request).await,
                WifiControlRequest::StartAccessPoint {
                    request_id,
                    request,
                } => {
                    reset_access_point_evidence();
                    ap_scheduler::reset();
                    DIAGNOSTIC_STAGE.store(40, Ordering::Release);
                    let channel = request.channel;
                    let bandwidth_mhz = request.channel_width.bandwidth_mhz();
                    runtime_log(format_args!(
                        "OPEN_RADIO_HIL AP start request entering supervisor"
                    ));
                    match await_stack_boundary!(
                        idle.start_access_point(access_point_request(&request))
                    ) {
                        Ok(owner) => {
                            runtime_log(format_args!(
                                "OPEN_RADIO_HIL AP supervisor start completed"
                            ));
                            apply_network_config(
                                WifiNetworkInterface::AccessPoint,
                                NetworkConfigCommand::Set(request.ipv4),
                            )
                            .await;
                            runtime_log(format_args!(
                                "OPEN_RADIO_HIL AP network configuration applied"
                            ));
                            set_wifi_role(WifiRole::AccessPoint);
                            complete_access_point_start(
                                request_id,
                                WifiRoleTransitionEvidence {
                                    previous: WifiRole::Idle,
                                    current: WifiRole::AccessPoint,
                                    generation: owner.generation().value(),
                                },
                            )
                            .await;
                            ProductWifiRole::AccessPoint {
                                owner,
                                channel,
                                bandwidth_mhz,
                            }
                        }
                        Err(WifiRoleStartFailure::Rejected {
                            wifi,
                            request: _,
                            error: _,
                        }) => {
                            complete_wifi_role_failure(
                                request_id,
                                WifiRoleFailureEvidence {
                                    role: WifiRole::AccessPoint,
                                    operation: WifiRoleOperation::Start,
                                    reason: WifiRoleFailureReason::Rejected,
                                },
                            )
                            .await;
                            ProductWifiRole::Idle(wifi)
                        }
                        Err(WifiRoleStartFailure::Faulted { error: _ }) => {
                            complete_wifi_role_failure(
                                request_id,
                                WifiRoleFailureEvidence {
                                    role: WifiRole::AccessPoint,
                                    operation: WifiRoleOperation::Start,
                                    reason: WifiRoleFailureReason::HardwareFault,
                                },
                            )
                            .await;
                            core::future::pending().await
                        }
                    }
                }
                WifiControlRequest::StartStation {
                    request_id,
                    credentials: requested_credentials,
                    power_save,
                } => {
                    let power_mode = station_power_mode(power_save)
                        .expect("the console admitted only a valid station power save");
                    let station = await_stack_boundary!(
                        idle.start_station(
                            station_request(
                                requested_credentials.ssid(),
                                requested_credentials.passphrase(),
                            )
                            .with_power_mode(power_mode)
                        )
                    )
                    .unwrap_or_else(|error| panic!("production station start failed: {error:?}"));
                    credentials = Some(requested_credentials);
                    DIAGNOSTIC_STAGE.store(30, Ordering::Release);
                    set_wifi_role(WifiRole::Station);
                    complete_wifi_role_transition(
                        request_id,
                        WifiRoleTransitionEvidence {
                            previous: WifiRole::Idle,
                            current: WifiRole::Station,
                            generation: station.generation().value(),
                        },
                    )
                    .await;
                    ProductWifiRole::Station(station)
                }
                WifiControlRequest::Scan {
                    request_id,
                    request,
                } => {
                    let mut channels = [0_u8; 13];
                    let mut channel_count = 0_usize;
                    for channel in 1_u8..=13 {
                        if request.channel_mask_2_4_ghz & (1_u16 << (channel - 1)) != 0 {
                            channels[channel_count] = channel;
                            channel_count += 1;
                        }
                    }
                    let scan_channels =
                        StationScanChannels::from_primary_channels(&channels[..channel_count])
                            .expect("console validates the scan channel mask");
                    let started_at = Instant::now();
                    let completed = idle
                        .scan(DriverWifiScanRequest::new(
                            scan_channels,
                            NonZeroU16::new(request.dwell_millis)
                                .expect("console validates nonzero scan dwell"),
                        ))
                        .await
                        .unwrap_or_else(|error| {
                            panic!("production standalone scan failed: {error:?}")
                        });
                    let (idle, report) = completed.into_parts();
                    let configured = credentials.as_ref().and_then(|credentials| {
                        report
                            .results()
                            .iter()
                            .find(|result| result.ssid() == credentials.ssid())
                    });
                    let evidence = WifiScanEvidence {
                        generation: report.generation().value(),
                        elapsed_micros: started_at.elapsed().as_micros(),
                        observed_frames: report.observed_frames,
                        unique_bss: report.results().len() as u8,
                        dropped_unique_bss: report.dropped_unique_bss,
                        configured_ssid_found: configured.is_some(),
                        configured_ssid_channel: configured.map_or(0, |result| result.channel),
                        configured_ssid_rssi_dbm: configured
                            .map_or(i8::MIN, |result| result.rssi_dbm),
                    };
                    set_wifi_role(WifiRole::Idle);
                    complete_wifi_scan(request_id, evidence).await;
                    ProductWifiRole::Idle(idle)
                }
                WifiControlRequest::StartMonitor {
                    request_id,
                    request,
                } => {
                    let requested_channel = WifiChannel::mhz20(request.channel)
                        .expect("console validates the monitor channel");
                    let mut monitor_request =
                        MonitorRequest::new(requested_channel, WifiMonitorConfig::normalized());
                    if let Some(snapshot_length) = NonZeroU16::new(request.snapshot_length) {
                        monitor_request = monitor_request.with_capture_policy(
                            MonitorCapturePolicy::truncate_at(snapshot_length),
                        );
                    }
                    let monitor =
                        idle.start_monitor(monitor_request)
                            .await
                            .unwrap_or_else(|error| {
                                panic!("production monitor start failed: {error:?}")
                            });
                    set_wifi_role(WifiRole::Monitor);
                    complete_monitor_start(
                        request_id,
                        WifiRoleTransitionEvidence {
                            previous: WifiRole::Idle,
                            current: WifiRole::Monitor,
                            generation: monitor.generation().value(),
                        },
                    )
                    .await;
                    ProductWifiRole::Monitor {
                        owner: monitor,
                        channel: MacChannel::from(requested_channel),
                        started_at_micros: Instant::now().as_micros(),
                        captured_frames: 0,
                        captured_bytes: 0,
                        generation_mismatches: 0,
                        channel_mismatches: 0,
                        channel_unavailable: 0,
                        last_observed_channel: None,
                    }
                }
                WifiControlRequest::CaptureMonitor {
                    request_id,
                    request,
                } => ProductWifiRole::Idle(
                    run_finite_monitor_capture(idle, &monitor_frames, request_id, request).await,
                ),
                _ => unreachable!("console admits only idle commands while Wi-Fi is idle"),
            },
            ProductWifiRole::Monitor {
                owner,
                channel,
                started_at_micros,
                mut captured_frames,
                mut captured_bytes,
                mut generation_mismatches,
                mut channel_mismatches,
                mut channel_unavailable,
                mut last_observed_channel,
            } => {
                let request_id = loop {
                    match select(receive_wifi_control_request(), monitor_frames.receive()).await {
                        Either::First(WifiControlRequest::StopMonitor { request_id }) => {
                            break request_id;
                        }
                        Either::First(_) => unreachable!(
                            "console admits only monitor stop while monitor owns Wi-Fi"
                        ),
                        Either::Second(frame) => {
                            captured_frames = captured_frames.saturating_add(1);
                            captured_bytes =
                                captured_bytes.saturating_add(frame.captured_length() as u64);
                            if frame.metadata().generation != owner.generation().value() {
                                generation_mismatches = generation_mismatches.saturating_add(1);
                            }
                            match frame.metadata().rx.channel {
                                MacRxEvidence::HardwareObserved(observed)
                                | MacRxEvidence::ProtocolValidated(observed) => {
                                    last_observed_channel = Some(observed);
                                    if observed != channel {
                                        channel_mismatches = channel_mismatches.saturating_add(1);
                                    }
                                }
                                MacRxEvidence::Unavailable => {
                                    channel_unavailable = channel_unavailable.saturating_add(1);
                                }
                            }
                        }
                    }
                };
                let generation = owner.generation().value();
                let idle = owner
                    .stop()
                    .await
                    .unwrap_or_else(|error| panic!("production monitor stop failed: {error:?}"));
                let statistics = monitor_frames.statistics();
                set_wifi_role(WifiRole::Idle);
                complete_monitor_stop(
                    request_id,
                    WifiMonitorEvidence {
                        generation,
                        elapsed_micros: Instant::now()
                            .as_micros()
                            .saturating_sub(started_at_micros),
                        channel: channel.number(),
                        captured_frames,
                        captured_bytes,
                        generation_mismatches,
                        channel_mismatches,
                        channel_unavailable,
                        last_observed_channel: last_observed_channel.map_or(0, MacChannel::number),
                        published_frames: statistics.published_frames,
                        full_drops: statistics.full_drops,
                        oversized_drops: statistics.oversized_drops,
                        discarded_frames: statistics.discarded_frames,
                        exported_frames: captured_frames,
                    },
                )
                .await;
                ProductWifiRole::Idle(idle)
            }
        };
    }
}

#[cfg(feature = "driver-observation")]
mod phy_diagnostics;
pub(crate) mod phy_register_image;

/// Every 100 ms, read the MAC local-time counter between two monotonic
/// readings; the gap between those bounds the pair's uncertainty. With the
/// frame timestamps the runtime logs, these pairs give the counter's unit,
/// its drift and each frame's age at handoff (`diagnostic-rx-clock`).
#[cfg(feature = "rx-clock-probe")]
#[embassy_executor::task]
async fn mac_clock_pairs_task(local_time: oer_esp32s31_radio_system::MacLocalTime) -> ! {
    loop {
        let before = embassy_time::Instant::now().as_micros();
        let mac = local_time.now();
        let after = embassy_time::Instant::now().as_micros();
        log::info!("rx_clock_pair mac={mac} mono_before_us={before} mono_after_us={after}");
        embassy_time::Timer::after_millis(100).await;
    }
}
