//! Executor-independent control state for one connected ESP32-S31 station.
//!
//! This module owns station BlockAck protocol, beacon-loss and power-save
//! transitions. The finite core does not own the physical RX BlockAck banks:
//! its caller supplies the one VIF-aware bank owner shared by STA and AP. A
//! runtime adapter supplies at most one received control event, a bounded
//! reorder-command sink and the shared TX owner.  No mailbox, executor timer
//! or task wakeup is part of this state machine.

use oer_ieee80211_mac::block_ack::TX_BLOCK_ACK_MAX_TIDS;
use oer_ieee80211_trace::{
    BeaconMonitorOp, BeaconMonitorTrace, BlockAckDirection, ExitReason, LinkControlTrace, LinkEvent,
};

use crate::{
    connected_rx::{AssociatedHeControlIdentity, ConnectedRxControlEvent},
    ftm::{StationFtmHardwareError, station_ftm_request_frontier},
    hardware::control::{ConnectedControlHardware, StationIndividualTwtHardwareError},
    modem_sleep::{PmActions, PmBeacon, PmTim},
    single_mpdu_tx::{ActionTxConfig, SingleMpduTx, SingleMpduTxError, SingleMpduTxOutcome},
};
use oer_ieee80211_mac::sequence::SequenceNumber;

use oer_esp32s31_ieee80211::{
    datapath::{DatapathControlContext, DatapathControlProgress},
    station_tsf::{StationTsf, StationTsfHardware},
};

use oer_esp32s31_ieee80211_mac::{
    MacInterface,
    rx::{
        ampdu::{
            RxBlockAckActivation, RxBlockAckRequest, RxBlockAckSessions, RxBlockAckSessionsError,
            RxReorderCommand, RxReorderCommandError,
        },
        hardware::S31RxBlockAckAgreementError,
    },
    tx::{
        HeTriggerBasedTxConfig, HeTriggerScheduledRate, HeTriggerScheduledRateError, TxHardware,
        ampdu::{BlockAckAction, TxBlockAckResponse},
        protection::{BssProtection, HeTxopDurationRtsThreshold},
    },
};

use oer_ieee80211_mac::{
    management_protection::SaQuery,
    station::{StaDisconnect, StaDisconnectKind},
    station_beacon::StaBeaconProtection,
    station_power_save::StaPowerManagement,
    trigger::TriggerCommonInfo,
    twt::{INDIVIDUAL_TWT_FLOW_CAPACITY, IndividualTwtAction, IndividualTwtFlowId},
};

use oer_espressif_ieee80211_policy::block_ack::STA_TX_BLOCK_ACK_TIDS;
use oer_ieee80211_mac::block_ack::{
    TxBlockAckOriginator, TxBlockAckOriginatorError, TxBlockAckOriginatorResponse,
    TxBlockAckResponseDisposition, TxBlockAckRetry,
};
use oer_ieee80211_sta::{
    ftm::{
        FtmRequester, FtmRequesterConfig, FtmRequesterError, FtmRequesterEvent, FtmRequesterService,
    },
    link_monitor::{
        StaBeaconLossConfig, StaBeaconLossConfigError, StaBeaconMonitor, StaLinkAction,
        StaLinkMonitor,
    },
    twt::{
        IndividualTwtAgreement, IndividualTwtProposal, IndividualTwtRequester,
        IndividualTwtRequesterConfig, IndividualTwtRequesterError, IndividualTwtRequesterEvent,
        IndividualTwtService, IndividualTwtSetupDisposition, IndividualTwtTransmission,
        IndividualTwtTxKind, IndividualTwtWakePlan, IndividualTwtWakePlanError,
    },
};

// The wire field allows 31 FTM frames; the initial frame has no predecessor,
// so a single ASAP burst can return at most 30 complete exchanges.
const CONNECTED_FTM_FRONTIER_SAMPLE_CAPACITY: usize = 30;

use oer_ieee80211_sta::time::earliest as earliest_deadline;

/// Protocol reason which ended one connected station epoch.
///
/// This remains below the public radio facade: applications normally need
/// only link state, while the station lifecycle and qualification harness need
/// the exact cause in order to choose and verify reconnect policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedDisconnectReason {
    BeaconLoss,
    PeerDeauthentication {
        reason_code: u16,
    },
    PeerDisassociation {
        reason_code: u16,
    },
    /// The executor-side bounded mailbox lost a semantic control event.
    /// Continuing would make the connected protocol state unknowable, so the
    /// complete station epoch must be torn down and rebuilt.
    ControlMailboxOverflow,
    ActiveStateRestoreFailed,
    GroupKeyHandshakeFailed,
    /// The access point did not answer the SA Query that an unprotected
    /// disconnect started, so the association no longer holds.
    SaQueryTimeout,
    /// The join beacon's TSF, advanced to the start of power management,
    /// crosses 2^64: the station cannot follow the access point's TSF.
    JoinTsfUnrepresentable,
}

impl From<ConnectedDisconnectReason> for ExitReason {
    fn from(reason: ConnectedDisconnectReason) -> Self {
        match reason {
            ConnectedDisconnectReason::BeaconLoss => Self::BeaconLoss,
            ConnectedDisconnectReason::PeerDeauthentication { reason_code } => {
                Self::PeerDeauthentication { reason_code }
            }
            ConnectedDisconnectReason::PeerDisassociation { reason_code } => {
                Self::PeerDisassociation { reason_code }
            }
            ConnectedDisconnectReason::ControlMailboxOverflow => Self::ControlMailboxOverflow,
            ConnectedDisconnectReason::ActiveStateRestoreFailed => Self::ActiveStateRestoreFailed,
            ConnectedDisconnectReason::GroupKeyHandshakeFailed => Self::GroupKeyHandshakeFailed,
            ConnectedDisconnectReason::SaQueryTimeout => Self::SaQueryTimeout,
            ConnectedDisconnectReason::JoinTsfUnrepresentable => Self::JoinTsfUnrepresentable,
        }
    }
}

impl ConnectedDisconnectReason {
    /// Whether the access point ended the association for a reason that
    /// invalidates its PMKSA: an expired authentication, a class 2 or 3
    /// frame, a four-way handshake timeout, or an invalid PMKID, MDE or FTE.
    ///
    /// SOURCE: ESP-IDF ``
    /// `components/wpa_supplicant/esp_supplicant/src/esp_wpa_main.c`
    /// (`wpa_sta_disconnected_cb`, which clears the current PMKSA for these
    /// reasons and keeps it for every other).
    pub const fn forgets_pmksa(self) -> bool {
        match self {
            Self::PeerDeauthentication { reason_code }
            | Self::PeerDisassociation { reason_code } => {
                matches!(reason_code, 2 | 6 | 7 | 15 | 49 | 50 | 51)
            }
            _ => false,
        }
    }
}

impl From<StaDisconnect> for ConnectedDisconnectReason {
    fn from(disconnect: StaDisconnect) -> Self {
        match disconnect.kind {
            StaDisconnectKind::Deauthentication => Self::PeerDeauthentication {
                reason_code: disconnect.reason_code,
            },
            StaDisconnectKind::Disassociation => Self::PeerDisassociation {
                reason_code: disconnect.reason_code,
            },
        }
    }
}

/// Reason code of the Deauthentication a leaving station sends
/// (IEEE 802.11 reason 3, the station is leaving).
const DEAUTHENTICATION_REASON_LEAVING: u16 = 3;

/// Control frame currently owning the shared ordinary TX transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedControlTxKind {
    RxAddbaResponse {
        tid: u8,
    },
    TxAddbaRequest {
        tid: u8,
    },
    BeaconProbe,
    PowerManagement(StaPowerManagement),
    IndividualTwt {
        flow_id: IndividualTwtFlowId,
        kind: IndividualTwtTxKind,
    },
    SaQuery,
    Deauthentication,
}

/// Association-scoped observable outcome of the portable TWT requester.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedIndividualTwtRuntimeOutcome {
    Protocol(IndividualTwtRequesterEvent),
    Response(IndividualTwtSetupDisposition),
    AgreementInstalled(IndividualTwtAgreement),
    PeerTeardown,
    HardwareRejected {
        flow_id: IndividualTwtFlowId,
        error: StationIndividualTwtHardwareError,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedIndividualTwtRuntimeEvidence {
    pub actions_received: u32,
    pub actions_published: u32,
    pub agreements_installed: u32,
    pub hardware_rejections: u32,
    pub last_outcome: Option<ConnectedIndividualTwtRuntimeOutcome>,
}

/// One production connected-control admission result for an FTM request.
///
/// This evidence intentionally omits the encoded Action body. It cannot be
/// reused as TX authority after the portable requester has consumed the
/// rejected transmission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedFtmRequestFrontier {
    pub peer: [u8; 6],
    pub session_generation: u32,
    pub transmission_generation: u32,
    pub attempt: u8,
    pub protocol_event: FtmRequesterEvent,
    pub hardware_error: StationFtmHardwareError,
}

/// Validated Basic-Trigger request crossing from connected control into the
/// unique aggregate/network TX owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeTriggerRuntimeRequest {
    pub identity: AssociatedHeControlIdentity,
    pub common: TriggerCommonInfo,
    pub schedule: HeTriggerScheduledRate,
    pub first_user: Option<[u8; 5]>,
    /// The monotonic time of the event frame's first executor handoff, which
    /// the runtime handoff window counts from.
    pub handoff: oer_time::Instant,
    /// The handoff plus the configured runtime handoff window.
    pub response_deadline: oer_time::Instant,
    pub queue_policy: HeTriggerBasedTxConfig,
}

/// Validated addressed HE-NDPA request crossing into the shared TX owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeNdpaRuntimeRequest {
    pub identity: AssociatedHeControlIdentity,
    pub dialog_token: u8,
    /// The monotonic time of the event frame's first executor handoff, which
    /// the runtime handoff window counts from.
    pub handoff: oer_time::Instant,
    /// The handoff plus the configured runtime handoff window.
    pub response_deadline: oer_time::Instant,
}

/// Fail-closed reason why one HE control event did not become an owned TX
/// publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
// CAPABILITY: wifi-802-11ax-he-uplink-ofdma, wifi-802-11ax-he-su-mu-beamformee
pub enum ConnectedHeControlRuntimeRejection {
    HeAssociationUnavailable,
    RuntimeDisabled,
    RuntimeHandoffWindowUnavailable,
    RuntimeTimestampUnavailable,
    ResponseDeadlineOverflow,
    MissedResponseWindow,
    PowerSaveWakeRequired,
    TriggerSchedule(HeTriggerScheduledRateError),
    NdpaNotAddressed,
    QueueOwnerUnavailable,
    QueuePolicyMismatch,
    QueueTidMismatch,
    TxOwnerBusy,
    PreparedQueueUnavailable,
    PreparedQueueFaulted,
    UnsupportedQueueFormat,
    UnsupportedQueueGeometry,
    /// Queue/MPLEN/BSR programming is reviewed, but no reviewed input proves
    /// the S31 HE-TB PHY vector/doorbell publication contract.
    TbPhyPublicationUnverified,
    /// RX detection is reviewed, but no software HE beamforming feedback
    /// formatter/publication contract is available.
    NdpaFeedbackPublicationUnverified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedHeControlRuntimeOutcome {
    TriggerPublished {
        identity: AssociatedHeControlIdentity,
        schedule: HeTriggerScheduledRate,
    },
    TriggerRejected {
        identity: AssociatedHeControlIdentity,
        reason: ConnectedHeControlRuntimeRejection,
    },
    NdpaPublished {
        identity: AssociatedHeControlIdentity,
        dialog_token: u8,
    },
    NdpaRejected {
        identity: AssociatedHeControlIdentity,
        dialog_token: u8,
        reason: ConnectedHeControlRuntimeRejection,
    },
}

/// Association-scoped bounded HE-control handoff telemetry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedHeControlRuntimeEvidence {
    pub triggers_observed: u32,
    pub ndpa_observed: u32,
    pub tx_handoffs: u32,
    pub tx_publications: u32,
    pub rejected: u32,
    pub last_outcome: Option<ConnectedHeControlRuntimeOutcome>,
}

enum ControlInFlight {
    RxAddba(RxBlockAckActivation),
    TxAddba { tid: u8 },
    BeaconProbe,
    PowerManagement(StaPowerManagement),
    IndividualTwt(IndividualTwtTransmission),
    SaQuery,
    Deauthentication,
}

impl ControlInFlight {
    fn kind(&self) -> ConnectedControlTxKind {
        match self {
            Self::RxAddba(activation) => ConnectedControlTxKind::RxAddbaResponse {
                tid: activation.negotiated().tid,
            },
            Self::TxAddba { tid } => ConnectedControlTxKind::TxAddbaRequest { tid: *tid },
            Self::BeaconProbe => ConnectedControlTxKind::BeaconProbe,
            Self::PowerManagement(mode) => ConnectedControlTxKind::PowerManagement(*mode),
            Self::IndividualTwt(transmission) => ConnectedControlTxKind::IndividualTwt {
                flow_id: transmission.flow_id,
                kind: transmission.kind,
            },
            Self::SaQuery => ConnectedControlTxKind::SaQuery,
            Self::Deauthentication => ConnectedControlTxKind::Deauthentication,
        }
    }
}

/// Runtime-neutral sink for semantic RX reorder commands.
pub trait ConnectedControlReorder {
    fn publish(&mut self, command: RxReorderCommand) -> Result<(), RxReorderCommandError>;
}

/// Mutually borrowed capabilities used by one finite connected-control step.
/// Grouping them makes the ownership boundary explicit and prevents event and
/// scheduling policy arguments from being confused with hardware owners.
pub struct ConnectedControlPorts<'a, H, X, R, const PEER_CAPACITY: usize> {
    pub hardware: &'a mut H,
    pub tx: &'a mut X,
    pub reorder: &'a mut R,
    pub rx_block_ack: &'a mut RxBlockAckSessions<PEER_CAPACITY>,
}

/// Shared ordinary-TX capability consumed by connected control. Its clock
/// is the transmitter's, the image's monotonic time.
pub trait ConnectedControlTx: oer_time::Clock {
    fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome>;

    /// Whether network data TX happened since the last
    /// [`Self::take_network_tx_report`]. An owner without network data has
    /// nothing to report.
    fn has_network_tx_report(&self) -> bool {
        false
    }

    /// Take the network data TX report for power management.
    fn take_network_tx_report(&mut self) -> NetworkTxPowerReport {
        NetworkTxPowerReport::default()
    }

    fn peek_qos_sequence(&self, tid: u8) -> Option<SequenceNumber>;

    fn start_action<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError>;

    /// Publish the Deauthentication of the leaving station.
    fn start_deauthentication<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        reason_code: u16,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError>;

    /// Publish one access-point reachability Probe Request: to the access
    /// point when `directed`, broadcast otherwise.
    fn start_beacon_probe<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        directed: bool,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError>;

    fn start_power_management_null<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError>;

    fn start_protected_eapol<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        payload: &[u8],
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError>;

    /// Publish the negotiated TX BlockAck agreement for one TID.
    ///
    /// `None` stops aggregation. Keeping the exact negotiated window here is
    /// essential: an operational boolean cannot prevent the data path from
    /// publishing more MPDUs than the peer's reorder window can retain.
    fn set_tx_block_ack_agreement(&mut self, tid: u8, agreement: Option<(u16, bool)>);

    /// BSS protection facts every later publication selects from.
    fn bss_protection(&self) -> BssProtection;

    /// Replace the BSS protection facts after the BSS changed them.
    fn install_bss_protection(&mut self, protection: BssProtection);

    /// Hand one validated Trigger response to the unique aggregate/network
    /// owner. A successful return means physical TX ownership was published;
    /// implementations without that exact contract must return a typed reason.
    fn publish_he_trigger_response<H: TxHardware>(
        &mut self,
        _hardware: &mut H,
        _request: HeTriggerRuntimeRequest,
    ) -> Result<(), ConnectedHeControlRuntimeRejection> {
        Err(ConnectedHeControlRuntimeRejection::QueueOwnerUnavailable)
    }

    /// Hand one validated NDPA feedback request to the unique TX owner.
    fn publish_he_ndpa_feedback<H: TxHardware>(
        &mut self,
        _hardware: &mut H,
        _request: HeNdpaRuntimeRequest,
    ) -> Result<(), ConnectedHeControlRuntimeRejection> {
        Err(ConnectedHeControlRuntimeRejection::QueueOwnerUnavailable)
    }
}

impl<P, E, T, const BUFFER_SIZE: usize> ConnectedControlTx
    for SingleMpduTx<'_, P, E, T, BUFFER_SIZE>
where
    P: oer_esp32s31_ieee80211::ordinary_tx::WifiTxPowerProfile,
    E: oer_esp32s31_ieee80211::ordinary_tx::WifiTxEntropy,
    T: oer_time::Timer,
{
    fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome> {
        SingleMpduTx::take_last_outcome(self)
    }

    fn start_beacon_probe<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        directed: bool,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        SingleMpduTx::start_beacon_probe(self, hardware, directed)
            .map(|_| DatapathControlProgress::TxPending)
    }

    fn peek_qos_sequence(&self, tid: u8) -> Option<SequenceNumber> {
        SingleMpduTx::peek_qos_sequence(self, tid)
    }

    fn start_action<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        SingleMpduTx::start_action(self, hardware, body, config)
            .map(|_| DatapathControlProgress::TxPending)
    }

    fn start_deauthentication<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        reason_code: u16,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        SingleMpduTx::start_deauthentication(self, hardware, reason_code)
            .map(|_| DatapathControlProgress::TxPending)
    }

    fn start_power_management_null<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        SingleMpduTx::start_power_management_null(self, hardware, power_management)
            .map(|_| DatapathControlProgress::TxPending)
    }

    fn start_protected_eapol<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        payload: &[u8],
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        SingleMpduTx::start_protected_eapol(self, hardware, payload)
            .map(|_| DatapathControlProgress::TxPending)
    }

    fn set_tx_block_ack_agreement(&mut self, _tid: u8, _agreement: Option<(u16, bool)>) {}

    fn bss_protection(&self) -> BssProtection {
        self.policy().protection().bss()
    }

    fn install_bss_protection(&mut self, protection: BssProtection) {
        self.policy_mut().install_bss_protection(protection);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedControlTxFailure {
    pub kind: ConnectedControlTxKind,
    pub outcome: SingleMpduTxOutcome,
}

/// Executor-independent portion of connected-control shutdown evidence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedControlCoreShutdown {
    pub rx_block_ack_agreements: u8,
    pub tx_block_ack_sessions: u8,
    pub in_flight: Option<ConnectedControlTxKind>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedControlError {
    RxSession(RxBlockAckSessionsError),
    TxSession(TxBlockAckOriginatorError),
    Hardware(S31RxBlockAckAgreementError),
    Tx(SingleMpduTxError),
    MissingTxOutcome,
    /// A control event arrived before the core consumed the completion of
    /// its transmission.
    EventBeforeTxCompletion,
    MissingQosSequence(u8),
    BeaconDeadline(StaBeaconLossConfigError),
    /// The station TBTT schedule rejected power management's request.
    PowerTbtt(oer_esp32s31_hal::types::StaTbttScheduleError),
    /// Power management produced more deferred work than the runtime drained.
    PowerCommandOverflow,
    /// The power agent could not perform a coexistence or RF command.
    PowerAgentFailed,
    IndividualTwt(IndividualTwtRequesterError),
    IndividualTwtWake(IndividualTwtWakePlanError),
    TsfTiming(oer_ieee80211_lower_mac::TsfTimingError),
    IndividualTwtHardware(StationIndividualTwtHardwareError),
    MissingIndividualTwtRequester,
    Ftm(FtmRequesterError),
    RxReorderCommand(RxReorderCommandError),
}

impl From<RxBlockAckSessionsError> for ConnectedControlError {
    fn from(error: RxBlockAckSessionsError) -> Self {
        Self::RxSession(error)
    }
}

impl From<TxBlockAckOriginatorError> for ConnectedControlError {
    fn from(error: TxBlockAckOriginatorError) -> Self {
        Self::TxSession(error)
    }
}

impl From<S31RxBlockAckAgreementError> for ConnectedControlError {
    fn from(error: S31RxBlockAckAgreementError) -> Self {
        Self::Hardware(error)
    }
}

impl From<SingleMpduTxError> for ConnectedControlError {
    fn from(error: SingleMpduTxError) -> Self {
        Self::Tx(error)
    }
}

impl From<StaBeaconLossConfigError> for ConnectedControlError {
    fn from(error: StaBeaconLossConfigError) -> Self {
        Self::BeaconDeadline(error)
    }
}

impl From<oer_esp32s31_hal::types::StaTbttScheduleError> for ConnectedControlError {
    fn from(error: oer_esp32s31_hal::types::StaTbttScheduleError) -> Self {
        Self::PowerTbtt(error)
    }
}

impl From<IndividualTwtRequesterError> for ConnectedControlError {
    fn from(error: IndividualTwtRequesterError) -> Self {
        Self::IndividualTwt(error)
    }
}

impl From<IndividualTwtWakePlanError> for ConnectedControlError {
    fn from(error: IndividualTwtWakePlanError) -> Self {
        Self::IndividualTwtWake(error)
    }
}

impl From<StationIndividualTwtHardwareError> for ConnectedControlError {
    fn from(error: StationIndividualTwtHardwareError) -> Self {
        Self::IndividualTwtHardware(error)
    }
}

impl From<FtmRequesterError> for ConnectedControlError {
    fn from(error: FtmRequesterError) -> Self {
        Self::Ftm(error)
    }
}

impl From<RxReorderCommandError> for ConnectedControlError {
    fn from(error: RxReorderCommandError) -> Self {
        Self::RxReorderCommand(error)
    }
}

#[derive(Default)]
struct ConnectedControlObservations {
    last_event: Option<ConnectedRxControlEvent>,
    last_tx_failure: Option<ConnectedControlTxFailure>,
    last_expired_tid: Option<u8>,
    stale_tx_block_ack_responses: u32,
    last_stale_tx_block_ack_token: Option<u8>,
    he_control: ConnectedHeControlRuntimeEvidence,
    individual_twt: ConnectedIndividualTwtRuntimeEvidence,
}

/// Complete protocol state for one ESP32-S31 station association.
pub struct ConnectedControlCore {
    peer: [u8; 6],
    he_enabled: bool,
    he_trigger_based: Option<HeTriggerBasedTxConfig>,
    tx_block_ack: TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS>,
    in_flight: Option<ControlInFlight>,
    beacon_monitor: Option<StaLinkMonitor>,
    beacon_lost: bool,
    power: ConnectedPower,
    individual_twt: Option<IndividualTwtRequester>,
    individual_twt_kick: bool,
    /// Random source of SA Query transaction identifiers; present while the
    /// association protects its management frames.
    sa_query_random: Option<fn() -> u32>,
    sa_query: StationSaQuery,
    /// The leaving station published its Deauthentication.
    left: bool,
    /// The owner of the station TSF writes for this association.
    station_tsf: StationTsf,
    observations: ConnectedControlObservations,
}

/// An individual-TWT wake plan and the station TSF generation it was
/// computed in. Its instants describe the station TSF only in that
/// generation: a TSF jump (a reconnection, a radio restart, a TSF crossing
/// 2^64) makes the plan stale, and a stale plan is never armed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationTwtWakePlan {
    pub plan: IndividualTwtWakePlan,
    pub generation: oer_ieee80211_lower_mac::TsfGeneration,
}

impl StationTwtWakePlan {
    /// Whether the plan still describes the station TSF `owner` writes.
    pub fn is_current(&self, owner: &StationTsf) -> bool {
        owner.generation() == self.generation
    }
}

impl ConnectedControlCore {
    /// Control of one association, whose station TSF owner takes
    /// `tsf_epoch`, a number no other owner took (`MacClockHandle::tsf_epoch`).
    pub fn new(
        peer: [u8; 6],
        he_enabled: bool,
        tx_block_ack: TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS>,
        tsf_epoch: u32,
    ) -> Self {
        Self {
            peer,
            he_enabled,
            he_trigger_based: None,
            tx_block_ack,
            in_flight: None,
            beacon_monitor: None,
            beacon_lost: false,
            power: ConnectedPower::new(),
            individual_twt: None,
            individual_twt_kick: false,
            sa_query_random: None,
            sa_query: StationSaQuery::new(),
            left: false,
            station_tsf: StationTsf::new(tsf_epoch),
            observations: ConnectedControlObservations::default(),
        }
    }

    /// The owner of the station TSF writes and its relation. It lives for
    /// one association, whose first write (at the start of power
    /// management) is a jump; its epoch keeps its generations apart from
    /// every earlier association's.
    pub const fn station_tsf(&self) -> &StationTsf {
        &self.station_tsf
    }

    /// Answer and start SA Queries for an association that protects its
    /// management frames. `random` draws the first transaction identifier of
    /// each procedure, as the vendor's `os_get_random` does.
    pub fn enable_management_protection(&mut self, random: fn() -> u32) {
        self.sa_query_random = Some(random);
        self.sa_query = StationSaQuery::new();
    }

    pub fn set_he_trigger_based(&mut self, config: Option<HeTriggerBasedTxConfig>) {
        self.he_trigger_based = config;
    }

    /// Supervise the link from beacons, probing a silent access point as
    /// the Espressif station does
    /// ([`STATION_LINK_PROBE`](oer_espressif_ieee80211_policy::station_link::STATION_LINK_PROBE)).
    pub fn enable_beacon_loss(&mut self, config: StaBeaconLossConfig) {
        self.beacon_monitor = Some(StaLinkMonitor::new(
            config,
            oer_espressif_ieee80211_policy::station_link::STATION_LINK_PROBE,
        ));
        self.beacon_lost = false;
    }

    /// Enable the portable requester owner without changing association
    /// capabilities. Production S31 still rejects every setup at the explicit
    /// hardware-admission boundary before an Action frame is published.
    pub fn enable_individual_twt_requester(&mut self, config: IndividualTwtRequesterConfig) {
        self.individual_twt = Some(IndividualTwtRequester::new(config));
    }

    /// Carry one bounded request through portable encoding and identity
    /// allocation to the current ESP32-S31 source frontier.
    ///
    /// The temporary requester consumes its transmission through a typed
    /// hardware rejection. No shared TX owner is borrowed and no sequence,
    /// DMA descriptor, PHY field or capability is published.
    pub fn evaluate_ftm_request_frontier(
        &self,
        config: FtmRequesterConfig,
        now: oer_time::Instant,
    ) -> Result<ConnectedFtmRequestFrontier, ConnectedControlError> {
        let mut requester = FtmRequester::<CONNECTED_FTM_FRONTIER_SAMPLE_CAPACITY>::new(config);
        let session_generation = requester.start(self.peer, now)?;
        let FtmRequesterService::Transmit(transmission) = requester.service(now)? else {
            unreachable!("a newly queued FTM request is immediately serviceable")
        };
        let hardware_error = station_ftm_request_frontier(&transmission);
        let protocol_event = requester.reject_hardware_admission(transmission)?;
        Ok(ConnectedFtmRequestFrontier {
            peer: self.peer,
            session_generation,
            transmission_generation: transmission.transmission_generation(),
            attempt: transmission.attempt(),
            protocol_event,
            hardware_error,
        })
    }

    pub fn queue_individual_twt_setup(
        &mut self,
        proposal: IndividualTwtProposal,
        now: oer_time::Instant,
    ) -> Result<(), ConnectedControlError> {
        self.individual_twt
            .as_mut()
            .ok_or(ConnectedControlError::MissingIndividualTwtRequester)?
            .queue_setup(proposal, now)?;
        self.individual_twt_kick = true;
        Ok(())
    }

    /// Remove the installed chip schedule first, then queue the wire teardown.
    pub fn queue_individual_twt_teardown<H: ConnectedControlHardware>(
        &mut self,
        hardware: &mut H,
        flow_id: IndividualTwtFlowId,
        now: oer_time::Instant,
    ) -> Result<(), ConnectedControlError> {
        let requester = self
            .individual_twt
            .as_mut()
            .ok_or(ConnectedControlError::MissingIndividualTwtRequester)?;
        if let Some(agreement) = requester.agreement(flow_id) {
            hardware.remove_station_individual_twt(&agreement)?;
        }
        requester.queue_teardown(flow_id, now)?;
        self.individual_twt_kick = true;
        Ok(())
    }

    /// Queue a bounded number of ADDBA publications for each recovered STA
    /// TID. A missing response or failed action-frame TX consumes one attempt
    /// and leaves the next one pending at once, as the vendor station; an
    /// explicit peer response is terminal.
    pub fn queue_initial_tx_block_ack(&mut self, attempt_limit: u8) {
        debug_assert!(attempt_limit != 0);
        self.tx_block_ack.queue_initial(TxBlockAckRetry {
            attempts: attempt_limit,
            interval: oer_time::Duration::ZERO,
        });
    }

    pub const fn tx_block_ack(&self) -> &TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS> {
        &self.tx_block_ack
    }

    pub const fn last_event(&self) -> Option<ConnectedRxControlEvent> {
        self.observations.last_event
    }

    pub const fn last_tx_failure(&self) -> Option<ConnectedControlTxFailure> {
        self.observations.last_tx_failure
    }

    pub const fn last_expired_tid(&self) -> Option<u8> {
        self.observations.last_expired_tid
    }

    pub const fn stale_tx_block_ack_responses(&self) -> u32 {
        self.observations.stale_tx_block_ack_responses
    }

    pub const fn last_stale_tx_block_ack_token(&self) -> Option<u8> {
        self.observations.last_stale_tx_block_ack_token
    }

    pub const fn he_control_runtime_evidence(&self) -> ConnectedHeControlRuntimeEvidence {
        self.observations.he_control
    }

    pub const fn individual_twt_runtime_evidence(&self) -> ConnectedIndividualTwtRuntimeEvidence {
        self.observations.individual_twt
    }

    pub const fn individual_twt_requester(&self) -> Option<&IndividualTwtRequester> {
        self.individual_twt.as_ref()
    }

    /// The next individual-TWT wake, planned at the station TSF now and
    /// waking `wake_guard` before the service starts, with the station TSF
    /// generation it was computed in.
    pub fn individual_twt_wake_plan<H: StationTsfHardware>(
        &self,
        hardware: &mut H,
        wake_guard: oer_ieee80211_mac::tsf::TsfDuration,
    ) -> Result<Option<StationTwtWakePlan>, ConnectedControlError> {
        let Some(requester) = self.individual_twt.as_ref() else {
            return Ok(None);
        };
        let now = self.station_tsf.read(hardware);
        let generation = self.station_tsf.generation();
        Ok(requester
            .plan_next_wake(now, wake_guard)?
            .map(|plan| StationTwtWakePlan { plan, generation }))
    }

    // CAPABILITY: wifi-802-11ax-he-triggered-response-scheduling
    pub const fn he_trigger_runtime_enabled(&self) -> bool {
        if !self.he_enabled {
            return false;
        }
        match self.he_trigger_based {
            Some(config) => config.runtime_handoff_window_micros().is_some(),
            None => false,
        }
    }

    pub const fn beacon_monitor(&self) -> Option<&StaBeaconMonitor> {
        match &self.beacon_monitor {
            Some(link) => Some(link.beacons()),
            None => None,
        }
    }

    pub const fn beacon_lost(&self) -> bool {
        self.beacon_lost
    }

    /// Whether the next step must consume the shared TX completion before a
    /// newly delivered control event.
    pub const fn tx_in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Whether a controlled stop must still send the leaving station's
    /// Deauthentication before the epoch ends.
    pub const fn leave_pending(&self) -> bool {
        !self.left
    }

    pub fn has_immediate_work(&self, control_event_pending: bool) -> bool {
        let transmit_work = self.individual_twt_kick || self.tx_block_ack.has_pending();
        self.in_flight.is_some()
            || control_event_pending
            || (transmit_work && !self.power.blocks_tx())
            || self.power.has_input()
    }

    oer_esp32s31_ieee80211_dma::place_rx_hot_path! {
      /// Return the first owned control deadline without allocating executor state.
      #[inline(never)]
      pub fn next_alarm_deadline(&self) -> Option<oer_time::Instant> {
          let block_ack = self.tx_block_ack.earliest_alarm_deadline();
          let power = self.power.next_deadline();
          // Link and TWT deadlines lead to frames, which wait for the slice.
          if self.power.blocks_tx() {
              return earliest_deadline(
                  earliest_deadline(block_ack, power),
                  self.sa_query.timeout(),
              );
          }
          let link = self
              .beacon_monitor
              .as_ref()
              .and_then(StaLinkMonitor::deadline);
          let individual_twt = self
              .individual_twt
              .as_ref()
              .and_then(IndividualTwtRequester::next_deadline);
          earliest_deadline(
              earliest_deadline(earliest_deadline(block_ack, link), individual_twt),
              earliest_deadline(power, self.sa_query.next_deadline()),
          )
      }
    }

    pub fn shutdown<H, X, const PEER_CAPACITY: usize>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        rx_block_ack: &mut RxBlockAckSessions<PEER_CAPACITY>,
    ) -> Result<ConnectedControlCoreShutdown, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let in_flight_kind = self.in_flight.as_ref().map(ControlInFlight::kind);
        if let Some(in_flight) = self.in_flight.take() {
            match in_flight {
                ControlInFlight::RxAddba(activation) => {
                    if let Err(error) =
                        hardware.clear_rx_block_ack(activation.hardware().hardware_index)
                    {
                        self.in_flight = Some(ControlInFlight::RxAddba(activation));
                        return Err(error.into());
                    }
                    rx_block_ack.cancel(activation)?;
                }
                ControlInFlight::TxAddba { tid } => {
                    self.tx_block_ack.stop(tid);
                    tx.set_tx_block_ack_agreement(tid, None);
                }
                ControlInFlight::BeaconProbe
                | ControlInFlight::SaQuery
                | ControlInFlight::Deauthentication => {}
                ControlInFlight::PowerManagement(_) => {}
                ControlInFlight::IndividualTwt(transmission) => {
                    self.individual_twt
                        .as_mut()
                        .ok_or(ConnectedControlError::MissingIndividualTwtRequester)?
                        .abort_transmission(transmission)?;
                }
            }
        }
        let mut actions = PmActions::new();
        let coex = self.power.coex_view();
        self.power.engine.stop(coex, &mut actions);
        self.apply_power_actions(hardware, tx, actions)?;

        let mut rx_block_ack_agreements = 0_u8;
        for agreement in rx_block_ack
            .snapshots_for(MacInterface::Station)
            .into_iter()
            .flatten()
        {
            hardware.clear_rx_block_ack(agreement.hardware_index)?;
            let stopped = rx_block_ack.stop(MacInterface::Station, self.peer, agreement.tid);
            debug_assert_eq!(stopped, Some(agreement));
            rx_block_ack_agreements = rx_block_ack_agreements.saturating_add(1);
        }
        rx_block_ack.prepare_interface(MacInterface::Station)?;

        let tx_block_ack_sessions = self.tx_block_ack.stop_all();
        for tid in STA_TX_BLOCK_ACK_TIDS {
            tx.set_tx_block_ack_agreement(tid, None);
            if self.he_enabled {
                hardware.set_he_tid_enabled(tid, false)?;
            }
        }
        if let Some(requester) = self.individual_twt.as_mut() {
            for value in 0..INDIVIDUAL_TWT_FLOW_CAPACITY as u8 {
                let flow_id = IndividualTwtFlowId::new(value)
                    .expect("the fixed flow range is wire-representable");
                if let Some(agreement) = requester.agreement(flow_id) {
                    hardware.remove_station_individual_twt(&agreement)?;
                    requester.commit_hardware_remove(agreement)?;
                }
            }
            requester.reset_for_reconnect();
        }
        self.individual_twt = None;
        self.individual_twt_kick = false;
        self.beacon_monitor = None;
        self.beacon_lost = false;
        self.clear_power_inputs();

        Ok(ConnectedControlCoreShutdown {
            rx_block_ack_agreements,
            tx_block_ack_sessions,
            in_flight: in_flight_kind,
        })
    }

    /// Execute at most one finite transition.
    ///
    /// A delivered `event` is always applied in this step. The caller
    /// delivers none while a transmission completes, and delivers one only
    /// when [`Self::power_input_first`] yields the step to it; otherwise
    /// the step performs one power input.
    pub fn service_step<H, X, R, const PEER_CAPACITY: usize>(
        &mut self,
        ports: ConnectedControlPorts<'_, H, X, R, PEER_CAPACITY>,
        event: Option<ConnectedRxControlEvent>,
        control_event_pending: bool,
        context: DatapathControlContext,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
        R: ConnectedControlReorder,
    {
        let ConnectedControlPorts {
            hardware,
            tx,
            reorder,
            rx_block_ack,
        } = ports;
        if let Some(monitor) = &mut self.beacon_monitor {
            monitor.arm(tx.now())?;
        }
        if event.is_some() && self.in_flight.is_some() {
            return Err(ConnectedControlError::EventBeforeTxCompletion);
        }
        if let Some(in_flight) = self.in_flight.take() {
            let outcome = tx
                .take_last_outcome()
                .ok_or(ConnectedControlError::MissingTxOutcome)?;
            let success = outcome.is_success();
            if !success {
                self.observations.last_tx_failure = Some(ConnectedControlTxFailure {
                    kind: in_flight.kind(),
                    outcome,
                });
            }
            match in_flight {
                ControlInFlight::RxAddba(activation) if success => {
                    let negotiated = activation.negotiated();
                    rx_block_ack.commit(activation)?;
                    trace_link(LinkEvent::BlockAckOperational {
                        direction: BlockAckDirection::Rx,
                        tid: negotiated.tid,
                        window: negotiated.window,
                    });
                }
                ControlInFlight::RxAddba(activation) => {
                    hardware.clear_rx_block_ack(activation.hardware().hardware_index)?;
                    reorder.publish(RxReorderCommand::Stop(activation.negotiated().identity()))?;
                    rx_block_ack.cancel(activation)?;
                }
                ControlInFlight::TxAddba { .. } if success => {}
                ControlInFlight::TxAddba { tid } => {
                    self.tx_block_ack.transmit_failed(tid, tx.now());
                    tx.set_tx_block_ack_agreement(tid, None);
                }
                ControlInFlight::BeaconProbe
                | ControlInFlight::SaQuery
                | ControlInFlight::Deauthentication => {}
                ControlInFlight::PowerManagement(advertised) => {
                    let traffic = self.power_traffic(context, control_event_pending);
                    let clock = power_clock(tx);
                    let coex = self.power.coex_view();
                    let mut actions = PmActions::new();
                    self.power.engine.null_done(
                        advertised == StaPowerManagement::PowerSave,
                        success,
                        clock,
                        coex,
                        traffic,
                        &mut actions,
                    );
                    self.apply_power_actions(hardware, tx, actions)?;
                    return Ok(DatapathControlProgress::More);
                }
                ControlInFlight::IndividualTwt(transmission) => {
                    let event = self
                        .individual_twt
                        .as_mut()
                        .ok_or(ConnectedControlError::MissingIndividualTwtRequester)?
                        .complete_transmission(transmission, success, tx.now())?;
                    if success {
                        self.observations.individual_twt.actions_published = self
                            .observations
                            .individual_twt
                            .actions_published
                            .saturating_add(1);
                    }
                    self.observations.individual_twt.last_outcome =
                        Some(ConnectedIndividualTwtRuntimeOutcome::Protocol(event));
                    return Ok(DatapathControlProgress::More);
                }
            }
            return Ok(DatapathControlProgress::More);
        }

        if context.stop_pending {
            return self.leave(hardware, tx);
        }

        if let Some(event) = event {
            self.power.control_event_turn = false;
            self.observations.last_event = Some(event);
            return self.apply_event(
                ConnectedControlPorts {
                    hardware,
                    tx,
                    reorder,
                    rx_block_ack,
                },
                event,
                context,
                control_event_pending,
            );
        }

        if let Some(progress) = self.service_power(hardware, tx, context, control_event_pending)? {
            return Ok(progress);
        }

        let now = tx.now();
        if let Some(tid) = self.tx_block_ack.expire_next(now) {
            tx.set_tx_block_ack_agreement(tid, None);
            self.observations.last_expired_tid = Some(tid);
            return Ok(DatapathControlProgress::More);
        }
        // The SA Query timeout runs whether or not frames may leave.
        if self.sa_query.timed_out(now) {
            return Ok(DatapathControlProgress::Exit(
                ConnectedDisconnectReason::SaQueryTimeout,
            ));
        }
        // Outside the Wi-Fi slice, or with the RF asleep, control frames wait
        // as the vendor's blocked TX queues hold them.
        if self.power.blocks_tx() {
            return Ok(DatapathControlProgress::Idle);
        }
        match self.sa_query.step(now) {
            SaQueryStep::Idle => {}
            SaQueryStep::Request(transaction) => {
                return self.start_sa_query(hardware, tx, SaQuery::Request { transaction });
            }
            SaQueryStep::TimedOut => {
                return Ok(DatapathControlProgress::Exit(
                    ConnectedDisconnectReason::SaQueryTimeout,
                ));
            }
        }
        if self.individual_twt.is_some()
            && let Some(progress) = self.service_individual_twt(hardware, tx)?
        {
            return Ok(progress);
        }
        match self.beacon_monitor.as_ref().and_then(|link| link.due(now)) {
            Some(StaLinkAction::Probe { directed }) => {
                return self.start_beacon_probe(hardware, tx, directed);
            }
            Some(StaLinkAction::Lost) => {
                return self.disconnect_for_beacon_loss(hardware, tx, reorder, rx_block_ack);
            }
            None => {}
        }

        if let Some(tid) = self.tx_block_ack.take_pending(now) {
            return self.start_tx_addba(hardware, tx, tid);
        }

        Ok(DatapathControlProgress::Idle)
    }

    fn service_individual_twt<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
    ) -> Result<Option<DatapathControlProgress<ConnectedDisconnectReason>>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        self.individual_twt_kick = false;
        let now = tx.now();
        let service = self
            .individual_twt
            .as_mut()
            .ok_or(ConnectedControlError::MissingIndividualTwtRequester)?
            .service(now)?;
        match service {
            IndividualTwtService::Idle => Ok(None),
            IndividualTwtService::Event(event) => {
                self.observations.individual_twt.last_outcome =
                    Some(ConnectedIndividualTwtRuntimeOutcome::Protocol(event));
                Ok(Some(DatapathControlProgress::More))
            }
            IndividualTwtService::Transmit(transmission) => {
                if transmission.kind == IndividualTwtTxKind::Setup {
                    let proposal = self
                        .individual_twt
                        .as_ref()
                        .and_then(|requester| requester.transmitting_proposal(transmission.flow_id))
                        .ok_or(ConnectedControlError::IndividualTwt(
                            IndividualTwtRequesterError::StaleTransmission,
                        ))?;
                    if let Err(error) = hardware.admit_station_individual_twt(&proposal) {
                        self.individual_twt
                            .as_mut()
                            .expect("requester produced this transmission")
                            .abort_transmission(transmission)?;
                        self.observations.individual_twt.hardware_rejections = self
                            .observations
                            .individual_twt
                            .hardware_rejections
                            .saturating_add(1);
                        self.observations.individual_twt.last_outcome =
                            Some(ConnectedIndividualTwtRuntimeOutcome::HardwareRejected {
                                flow_id: transmission.flow_id,
                                error,
                            });
                        return Ok(Some(DatapathControlProgress::More));
                    }
                }

                if tx
                    .start_action(
                        hardware,
                        transmission.body.as_slice(),
                        ActionTxConfig::STANDARD_MANAGEMENT,
                    )
                    .is_err()
                {
                    let event = self
                        .individual_twt
                        .as_mut()
                        .expect("requester produced this transmission")
                        .complete_transmission(transmission, false, tx.now())?;
                    self.observations.individual_twt.last_outcome =
                        Some(ConnectedIndividualTwtRuntimeOutcome::Protocol(event));
                    return Ok(Some(DatapathControlProgress::More));
                }
                self.in_flight = Some(ControlInFlight::IndividualTwt(transmission));
                Ok(Some(DatapathControlProgress::TxPending))
            }
        }
    }

    fn apply_event<H, X, R, const PEER_CAPACITY: usize>(
        &mut self,
        ports: ConnectedControlPorts<'_, H, X, R, PEER_CAPACITY>,
        event: ConnectedRxControlEvent,
        context: DatapathControlContext,
        control_event_pending: bool,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
        R: ConnectedControlReorder,
    {
        let ConnectedControlPorts {
            hardware,
            tx,
            reorder,
            rx_block_ack,
        } = ports;
        if let ConnectedRxControlEvent::PeerDisconnect(disconnect) = event {
            return Ok(DatapathControlProgress::Exit(disconnect.into()));
        }
        if let ConnectedRxControlEvent::UnprotectedDisconnect(_) = event {
            let Some(random) = self.sa_query_random else {
                return Ok(DatapathControlProgress::More);
            };
            let Some(transaction) = self.sa_query.start(tx.now(), random()) else {
                return Ok(DatapathControlProgress::More);
            };
            return self.start_sa_query(hardware, tx, SaQuery::Request { transaction });
        }
        if let ConnectedRxControlEvent::SaQuery(query) = event {
            return match query {
                SaQuery::Request { transaction } => {
                    self.start_sa_query(hardware, tx, SaQuery::Response { transaction })
                }
                SaQuery::Response { transaction } => {
                    self.sa_query.response(transaction);
                    Ok(DatapathControlProgress::More)
                }
            };
        }
        if let ConnectedRxControlEvent::PowerSaveData(data) = event {
            let coex = self.power.coex_view();
            let mut actions = PmActions::new();
            self.power
                .engine
                .rx_data(data.group, data.more_data, coex, &mut actions);
            self.apply_power_actions(hardware, tx, actions)?;
            return Ok(DatapathControlProgress::More);
        }
        if let ConnectedRxControlEvent::Beacon(beacon) = event {
            let observation = beacon.observation;
            // The station follows the access point's TSF from each beacon,
            // so the TBTT schedule, which is in that TSF, stays reachable.
            //
            // SOURCE: complete pinned `libnet80211.a[ieee80211_sta.o]::
            // sta_recv_mgmt` calls `ic_update_sta_tsf` with a beacon of the
            // associated BSS; it also tests a flag at byte 0x94 of a
            // structure this port does not model, and updates here for every
            // such beacon.
            //
            // The time since the frame arrived is the difference of the MAC
            // local time, the counter its receive timestamp is a reading of.
            if let Some(stamp) = beacon.stamp {
                let elapsed = hardware.mac_local_time().wrapping_sub(stamp);
                let tsf = access_point_tsf_after(
                    observation.timestamp_tsf,
                    tsf_elapsed_at_nominal_rate(u64::from(elapsed)),
                )
                .expect(
                    "connected RX publishes only beacons whose TSF a 32-bit elapsed time advances",
                );
                self.station_tsf
                    .set(hardware, tsf)
                    .map_err(ConnectedControlError::TsfTiming)?;
            }
            follow_beacon_protection(tx, observation.protection);
            if let Some(monitor) = &mut self.beacon_monitor {
                monitor.observe_beacon(tx.now(), observation)?;
                self.trace_beacon_monitor(BeaconMonitorOp::Refreshed);
            }
            let beacon = PmBeacon {
                timestamp_tsf: observation.timestamp_tsf,
                interval_tu: observation.interval_tu,
                tim: observation.tim.map(|tim| PmTim {
                    dtim_count: tim.dtim_count,
                    dtim_period: tim.dtim_period,
                    unicast: tim.unicast_buffered,
                    group: tim.group_buffered,
                }),
            };
            let traffic = self.power_traffic(context, control_event_pending);
            let clock = power_clock(tx);
            let coex = self.power.coex_view();
            let mut actions = PmActions::new();
            self.power
                .engine
                .beacon(beacon, clock, coex, traffic, &mut actions);
            self.apply_power_actions(hardware, tx, actions)?;
            return Ok(DatapathControlProgress::More);
        }
        if let ConnectedRxControlEvent::ProbeResponse = event {
            if let Some(monitor) = &mut self.beacon_monitor {
                monitor.observe_probe_response(tx.now())?;
                self.trace_beacon_monitor(BeaconMonitorOp::ProbeAnswered);
            }
            return Ok(DatapathControlProgress::More);
        }
        if matches!(
            event,
            ConnectedRxControlEvent::Trigger { .. } | ConnectedRxControlEvent::Ndpa { .. }
        ) {
            return Ok(self.apply_he_control_event(hardware, tx, event));
        }
        if let ConnectedRxControlEvent::IndividualTwt(action) = event {
            return self.apply_individual_twt_action(hardware, tx, action);
        }
        let ConnectedRxControlEvent::BlockAck(action) = event else {
            return Ok(DatapathControlProgress::More);
        };
        match action {
            BlockAckAction::AddbaRequest {
                dialog_token,
                tid,
                immediate,
                window,
                timeout_tu,
                starting_sequence,
                ..
            } => {
                rx_block_ack.offer(RxBlockAckRequest {
                    interface: MacInterface::Station,
                    peer: self.peer,
                    dialog_token,
                    tid,
                    immediate,
                    requested_window: window,
                    timeout_tu,
                    starting_sequence,
                })?;
                let Some(activation) = rx_block_ack.begin_pending()? else {
                    return Ok(DatapathControlProgress::More);
                };
                self.start_rx_addba_response(hardware, tx, reorder, rx_block_ack, activation)
            }
            BlockAckAction::AddbaResponse { .. } => {
                let response = match self.tx_block_ack.on_response_action(action)? {
                    TxBlockAckResponseDisposition::Matched(response) => response,
                    TxBlockAckResponseDisposition::StaleDialogToken(token) => {
                        self.observations.stale_tx_block_ack_responses = self
                            .observations
                            .stale_tx_block_ack_responses
                            .saturating_add(1);
                        self.observations.last_stale_tx_block_ack_token = Some(token);
                        return Ok(DatapathControlProgress::More);
                    }
                };
                let TxBlockAckOriginatorResponse { tid, response } = response;
                let negotiated_agreement = match response {
                    TxBlockAckResponse::Operational(agreement) => {
                        trace_link(LinkEvent::BlockAckOperational {
                            direction: BlockAckDirection::Tx,
                            tid,
                            window: agreement.window,
                        });
                        Some((agreement.window, agreement.amsdu))
                    }
                    TxBlockAckResponse::Rejected(status) => {
                        trace_link(LinkEvent::BlockAckRejected { tid, status });
                        None
                    }
                };
                tx.set_tx_block_ack_agreement(tid, negotiated_agreement);
                if let TxBlockAckResponse::Operational(agreement) = response {
                    if self.he_enabled {
                        hardware.set_he_tid_enabled(agreement.tid, true)?;
                    }
                } else if self.he_enabled {
                    hardware.set_he_tid_enabled(tid, false)?;
                }
                Ok(DatapathControlProgress::More)
            }
            BlockAckAction::Delba { tid, initiator, .. } => {
                if initiator {
                    if let Some(agreement) =
                        rx_block_ack.stop(MacInterface::Station, self.peer, tid)
                    {
                        hardware.clear_rx_block_ack(agreement.hardware_index)?;
                        reorder.publish(RxReorderCommand::Stop(agreement.identity()))?;
                        trace_link(LinkEvent::BlockAckEnded {
                            direction: BlockAckDirection::Rx,
                            tid,
                        });
                    }
                } else {
                    if self.tx_block_ack.operational(tid).is_some() {
                        trace_link(LinkEvent::BlockAckEnded {
                            direction: BlockAckDirection::Tx,
                            tid,
                        });
                    }
                    self.tx_block_ack.stop(tid);
                    tx.set_tx_block_ack_agreement(tid, None);
                    if self.he_enabled {
                        hardware.set_he_tid_enabled(tid, false)?;
                    }
                }
                Ok(DatapathControlProgress::More)
            }
        }
    }

    fn apply_individual_twt_action<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        action: IndividualTwtAction,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let Some(requester) = self.individual_twt.as_mut() else {
            // The association still advertises no requester capability. A
            // stray peer action cannot implicitly create an owner.
            return Ok(DatapathControlProgress::More);
        };
        self.observations.individual_twt.actions_received = self
            .observations
            .individual_twt
            .actions_received
            .saturating_add(1);
        match action {
            IndividualTwtAction::Setup(setup) => {
                let disposition = requester.on_setup_response(setup)?;
                self.observations.individual_twt.last_outcome =
                    Some(ConnectedIndividualTwtRuntimeOutcome::Response(disposition));
                if matches!(
                    disposition,
                    IndividualTwtSetupDisposition::ExplicitInformationUnsupported { .. }
                ) {
                    // The portable owner already queued a fail-closed
                    // teardown for the peer-accepted explicit agreement.
                    self.individual_twt_kick = true;
                }
                if let IndividualTwtSetupDisposition::InstallRequired {
                    flow_id,
                    generation,
                    agreement,
                } = disposition
                {
                    match hardware.install_station_individual_twt(&agreement) {
                        Ok(()) => {
                            let agreement =
                                requester.commit_hardware_install(flow_id, generation)?;
                            self.observations.individual_twt.agreements_installed = self
                                .observations
                                .individual_twt
                                .agreements_installed
                                .saturating_add(1);
                            self.observations.individual_twt.last_outcome = Some(
                                ConnectedIndividualTwtRuntimeOutcome::AgreementInstalled(agreement),
                            );
                        }
                        Err(error) => {
                            requester.reject_hardware_install(flow_id, generation, tx.now())?;
                            self.individual_twt_kick = true;
                            self.observations.individual_twt.hardware_rejections = self
                                .observations
                                .individual_twt
                                .hardware_rejections
                                .saturating_add(1);
                            self.observations.individual_twt.last_outcome =
                                Some(ConnectedIndividualTwtRuntimeOutcome::HardwareRejected {
                                    flow_id,
                                    error,
                                });
                        }
                    }
                }
            }
            IndividualTwtAction::Teardown(teardown) => {
                let installed = requester.installed_for_teardown(teardown);
                for agreement in installed.into_iter().flatten() {
                    hardware.remove_station_individual_twt(&agreement)?;
                    requester.commit_hardware_remove(agreement)?;
                }
                requester.on_peer_teardown(teardown);
                self.observations.individual_twt.last_outcome =
                    Some(ConnectedIndividualTwtRuntimeOutcome::PeerTeardown);
            }
        }
        Ok(DatapathControlProgress::More)
    }

    fn apply_he_control_event<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        event: ConnectedRxControlEvent,
    ) -> DatapathControlProgress<ConnectedDisconnectReason>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        match event {
            ConnectedRxControlEvent::Trigger {
                identity,
                common,
                schedule,
                first_user,
                handoff,
            } => {
                self.observations.he_control.triggers_observed = self
                    .observations
                    .he_control
                    .triggers_observed
                    .saturating_add(1);
                if !self.he_enabled {
                    return self.reject_he_trigger(
                        identity,
                        ConnectedHeControlRuntimeRejection::HeAssociationUnavailable,
                    );
                }
                let Some(queue_policy) = self.he_trigger_based else {
                    return self.reject_he_trigger(
                        identity,
                        ConnectedHeControlRuntimeRejection::RuntimeDisabled,
                    );
                };
                let Some(window) = queue_policy.runtime_handoff_window_micros() else {
                    return self.reject_he_trigger(
                        identity,
                        ConnectedHeControlRuntimeRejection::RuntimeHandoffWindowUnavailable,
                    );
                };
                let schedule = match schedule {
                    Ok(schedule) => schedule,
                    Err(error) => {
                        return self.reject_he_trigger(
                            identity,
                            ConnectedHeControlRuntimeRejection::TriggerSchedule(error),
                        );
                    }
                };
                let Some(handoff) = handoff else {
                    return self.reject_he_trigger(
                        identity,
                        ConnectedHeControlRuntimeRejection::RuntimeTimestampUnavailable,
                    );
                };
                let Some(response_deadline) =
                    handoff.checked_add(oer_time::Duration::from_micros(window.get()))
                else {
                    return self.reject_he_trigger(
                        identity,
                        ConnectedHeControlRuntimeRejection::ResponseDeadlineOverflow,
                    );
                };
                if tx.now() >= response_deadline {
                    return self.reject_he_trigger(
                        identity,
                        ConnectedHeControlRuntimeRejection::MissedResponseWindow,
                    );
                }
                if self.wants_power_save_data() {
                    return self.reject_he_trigger(
                        identity,
                        ConnectedHeControlRuntimeRejection::PowerSaveWakeRequired,
                    );
                }
                let request = HeTriggerRuntimeRequest {
                    identity,
                    common,
                    schedule,
                    first_user,
                    handoff,
                    response_deadline,
                    queue_policy,
                };
                self.observations.he_control.tx_handoffs =
                    self.observations.he_control.tx_handoffs.saturating_add(1);
                match tx.publish_he_trigger_response(hardware, request) {
                    Ok(()) => {
                        self.observations.he_control.tx_publications = self
                            .observations
                            .he_control
                            .tx_publications
                            .saturating_add(1);
                        self.observations.he_control.last_outcome =
                            Some(ConnectedHeControlRuntimeOutcome::TriggerPublished {
                                identity,
                                schedule,
                            });
                        DatapathControlProgress::TxPending
                    }
                    Err(reason) => self.reject_he_trigger(identity, reason),
                }
            }
            ConnectedRxControlEvent::Ndpa {
                identity,
                dialog_token,
                addressed_to_station,
                handoff,
            } => {
                self.observations.he_control.ndpa_observed =
                    self.observations.he_control.ndpa_observed.saturating_add(1);
                if !addressed_to_station {
                    return self.reject_he_ndpa(
                        identity,
                        dialog_token,
                        ConnectedHeControlRuntimeRejection::NdpaNotAddressed,
                    );
                }
                if !self.he_enabled {
                    return self.reject_he_ndpa(
                        identity,
                        dialog_token,
                        ConnectedHeControlRuntimeRejection::HeAssociationUnavailable,
                    );
                }
                let Some(queue_policy) = self.he_trigger_based else {
                    return self.reject_he_ndpa(
                        identity,
                        dialog_token,
                        ConnectedHeControlRuntimeRejection::RuntimeDisabled,
                    );
                };
                let Some(window) = queue_policy.runtime_handoff_window_micros() else {
                    return self.reject_he_ndpa(
                        identity,
                        dialog_token,
                        ConnectedHeControlRuntimeRejection::RuntimeHandoffWindowUnavailable,
                    );
                };
                let Some(handoff) = handoff else {
                    return self.reject_he_ndpa(
                        identity,
                        dialog_token,
                        ConnectedHeControlRuntimeRejection::RuntimeTimestampUnavailable,
                    );
                };
                let Some(response_deadline) =
                    handoff.checked_add(oer_time::Duration::from_micros(window.get()))
                else {
                    return self.reject_he_ndpa(
                        identity,
                        dialog_token,
                        ConnectedHeControlRuntimeRejection::ResponseDeadlineOverflow,
                    );
                };
                if tx.now() >= response_deadline {
                    return self.reject_he_ndpa(
                        identity,
                        dialog_token,
                        ConnectedHeControlRuntimeRejection::MissedResponseWindow,
                    );
                }
                if self.wants_power_save_data() {
                    return self.reject_he_ndpa(
                        identity,
                        dialog_token,
                        ConnectedHeControlRuntimeRejection::PowerSaveWakeRequired,
                    );
                }
                let request = HeNdpaRuntimeRequest {
                    identity,
                    dialog_token,
                    handoff,
                    response_deadline,
                };
                self.observations.he_control.tx_handoffs =
                    self.observations.he_control.tx_handoffs.saturating_add(1);
                match tx.publish_he_ndpa_feedback(hardware, request) {
                    Ok(()) => {
                        self.observations.he_control.tx_publications = self
                            .observations
                            .he_control
                            .tx_publications
                            .saturating_add(1);
                        self.observations.he_control.last_outcome =
                            Some(ConnectedHeControlRuntimeOutcome::NdpaPublished {
                                identity,
                                dialog_token,
                            });
                        DatapathControlProgress::TxPending
                    }
                    Err(reason) => self.reject_he_ndpa(identity, dialog_token, reason),
                }
            }
            ConnectedRxControlEvent::Beacon(_)
            | ConnectedRxControlEvent::ProbeResponse
            | ConnectedRxControlEvent::BlockAck(_)
            | ConnectedRxControlEvent::IndividualTwt(_)
            | ConnectedRxControlEvent::PeerDisconnect(_)
            | ConnectedRxControlEvent::UnprotectedDisconnect(_)
            | ConnectedRxControlEvent::SaQuery(_)
            | ConnectedRxControlEvent::PowerSaveData(_) => DatapathControlProgress::Idle,
        }
    }

    fn reject_he_trigger(
        &mut self,
        identity: AssociatedHeControlIdentity,
        reason: ConnectedHeControlRuntimeRejection,
    ) -> DatapathControlProgress<ConnectedDisconnectReason> {
        self.observations.he_control.rejected =
            self.observations.he_control.rejected.saturating_add(1);
        self.observations.he_control.last_outcome =
            Some(ConnectedHeControlRuntimeOutcome::TriggerRejected { identity, reason });
        // A diagnostic/coalescing HE lane consumes at most one scheduler turn.
        // Returning Idle yields immediately to an already queued network frame.
        DatapathControlProgress::Idle
    }

    fn reject_he_ndpa(
        &mut self,
        identity: AssociatedHeControlIdentity,
        dialog_token: u8,
        reason: ConnectedHeControlRuntimeRejection,
    ) -> DatapathControlProgress<ConnectedDisconnectReason> {
        self.observations.he_control.rejected =
            self.observations.he_control.rejected.saturating_add(1);
        self.observations.he_control.last_outcome =
            Some(ConnectedHeControlRuntimeOutcome::NdpaRejected {
                identity,
                dialog_token,
                reason,
            });
        DatapathControlProgress::Idle
    }

    fn start_beacon_probe<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        directed: bool,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        self.beacon_monitor
            .as_mut()
            .expect("beacon probes require an enabled beacon monitor")
            .probe_sent(tx.now())?;
        self.trace_beacon_monitor(BeaconMonitorOp::ProbeStarted);
        let progress = tx.start_beacon_probe(hardware, directed)?;
        self.in_flight = Some(ControlInFlight::BeaconProbe);
        Ok(progress)
    }

    fn trace_beacon_monitor(&self, op: BeaconMonitorOp) {
        let deadline = self
            .beacon_monitor
            .as_ref()
            .and_then(StaLinkMonitor::deadline)
            .map_or(0, oer_time::Instant::as_micros);
        oer_trace::emit(&BeaconMonitorTrace {
            op,
            deadline_micros_low: deadline as u32,
        });
    }

    fn disconnect_for_beacon_loss<H, X, R, const PEER_CAPACITY: usize>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        reorder: &mut R,
        rx_block_ack: &mut RxBlockAckSessions<PEER_CAPACITY>,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
        R: ConnectedControlReorder,
    {
        for agreement in rx_block_ack
            .snapshots_for(MacInterface::Station)
            .into_iter()
            .flatten()
        {
            rx_block_ack.stop(MacInterface::Station, self.peer, agreement.tid);
            hardware.clear_rx_block_ack(agreement.hardware_index)?;
        }
        reorder.publish(RxReorderCommand::StopInterface(MacInterface::Station))?;
        for tid in STA_TX_BLOCK_ACK_TIDS {
            self.tx_block_ack.stop(tid);
            tx.set_tx_block_ack_agreement(tid, None);
            if self.he_enabled {
                hardware.set_he_tid_enabled(tid, false)?;
            }
        }
        self.beacon_lost = true;
        self.trace_beacon_monitor(BeaconMonitorOp::Lost);
        Ok(DatapathControlProgress::Exit(
            ConnectedDisconnectReason::BeaconLoss,
        ))
    }

    fn start_rx_addba_response<H, X, R, const PEER_CAPACITY: usize>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        reorder: &mut R,
        rx_block_ack: &mut RxBlockAckSessions<PEER_CAPACITY>,
        activation: RxBlockAckActivation,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
        R: ConnectedControlReorder,
    {
        if let Some(replaced) = activation.replaced() {
            if let Err(error) = reorder.publish(RxReorderCommand::Stop(replaced.identity())) {
                hardware.clear_rx_block_ack(replaced.hardware_index)?;
                rx_block_ack.cancel(activation)?;
                return Err(error.into());
            }
            hardware.clear_rx_block_ack(replaced.hardware_index)?;
        }
        hardware.program_rx_block_ack(activation.hardware())?;
        let negotiated = activation.negotiated();
        if let Err(error) = reorder.publish(RxReorderCommand::Start(negotiated)) {
            hardware.clear_rx_block_ack(activation.hardware().hardware_index)?;
            rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        if let Err(error) = tx.start_action(
            hardware,
            activation.response_body(),
            ActionTxConfig::RX_ADDBA_RESPONSE,
        ) {
            hardware.clear_rx_block_ack(activation.hardware().hardware_index)?;
            reorder.publish(RxReorderCommand::Stop(activation.negotiated().identity()))?;
            rx_block_ack.cancel(activation)?;
            return Err(error.into());
        }
        self.in_flight = Some(ControlInFlight::RxAddba(activation));
        Ok(DatapathControlProgress::TxPending)
    }

    /// Leave the access point on a controlled stop: wake the station and send
    /// one Deauthentication with reason 3 (the station is leaving), then let
    /// the epoch end whatever its transmission outcome.
    ///
    /// SOURCE: complete pinned `libnet80211.a[ieee80211_ioctl.o]::
    /// ieee80211_sta_disconnect`, which calls `pm_wake_up` before
    /// `ieee80211_sta_new_state(ic, IEEE80211_S_INIT, 0)`, and the RUN to
    /// INIT branch of `[ieee80211_sta.o]::ieee80211_sta_new_state`, which
    /// sends the Deauthentication (`ieee80211_send_mgmt` subtype `0xc0`,
    /// reason 3) without retrying it.
    fn leave<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        if self.left {
            return Ok(DatapathControlProgress::Idle);
        }
        let mut actions = PmActions::new();
        let coex = self.power.coex_view();
        self.power.engine.stop(coex, &mut actions);
        self.apply_power_actions(hardware, tx, actions)?;
        self.clear_power_inputs();
        tx.start_deauthentication(hardware, DEAUTHENTICATION_REASON_LEAVING)?;
        self.left = true;
        self.in_flight = Some(ControlInFlight::Deauthentication);
        Ok(DatapathControlProgress::TxPending)
    }

    /// Send one SA Query Action; the transmitter protects it with the
    /// pairwise key as a robust Action frame.
    fn start_sa_query<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        query: SaQuery,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        tx.start_action(hardware, &query.encode(), ActionTxConfig::VENDOR_MANAGEMENT)?;
        self.in_flight = Some(ControlInFlight::SaQuery);
        Ok(DatapathControlProgress::TxPending)
    }

    fn start_tx_addba<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        tid: u8,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let sequence = tx
            .peek_qos_sequence(tid)
            .ok_or(ConnectedControlError::MissingQosSequence(tid))?;
        let request = self.tx_block_ack.begin(tid, sequence, tx.now())?;
        if let Err(error) =
            tx.start_action(hardware, &request.body, ActionTxConfig::VENDOR_MANAGEMENT)
        {
            self.tx_block_ack.stop(tid);
            return Err(error.into());
        }
        self.in_flight = Some(ControlInFlight::TxAddba { tid });
        Ok(DatapathControlProgress::TxPending)
    }
}

/// Apply the associated BSS's current ERP, HT Operation and HE Operation
/// protection fields. Basic rates, the short-preamble capability and the
/// peer packet padding were fixed at association and are retained.
fn follow_beacon_protection<X: ConnectedControlTx>(tx: &mut X, beacon: StaBeaconProtection) {
    let current = tx.bss_protection();
    let followed = BssProtection {
        erp: beacon.erp,
        ht: beacon.ht,
        he_txop_rts_threshold: beacon
            .he_txop_rts_threshold
            .and_then(HeTxopDurationRtsThreshold::new),
        ..current
    };
    if followed != current {
        tx.install_bss_protection(followed);
    }
}

mod power;

use oer_ieee80211_sta::sa_query::{SaQueryStep, StationSaQuery};

use power::{ConnectedPower, power_clock};

fn trace_link(event: LinkEvent) {
    oer_trace::emit(&LinkControlTrace { event });
}
pub use power::{
    ConnectedPowerCommand, JoinBeacon, JoinTsfUnrepresentable, NetworkTxPowerReport,
    POWER_COMMAND_CAPACITY, PowerCoexSnapshot, access_point_tsf_after, beacon_tsf_followable,
    tsf_elapsed_at_nominal_rate,
};

#[cfg(test)]
mod tests;
