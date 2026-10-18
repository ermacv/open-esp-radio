//! Bounded IEEE 802.15.4 diagnostic requests and observations.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Bounds for one IEEE 802.15.4 `EVENT_STATUS` observation probe.
///
/// `poll_limit` bounds every target-side wait loop. `timer_threshold` is the
/// target-defined timer separation used to make the two timer observations
/// distinct; it is intentionally a protocol value rather than an MMIO layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154EventStatusProbeRequest {
    pub poll_limit: u32,
    pub timer_threshold: u32,
}

impl Ieee802154EventStatusProbeRequest {
    /// Returns whether both probe bounds are finite and supported by the wire
    /// contract.
    pub const fn validate(self) -> bool {
        self.poll_limit >= 1
            && self.poll_limit <= 1_000_000
            && self.timer_threshold >= 1
            && self.timer_threshold <= 1_000
    }
}

/// Terminal observation reached by an IEEE 802.15.4 `EVENT_STATUS` probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154EventStatusProbeStop {
    Complete,
    UnsupportedSetup,
    RouteNotQuiesced,
    ResetNotClear,
    EventEnableReadbackMismatch,
    PostEnableStatusNotClear,
    TimerActivityTimeout,
    DualLatchTimeout,
    SelectiveAcknowledgeMismatch,
    DistinctFirstLatchTimeout,
    DistinctSecondLatchTimeout,
    CleanupNotClear,
}

/// Target-neutral semantic classification of one complete MAC event sample.
///
/// `UnexpectedNamed` retains a source-confirmed combination which is outside
/// the probe vocabulary. `Unclassified` retains a physical event for which no
/// reviewed semantic identity exists. Neither variant exposes register
/// positions or can be replayed as a write image.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154ObservedEventState {
    #[default]
    Clear,
    Timer0Only,
    Timer1Only,
    Timer0AndTimer1,
    EdDoneOnly,
    EdDoneAndTimer0,
    RxAbortOnly,
    RxAbortWithOther,
    EdDoneWithOther,
    EdDoneAndRxAbortWithOther,
    UnexpectedNamed,
    Unclassified,
}

/// Target-neutral semantic readback of a validation event window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154ValidationEventEnableState {
    #[default]
    AllMasked,
    TimerPairOnly,
    EdDoneTimer0RxAbortOnly,
    Unexpected,
}

/// Target-neutral semantic readback of the validation RX-abort window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154ValidationRxAbortEnableState {
    #[default]
    AllMasked,
    EdOperationReasonsOnly,
    Unexpected,
}

/// Target-neutral semantic readback of the fixed validation ED duration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154ValidationEdDurationState {
    ValidationEight,
    #[default]
    Other,
}

/// One source-confirmed receive-abort reason retained by HIL evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154RxAbortReason {
    RxStop,
    SfdTimeout,
    CrcError,
    InvalidLength,
    FilterFail,
    NoRss,
    CoexistenceBreak,
    UnexpectedAck,
    RxRestart,
    TxAckTimeout,
    TxAckStop,
    TxAckCoexistenceBreak,
    EnhancedAckSecurityError,
    EdAbort,
    EdStop,
    EdCoexistenceReject,
}

/// Semantic classification of one sampled receive-abort reason field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154RxAbortObservation {
    Named(Ieee802154RxAbortReason),
    Unclassified,
}

/// Semantic snapshots from one bounded IEEE 802.15.4 `EVENT_STATUS` probe.
///
/// This evidence is observation only. Even a [`Ieee802154EventStatusProbeStop::Complete`]
/// result does not prove same-bit concurrency, level-triggered retrigger
/// behavior, or readiness of a production interrupt path.
/// `dual_observed_events` is the union of the first bounded wait;
/// `dual_latched_events` is its terminal sample. `cleanup_pending_events` is
/// the observation after delivery is masked again; it may hide a retained
/// latch and is therefore not the source of the best-effort cleanup selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154EventStatusProbeEvidence {
    pub stop: Ieee802154EventStatusProbeStop,
    pub event_enable_before: Ieee802154ValidationEventEnableState,
    pub event_enable_active: Ieee802154ValidationEventEnableState,
    pub event_enable_after: Ieee802154ValidationEventEnableState,
    pub post_enable_events: Ieee802154ObservedEventState,
    pub timer0_value_before_start: u32,
    pub timer1_value_before_start: u32,
    pub timer0_value_min: u32,
    pub timer0_value_max: u32,
    pub timer1_value_min: u32,
    pub timer1_value_max: u32,
    pub timer0_value_after_stop: u32,
    pub timer1_value_after_stop: u32,
    pub reset_events: Ieee802154ObservedEventState,
    pub dual_observed_events: Ieee802154ObservedEventState,
    pub dual_latched_events: Ieee802154ObservedEventState,
    pub after_timer0_ack_events: Ieee802154ObservedEventState,
    pub after_timer1_ack_events: Ieee802154ObservedEventState,
    pub distinct_snapshot_events: Ieee802154ObservedEventState,
    pub distinct_before_ack_events: Ieee802154ObservedEventState,
    pub distinct_after_ack_events: Ieee802154ObservedEventState,
    pub cleanup_pending_events: Ieee802154ObservedEventState,
    pub final_events: Ieee802154ObservedEventState,
}

/// Bounds for one IEEE 802.15.4 ED-DONE/TIMER0 discriminator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154EdEventProbeRequest {
    pub poll_limit: u32,
    pub timer_threshold: u32,
}

impl Ieee802154EdEventProbeRequest {
    /// Return whether both target-side bounds are finite and supported.
    pub const fn validate(self) -> bool {
        self.poll_limit >= 1
            && self.poll_limit <= 1_000_000
            && self.timer_threshold >= 1
            && self.timer_threshold <= 1_000
    }
}

/// Terminal classification from the ED-DONE/TIMER0 discriminator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154EdEventProbeStop {
    Complete,
    ProductionEdFailed,
    UnsupportedSetup,
    RouteNotQuiesced,
    ResetNotClear,
    EdDurationReadbackMismatch,
    EventEnableReadbackMismatch,
    RxAbortEnableReadbackMismatch,
    PostEnableStatusNotClear,
    TimerActivityTimeout,
    PairLatchTimeout,
    EdAborted,
    UnexpectedEvent,
    SelectiveWriteMismatch,
    CleanupNotClear,
}

/// Checkpoint retained when a production polled ED invariant fails.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154PolledEdStage {
    Prepare,
    StartEventWindow,
    StartCommand,
    Poll,
    TerminalSample,
    AcknowledgeTerminalEvent,
    Cleanup,
}

/// Semantic enable-mask observation retained without exposing a writable
/// register image.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154PolledEdMaskState {
    AllMasked,
    OperationOnly,
    Unexpected,
}

/// Complete terminal evidence from one production polled ED attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154PolledEdOutcome {
    NotRun,
    Complete {
        rss_code: i8,
        polls: u32,
    },
    Aborted {
        event_status: Ieee802154ObservedEventState,
        rx_abort_reason: Ieee802154RxAbortObservation,
        polls: u32,
    },
    Timeout {
        polls: u32,
    },
    CpuInterruptRouteAttached {
        stage: Ieee802154PolledEdStage,
    },
    UnexpectedEventMask {
        stage: Ieee802154PolledEdStage,
        observed: Ieee802154PolledEdMaskState,
    },
    UnexpectedRxAbortMask {
        stage: Ieee802154PolledEdStage,
        observed: Ieee802154PolledEdMaskState,
    },
    StaleEventStatus {
        event_status: Ieee802154ObservedEventState,
    },
    UnexpectedTerminalStatus {
        event_status: Ieee802154ObservedEventState,
    },
    UnexpectedAcknowledgedEvents {
        event_status: Ieee802154ObservedEventState,
    },
    ConflictingTerminalEvents {
        event_status: Ieee802154ObservedEventState,
    },
}

/// Complete semantic evidence from one bounded ED-DONE/TIMER0 discriminator.
///
/// A successful result proves only the selected ED-DONE/TIMER0 relation in
/// this reset-isolated transaction; it is not a register-wide W1C or
/// production ED-readiness claim. `rx_abort_reason` is present only when
/// RX-ABORT was observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154EdEventProbeEvidence {
    pub stop: Ieee802154EdEventProbeStop,
    pub production_ed_first: Ieee802154PolledEdOutcome,
    pub production_ed_second: Option<Ieee802154PolledEdOutcome>,
    pub event_enable_before: Ieee802154ValidationEventEnableState,
    pub event_enable_active: Ieee802154ValidationEventEnableState,
    pub event_enable_after: Ieee802154ValidationEventEnableState,
    pub rx_abort_enable_before: Ieee802154ValidationRxAbortEnableState,
    pub rx_abort_enable_active: Ieee802154ValidationRxAbortEnableState,
    pub rx_abort_enable_after: Ieee802154ValidationRxAbortEnableState,
    pub ed_duration_before: Ieee802154ValidationEdDurationState,
    pub ed_duration_active: Ieee802154ValidationEdDurationState,
    pub ed_duration_after: Ieee802154ValidationEdDurationState,
    pub timer0_value_before_start: u32,
    pub timer0_value_min: u32,
    pub timer0_value_max: u32,
    pub timer0_value_after_stop: u32,
    pub reset_events: Ieee802154ObservedEventState,
    pub post_enable_events: Ieee802154ObservedEventState,
    pub observed_events: Ieee802154ObservedEventState,
    pub terminal_events: Ieee802154ObservedEventState,
    pub after_ed_done_write_events: Ieee802154ObservedEventState,
    pub after_timer0_write_events: Ieee802154ObservedEventState,
    pub cleanup_pending_events: Ieee802154ObservedEventState,
    pub final_events: Ieee802154ObservedEventState,
    pub rx_abort_reason: Option<Ieee802154RxAbortObservation>,
    pub stop_command_issued: bool,
    pub cleanup_clear: bool,
}

/// Most start/stop cycles one air check runs.
pub const IEEE802154_AIR_CHECK_MAX_CYCLES: usize = 4;

/// Bounds for one single-device IEEE 802.15.4 on-air check.
///
/// Each cycle starts the IEEE 802.15.4 client on the shared radio arbiter,
/// runs one energy scan, one clear-channel assessment, one direct transmit
/// without an acknowledgement request, two scheduled transmits, one receive
/// window and one scheduled receive window on `channel`, then stops the
/// client again.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154AirCheckRequest {
    /// IEEE 802.15.4 channel, 11 through 26.
    pub channel: u8,
    /// Start/stop cycles, 1 through [`IEEE802154_AIR_CHECK_MAX_CYCLES`].
    pub cycles: u8,
    /// Energy-scan duration in microseconds.
    pub energy_scan_micros: u32,
    /// Receive-window length in milliseconds.
    pub receive_window_millis: u32,
    /// Lead from the request to each scheduled transmit start and to the
    /// scheduled receive window, in microseconds.
    pub scheduled_lead_micros: u32,
    /// Scheduled receive window length in microseconds.
    pub scheduled_window_micros: u32,
}

impl Ieee802154AirCheckRequest {
    /// Returns whether every bound is inside the wire contract.
    pub const fn validate(self) -> bool {
        self.channel >= 11
            && self.channel <= 26
            && self.cycles >= 1
            && self.cycles as usize <= IEEE802154_AIR_CHECK_MAX_CYCLES
            && self.energy_scan_micros >= 128
            && self.energy_scan_micros <= 1_000_000
            && self.receive_window_millis >= 1
            && self.receive_window_millis <= 10_000
            && self.scheduled_lead_micros >= 1_000
            && self.scheduled_lead_micros <= 1_000_000
            && self.scheduled_window_micros >= 1_000
            && self.scheduled_window_micros <= 1_000_000
    }
}

/// Where an air check stopped.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154AirCheckStop {
    /// Every cycle completed.
    Complete,
    /// The image could not claim the radio.
    #[default]
    UnsupportedSetup,
    /// Bring-up of the IEEE 802.15.4 client failed.
    StartFailed,
    /// The runtime rejected a command.
    CommandRejected,
    /// No terminal event arrived in time.
    EventTimeout,
    /// The runtime's event queue overflowed.
    EventsLost,
    /// A terminal event did not match the command.
    UnexpectedEvent,
    /// Teardown of the IEEE 802.15.4 client failed.
    StopFailed,
}

/// Outcome of one energy scan.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154AirEnergyOutcome {
    #[default]
    NotRun,
    /// Averaged channel energy in dBm.
    Energy(i8),
    Failed,
}

/// Outcome of one clear-channel assessment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154AirCcaOutcome {
    #[default]
    NotRun,
    Clear,
    Busy,
    Failed,
}

/// Terminal status of one transmit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154AirTxOutcome {
    #[default]
    NotRun,
    Success,
    ChannelBusy,
    NoAcknowledgement,
    Aborted,
    InvalidFrame,
    HardwareFailure,
    CoexistenceRejected,
    SecurityFailure,
    InvalidAcknowledgement,
}

/// One transmit and its completion time.
///
/// Times are target-monotonic microseconds. A direct transmit's
/// `requested_at_micros` is the submission time; a scheduled transmit's is the
/// requested start.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154AirTransmit {
    pub outcome: Ieee802154AirTxOutcome,
    pub requested_at_micros: u64,
    pub done_at_micros: u64,
}

/// One scheduled receive window and its end.
///
/// Times are target-monotonic microseconds; `start_micros` and `end_micros`
/// are the requested window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154AirWindow {
    /// The window's end was reported.
    pub ended: bool,
    pub start_micros: u64,
    pub end_micros: u64,
    /// When the target observed the window's end.
    pub done_at_micros: u64,
    /// Frames received in the window.
    pub received_frames: u16,
    /// Start-of-frame time of the first frame received in the window.
    pub first_frame_at_micros: Option<u64>,
}

/// Observations of one start/stop cycle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154AirCycle {
    pub energy: Ieee802154AirEnergyOutcome,
    pub cca: Ieee802154AirCcaOutcome,
    pub direct: Ieee802154AirTransmit,
    pub scheduled: [Ieee802154AirTransmit; 2],
    /// Frames received during the receive window.
    pub received_frames: u16,
    /// Strongest receive RSSI in dBm, when any frame arrived.
    pub strongest_rssi_dbm: Option<i8>,
    /// The scheduled receive window.
    pub scheduled_window: Ieee802154AirWindow,
}

/// Terminal observation of one air check.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154AirCheckEvidence {
    pub stop: Ieee802154AirCheckStop,
    /// Cycles that completed, including teardown.
    pub completed_cycles: u8,
    pub cycles: [Ieee802154AirCycle; IEEE802154_AIR_CHECK_MAX_CYCLES],
}

/// Largest MAC frame a session carries: a 127-byte PSDU without its FCS.
pub const IEEE802154_SESSION_FRAME_CAPACITY: usize = 125;
/// Received frames one collection reports individually.
pub const IEEE802154_SESSION_RECORDED_FRAMES: usize = 16;

/// MAC bytes of one frame, without PHR and FCS.
pub type Ieee802154SessionFrame = heapless::Vec<u8, IEEE802154_SESSION_FRAME_CAPACITY>;

/// Identity and filtering of the device under test in a peer session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionConfig {
    /// IEEE 802.15.4 channel, 11 through 26.
    pub channel: u8,
    pub pan_id: u16,
    pub short_address: u16,
    /// Extended address in over-the-air byte order.
    pub extended_address: [u8; 8],
    pub promiscuous: bool,
    /// Track the shared PHY in the background once per tracking period,
    /// instead of on `MaintainISessionPhy` requests.
    pub background_maintenance: bool,
    /// Answer 2015 frames that request an ACK with an unsecured enhanced
    /// ACK; otherwise they get no ACK.
    pub enhanced_ack: bool,
    /// Take part in coexistence with Wi-Fi for the session, as a Thread
    /// border router does (`esp_coex_wifi_i154_enable`), with the
    /// coexistence schedule running.
    pub wifi_coexistence: bool,
}

/// Coexistence of one session with Wi-Fi.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionCoexistence {
    /// The session took part in coexistence.
    pub enabled: bool,
    /// Leaving coexistence at the stop failed.
    pub disable_failed: bool,
}

impl Ieee802154SessionConfig {
    /// Returns whether the configuration is inside the wire contract.
    pub const fn validate(self) -> bool {
        self.channel >= 11 && self.channel <= 26
    }
}

/// One session transmission.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionTransmitRequest {
    /// MAC bytes; the frame's acknowledgement-request bit selects whether an
    /// acknowledgement is awaited.
    pub frame: Ieee802154SessionFrame,
    /// How the transmission acquires the channel.
    pub mode: Ieee802154SessionTxMode,
    /// Retransmissions after an attempt without acknowledgement or channel
    /// access; zero sends the frame once.
    pub max_frame_retries: u8,
}

/// How one session transmission acquires the channel.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154SessionTxMode {
    /// Transmit at once.
    #[default]
    Direct,
    /// One clear-channel assessment first.
    ClearChannelAssessment,
    /// Unslotted CSMA-CA with up to `max_backoffs` backoffs after a busy
    /// channel.
    CsmaCa {
        /// Backoffs after a busy channel before reporting it busy.
        max_backoffs: u8,
    },
}

impl Ieee802154SessionTransmitRequest {
    /// Returns whether the frame holds at least a frame control field.
    pub fn validate(&self) -> bool {
        self.frame.len() >= 3
    }
}

/// Energy scan and clear-channel assessment of one channel in a session.
///
/// Each runs as its own radio command on `channel`, so the radio retunes to
/// it and back to the session channel for the next transmit or receive.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionAssessRequest {
    /// IEEE 802.15.4 channel, 11 through 26.
    pub channel: u8,
    /// Energy-scan duration in microseconds, 128 through 1 000 000.
    pub energy_scan_micros: u32,
}

impl Ieee802154SessionAssessRequest {
    /// Returns whether the request is inside the wire contract.
    pub const fn validate(self) -> bool {
        self.channel >= 11
            && self.channel <= 26
            && self.energy_scan_micros >= 128
            && self.energy_scan_micros <= 1_000_000
    }
}

/// Outcome of one channel assessment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionAssessment {
    pub result: Ieee802154SessionResult,
    pub energy: Ieee802154AirEnergyOutcome,
    pub cca: Ieee802154AirCcaOutcome,
}

/// Automatic frame-pending decision of the device under test.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154SessionPendingMode {
    Disabled,
    Enabled,
    Enhanced,
    Zigbee,
}

/// Change the pending mode and optionally add one short address.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionPendingRequest {
    pub mode: Ieee802154SessionPendingMode,
    pub short_address: Option<u16>,
}

/// Outcome of one session step.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154SessionResult {
    Done,
    /// The image could not claim the radio.
    #[default]
    UnsupportedSetup,
    StartFailed,
    StopFailed,
    CommandRejected,
    EventTimeout,
    EventsLost,
    UnexpectedEvent,
}

/// The acknowledgement a session transmission received.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionAck {
    pub frame: Ieee802154SessionFrame,
    pub rssi_dbm: i8,
    pub lqi: u8,
}

/// Terminal observation of one session transmission.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionTransmitEvidence {
    pub result: Ieee802154SessionResult,
    pub outcome: Ieee802154AirTxOutcome,
    pub acknowledgement: Option<Ieee802154SessionAck>,
}

/// One received frame, identified by the digest of its MAC bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionReceivedFrame {
    pub length: u8,
    /// `i_frame_crc32c` of the MAC bytes.
    pub crc32c: u32,
    pub rssi_dbm: i8,
    pub lqi: u8,
}

/// Frames received since the previous collection.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionReceiveEvidence {
    pub result: Ieee802154SessionResult,
    /// Every frame received, including those not recorded individually.
    pub total: u16,
    pub frames: heapless::Vec<Ieee802154SessionReceivedFrame, IEEE802154_SESSION_RECORDED_FRAMES>,
}

/// Outcome of restarting the radio of a running session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionRestartEvidence {
    pub result: Ieee802154SessionResult,
    /// The client's stop closed RF, as the last shared PHY client.
    pub rf_closed: bool,
}

/// The live RSSI a running session read.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionRecentRssi {
    pub result: Ieee802154SessionResult,
    /// The signed RSSI in dBm of the most recent baseband reception.
    pub rssi_dbm: i8,
}

/// Outcome of one PHY maintenance request in a session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154SessionPhyMaintenance {
    /// No tracking was due.
    NotDue,
    /// Tracking ran with the radio running.
    Tracked,
    /// The domain rejected the maintenance or tracking failed.
    #[default]
    Failed,
}

/// Outcomes of the background PHY maintenance of one session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionMaintenanceCounts {
    pub not_due: u16,
    pub tracked: u16,
    /// An attempt failed and background maintenance ended.
    pub failed: bool,
}

/// Terminal observation of one session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154SessionStopEvidence {
    pub result: Ieee802154SessionResult,
    /// Background maintenance outcomes; all zero without it.
    pub maintenance: Ieee802154SessionMaintenanceCounts,
    /// Coexistence with Wi-Fi; all false without it.
    pub coexistence: Ieee802154SessionCoexistence,
}

/// Active operational dataset TLVs. A dataset OpenThread creates takes about
/// 110 bytes; this leaves room for the optional TLVs while the command stays
/// within the embedded command queue's budget, below OpenThread's 254-byte
/// `OT_OPERATIONAL_DATASET_MAX_LENGTH`.
pub const IEEE802154_THREAD_DATASET_CAPACITY: usize = 224;
/// UDP payload bytes one Thread session message carries.
pub const IEEE802154_THREAD_PAYLOAD_CAPACITY: usize = 128;
/// Datagrams one collection reports individually.
pub const IEEE802154_THREAD_RECORDED_DATAGRAMS: usize = 3;

/// Active operational dataset TLVs.
pub type Ieee802154ThreadDataset = heapless::Vec<u8, IEEE802154_THREAD_DATASET_CAPACITY>;
/// One UDP payload.
pub type Ieee802154ThreadPayload = heapless::Vec<u8, IEEE802154_THREAD_PAYLOAD_CAPACITY>;

/// Join a Thread network with OpenThread over the composed client.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154ThreadStartRequest {
    /// The network's active operational dataset.
    pub dataset: Ieee802154ThreadDataset,
    /// Keep the receiver on when idle: a minimal end device; otherwise a
    /// sleepy one.
    pub rx_on_when_idle: bool,
    /// The UDP port the device's socket binds.
    pub udp_port: u16,
}

impl Ieee802154ThreadStartRequest {
    /// Returns whether the request is inside the wire contract.
    pub fn validate(&self) -> bool {
        !self.dataset.is_empty() && self.udp_port != 0
    }
}

/// OpenThread's device role.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154ThreadRole {
    #[default]
    Disabled,
    Detached,
    Child,
    Router,
    Leader,
    Other,
}

/// The device's Thread interface.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154ThreadState {
    pub result: Ieee802154SessionResult,
    pub role: Ieee802154ThreadRole,
    pub rloc16: u16,
    /// The mesh-local EID, once the device has one.
    pub mesh_local_eid: Option<[u8; 16]>,
}

/// Send one datagram from the device's socket.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154ThreadSendRequest {
    pub destination: [u8; 16],
    pub port: u16,
    pub payload: Ieee802154ThreadPayload,
}

impl Ieee802154ThreadSendRequest {
    /// Returns whether the request is inside the wire contract.
    pub fn validate(&self) -> bool {
        self.port != 0 && !self.payload.is_empty()
    }
}

/// One datagram the device's socket received.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154ThreadDatagram {
    pub source: [u8; 16],
    pub port: u16,
    pub payload: Ieee802154ThreadPayload,
}

/// Datagrams received since the previous collection.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154ThreadReceiveEvidence {
    pub result: Ieee802154SessionResult,
    /// Every datagram received, including those not recorded individually.
    pub total: u16,
    pub datagrams: heapless::Vec<Ieee802154ThreadDatagram, IEEE802154_THREAD_RECORDED_DATAGRAMS>,
}

/// Entries one routed phase of the route probe records.
pub const IEEE802154_ROUTE_PROBE_MAX_ENTRIES: usize = 4;

/// Timing of one IEEE 802.15.4 route probe: the MAC timers fire
/// `threshold_micros` after their start, and each wait for a second arrival
/// lasts `settle_micros`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154RouteProbeRequest {
    pub threshold_micros: u32,
    pub settle_micros: u32,
}

impl Ieee802154RouteProbeRequest {
    /// Returns whether the timing is inside the wire contract: a threshold
    /// of 1 µs to 10 ms and a settle of at least four thresholds, at most
    /// 100 ms.
    pub const fn validate(self) -> bool {
        self.threshold_micros >= 1
            && self.threshold_micros <= 10_000
            && self.settle_micros <= 100_000
            && self.settle_micros >= self.threshold_micros.saturating_mul(4)
    }
}

/// Why a route probe ended.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154RouteProbeStop {
    Complete,
    /// The image could not claim or bring up the MAC.
    #[default]
    UnsupportedSetup,
    /// The event field was not clear before a phase.
    NotClear,
    /// A timer event did not latch within the bound.
    LatchTimeout,
    /// A phase observed an event it did not raise.
    UnexpectedEvent,
    /// The CPU route could not be bound or quiesced.
    RouteFailed,
}

/// What acknowledging the first arrival left of a second arrival of the
/// same, still latched event.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Ieee802154SameBitOutcome {
    #[default]
    NotRun,
    /// The second arrival was cleared with the first.
    Coalesced,
    /// The bit stayed latched after the acknowledgement.
    Retained,
}

/// One entry of the validation ISR.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154RouteProbeEntry {
    /// The snapshot the entry sampled and consumed.
    pub snapshot: Ieee802154ObservedEventState,
    /// The field just before the snapshot was consumed.
    pub before_acknowledgement: Ieee802154ObservedEventState,
}

/// Terminal observation of one route probe.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ieee802154RouteProbeEvidence {
    pub stop: Ieee802154RouteProbeStop,
    /// Polled phase, the route unbound: the first TIMER0 snapshot, the field
    /// after acknowledging it once TIMER0 fired again, and the control
    /// arrival after a restart that followed the acknowledgement.
    pub polled_snapshot: Ieee802154ObservedEventState,
    pub polled_after_acknowledgement: Ieee802154ObservedEventState,
    pub polled_control: Ieee802154ObservedEventState,
    pub polled_outcome: Ieee802154SameBitOutcome,
    /// Routed phase: the first entry raises TIMER1 after its TIMER0 snapshot.
    pub retrigger_entries:
        heapless::Vec<Ieee802154RouteProbeEntry, IEEE802154_ROUTE_PROBE_MAX_ENTRIES>,
    /// Routed phase: the first entry lets TIMER0 fire again after its
    /// snapshot.
    pub same_bit_entries:
        heapless::Vec<Ieee802154RouteProbeEntry, IEEE802154_ROUTE_PROBE_MAX_ENTRIES>,
    /// The field consumed after the last phase.
    pub final_events: Ieee802154ObservedEventState,
}

/// Digest of one IEEE 802.15.4 MAC frame reported by a session, so both the
/// target and the host identify frame bytes without carrying them.
pub fn ieee802154_frame_crc32c(bytes: &[u8]) -> u32 {
    crate::framing::crc32c(bytes)
}
