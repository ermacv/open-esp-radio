//! The product image's requests and the messages they publish.
//!
//! The console itself is the target core's, bound to this chip in
//! [`crate::transport`]; this module serves the product image's requests and
//! keeps its publishing helpers.

use crate::transport::CONSOLE;

use core::{
    cell::RefCell,
    fmt::Arguments,
    sync::atomic::{AtomicU32, Ordering},
};
use embassy_futures::select::{Either, select};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    channel::Channel,
};
use embassy_time::{Instant, Timer};
#[cfg(feature = "i-ed-event-probe")]
use oer_hil_protocol::ieee802154::Ieee802154EdEventProbeRequest;
#[cfg(feature = "i-event-status-probe")]
use oer_hil_protocol::ieee802154::Ieee802154EventStatusProbeRequest;
use oer_hil_protocol::phy::{ControlFault, UploadStartupArtifact};
#[cfg(not(feature = "memory-benchmark"))]
use oer_hil_protocol::phy::{
    ControlTracking, ReadAnalogImage, ReadCalibrationProjection, ReadRegisterImage,
};
#[cfg(not(feature = "memory-benchmark"))]
use oer_hil_protocol::system::HangTarget;
use oer_hil_protocol::system::{
    CallAcrossCores, GetInterruptStacks, GetStacks, InterruptStacks, ProbeTimebase, Stacks,
};
#[cfg(not(feature = "memory-benchmark"))]
use oer_hil_protocol::system::{HangInjected, InjectHang};
#[cfg(feature = "pc-profile")]
use oer_hil_protocol::telemetry::{ControlProfile, GetProfileSamples};
#[cfg(trace_journal)]
use oer_hil_protocol::telemetry::{
    ControlTrace, GetTraceEntries, GetTraceSnapshot, TraceEntriesPage, TraceSnapshot, TraceState,
};
use oer_hil_protocol::{
    Envelope, Message,
    base::{LinkHealth, RejectReason, Rejected},
    network::Direction,
    network::EvidenceRecord,
    network::FailureCode,
    network::Finished,
    network::FlowTransportEvidence,
    network::ResultSummary,
    network::RxDeliveryEvidence,
    network::SESSION_FLOW_CAPACITY,
    network::SessionConfig,
    network::SessionFailure,
    network::SessionState,
    network::SessionVerdict,
    network::StateChange,
    network::Transport,
    network::TransportEvidence,
    network::evidence_crc32c,
    phy::StartupArtifactChunk,
    phy::startup_artifact_crc32c,
    system::TimebaseProbeEvidence,
    system::TimebaseProbeRequest,
    wifi::NetworkCredentials,
    wifi::NetworkIpv4Configuration,
    wifi::WifiAccessPointRequest,
    wifi::WifiMonitorCaptureRequest,
    wifi::WifiMonitorRequest,
    wifi::WifiRole,
    wifi::WifiScanRequest,
    wifi::WifiStationAccessPointRequest,
};
#[cfg(feature = "i-radio")]
use oer_hil_protocol::{
    ieee802154::Ieee802154AirCheckRequest, ieee802154::Ieee802154SessionAssessRequest,
    ieee802154::Ieee802154SessionConfig, ieee802154::Ieee802154SessionPendingRequest,
    ieee802154::Ieee802154SessionTransmitRequest,
};
#[cfg(feature = "i-thread")]
use oer_hil_protocol::{
    ieee802154::Ieee802154ThreadSendRequest, ieee802154::Ieee802154ThreadStartRequest,
};

#[cfg(not(feature = "memory-benchmark"))]
mod radio;
#[cfg(not(feature = "memory-benchmark"))]
pub(crate) use radio::*;

pub(crate) const STARTUP_ARTIFACT_CAPACITY: usize = 512;
const COMMAND_QUEUE_CAPACITY: usize = 4;

oer_hil_agent::requests! {
    /// The product image's requests beyond the base module.
    pub(crate) enum Request (sessions = true) {
        #[cfg(not(feature = "memory-benchmark"))]
        ControlTracking(ControlTracking),
        #[cfg(feature = "phy-tracking-hold")]
        MeasureTrackingHold(oer_hil_protocol::phy::MeasureTrackingHold),
        #[cfg(not(feature = "memory-benchmark"))]
        ReadRegisterImage(ReadRegisterImage),
        #[cfg(not(feature = "memory-benchmark"))]
        ReadAnalogImage(ReadAnalogImage),
        #[cfg(not(feature = "memory-benchmark"))]
        ReadCalibrationProjection(ReadCalibrationProjection),
        ControlFault(ControlFault),
        UploadStartupArtifact(UploadStartupArtifact),
        #[cfg(feature = "pc-profile")]
        ControlProfile(ControlProfile),
        #[cfg(feature = "pc-profile")]
        GetProfileSamples(GetProfileSamples),
        #[cfg(trace_journal)]
        ControlTrace(ControlTrace),
        #[cfg(trace_journal)]
        GetTraceEntries(GetTraceEntries),
        #[cfg(trace_journal)]
        GetTraceSnapshot(GetTraceSnapshot),
        #[cfg(feature = "wifi-ble-coex")]
        GetGatt(oer_hil_protocol::bluetooth::GetGatt),
        #[cfg(not(feature = "memory-benchmark"))]
        InjectHang(InjectHang),
        GetStacks(GetStacks),
        GetInterruptStacks(GetInterruptStacks),
        ProbeTimebase(ProbeTimebase),
        CallAcrossCores(CallAcrossCores),
        #[cfg(feature = "interrupt-latency-probe")]
        MeasureInterruptLatency(oer_hil_protocol::system::MeasureInterruptLatency),
        #[cfg(not(feature = "memory-benchmark"))]
        ProbeSourceGate(oer_hil_protocol::system::ProbeSourceGate),
        #[cfg(feature = "memory-benchmark")]
        RunMemoryBenchmark(oer_hil_protocol::system::RunMemoryBenchmark),
        #[cfg(feature = "i-diagnostic")]
        ProbeEventStatus(oer_hil_protocol::ieee802154::ProbeEventStatus),
        #[cfg(feature = "i-diagnostic")]
        ProbeRoute(oer_hil_protocol::ieee802154::ProbeRoute),
        #[cfg(feature = "i-diagnostic")]
        ProbeEdEvent(oer_hil_protocol::ieee802154::ProbeEdEvent),
        #[cfg(feature = "i-diagnostic")]
        RunAirCheck(oer_hil_protocol::ieee802154::RunAirCheck),
        #[cfg(feature = "i-diagnostic")]
        StartSession(oer_hil_protocol::ieee802154::StartSession),
        #[cfg(feature = "i-diagnostic")]
        TransmitSession(oer_hil_protocol::ieee802154::TransmitSession),
        #[cfg(feature = "i-diagnostic")]
        ReceiveSession(oer_hil_protocol::ieee802154::ReceiveSession),
        #[cfg(feature = "i-diagnostic")]
        CollectSession(oer_hil_protocol::ieee802154::CollectSession),
        #[cfg(feature = "i-diagnostic")]
        SetSessionPending(oer_hil_protocol::ieee802154::SetSessionPending),
        #[cfg(feature = "i-diagnostic")]
        MaintainSessionPhy(oer_hil_protocol::ieee802154::MaintainSessionPhy),
        #[cfg(feature = "i-diagnostic")]
        AssessSessionChannel(oer_hil_protocol::ieee802154::AssessSessionChannel),
        #[cfg(feature = "i-diagnostic")]
        RestartSessionRadio(oer_hil_protocol::ieee802154::RestartSessionRadio),
        #[cfg(feature = "i-diagnostic")]
        ReadSessionRecentRssi(oer_hil_protocol::ieee802154::ReadSessionRecentRssi),
        #[cfg(feature = "i-diagnostic")]
        StopSession(oer_hil_protocol::ieee802154::StopSession),
        #[cfg(feature = "i-diagnostic")]
        StartThread(oer_hil_protocol::ieee802154::StartThread),
        #[cfg(feature = "i-diagnostic")]
        GetThread(oer_hil_protocol::ieee802154::GetThread),
        #[cfg(feature = "i-diagnostic")]
        SendThread(oer_hil_protocol::ieee802154::SendThread),
        #[cfg(feature = "i-diagnostic")]
        CollectThread(oer_hil_protocol::ieee802154::CollectThread),
        #[cfg(feature = "i-diagnostic")]
        StopThread(oer_hil_protocol::ieee802154::StopThread),
        Initialize(oer_hil_protocol::wifi::Initialize),
        Configure(oer_hil_protocol::network::Configure),
        Arm(oer_hil_protocol::network::Arm),
        Start(oer_hil_protocol::network::Start),
        GetStatus(oer_hil_protocol::network::GetStatus),
        Cancel(oer_hil_protocol::network::Cancel),
        ReplayResult(oer_hil_protocol::network::ReplayResult),
        AcknowledgeResult(oer_hil_protocol::network::AcknowledgeResult),
        Recover(oer_hil_protocol::network::Recover),
        CycleStationEpoch(oer_hil_protocol::wifi::CycleStationEpoch),
        StopStation(oer_hil_protocol::wifi::StopStation),
        StartStation(oer_hil_protocol::wifi::StartStation),
        RestartRadio(oer_hil_protocol::wifi::RestartRadio),
        Scan(oer_hil_protocol::wifi::Scan),
        StartMonitor(oer_hil_protocol::wifi::StartMonitor),
        StopMonitor(oer_hil_protocol::wifi::StopMonitor),
        StartAccessPoint(oer_hil_protocol::wifi::StartAccessPoint),
        StopAccessPoint(oer_hil_protocol::wifi::StopAccessPoint),
        StartStationAccessPoint(oer_hil_protocol::wifi::StartStationAccessPoint),
        StopStationAccessPoint(oer_hil_protocol::wifi::StopStationAccessPoint),
        CaptureMonitor(oer_hil_protocol::wifi::CaptureMonitor),
    }
}

#[unsafe(link_section = ".critical.data.logging")]
static WIFI_ROLE_STATE: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static COMMANDS: Channel<CriticalSectionRawMutex, Envelope<Request>, COMMAND_QUEUE_CAPACITY> =
    Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static STARTUP_CONFIGURATIONS: Channel<CriticalSectionRawMutex, StartupConfiguration, 1> =
    Channel::new();
#[cfg(feature = "i-event-status-probe")]
#[unsafe(link_section = ".critical.data.logging")]
static IEEE802154_EVENT_STATUS_PROBES: Channel<
    CriticalSectionRawMutex,
    Ieee802154EventStatusProbe,
    1,
> = Channel::new();
#[cfg(feature = "i-route-probe")]
#[unsafe(link_section = ".critical.data.logging")]
static IEEE802154_ROUTE_PROBES: Channel<CriticalSectionRawMutex, Ieee802154RouteProbe, 1> =
    Channel::new();
#[cfg(feature = "i-ed-event-probe")]
#[unsafe(link_section = ".critical.data.logging")]
static IEEE802154_ED_EVENT_PROBES: Channel<CriticalSectionRawMutex, Ieee802154EdEventProbe, 1> =
    Channel::new();
#[cfg(feature = "i-radio")]
#[unsafe(link_section = ".critical.data.logging")]
static IEEE802154_AIR_CHECKS: Channel<CriticalSectionRawMutex, Ieee802154AirCheck, 1> =
    Channel::new();
#[cfg(feature = "i-radio")]
#[unsafe(link_section = ".critical.data.logging")]
static IEEE802154_SESSION_STARTS: Channel<CriticalSectionRawMutex, Ieee802154SessionStart, 1> =
    Channel::new();
#[cfg(feature = "i-radio")]
#[unsafe(link_section = ".critical.data.logging")]
static IEEE802154_SESSION_COMMANDS: Channel<CriticalSectionRawMutex, Ieee802154SessionCommand, 1> =
    Channel::new();
#[cfg(feature = "i-thread")]
#[unsafe(link_section = ".critical.data.logging")]
static IEEE802154_THREAD_STARTS: Channel<CriticalSectionRawMutex, Ieee802154ThreadStart, 1> =
    Channel::new();
#[cfg(feature = "i-thread")]
#[unsafe(link_section = ".critical.data.logging")]
static IEEE802154_THREAD_COMMANDS: Channel<CriticalSectionRawMutex, Ieee802154ThreadCommand, 1> =
    Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static SESSION_STARTS: Channel<CriticalSectionRawMutex, ActiveSession, 1> = Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static SESSION_RESULTS: Channel<CriticalSectionRawMutex, SessionResult, 1> = Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static WIFI_CONTROL_REQUESTS: Channel<CriticalSectionRawMutex, WifiControlRequest, 1> =
    Channel::new();

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WifiControlRequest {
    Cycle {
        request_id: u32,
    },
    StopStation {
        request_id: u32,
    },
    StartStation {
        request_id: u32,
        credentials: NetworkCredentials,
        power_save: oer_hil_protocol::wifi::WifiStationPowerSave,
    },
    RestartRadio {
        request_id: u32,
    },
    Scan {
        request_id: u32,
        request: WifiScanRequest,
    },
    StartMonitor {
        request_id: u32,
        request: WifiMonitorRequest,
    },
    StopMonitor {
        request_id: u32,
    },
    CaptureMonitor {
        request_id: u32,
        request: WifiMonitorCaptureRequest,
    },
    StartAccessPoint {
        request_id: u32,
        request: WifiAccessPointRequest,
    },
    StopAccessPoint {
        request_id: u32,
    },
    StartStationAccessPoint {
        request_id: u32,
        request: WifiStationAccessPointRequest,
    },
    StopStationAccessPoint {
        request_id: u32,
    },
}

#[derive(Clone, Copy)]
pub struct ActiveSession {
    pub session_id: u64,
    pub config: SessionConfig,
}

#[cfg_attr(
    feature = "memory-benchmark",
    allow(
        dead_code,
        reason = "shared protocol admission retains the startup message shape; this image has no radio consumer"
    )
)]
pub struct StartupConfiguration {
    pub ap_scheduler: oer_hil_protocol::wifi::WifiApScheduler,
    pub request_id: u32,
    pub ipv4: NetworkIpv4Configuration,
    pub data_plane: oer_hil_protocol::wifi::WifiDataPlanePlacement,
    pub rx_checksum: oer_hil_protocol::wifi::WifiRxChecksumPolicy,
    pub tx_udp_checksum: oer_hil_protocol::wifi::WifiTxUdpChecksumPolicy,
    pub tx_buffer: oer_hil_protocol::wifi::WifiTxBufferPolicy,
    pub rx_continuation: oer_hil_protocol::wifi::WifiRxContinuationPolicy,
    pub l1_cache_counters: bool,
    pub phy_calibration_artifact: Option<StartupArtifact>,
}

#[cfg(feature = "i-event-status-probe")]
pub struct Ieee802154EventStatusProbe {
    pub request_id: u32,
    pub request: Ieee802154EventStatusProbeRequest,
}

#[cfg(feature = "i-route-probe")]
pub struct Ieee802154RouteProbe {
    pub request_id: u32,
    pub request: oer_hil_protocol::ieee802154::Ieee802154RouteProbeRequest,
}

#[cfg(feature = "i-ed-event-probe")]
pub struct Ieee802154EdEventProbe {
    pub request_id: u32,
    pub request: Ieee802154EdEventProbeRequest,
}

#[cfg(feature = "i-radio")]
pub struct Ieee802154AirCheck {
    pub request_id: u32,
    pub request: Ieee802154AirCheckRequest,
}

#[cfg(feature = "i-radio")]
pub struct Ieee802154SessionStart {
    pub request_id: u32,
    pub config: Ieee802154SessionConfig,
}

#[cfg(feature = "i-thread")]
pub struct Ieee802154ThreadStart {
    pub request_id: u32,
    pub request: Ieee802154ThreadStartRequest,
}

/// One command of a running Thread session.
#[cfg(feature = "i-thread")]
pub enum Ieee802154ThreadCommand {
    Query {
        request_id: u32,
    },
    Send {
        request_id: u32,
        request: Ieee802154ThreadSendRequest,
    },
    Collect {
        request_id: u32,
    },
    Stop {
        request_id: u32,
    },
}

/// The next command of the running Thread session.
#[cfg(feature = "i-thread")]
pub async fn receive_ieee802154_thread_command() -> Ieee802154ThreadCommand {
    IEEE802154_THREAD_COMMANDS.receive().await
}

/// Admit one command of a running Thread session, as
/// `admit_i_session_command` does for peer sessions.
#[cfg(feature = "i-thread")]
async fn admit_ieee802154_thread_command(
    thread_open: bool,
    session_id: u64,
    request_id: u32,
    command: Ieee802154ThreadCommand,
) -> bool {
    if !thread_open {
        publish_event_reliably(
            session_id,
            request_id,
            oer_hil_protocol::base::Rejected(RejectReason::InvalidState),
        )
        .await;
        return false;
    }
    if IEEE802154_THREAD_COMMANDS.try_send(command).is_err() {
        publish_event_reliably(
            session_id,
            request_id,
            oer_hil_protocol::base::Rejected(RejectReason::Busy),
        )
        .await;
        return false;
    }
    true
}

/// One command of a running IEEE 802.15.4 peer session.
#[cfg(feature = "i-radio")]
pub enum Ieee802154SessionCommand {
    Transmit {
        request_id: u32,
        request: Ieee802154SessionTransmitRequest,
    },
    Receive {
        request_id: u32,
    },
    Collect {
        request_id: u32,
    },
    Pending {
        request_id: u32,
        request: Ieee802154SessionPendingRequest,
    },
    Stop {
        request_id: u32,
    },
    MaintainPhy {
        request_id: u32,
    },
    Assess {
        request_id: u32,
        request: Ieee802154SessionAssessRequest,
    },
    Restart {
        request_id: u32,
    },
    RecentRssi {
        request_id: u32,
    },
}

/// The next command of the running IEEE 802.15.4 session.
#[cfg(feature = "i-radio")]
pub async fn receive_ieee802154_session_command() -> Ieee802154SessionCommand {
    IEEE802154_SESSION_COMMANDS.receive().await
}

/// Admit one command of a running session: the image must support sessions
/// and one must be open; a queued command makes the next one wait.
#[cfg(feature = "i-radio")]
async fn admit_ieee802154_session_command(
    session_open: bool,
    session_id: u64,
    request_id: u32,
    command: Ieee802154SessionCommand,
) -> bool {
    if !session_open {
        publish_event_reliably(
            session_id,
            request_id,
            oer_hil_protocol::base::Rejected(RejectReason::InvalidState),
        )
        .await;
        return false;
    }
    if IEEE802154_SESSION_COMMANDS.try_send(command).is_err() {
        publish_event_reliably(
            session_id,
            request_id,
            oer_hil_protocol::base::Rejected(RejectReason::Busy),
        )
        .await;
        return false;
    }
    true
}

#[derive(Clone, Copy)]
#[cfg_attr(
    feature = "memory-benchmark",
    allow(
        dead_code,
        reason = "shared protocol artifact assembly retains its value shape; this image has no calibration consumer"
    )
)]
pub struct StartupArtifact {
    bytes: [u8; STARTUP_ARTIFACT_CAPACITY],
    len: u16,
}

#[cfg(not(feature = "memory-benchmark"))]
impl StartupArtifact {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

struct StartupArtifactAssembler {
    bytes: [u8; STARTUP_ARTIFACT_CAPACITY],
    expected_total: Option<u16>,
    expected_crc32c: u32,
    received: usize,
    complete: bool,
}

impl StartupArtifactAssembler {
    const fn new() -> Self {
        Self {
            bytes: [0; STARTUP_ARTIFACT_CAPACITY],
            expected_total: None,
            expected_crc32c: 0,
            received: 0,
            complete: false,
        }
    }

    fn push(&mut self, chunk: &StartupArtifactChunk) -> Result<(), ()> {
        chunk.validate().map_err(|_| ())?;
        if usize::from(chunk.total_length()) > self.bytes.len() {
            return Err(());
        }
        if chunk.offset() == 0 {
            self.expected_total = Some(chunk.total_length());
            self.expected_crc32c = chunk.crc32c();
            self.received = 0;
            self.complete = false;
        }
        if self.complete
            || self.expected_total != Some(chunk.total_length())
            || self.expected_crc32c != chunk.crc32c()
            || usize::from(chunk.offset()) != self.received
        {
            return Err(());
        }
        let end = self.received + chunk.bytes().len();
        self.bytes[self.received..end].copy_from_slice(chunk.bytes());
        self.received = end;
        if chunk.is_final() {
            if self.received != usize::from(chunk.total_length())
                || startup_artifact_crc32c(&self.bytes[..self.received]) != self.expected_crc32c
            {
                self.expected_total = None;
                self.received = 0;
                return Err(());
            }
            self.complete = true;
        }
        Ok(())
    }

    fn started_but_incomplete(&self) -> bool {
        self.expected_total.is_some() && !self.complete
    }

    fn completed_artifact(&self) -> Option<StartupArtifact> {
        self.complete.then_some(StartupArtifact {
            bytes: self.bytes,
            len: u16::try_from(self.received).ok()?,
        })
    }
}

#[derive(Clone, Copy)]
struct SessionResult {
    session_id: u64,
    flow_evidence: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY],
    evidence: TransportEvidence,
    radio: Option<oer_hil_protocol::network::RadioEvidence>,
    tx_timing: Option<oer_hil_protocol::network::TxAggregateTimingEvidence>,
    rx_delivery: Option<RxDeliveryEvidence>,
    rx_zero_copy: Option<oer_hil_protocol::network::RxZeroCopyEvidence>,
    cpu_load: Option<oer_hil_protocol::network::CpuLoadEvidence>,
    verdict: SessionVerdict,
}

/// Frozen before publication. Replaying a result must not sample live
/// counters or stack watermarks again.
#[derive(Clone, Copy)]
struct RetainedSessionResult {
    measurement: SessionResult,
    link: LinkHealth,
    stack: oer_hil_protocol::system::StackUsage,
}

#[derive(Clone, Copy)]
struct ProtocolSession {
    active: ActiveSession,
    state: SessionState,
}

static RETAINED_SESSION_RESULTS: Mutex<
    CriticalSectionRawMutex,
    RefCell<[Option<RetainedSessionResult>; 2]>,
> = Mutex::new(RefCell::new([None; 2]));

fn retained_session_result(session_id: u64) -> Option<RetainedSessionResult> {
    RETAINED_SESSION_RESULTS.lock(|results| {
        results
            .borrow()
            .iter()
            .flatten()
            .copied()
            .find(|result| result.measurement.session_id == session_id)
    })
}

fn retain_session_result(result: RetainedSessionResult) -> bool {
    RETAINED_SESSION_RESULTS.lock(|results| {
        let mut results = results.borrow_mut();
        let Some(slot) = results.iter_mut().find(|slot| slot.is_none()) else {
            return false;
        };
        *slot = Some(result);
        true
    })
}

fn discard_session_result(session_id: u64) -> bool {
    RETAINED_SESSION_RESULTS.lock(|results| {
        let mut results = results.borrow_mut();
        let Some(slot) = results
            .iter_mut()
            .find(|slot| slot.is_some_and(|result| result.measurement.session_id == session_id))
        else {
            return false;
        };
        *slot = None;
        true
    })
}

#[cfg(feature = "i-radio")]
unsafe extern "C" {
    fn ets_printf(format: *const u8, ...) -> i32;
}

/// Reports the IEEE 802.15.4 MAC power-sequencing words once, before a
/// session's first transmission, for comparison with an ESP-IDF image. Each
/// word from `PAON_DELAY` (+0x100) to `DCDC_CTRL` (+0x11c) prints as its named
/// field and its vendor-reserved remainder.
#[cfg(feature = "i-radio")]
pub fn ieee802154_power_sequence_report(
    sequence: oer_esp32s31_hal::ieee802154::Ieee802154PowerSequence,
) {
    let reserved = sequence.vendor_reserved_bits();
    unsafe {
        ets_printf(
            c"ieee802154 power_sequence paon=%x/%x txon=%x/%x txen_stop=%x/%x txoff=%x/%x rxon=%x/%x txrx_switch=%x/%x cont_rx=%x/%x\r\n"
                .as_ptr()
                .cast(),
            u32::from(sequence.pa_on_delay()),
            reserved[0],
            u32::from(sequence.tx_on_delay()),
            reserved[1],
            u32::from(sequence.tx_enable_stop_delay()),
            reserved[2],
            u32::from(sequence.tx_off_delay()),
            reserved[3],
            u32::from(sequence.rx_on_delay()),
            reserved[4],
            u32::from(sequence.txrx_switch_delay()),
            reserved[5],
            u32::from(sequence.continuous_rx_delay()),
            reserved[6],
        );
        ets_printf(
            c"ieee802154 power_sequence dcdc pre_up=%x down=%x ctrl_en=%u tx_dcdc_up=%u reserved=%x\r\n"
                .as_ptr()
                .cast(),
            u32::from(sequence.dcdc_pre_raise_delay()),
            u32::from(sequence.dcdc_drop_delay()),
            u32::from(sequence.dcdc_control_enabled()),
            u32::from(sequence.dcdc_raise_for_tx()),
            reserved[7],
        );
    }
}

fn wifi_role_is(role: WifiRole) -> bool {
    let expected = match role {
        WifiRole::Idle => 1,
        WifiRole::Station => 2,
        WifiRole::Monitor => 3,
        WifiRole::AccessPoint => 4,
        WifiRole::StationAccessPoint => 5,
    };
    WIFI_ROLE_STATE.load(Ordering::Acquire) == expected
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ieee802154EventStatusProbeAdmission {
    Admit,
    Reject(RejectReason),
}

/// Decide admission without touching the radio owner or any validation MMIO.
///
/// Unsupported images take precedence over state and request validation.
/// Once supported, every owner/state mismatch takes precedence over malformed
/// bounds so a caller cannot use configuration errors to probe runtime state.
const fn ieee802154_event_status_probe_admission(
    supported: bool,
    initialized: bool,
    waiting_for_initialization: bool,
    session_id: u64,
    already_requested: bool,
    request_valid: bool,
) -> Ieee802154EventStatusProbeAdmission {
    if !supported {
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::Unsupported)
    } else if initialized || !waiting_for_initialization || session_id != 0 || already_requested {
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidState)
    } else if !request_valid {
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidConfiguration)
    } else {
        Ieee802154EventStatusProbeAdmission::Admit
    }
}

/// Reserve normal initialization once either initialization itself or the
/// one-shot owner-consuming IEEE diagnostic has been admitted.
const fn initialization_owner_rejection(
    initialized: bool,
    ieee802154_diagnostic_requested: bool,
) -> Option<RejectReason> {
    if initialized || ieee802154_diagnostic_requested {
        Some(RejectReason::InvalidState)
    } else {
        None
    }
}

// Compile-time regression matrix for the pure pre-initialization admission
// boundary. These assertions are built in both ordinary and diagnostic target
// graphs and add no runtime code or validation capability.
const _: () = {
    assert!(matches!(
        ieee802154_event_status_probe_admission(false, true, false, 7, true, false),
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::Unsupported)
    ));
    assert!(matches!(
        ieee802154_event_status_probe_admission(true, true, true, 0, false, false),
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidState)
    ));
    assert!(matches!(
        ieee802154_event_status_probe_admission(true, false, false, 0, false, false),
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidState)
    ));
    assert!(matches!(
        ieee802154_event_status_probe_admission(true, false, true, 1, false, false),
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidState)
    ));
    assert!(matches!(
        ieee802154_event_status_probe_admission(true, false, true, 0, true, false),
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidState)
    ));
    assert!(matches!(
        ieee802154_event_status_probe_admission(true, false, true, 0, false, false),
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidConfiguration)
    ));
    assert!(matches!(
        ieee802154_event_status_probe_admission(true, false, true, 0, false, true),
        Ieee802154EventStatusProbeAdmission::Admit
    ));
    assert!(matches!(
        initialization_owner_rejection(false, true),
        Some(RejectReason::InvalidState)
    ));
};

/// Owns commands while individual benchmark services migrate to
/// runtime-configured sessions. Unsupported mutations receive an explicit
/// response instead of being silently ignored.
#[embassy_executor::task]
#[allow(
    large_assignments,
    reason = "the protocol task moves bounded typed session results into its static Embassy arena; the linked-image stack audit remains authoritative"
)]
pub async fn protocol_task() {
    publish_event_reliably(
        0,
        0,
        oer_hil_protocol::network::StateChanged(StateChange {
            previous: SessionState::Booting,
            current: SessionState::WaitingForInitialization,
        }),
    )
    .await;
    let mut initialized = false;
    let mut state = SessionState::WaitingForInitialization;
    // Once this owner-consuming diagnostic has been queued, normal radio
    // initialization must not race it. The diagnostic image is one-shot and
    // publishes its completion before the product task returns.
    #[cfg(feature = "i-diagnostic")]
    let mut ieee802154_diagnostic_requested = false;
    // An admitted session start opens the session; an admitted stop closes it.
    #[cfg(feature = "i-radio")]
    let mut ieee802154_session_open = false;
    #[cfg(feature = "i-thread")]
    let mut ieee802154_thread_open = false;
    #[cfg(not(feature = "i-diagnostic"))]
    let ieee802154_diagnostic_requested = false;
    // One slot per physical STA+AP network endpoint. A slot is keyed by its
    // opaque session ID and no two live slots may target the same interface.
    let mut sessions = [None::<ProtocolSession>; 2];
    let mut startup_artifact = StartupArtifactAssembler::new();
    let mut ap_scheduler = oer_hil_protocol::wifi::WifiApScheduler::Disabled;
    loop {
        #[cfg(not(feature = "memory-benchmark"))]
        crate::hang_watchdog::console_stall_point().await;
        match select(COMMANDS.receive(), SESSION_RESULTS.receive()).await {
            Either::First(command) => {
                #[cfg(not(feature = "memory-benchmark"))]
                crate::hang_watchdog::took_work(
                    oer_hil_protocol::base::TaskSlot::Console,
                    !COMMANDS.is_empty(),
                );
                let session_id = command.session_id;
                let request_id = command.request_id;
                match command.body {
                    #[cfg(not(feature = "memory-benchmark"))]
                    Request::ControlTracking(ControlTracking(control)) => {
                        let reply = if session_id != 0 {
                            Err(RejectReason::InvalidState)
                        } else {
                            Ok(crate::phy_tracking::control(control))
                        };
                        respond(session_id, request_id, reply).await;
                    }
                    #[cfg(feature = "phy-tracking-hold")]
                    Request::MeasureTrackingHold(oer_hil_protocol::phy::MeasureTrackingHold(
                        request,
                    )) => {
                        let reply = if session_id != 0 {
                            Err(RejectReason::InvalidState)
                        } else {
                            crate::phy_tracking::measure_hold(request)
                                .await
                                .map(oer_hil_protocol::phy::TrackingHold)
                        };
                        respond(session_id, request_id, reply).await;
                    }
                    #[cfg(not(feature = "memory-benchmark"))]
                    Request::ReadRegisterImage(ReadRegisterImage(request)) => {
                        let reply = if session_id != 0 {
                            Err(RejectReason::InvalidState)
                        } else {
                            crate::product_hil::phy_register_image::read(request).await
                        };
                        respond(session_id, request_id, reply).await;
                    }
                    // The program-counter profile: served by images that
                    // compile the sampler (`pc-profile`).
                    #[cfg(feature = "pc-profile")]
                    Request::ControlProfile(ControlProfile(control)) => {
                        respond(session_id, request_id, crate::pc_profile::control(control)).await;
                    }
                    #[cfg(feature = "pc-profile")]
                    Request::GetProfileSamples(GetProfileSamples { hart, first }) => {
                        respond(
                            session_id,
                            request_id,
                            crate::pc_profile::samples(hart, first),
                        )
                        .await;
                    }
                    #[cfg(not(feature = "memory-benchmark"))]
                    Request::ReadAnalogImage(ReadAnalogImage(request)) => {
                        let reply = if session_id != 0 {
                            Err(RejectReason::InvalidState)
                        } else {
                            crate::product_hil::phy_register_image::read_analog(request).await
                        };
                        respond(session_id, request_id, reply).await;
                    }
                    #[cfg(not(feature = "memory-benchmark"))]
                    Request::ReadCalibrationProjection(ReadCalibrationProjection(request)) => {
                        let reply = if session_id != 0 {
                            Err(RejectReason::InvalidState)
                        } else {
                            crate::phy_calibration_projection::read(request)
                        };
                        respond(session_id, request_id, reply).await;
                    }
                    Request::ControlFault(ControlFault(control)) => {
                        let reply = if session_id == 0 {
                            crate::phy_fault::control(control)
                        } else {
                            Err(RejectReason::InvalidState)
                        };
                        let accepted = reply.is_ok();
                        let sequence = respond(session_id, request_id, reply).await;
                        if accepted {
                            CONSOLE.written(sequence).await;
                            crate::phy_fault::after_response(control, true);
                        }
                    }
                    #[cfg(feature = "wifi-ble-coex")]
                    Request::GetGatt(oer_hil_protocol::bluetooth::GetGatt) => {
                        respond(
                            session_id,
                            request_id,
                            Ok(oer_hil_protocol::bluetooth::GattState(
                                crate::bluetooth::shared::evidence(),
                            )),
                        )
                        .await;
                    }
                    #[cfg(not(feature = "memory-benchmark"))]
                    Request::InjectHang(InjectHang(target)) => {
                        let reply = if session_id == 0 {
                            match target {
                                HangTarget::ProtocolExecutor => crate::hang_watchdog::inject(
                                    crate::hang_watchdog::Executor::Protocol,
                                ),
                                HangTarget::NetworkExecutor => crate::hang_watchdog::inject(
                                    crate::hang_watchdog::Executor::Network,
                                ),
                                HangTarget::Console => crate::hang_watchdog::inject_console_stall(),
                            }
                            Ok(HangInjected(target))
                        } else {
                            Err(RejectReason::InvalidState)
                        };
                        respond(session_id, request_id, reply).await;
                    }
                    #[cfg(trace_journal)]
                    Request::ControlTrace(ControlTrace(control)) => {
                        let reply = if session_id == 0 {
                            Ok(TraceState(crate::trace::control(control)))
                        } else {
                            Err(RejectReason::InvalidState)
                        };
                        respond(session_id, request_id, reply).await;
                    }
                    #[cfg(trace_journal)]
                    Request::GetTraceEntries(GetTraceEntries { first }) => {
                        let reply = if session_id == 0 {
                            Ok(TraceEntriesPage(crate::trace::entries(first)))
                        } else {
                            Err(RejectReason::InvalidState)
                        };
                        respond(session_id, request_id, reply).await;
                    }
                    #[cfg(trace_journal)]
                    Request::GetTraceSnapshot(GetTraceSnapshot { slot, offset }) => {
                        let reply = if session_id == 0 {
                            Ok(TraceSnapshot(crate::trace::snapshot(slot, offset)))
                        } else {
                            Err(RejectReason::InvalidState)
                        };
                        respond(session_id, request_id, reply).await;
                    }
                    Request::GetStacks(GetStacks) => {
                        let reply = if stacks_observable(initialized, state, &sessions, session_id)
                        {
                            Ok(Stacks(crate::stack_usage_snapshot().await))
                        } else {
                            Err(RejectReason::InvalidState)
                        };
                        respond(session_id, request_id, reply).await;
                    }
                    Request::GetInterruptStacks(GetInterruptStacks) => {
                        let reply = if stacks_observable(initialized, state, &sessions, session_id)
                        {
                            let stack = crate::stack_usage_snapshot().await;
                            Ok(InterruptStacks {
                                cpu0: stack.cpu0_irq,
                                cpu1: stack.cpu1_irq,
                            })
                        } else {
                            Err(RejectReason::InvalidState)
                        };
                        respond(session_id, request_id, reply).await;
                    }
                    #[cfg(feature = "memory-benchmark")]
                    Request::RunMemoryBenchmark(oer_hil_protocol::system::RunMemoryBenchmark(
                        request,
                    )) => {
                        let rejection = if !crate::image_features::has::<
                            oer_hil_protocol::system::MemoryBenchmark,
                        >() {
                            Some(RejectReason::Unsupported)
                        } else if initialized
                            || state != SessionState::WaitingForInitialization
                            || session_id != 0
                        {
                            Some(RejectReason::InvalidState)
                        } else if !request.validate() {
                            Some(RejectReason::InvalidConfiguration)
                        } else {
                            #[cfg(feature = "memory-benchmark")]
                            {
                                crate::memory_benchmark::submit(request_id, request).err()
                            }
                            #[cfg(not(feature = "memory-benchmark"))]
                            {
                                Some(RejectReason::Unsupported)
                            }
                        };
                        if let Some(reason) = rejection {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(reason),
                            )
                            .await;
                        }
                    }
                    Request::CallAcrossCores(CallAcrossCores) => {
                        let response =
                            if crate::image_features::has::<oer_hil_protocol::system::IpcCall>() {
                                crate::system::ipc_call::call_across_cores()
                                    .await
                                    .ok_or(RejectReason::InvalidState)
                            } else {
                                Err(RejectReason::Unsupported)
                            };
                        respond(session_id, request_id, response).await;
                    }
                    #[cfg(feature = "interrupt-latency-probe")]
                    Request::MeasureInterruptLatency(
                        oer_hil_protocol::system::MeasureInterruptLatency(request),
                    ) => {
                        let evidence = crate::system::interrupt_latency::measure(request).await;
                        respond(
                            session_id,
                            request_id,
                            Ok(oer_hil_protocol::system::InterruptLatency(evidence)),
                        )
                        .await;
                    }
                    #[cfg(not(feature = "memory-benchmark"))]
                    Request::ProbeSourceGate(_) => {
                        let response = if crate::image_features::has::<
                            oer_hil_protocol::system::SourceGate,
                        >() {
                            crate::source_gate::probe()
                                .await
                                .ok_or(RejectReason::InvalidState)
                        } else {
                            Err(RejectReason::Unsupported)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::ProbeTimebase(oer_hil_protocol::system::ProbeTimebase(request)) => {
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::system::TimebaseProbe,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if initialized
                            || state != SessionState::WaitingForInitialization
                            || session_id != 0
                        {
                            Err(RejectReason::InvalidState)
                        } else if !request.validate() {
                            Err(RejectReason::InvalidConfiguration)
                        } else {
                            Ok(oer_hil_protocol::system::TimebaseProbed(
                                run_timebase_probe(request).await,
                            ))
                        };
                        respond(session_id, request_id, response).await;
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::ProbeEventStatus(oer_hil_protocol::ieee802154::ProbeEventStatus(
                        request,
                    )) => {
                        let admission = ieee802154_event_status_probe_admission(
                            crate::image_features::has::<
                                oer_hil_protocol::ieee802154::EventStatusProbe,
                            >(),
                            initialized,
                            state == SessionState::WaitingForInitialization,
                            session_id,
                            ieee802154_diagnostic_requested,
                            request.validate(),
                        );
                        match admission {
                            Ieee802154EventStatusProbeAdmission::Reject(reason) => {
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Rejected(reason),
                                )
                                .await;
                            }
                            Ieee802154EventStatusProbeAdmission::Admit => {
                                #[cfg(feature = "i-event-status-probe")]
                                {
                                    if IEEE802154_EVENT_STATUS_PROBES
                                        .try_send(Ieee802154EventStatusProbe {
                                            request_id,
                                            request,
                                        })
                                        .is_err()
                                    {
                                        publish_event_reliably(
                                            session_id,
                                            request_id,
                                            oer_hil_protocol::base::Rejected(RejectReason::Busy),
                                        )
                                        .await;
                                    } else {
                                        ieee802154_diagnostic_requested = true;
                                    }
                                }
                                #[cfg(not(feature = "i-event-status-probe"))]
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                                )
                                .await;
                            }
                        }
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::ProbeRoute(oer_hil_protocol::ieee802154::ProbeRoute(request)) => {
                        let admission = ieee802154_event_status_probe_admission(
                            crate::image_features::has::<oer_hil_protocol::ieee802154::RouteProbe>(
                            ),
                            initialized,
                            state == SessionState::WaitingForInitialization,
                            session_id,
                            ieee802154_diagnostic_requested,
                            request.validate(),
                        );
                        match admission {
                            Ieee802154EventStatusProbeAdmission::Reject(reason) => {
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Rejected(reason),
                                )
                                .await;
                            }
                            Ieee802154EventStatusProbeAdmission::Admit => {
                                #[cfg(feature = "i-route-probe")]
                                {
                                    if IEEE802154_ROUTE_PROBES
                                        .try_send(Ieee802154RouteProbe {
                                            request_id,
                                            request,
                                        })
                                        .is_err()
                                    {
                                        publish_event_reliably(
                                            session_id,
                                            request_id,
                                            oer_hil_protocol::base::Rejected(RejectReason::Busy),
                                        )
                                        .await;
                                    } else {
                                        ieee802154_diagnostic_requested = true;
                                    }
                                }
                                #[cfg(not(feature = "i-route-probe"))]
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                                )
                                .await;
                            }
                        }
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::ProbeEdEvent(oer_hil_protocol::ieee802154::ProbeEdEvent(request)) => {
                        let admission = ieee802154_event_status_probe_admission(
                            crate::image_features::has::<oer_hil_protocol::ieee802154::EdEventProbe>(
                            ),
                            initialized,
                            state == SessionState::WaitingForInitialization,
                            session_id,
                            ieee802154_diagnostic_requested,
                            request.validate(),
                        );
                        match admission {
                            Ieee802154EventStatusProbeAdmission::Reject(reason) => {
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Rejected(reason),
                                )
                                .await;
                            }
                            Ieee802154EventStatusProbeAdmission::Admit => {
                                #[cfg(feature = "i-ed-event-probe")]
                                {
                                    if IEEE802154_ED_EVENT_PROBES
                                        .try_send(Ieee802154EdEventProbe {
                                            request_id,
                                            request,
                                        })
                                        .is_err()
                                    {
                                        publish_event_reliably(
                                            session_id,
                                            request_id,
                                            oer_hil_protocol::base::Rejected(RejectReason::Busy),
                                        )
                                        .await;
                                    } else {
                                        ieee802154_diagnostic_requested = true;
                                    }
                                }
                                #[cfg(not(feature = "i-ed-event-probe"))]
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                                )
                                .await;
                            }
                        }
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::RunAirCheck(oer_hil_protocol::ieee802154::RunAirCheck(request)) => {
                        let admission = ieee802154_event_status_probe_admission(
                            crate::image_features::has::<oer_hil_protocol::ieee802154::AirCheck>(),
                            initialized,
                            state == SessionState::WaitingForInitialization,
                            session_id,
                            ieee802154_diagnostic_requested,
                            request.validate(),
                        );
                        match admission {
                            Ieee802154EventStatusProbeAdmission::Reject(reason) => {
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Rejected(reason),
                                )
                                .await;
                            }
                            Ieee802154EventStatusProbeAdmission::Admit => {
                                #[cfg(feature = "i-radio")]
                                {
                                    if IEEE802154_AIR_CHECKS
                                        .try_send(Ieee802154AirCheck {
                                            request_id,
                                            request,
                                        })
                                        .is_err()
                                    {
                                        publish_event_reliably(
                                            session_id,
                                            request_id,
                                            oer_hil_protocol::base::Rejected(RejectReason::Busy),
                                        )
                                        .await;
                                    } else {
                                        ieee802154_diagnostic_requested = true;
                                    }
                                }
                                #[cfg(not(feature = "i-radio"))]
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                                )
                                .await;
                            }
                        }
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::StartSession(oer_hil_protocol::ieee802154::StartSession(config)) => {
                        let admission = ieee802154_event_status_probe_admission(
                            crate::image_features::has::<oer_hil_protocol::ieee802154::Session>(),
                            initialized,
                            state == SessionState::WaitingForInitialization,
                            session_id,
                            ieee802154_diagnostic_requested,
                            config.validate(),
                        );
                        match admission {
                            Ieee802154EventStatusProbeAdmission::Reject(reason) => {
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Rejected(reason),
                                )
                                .await;
                            }
                            Ieee802154EventStatusProbeAdmission::Admit => {
                                #[cfg(feature = "i-radio")]
                                {
                                    if IEEE802154_SESSION_STARTS
                                        .try_send(Ieee802154SessionStart { request_id, config })
                                        .is_err()
                                    {
                                        publish_event_reliably(
                                            session_id,
                                            request_id,
                                            oer_hil_protocol::base::Rejected(RejectReason::Busy),
                                        )
                                        .await;
                                    } else {
                                        ieee802154_diagnostic_requested = true;
                                        ieee802154_session_open = true;
                                    }
                                }
                                #[cfg(not(feature = "i-radio"))]
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                                )
                                .await;
                            }
                        }
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::TransmitSession(oer_hil_protocol::ieee802154::TransmitSession(
                        request,
                    )) => {
                        #[cfg(feature = "i-radio")]
                        if request.validate() {
                            admit_ieee802154_session_command(
                                ieee802154_session_open,
                                session_id,
                                request_id,
                                Ieee802154SessionCommand::Transmit {
                                    request_id,
                                    request,
                                },
                            )
                            .await;
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(
                                    RejectReason::InvalidConfiguration,
                                ),
                            )
                            .await;
                        }
                        #[cfg(not(feature = "i-radio"))]
                        {
                            let _ = request;
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                            )
                            .await;
                        }
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::ReceiveSession(oer_hil_protocol::ieee802154::ReceiveSession) => {
                        #[cfg(feature = "i-radio")]
                        admit_ieee802154_session_command(
                            ieee802154_session_open,
                            session_id,
                            request_id,
                            Ieee802154SessionCommand::Receive { request_id },
                        )
                        .await;
                        #[cfg(not(feature = "i-radio"))]
                        publish_event_reliably(
                            session_id,
                            request_id,
                            oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                        )
                        .await;
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::CollectSession(oer_hil_protocol::ieee802154::CollectSession) => {
                        #[cfg(feature = "i-radio")]
                        admit_ieee802154_session_command(
                            ieee802154_session_open,
                            session_id,
                            request_id,
                            Ieee802154SessionCommand::Collect { request_id },
                        )
                        .await;
                        #[cfg(not(feature = "i-radio"))]
                        publish_event_reliably(
                            session_id,
                            request_id,
                            oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                        )
                        .await;
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::SetSessionPending(
                        oer_hil_protocol::ieee802154::SetSessionPending(request),
                    ) => {
                        #[cfg(feature = "i-radio")]
                        admit_ieee802154_session_command(
                            ieee802154_session_open,
                            session_id,
                            request_id,
                            Ieee802154SessionCommand::Pending {
                                request_id,
                                request,
                            },
                        )
                        .await;
                        #[cfg(not(feature = "i-radio"))]
                        {
                            let _ = request;
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                            )
                            .await;
                        }
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::MaintainSessionPhy(
                        oer_hil_protocol::ieee802154::MaintainSessionPhy,
                    ) => {
                        #[cfg(feature = "i-radio")]
                        admit_ieee802154_session_command(
                            ieee802154_session_open,
                            session_id,
                            request_id,
                            Ieee802154SessionCommand::MaintainPhy { request_id },
                        )
                        .await;
                        #[cfg(not(feature = "i-radio"))]
                        publish_event_reliably(
                            session_id,
                            request_id,
                            oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                        )
                        .await;
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::AssessSessionChannel(
                        oer_hil_protocol::ieee802154::AssessSessionChannel(request),
                    ) => {
                        #[cfg(feature = "i-radio")]
                        if request.validate() {
                            admit_ieee802154_session_command(
                                ieee802154_session_open,
                                session_id,
                                request_id,
                                Ieee802154SessionCommand::Assess {
                                    request_id,
                                    request,
                                },
                            )
                            .await;
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(
                                    RejectReason::InvalidConfiguration,
                                ),
                            )
                            .await;
                        }
                        #[cfg(not(feature = "i-radio"))]
                        {
                            let _ = request;
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                            )
                            .await;
                        }
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::StartThread(oer_hil_protocol::ieee802154::StartThread(request)) => {
                        let admission = ieee802154_event_status_probe_admission(
                            crate::image_features::has::<oer_hil_protocol::ieee802154::Thread>(),
                            initialized,
                            state == SessionState::WaitingForInitialization,
                            session_id,
                            ieee802154_diagnostic_requested,
                            request.validate(),
                        );
                        match admission {
                            Ieee802154EventStatusProbeAdmission::Reject(reason) => {
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Rejected(reason),
                                )
                                .await;
                            }
                            Ieee802154EventStatusProbeAdmission::Admit => {
                                #[cfg(feature = "i-thread")]
                                {
                                    if IEEE802154_THREAD_STARTS
                                        .try_send(Ieee802154ThreadStart {
                                            request_id,
                                            request,
                                        })
                                        .is_err()
                                    {
                                        publish_event_reliably(
                                            session_id,
                                            request_id,
                                            oer_hil_protocol::base::Rejected(RejectReason::Busy),
                                        )
                                        .await;
                                    } else {
                                        ieee802154_diagnostic_requested = true;
                                        ieee802154_thread_open = true;
                                    }
                                }
                                #[cfg(not(feature = "i-thread"))]
                                {
                                    let _ = request;
                                    publish_event_reliably(
                                        session_id,
                                        request_id,
                                        oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::GetThread(oer_hil_protocol::ieee802154::GetThread) => {
                        #[cfg(feature = "i-thread")]
                        admit_ieee802154_thread_command(
                            ieee802154_thread_open,
                            session_id,
                            request_id,
                            Ieee802154ThreadCommand::Query { request_id },
                        )
                        .await;
                        #[cfg(not(feature = "i-thread"))]
                        publish_event_reliably(
                            session_id,
                            request_id,
                            oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                        )
                        .await;
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::SendThread(oer_hil_protocol::ieee802154::SendThread(request)) => {
                        #[cfg(feature = "i-thread")]
                        if request.validate() {
                            admit_ieee802154_thread_command(
                                ieee802154_thread_open,
                                session_id,
                                request_id,
                                Ieee802154ThreadCommand::Send {
                                    request_id,
                                    request,
                                },
                            )
                            .await;
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(
                                    RejectReason::InvalidConfiguration,
                                ),
                            )
                            .await;
                        }
                        #[cfg(not(feature = "i-thread"))]
                        {
                            let _ = request;
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                            )
                            .await;
                        }
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::CollectThread(oer_hil_protocol::ieee802154::CollectThread) => {
                        #[cfg(feature = "i-thread")]
                        admit_ieee802154_thread_command(
                            ieee802154_thread_open,
                            session_id,
                            request_id,
                            Ieee802154ThreadCommand::Collect { request_id },
                        )
                        .await;
                        #[cfg(not(feature = "i-thread"))]
                        publish_event_reliably(
                            session_id,
                            request_id,
                            oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                        )
                        .await;
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::StopThread(oer_hil_protocol::ieee802154::StopThread) => {
                        #[cfg(feature = "i-thread")]
                        if admit_ieee802154_thread_command(
                            ieee802154_thread_open,
                            session_id,
                            request_id,
                            Ieee802154ThreadCommand::Stop { request_id },
                        )
                        .await
                        {
                            ieee802154_thread_open = false;
                        }
                        #[cfg(not(feature = "i-thread"))]
                        publish_event_reliably(
                            session_id,
                            request_id,
                            oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                        )
                        .await;
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::RestartSessionRadio(
                        oer_hil_protocol::ieee802154::RestartSessionRadio,
                    ) => {
                        #[cfg(feature = "i-radio")]
                        admit_ieee802154_session_command(
                            ieee802154_session_open,
                            session_id,
                            request_id,
                            Ieee802154SessionCommand::Restart { request_id },
                        )
                        .await;
                        #[cfg(not(feature = "i-radio"))]
                        publish_event_reliably(
                            session_id,
                            request_id,
                            oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                        )
                        .await;
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::ReadSessionRecentRssi(
                        oer_hil_protocol::ieee802154::ReadSessionRecentRssi,
                    ) => {
                        #[cfg(feature = "i-radio")]
                        admit_ieee802154_session_command(
                            ieee802154_session_open,
                            session_id,
                            request_id,
                            Ieee802154SessionCommand::RecentRssi { request_id },
                        )
                        .await;
                        #[cfg(not(feature = "i-radio"))]
                        publish_event_reliably(
                            session_id,
                            request_id,
                            oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                        )
                        .await;
                    }
                    #[cfg(feature = "i-diagnostic")]
                    Request::StopSession(oer_hil_protocol::ieee802154::StopSession) => {
                        #[cfg(feature = "i-radio")]
                        if admit_ieee802154_session_command(
                            ieee802154_session_open,
                            session_id,
                            request_id,
                            Ieee802154SessionCommand::Stop { request_id },
                        )
                        .await
                        {
                            ieee802154_session_open = false;
                        }
                        #[cfg(not(feature = "i-radio"))]
                        publish_event_reliably(
                            session_id,
                            request_id,
                            oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                        )
                        .await;
                    }
                    Request::UploadStartupArtifact(
                        oer_hil_protocol::phy::UploadStartupArtifact(chunk),
                    ) => {
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::phy::StartupArtifact,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if initialized || state != SessionState::WaitingForInitialization {
                            Err(RejectReason::InvalidState)
                        } else if startup_artifact.push(&chunk).is_err() {
                            Err(RejectReason::InvalidConfiguration)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::Initialize(oer_hil_protocol::wifi::Initialize(configuration)) => {
                        // This diagnostic image reserves pre-radio execution for
                        // repeated memory cases; it never initializes Wi-Fi.
                        if crate::image_features::has::<oer_hil_protocol::system::MemoryBenchmark>()
                        {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(RejectReason::Unsupported),
                            )
                            .await;
                            continue;
                        }
                        let (response, accepted) = if let Some(reason) =
                            initialization_owner_rejection(
                                initialized,
                                ieee802154_diagnostic_requested,
                            ) {
                            (Err(reason), false)
                        } else if configuration.ap_scheduler
                            != oer_hil_protocol::wifi::WifiApScheduler::Disabled
                            && !cfg!(feature = "owned-network")
                        {
                            (Err(RejectReason::Unsupported), false)
                        } else if !configuration.validate()
                            || startup_artifact.started_but_incomplete()
                        {
                            (Err(RejectReason::InvalidConfiguration), false)
                        } else if STARTUP_CONFIGURATIONS
                            .try_send(StartupConfiguration {
                                ap_scheduler: configuration.ap_scheduler,
                                request_id,
                                ipv4: configuration.ipv4,
                                data_plane: configuration.data_plane,
                                rx_checksum: configuration.rx_checksum,
                                tx_udp_checksum: configuration.tx_udp_checksum,
                                tx_buffer: configuration.tx_buffer,
                                rx_continuation: configuration.rx_continuation,
                                l1_cache_counters: configuration.l1_cache_counters,
                                phy_calibration_artifact: startup_artifact.completed_artifact(),
                            })
                            .is_err()
                        {
                            (Err(RejectReason::Busy), false)
                        } else {
                            initialized = true;
                            ap_scheduler = configuration.ap_scheduler;
                            (Ok(oer_hil_protocol::base::Accepted), true)
                        };
                        respond(session_id, request_id, response).await;
                        if accepted {
                            transition_state(&mut state, SessionState::Idle, 0, request_id).await;
                        }
                    }
                    Request::Configure(oer_hil_protocol::network::Configure(config)) => {
                        let duplicate_interface = sessions.iter().flatten().any(|session| {
                            session.active.config.network_interface == config.network_interface
                        });
                        let free = sessions.iter().position(Option::is_none);
                        let rejection = if !crate::image_features::has::<
                            oer_hil_protocol::network::RuntimeConfiguration,
                        >() {
                            Some(RejectReason::Unsupported)
                        } else if !initialized || state != SessionState::Idle {
                            Some(RejectReason::InvalidState)
                        } else if session_id == 0 {
                            Some(RejectReason::SessionId)
                        } else if !valid_session_config(config) {
                            Some(RejectReason::InvalidConfiguration)
                        } else if duplicate_interface || free.is_none() {
                            Some(RejectReason::Busy)
                        } else {
                            None
                        };
                        if let Some(reason) = rejection {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(reason),
                            )
                            .await;
                        } else {
                            let index = free.expect("validated session capacity has a free slot");
                            sessions[index] = Some(ProtocolSession {
                                active: ActiveSession { session_id, config },
                                state: SessionState::Idle,
                            });
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Accepted,
                            )
                            .await;
                            transition_state(
                                &mut sessions[index]
                                    .as_mut()
                                    .expect("configured slot remains owned")
                                    .state,
                                SessionState::Configured,
                                session_id,
                                request_id,
                            )
                            .await;
                        }
                    }
                    Request::Arm(oer_hil_protocol::network::Arm) => {
                        let slot = sessions.iter().position(|slot| {
                            slot.is_some_and(|session| session.active.session_id == session_id)
                        });
                        if slot.is_none_or(|index| {
                            sessions[index]
                                .is_none_or(|session| session.state != SessionState::Configured)
                        }) {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        } else {
                            let index = slot.expect("validated arm has a session slot");
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Accepted,
                            )
                            .await;
                            transition_state(
                                &mut sessions[index]
                                    .as_mut()
                                    .expect("armed slot remains owned")
                                    .state,
                                SessionState::Armed,
                                session_id,
                                request_id,
                            )
                            .await;
                        }
                    }
                    Request::Start(oer_hil_protocol::network::Start) => {
                        let slot = sessions.iter().position(|slot| {
                            slot.is_some_and(|session| {
                                session.active.session_id == session_id
                                    && session.state == SessionState::Armed
                            })
                        });
                        if let Some(index) = slot {
                            let session = sessions[index]
                                .expect("located session slot remains owned")
                                .active;
                            if SESSION_STARTS.try_send(session).is_err() {
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Rejected(RejectReason::Busy),
                                )
                                .await;
                            } else {
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    oer_hil_protocol::base::Accepted,
                                )
                                .await;
                                transition_state(
                                    &mut sessions[index]
                                        .as_mut()
                                        .expect("running slot remains owned")
                                        .state,
                                    SessionState::Running,
                                    session_id,
                                    request_id,
                                )
                                .await;
                            }
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        }
                    }
                    Request::GetStatus(oer_hil_protocol::network::GetStatus) => {
                        let session = sessions.iter().flatten().find(|session| {
                            session_id == 0 || session.active.session_id == session_id
                        });
                        publish_event_reliably(
                            session_id,
                            request_id,
                            oer_hil_protocol::network::Status(
                                oer_hil_protocol::network::OperationStatus {
                                    state: session.map_or(state, |session| session.state),
                                    configured_session_id: session
                                        .map(|session| session.active.session_id),
                                    completed_session_id: session
                                        .and_then(|session| {
                                            retained_session_result(session.active.session_id)
                                        })
                                        .map(|result| result.measurement.session_id),
                                },
                            ),
                        )
                        .await;
                    }
                    Request::Cancel(oer_hil_protocol::network::Cancel) => {
                        let slot = sessions.iter().position(|slot| {
                            slot.is_some_and(|session| {
                                session.active.session_id == session_id
                                    && matches!(
                                        session.state,
                                        SessionState::Configured | SessionState::Armed
                                    )
                            })
                        });
                        if let Some(index) = slot {
                            let previous = sessions[index]
                                .expect("located cancel slot remains owned")
                                .state;
                            sessions[index] = None;
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Accepted,
                            )
                            .await;
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::network::StateChanged(StateChange {
                                    previous,
                                    current: SessionState::Idle,
                                }),
                            )
                            .await;
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        }
                    }
                    Request::ReplayResult(oer_hil_protocol::network::ReplayResult) => {
                        if let Some(result) = retained_session_result(session_id) {
                            publish_result(result, request_id).await;
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        }
                    }
                    Request::AcknowledgeResult(oer_hil_protocol::network::AcknowledgeResult) => {
                        let slot = sessions.iter().position(|slot| {
                            slot.is_some_and(|session| {
                                session.active.session_id == session_id
                                    && session.state == SessionState::Finished
                                    && retained_session_result(session_id).is_some()
                            })
                        });
                        if let Some(index) = slot {
                            let _ = discard_session_result(session_id);
                            sessions[index] = None;
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Accepted,
                            )
                            .await;
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::network::StateChanged(StateChange {
                                    previous: SessionState::Finished,
                                    current: SessionState::Idle,
                                }),
                            )
                            .await;
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        }
                    }
                    Request::Recover(oer_hil_protocol::network::Recover) => {
                        let slot = sessions.iter().position(|slot| {
                            slot.is_some_and(|session| {
                                session.active.session_id == session_id
                                    && matches!(
                                        session.state,
                                        SessionState::Finished | SessionState::Failed
                                    )
                            })
                        });
                        if let Some(index) = slot {
                            let previous = sessions[index]
                                .expect("located recovery slot remains owned")
                                .state;
                            sessions[index] = None;
                            let _ = discard_session_result(session_id);
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Accepted,
                            )
                            .await;
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::network::StateChanged(StateChange {
                                    previous,
                                    current: SessionState::Idle,
                                }),
                            )
                            .await;
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                oer_hil_protocol::base::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        }
                    }
                    Request::CycleStationEpoch(oer_hil_protocol::wifi::CycleStationEpoch) => {
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::wifi::StationEpochControl,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Station)
                        {
                            Err(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::Cycle { request_id })
                            .is_err()
                        {
                            Err(RejectReason::Busy)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::StopStation(oer_hil_protocol::wifi::StopStation) => {
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::wifi::RoleControl,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Station)
                        {
                            Err(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StopStation { request_id })
                            .is_err()
                        {
                            Err(RejectReason::Busy)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::StartStation(oer_hil_protocol::wifi::StartStation(
                        oer_hil_protocol::wifi::StationStart {
                            credentials,
                            power_save,
                        },
                    )) => {
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::wifi::RoleControl,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if credentials.validate().is_err() || !power_save.is_valid() {
                            Err(RejectReason::InvalidConfiguration)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Err(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StartStation {
                                request_id,
                                credentials,
                                power_save,
                            })
                            .is_err()
                        {
                            Err(RejectReason::Busy)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::RestartRadio(oer_hil_protocol::wifi::RestartRadio) => {
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::wifi::RoleControl,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Err(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::RestartRadio { request_id })
                            .is_err()
                        {
                            Err(RejectReason::Busy)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::Scan(oer_hil_protocol::wifi::Scan(request)) => {
                        let valid = request.channel_mask_2_4_ghz != 0
                            && request.channel_mask_2_4_ghz & !0x1fff == 0
                            && (1..=1_000).contains(&request.dwell_millis);
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::wifi::RoleControl,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if !valid {
                            Err(RejectReason::InvalidConfiguration)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Err(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::Scan {
                                request_id,
                                request,
                            })
                            .is_err()
                        {
                            Err(RejectReason::Busy)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::StartMonitor(oer_hil_protocol::wifi::StartMonitor(request)) => {
                        let valid =
                            (1..=13).contains(&request.channel) && request.snapshot_length <= 2_304;
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::wifi::RoleControl,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if !valid {
                            Err(RejectReason::InvalidConfiguration)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Err(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StartMonitor {
                                request_id,
                                request,
                            })
                            .is_err()
                        {
                            Err(RejectReason::Busy)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::StopMonitor(oer_hil_protocol::wifi::StopMonitor) => {
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::wifi::RoleControl,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Monitor)
                        {
                            Err(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StopMonitor { request_id })
                            .is_err()
                        {
                            Err(RejectReason::Busy)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::StartAccessPoint(oer_hil_protocol::wifi::StartAccessPoint(
                        request,
                    )) => {
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::wifi::AccessPoint,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if request.validate().is_err() {
                            Err(RejectReason::InvalidConfiguration)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Err(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StartAccessPoint {
                                request_id,
                                request,
                            })
                            .is_err()
                        {
                            Err(RejectReason::Busy)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::StopAccessPoint(oer_hil_protocol::wifi::StopAccessPoint) => {
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::wifi::AccessPoint,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::AccessPoint)
                        {
                            Err(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StopAccessPoint { request_id })
                            .is_err()
                        {
                            Err(RejectReason::Busy)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::StartStationAccessPoint(
                        oer_hil_protocol::wifi::StartStationAccessPoint(request),
                    ) => {
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::wifi::StationAccessPoint,
                        >() || ap_scheduler
                            != oer_hil_protocol::wifi::WifiApScheduler::Disabled
                        {
                            Err(RejectReason::Unsupported)
                        } else if request.validate().is_err() {
                            Err(RejectReason::InvalidConfiguration)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Err(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StartStationAccessPoint {
                                request_id,
                                request,
                            })
                            .is_err()
                        {
                            Err(RejectReason::Busy)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::StopStationAccessPoint(
                        oer_hil_protocol::wifi::StopStationAccessPoint,
                    ) => {
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::wifi::StationAccessPoint,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::StationAccessPoint)
                        {
                            Err(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StopStationAccessPoint { request_id })
                            .is_err()
                        {
                            Err(RejectReason::Busy)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                    Request::CaptureMonitor(oer_hil_protocol::wifi::CaptureMonitor(request)) => {
                        let valid = (1..=13).contains(&request.channel)
                            && request.snapshot_length <= 2_304
                            && (100..=30_000).contains(&request.duration_millis);
                        let response = if !crate::image_features::has::<
                            oer_hil_protocol::wifi::MonitorCapture,
                        >() {
                            Err(RejectReason::Unsupported)
                        } else if !valid {
                            Err(RejectReason::InvalidConfiguration)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Err(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::CaptureMonitor {
                                request_id,
                                request,
                            })
                            .is_err()
                        {
                            Err(RejectReason::Busy)
                        } else {
                            Ok(oer_hil_protocol::base::Accepted)
                        };
                        respond(session_id, request_id, response).await;
                    }
                }
            }
            Either::Second(result) => {
                let slot = sessions.iter().position(|slot| {
                    slot.is_some_and(|session| {
                        session.state == SessionState::Running
                            && session.active.session_id == result.session_id
                    })
                });
                if let Some(index) = slot {
                    transition_state(
                        &mut sessions[index]
                            .as_mut()
                            .expect("completed slot remains owned")
                            .state,
                        SessionState::Draining,
                        result.session_id,
                        0,
                    )
                    .await;
                    let retained = RetainedSessionResult {
                        measurement: result,
                        link: link_health_snapshot(),
                        stack: crate::stack_usage_snapshot().await,
                    };
                    if !retain_session_result(retained) {
                        publish_event_reliably(
                            result.session_id,
                            0,
                            oer_hil_protocol::network::Failed(FailureCode::EvidenceOverflow),
                        )
                        .await;
                        transition_state(
                            &mut sessions[index]
                                .as_mut()
                                .expect("failed slot remains owned")
                                .state,
                            SessionState::Failed,
                            result.session_id,
                            0,
                        )
                        .await;
                        continue;
                    }
                    publish_result(retained, 0).await;
                    transition_state(
                        &mut sessions[index]
                            .as_mut()
                            .expect("finished slot remains owned")
                            .state,
                        SessionState::Finished,
                        result.session_id,
                        0,
                    )
                    .await;
                } else {
                    publish_event_reliably(
                        result.session_id,
                        0,
                        oer_hil_protocol::base::Rejected(RejectReason::InvalidState),
                    )
                    .await;
                }
            }
        }
    }
}

fn valid_session_config(config: SessionConfig) -> bool {
    let direction_supported = match config.direction {
        Direction::Rx => crate::image_features::has::<oer_hil_protocol::network::Rx>(),
        Direction::Tx => crate::image_features::has::<oer_hil_protocol::network::Tx>(),
        Direction::Bidirectional => {
            crate::image_features::has::<oer_hil_protocol::network::Bidirectional>()
                && crate::image_features::has::<oer_hil_protocol::network::Rx>()
                && crate::image_features::has::<oer_hil_protocol::network::Tx>()
        }
    };
    let transport_valid = match config.transport {
        Transport::Udp => crate::image_features::has::<oer_hil_protocol::network::Udp>(),
        Transport::Tcp => crate::image_features::has::<oer_hil_protocol::network::Tcp>(),
    };
    transport_valid
        && direction_supported
        && config.structurally_valid(
            crate::limits::MAXIMUM_PAYLOAD_BYTES,
            crate::image_features::has::<oer_hil_protocol::network::UdpMultiFlow>(),
        )
}

async fn transition_state(
    state: &mut SessionState,
    current: SessionState,
    session_id: u64,
    request_id: u32,
) {
    let previous = *state;
    *state = current;
    publish_event_reliably(
        session_id,
        request_id,
        oer_hil_protocol::network::StateChanged(StateChange { previous, current }),
    )
    .await;
}

async fn publish_result(retained: RetainedSessionResult, request_id: u32) {
    let RetainedSessionResult {
        measurement: result,
        link,
        stack,
    } = retained;
    let mut evidence = heapless::Vec::<EvidenceRecord, 10>::new();
    evidence
        .push(EvidenceRecord::Transport(result.evidence))
        .expect("session evidence has fixed capacity");
    for flow in result.flow_evidence.iter().flatten().copied() {
        evidence
            .push(EvidenceRecord::FlowTransport(flow))
            .expect("session evidence has fixed capacity");
    }
    if let Some(radio) = result.radio {
        evidence
            .push(EvidenceRecord::Radio(radio))
            .expect("session evidence has fixed capacity");
    }
    if let Some(timing) = result.tx_timing {
        evidence
            .push(EvidenceRecord::TxAggregateTiming(timing))
            .expect("session evidence has fixed capacity");
    }
    if let Some(rx_delivery) = result.rx_delivery {
        evidence
            .push(EvidenceRecord::RxDelivery(rx_delivery))
            .expect("session evidence has fixed capacity");
    }
    if let Some(zero_copy) = result.rx_zero_copy {
        evidence
            .push(EvidenceRecord::RxZeroCopy(zero_copy))
            .expect("session evidence has fixed capacity");
    }
    if let Some(cpu_load) = result.cpu_load {
        evidence
            .push(EvidenceRecord::CpuLoad(cpu_load))
            .expect("session evidence has fixed capacity");
    }
    evidence
        .push(EvidenceRecord::Link(link))
        .expect("session evidence has fixed capacity");
    evidence
        .push(EvidenceRecord::Stack(stack))
        .expect("session evidence has fixed capacity");
    let checksum = evidence_crc32c(evidence.as_slice())
        .expect("transport and stack evidence fit the protocol digest buffer");
    let evidence_records = evidence.len() as u16;
    for record in evidence {
        publish_event_reliably(
            result.session_id,
            request_id,
            oer_hil_protocol::network::Evidence(record),
        )
        .await;
    }
    publish_event_reliably(
        result.session_id,
        request_id,
        Finished {
            summary: ResultSummary {
                verdict: session_verdict(result.verdict, &link),
                evidence_records,
            },
            evidence_crc32c: checksum,
        },
    )
    .await;
}

/// The session's verdict: the workload's own, then the control link's.
fn session_verdict(workload: SessionVerdict, link: &LinkHealth) -> SessionVerdict {
    if let SessionVerdict::Failed(_) = workload {
        return workload;
    }
    if link.rx_cobs_errors != 0
        || link.rx_checksum_errors != 0
        || link.rx_decode_errors != 0
        || link.rx_overflows != 0
        || link.tx_dropped != 0
    {
        return SessionVerdict::Failed(SessionFailure::ControlLink {
            cobs_errors: link.rx_cobs_errors,
            checksum_errors: link.rx_checksum_errors,
            decode_errors: link.rx_decode_errors,
            overflows: link.rx_overflows,
            tx_dropped: link.tx_dropped,
        });
    }
    SessionVerdict::Passed
}

async fn run_timebase_probe(request: TimebaseProbeRequest) -> TimebaseProbeEvidence {
    let started = Instant::now();
    let mut previous = started;
    let mut minimum_interval_micros = u32::MAX;
    let mut maximum_interval_micros = 0_u32;
    let mut early_intervals = 0_u16;
    for _ in 0..request.intervals {
        Timer::after_micros(u64::from(request.period_micros)).await;
        let now = Instant::now();
        let measured = now
            .duration_since(previous)
            .as_micros()
            .min(u64::from(u32::MAX)) as u32;
        minimum_interval_micros = minimum_interval_micros.min(measured);
        maximum_interval_micros = maximum_interval_micros.max(measured);
        early_intervals =
            early_intervals.saturating_add(u16::from(measured < request.period_micros));
        previous = now;
    }
    TimebaseProbeEvidence {
        intervals: request.intervals,
        period_micros: request.period_micros,
        elapsed_micros: Instant::now().duration_since(started).as_micros(),
        minimum_interval_micros,
        maximum_interval_micros,
        early_intervals,
    }
}

/// The image's requests, handed to [`protocol_task`] in order.
pub(crate) fn serve(request: oer_hil_protocol::RequestIdentity, command: Request) {
    // Armed before the command is queued, so the consumer taking it can
    // never be followed by a stale arm.
    #[cfg(not(feature = "memory-benchmark"))]
    crate::hang_watchdog::arm(oer_hil_protocol::base::TaskSlot::Console);
    let command = Envelope::new(
        request.boot_id,
        0,
        request.session_id,
        request.request_id,
        command,
    );
    if COMMANDS.try_send(command).is_err() {
        publish_event(
            request.session_id,
            request.request_id,
            oer_hil_protocol::base::Rejected(RejectReason::Busy),
        );
    }
}

/// Queues a typed event without making a radio or network task wait for USB.
pub fn publish_event<M: Message>(session_id: u64, request_id: u32, body: M) {
    CONSOLE.publish(session_id, request_id, &body);
}

/// Queue a control-plane event without allowing a full telemetry queue to
/// erase a required host/target state transition.
///
/// Use this for readiness and lifecycle boundaries outside measured traffic.
/// High-rate observations should continue to use [`publish_event`] so they
/// cannot apply backpressure to the radio or network hot path.
pub(crate) async fn publish_event_reliably<M: Message>(session_id: u64, request_id: u32, body: M) {
    CONSOLE
        .publish_reliably(session_id, request_id, &body)
        .await;
}

/// Publishes `body` reliably and returns its message sequence.
#[cfg(not(feature = "memory-benchmark"))]
async fn queue_event_reliably<M: Message>(session_id: u64, request_id: u32, body: M) -> u32 {
    CONSOLE
        .publish_reliably(session_id, request_id, &body)
        .await
}

/// Publishes `reply` to request `request_id` reliably: its response, or the
/// reason the request was refused. Returns the reply's message sequence.
pub(crate) async fn respond<M: Message>(
    session_id: u64,
    request_id: u32,
    reply: Result<M, RejectReason>,
) -> u32 {
    match reply {
        Ok(response) => {
            CONSOLE
                .publish_reliably(session_id, request_id, &response)
                .await
        }
        Err(reason) => {
            CONSOLE
                .publish_reliably(session_id, request_id, &Rejected(reason))
                .await
        }
    }
}

/// Whether the stacks may be sampled: the radio runs, idle, outside every
/// session.
fn stacks_observable(
    initialized: bool,
    state: SessionState,
    sessions: &[Option<ProtocolSession>],
    session_id: u64,
) -> bool {
    initialized
        && state == SessionState::Idle
        && sessions.iter().all(Option::is_none)
        && session_id == 0
}

fn link_health_snapshot() -> LinkHealth {
    CONSOLE.link_health()
}

/// The product image's console: the base module and the legacy commands.
#[embassy_executor::task]
pub async fn console_task(usb: crate::transport::Usb, boot: u64) {
    crate::transport::serve::<Request>(usb, boot, crate::limits::MAXIMUM_PAYLOAD_BYTES, serve).await
}
