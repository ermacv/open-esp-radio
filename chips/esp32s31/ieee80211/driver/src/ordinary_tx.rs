//! Shared owner for one ESP32-S31 ordinary legacy/HT/HE TX transaction.
//!
//! Protocol layers encode an MPDU while this owner is free, then hand it a
//! compact publication plan. Descriptor ownership, EDCA state, retry state,
//! calibrated power, entropy and executor deadlines remain here across both
//! the pre-connected control path and the connected data path.
//! A diagnostic-only completion retains the publication while the same step
//! still handles collision, timeout and an expired watchdog. BAR terminals
//! use the independent valid-BlockAck observation; raw TX status alone cannot
//! prove either receipt or absence of the solicited response.

use core::pin::Pin;
use oer_ieee80211_port::backend::{BackendMpduRetry, BackendTxProfile};
use oer_ieee80211_port::backend::{
    FrameClass, OrdinaryCollisionDecision, OrdinaryRetryDecision, OrdinaryRetryError,
    OrdinaryRetryRatePolicy, RetryCounters,
};

use oer_esp32s31_hal::layouts::{WifiTxMetadataHead, WifiTxMetadataHeadMut};
use oer_time::{Clock, Instant, Timer};

pub use crate::tx::{WifiTxEntropy, WifiTxPowerPair, WifiTxPowerProfile, WifiTxResources};
use oer_esp32s31_ieee80211_mac::{
    MacInterface,
    edca::EdcaContentionParameters,
    tx::{
        CompletionObservation, HeSmpduTxConfig, HtTxConfig, LegacyTxConfig, LegacyTxQueue,
        MacLegacyTxResponse, TxCompletion, TxControlFrame, TxCookie, TxDmaState, TxError,
        TxHardware, TxPhyRate, TxSlot, TxSlotAccess, TxSlotFree,
        ampdu::HtBlockAckObservation,
        protection::{ProtectedPpdu, TxProtection, TxProtectionDecision, TxPsdu, TxReceiver},
        runtime::WifiTxRuntimePolicy,
    },
};
use oer_ieee80211_port::{MacTxPlan, MacTxQueueState, MacTxResult, MacTxStatus};
use oer_ieee80211_port::{ManagementTxPlan, VifRole};
use oer_ieee80211_wire::frame::FrameControl;

use crate::tx::{WifiTxProgress, WifiTxWake, capped_power};

pub const TX_METADATA_SIZE: usize = 8;
/// Hardware-appended CCMP MIC bytes accounted for by descriptor publication.
pub const TX_CCMP_MIC_SIZE: usize = 8;
pub const TX_FCS_SIZE: usize = 4;
/// Settle interval between the forced-CCA edge of a queue's timeout abort
/// and its detach (`SOURCE[PROMOTED_LMAC_TX]`, see
/// `TxSlot::begin_timeout_abort`).
pub(crate) const TX_ABORT_SETTLE: oer_time::Duration = oer_time::Duration::from_micros(16);
/// First Frame Control byte of a BlockAckReq: control type, subtype eight.
const BLOCK_ACK_REQUEST_FRAME_CONTROL: u8 = 0x84;

/// ESP32-S31 MAC interface context selected for one ordinary TX queue.
///
/// This selector is independent from the hardware key-slot number. Vendor
/// `ppInstallKey` installs station keys in context zero and AP keys in context
/// one; the TX queue must select the same context for hardware CCMP to find
/// the installed key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryTxInterface {
    Station,
    AccessPoint,
}

impl OrdinaryTxInterface {
    const fn hardware_index(self) -> u8 {
        match self {
            Self::Station => 0,
            Self::AccessPoint => 1,
        }
    }

    /// The interface whose EDCA set the queue's attempts contend with.
    const fn role(self) -> VifRole {
        match self {
            Self::Station => VifRole::Station,
            Self::AccessPoint => VifRole::AccessPoint,
        }
    }
}

/// Compact hardware route retained across retries.
///
/// Queue and interface each fit in two bits. Keeping them in one byte avoids
/// growing every suspended TX/supervisor future merely to retain the AP/STA
/// selector until a retry publication.
#[derive(Clone, Copy)]
struct ActiveTxRoute(u8);

impl ActiveTxRoute {
    const fn new(queue: LegacyTxQueue, interface: OrdinaryTxInterface) -> Self {
        Self((queue as u8) | (interface.hardware_index() << 2))
    }

    const fn queue(self) -> LegacyTxQueue {
        match self.0 & 0x03 {
            0 => LegacyTxQueue::Voice,
            1 => LegacyTxQueue::Video,
            2 => LegacyTxQueue::BestEffort,
            _ => LegacyTxQueue::Background,
        }
    }

    /// Only [`Self::new`] writes the interface bit.
    const fn interface(self) -> OrdinaryTxInterface {
        match (self.0 >> 2) & 1 {
            0 => OrdinaryTxInterface::Station,
            _ => OrdinaryTxInterface::AccessPoint,
        }
    }

    const fn role(self) -> VifRole {
        self.interface().role()
    }

    const fn mac_interface(self) -> MacInterface {
        match self.interface() {
            OrdinaryTxInterface::Station => MacInterface::Station,
            OrdinaryTxInterface::AccessPoint => MacInterface::AccessPoint,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrdinaryTxReport {
    /// Portable terminal exchange status consumed by HMAC policy.
    pub status: MacTxStatus<TxPhyRate>,
    /// Exact ESP32-S31 completion retained for low-level rate evidence.
    ///
    /// Timeout/collision terminal paths have no detached completion record.
    pub completion: Option<TxCompletion>,
    /// Re-publications performed while this transaction retained its MPDU.
    pub retries: OrdinaryTxRetryReport,
    /// Protection selected for the final publication.
    pub protection: TxProtectionDecision,
    /// BlockAck received for a BlockAckReq.
    pub block_ack: Option<HtBlockAckObservation>,
}

/// Exact causes of re-publication within one ordinary-MPDU transaction.
///
/// These counters exclude the initial publication and terminal failures that
/// are not re-published. Keeping the causes separate lets diagnostics
/// distinguish ordinary ACK failure, CTS failure and collision without
/// changing the recovered retry policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OrdinaryTxRetryReport {
    pub cts_timeouts: u8,
    pub ack_timeouts: u8,
    pub collisions: u8,
}

/// Read-only state of one active production ordinary-MPDU transaction.
///
/// This projection is intentionally narrower than [`OrdinaryTxOwner`]: it
/// exposes no descriptor, DMA buffer, PAC capability or mutation route.
/// Hardware diagnostics and compiled-vendor comparison use it to observe the retry state retained
/// by the real owner instead of maintaining a parallel verification model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrdinaryTxActiveSnapshot {
    pub counters: RetryCounters,
    pub publications: u8,
    pub current_rate: TxPhyRate,
    pub retry_bit_set: bool,
    pub retries: OrdinaryTxRetryReport,
    /// Protection selected for the current publication.
    pub protection: TxProtectionDecision,
}

impl OrdinaryTxRetryReport {
    pub const fn total(self) -> u8 {
        self.cts_timeouts
            .saturating_add(self.ack_timeouts)
            .saturating_add(self.collisions)
    }
}

/// A completed publication, retaining the decoder's status proof.
/// The diagnostic report alone has an optional raw completion and cannot
/// prove this fact. Only the observed finish path constructs this value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrdinaryTxCompleted {
    completion: TxCompletion,
    status: oer_ieee80211_port::TxStatus,
    report: OrdinaryTxReport,
}

impl OrdinaryTxCompleted {
    pub const fn status(&self) -> oer_ieee80211_port::TxStatus {
        self.status
    }

    pub const fn report(self) -> OrdinaryTxReport {
        self.report
    }

    pub const fn completion(self) -> TxCompletion {
        self.completion
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryTxOutcome {
    Success(OrdinaryTxCompleted),
    HardwareFailure(OrdinaryTxCompleted),
    HardwareTimeout(OrdinaryTxReport),
    CollisionLimit(OrdinaryTxReport),
}

impl OrdinaryTxOutcome {
    pub const fn report(self) -> OrdinaryTxReport {
        match self {
            Self::HardwareTimeout(report) | Self::CollisionLimit(report) => report,
            Self::Success(completed) | Self::HardwareFailure(completed) => completed.report(),
        }
    }

    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxResetReason {
    CompletionInterruptWithoutState,
    TimeoutInterruptWithoutState,
    ConflictingInterruptEvents(u32),
    ExecutorDeadline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryTxError {
    BufferSizeOverflow,
    UnsupportedPhyRate,
    DeadlineOverflow,
    Tx(TxError),
    Retry(OrdinaryRetryError<TxPhyRate>),
    RadioResetRequired(TxResetReason),
    /// A BlockAckReq was planned at a non-legacy rate; only the legacy
    /// publication program solicits a BlockAck for a single frame.
    BlockAckRequestRate,
    /// A retry needs a complete, supported management/data Frame Control field.
    InvalidRetryHeader,
}

/// Mark a retry before publication, preserving every other MPDU byte.
/// The encoded MPDU length bounds the field, independently of slot capacity.
pub(crate) fn mark_retry(buffer: &mut [u8], frame_length: usize) -> Result<(), OrdinaryTxError> {
    let frame = buffer
        .get_mut(TX_METADATA_SIZE..)
        .and_then(|frame| frame.get_mut(..frame_length))
        .ok_or(OrdinaryTxError::BufferSizeOverflow)?;
    let control = frame
        .first_chunk_mut::<2>()
        .ok_or(OrdinaryTxError::InvalidRetryHeader)?;
    let checked = FrameControl::new(u16::from_le_bytes(*control))
        .ok_or(OrdinaryTxError::InvalidRetryHeader)?;
    *control = checked.with_retry(true).get().to_le_bytes();
    Ok(())
}

impl From<TxError> for OrdinaryTxError {
    fn from(error: TxError) -> Self {
        Self::Tx(error)
    }
}

impl From<OrdinaryRetryError<TxPhyRate>> for OrdinaryTxError {
    fn from(error: OrdinaryRetryError<TxPhyRate>) -> Self {
        Self::Retry(error)
    }
}

/// Control frame for one PPDU: the selected protection and the calibrated
/// power of its control rate.
pub fn control_frame<P: WifiTxPowerProfile>(
    power: &P,
    data: TxPhyRate,
    decision: TxProtectionDecision,
) -> TxControlFrame {
    capped_control_frame(power, data, decision, None)
}

/// [`control_frame`] with its power codes bounded by `ceiling`.
fn capped_control_frame<P: WifiTxPowerProfile>(
    power: &P,
    data: TxPhyRate,
    decision: TxProtectionDecision,
    ceiling: Option<i8>,
) -> TxControlFrame {
    let mut control = TxControlFrame {
        protection: decision.protection,
        ..TxControlFrame::UNPROTECTED
    };
    let pair = capped_power(power.power_pair(control.rate(data).code()), ceiling);
    control.power_primary = pair.primary as u8;
    control.power_alternate = pair.alternate as u8;
    control
}

/// Everything needed to publish one already encoded MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrdinaryTxPlan {
    pub frame_length: usize,
    pub descriptor_capacity: Option<u32>,
    /// Portable exchange policy translated by this ESP32-S31 adapter.
    pub exchange: MacTxPlan<TxPhyRate>,
    pub hardware_mic_length: usize,
    pub hardware_key_selector: u8,
    pub interface: OrdinaryTxInterface,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
    /// The priority count `mac_tx_set_pti` publishes beside the packet
    /// priority: one for every frame, 4000 for a connection frame under the
    /// coexistence reconnect policy.
    pub priority_count: u16,
}

/// The protection exchange a caller chose for one single-attempt
/// publication ([`OrdinaryTxIdle::start_single_attempt`]).
///
/// The control frame's rate is still the BSS policy's
/// ([`WifiTxProtectionPolicy::control_rate`](oer_esp32s31_ieee80211_mac::tx::protection::WifiTxProtectionPolicy::control_rate));
/// only the choice of exchange moves to the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleAttemptProtection {
    None,
    RtsCts,
    CtsToSelf,
}

/// The caller's choices for one single-attempt publication
/// ([`OrdinaryTxIdle::start_single_attempt`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SingleAttempt {
    pub protection: SingleAttemptProtection,
    /// Backoff slots the queue counts down after the AIFS: the value of its
    /// ten-bit contention-window field, at most
    /// [`MAX_SINGLE_ATTEMPT_BACKOFF_SLOTS`].
    pub backoff_slots: u16,
    /// Ceiling on the data and control power codes, in dBm (see
    /// [`WifiTxPowerProfile`]); `None` publishes the calibrated codes.
    pub power_ceiling_dbm: Option<i8>,
    /// Publish an individually addressed frame without soliciting an ACK.
    /// Only the legacy program carries the response field, so a non-legacy
    /// rate is refused. On-air behaviour of an unsolicited-ACK unicast is
    /// not confirmed on hardware.
    pub no_ack: bool,
}

impl SingleAttempt {
    /// The caller's protection at the calibrated power, with no backoff,
    /// soliciting the response the frame's addresses imply.
    pub const fn new(protection: SingleAttemptProtection) -> Self {
        Self {
            protection,
            backoff_slots: 0,
            power_ceiling_dbm: None,
            no_ack: false,
        }
    }
}

/// The largest backoff the ordinary queue's ten-bit contention-window field
/// holds.
pub const MAX_SINGLE_ATTEMPT_BACKOFF_SLOTS: u16 = 0x03ff;

/// A single attempt published on its EDCA queue and detached from the
/// owner, so that the owner can publish on another queue meanwhile
/// ([`OrdinaryTxOwner::start_queued_single_attempt`]).
///
/// It retains the slot the hardware reads and the transaction's cookie,
/// deadline and phase. Only [`OrdinaryTxOwner::service_queued_single_attempt`]
/// advances it; dropping it forgets a descriptor the hardware may still
/// own, which only a radio reset recovers.
#[must_use = "service the published attempt until its slot returns"]
pub struct QueuedSingleAttempt<'slot, X: BackendTxProfile, const BUFFER_SIZE: usize> {
    slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
    active: ActiveTx<X>,
}

impl<X: BackendTxProfile, const BUFFER_SIZE: usize> QueuedSingleAttempt<'_, X, BUFFER_SIZE> {
    /// The ordinary queue the attempt is published on.
    pub const fn queue(&self) -> LegacyTxQueue {
        self.active.route.queue()
    }

    /// The publication deadline, or the end of the abort settle while one
    /// runs.
    pub const fn deadline(&self) -> oer_time::Instant {
        self.active.deadline
    }

    /// Whether the queue's timeout abort is settling: CCA is forced until
    /// its detach.
    pub const fn abort_settling(&self) -> bool {
        matches!(self.active.phase, OrdinaryTxPhase::AbortSettling)
    }

    /// The slot the hardware reads, for observation.
    pub fn slot(&self) -> Pin<&TxSlot<BUFFER_SIZE>> {
        self.slot.as_ref()
    }
}

/// How a serviced [`QueuedSingleAttempt`] stands.
#[must_use = "retain the pending attempt or its completed slot"]
pub enum QueuedSingleAttemptProgress<'slot, X: BackendTxProfile, const BUFFER_SIZE: usize> {
    /// The hardware still owns the attempt.
    Pending(QueuedSingleAttempt<'slot, X, BUFFER_SIZE>),
    /// The attempt ended; its slot is idle again.
    Complete {
        outcome: OrdinaryTxOutcome,
        slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
    },
}

/// An execution fault retains the potentially published DMA slot for reset.
/// Its ownership cannot be projected back into an ordinary submission.
#[must_use = "retain the quarantined DMA owner until the backend is reset"]
pub struct QuarantinedSingleAttempt<'slot, const BUFFER_SIZE: usize> {
    _slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
}

/// A failed execution returns custody of its publication, never a free slot.
#[must_use = "move the failed publication into the backend's quarantine"]
pub struct QueuedSingleAttemptFailed<'slot, const BUFFER_SIZE: usize> {
    pub error: OrdinaryTxError,
    pub quarantine: QuarantinedSingleAttempt<'slot, BUFFER_SIZE>,
}

/// A queued attempt that was not published; its slot comes back.
#[must_use = "retain the unpublished slot and handle the refusal"]
pub struct QueuedSingleAttemptRefused<'slot, const BUFFER_SIZE: usize> {
    pub error: OrdinaryTxError,
    pub slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
}

struct ActiveTx<X: BackendTxProfile> {
    cookie: TxCookie,
    /// `Some` for a single-attempt publication: the caller's protection,
    /// backoff and power ceiling, and no re-publication whatever the
    /// completion says.
    single_attempt: Option<SingleAttempt>,
    /// The queued lower-MAC caller owns the complete logical retry plan.
    external_contention: bool,
    retry: X::MpduRetry<TxPhyRate, LegacyTxQueue>,
    frame_length: usize,
    descriptor_capacity: u32,
    transfer_length: u32,
    route: ActiveTxRoute,
    hardware_mic_length: usize,
    hardware_key_selector: u8,
    scheduler_priority: u8,
    packet_priority: u8,
    priority_count: u16,
    receiver: TxReceiver,
    /// Immediate response the publication solicits.
    response: MacLegacyTxResponse,
    /// Protection selected for the current publication.
    protection: TxProtectionDecision,
    completion_timeout: oer_time::Duration,
    /// Next service boundary: publication expiry or abort-settle expiry.
    /// The previous phase's deadline is no longer actionable after transition.
    deadline: oer_time::Instant,
    phase: OrdinaryTxPhase,
    retries: OrdinaryTxRetryReport,
}

type OrdinaryCompletion = CompletionObservation<(TxCompletion, Option<HtBlockAckObservation>)>;

/// A published operation moves into this value only after Observed(status).
struct FinishedTx<X: BackendTxProfile> {
    active: ActiveTx<X>,
    status: oer_ieee80211_port::TxStatus,
}

impl<X: BackendTxProfile> ActiveTx<X> {
    fn finish(
        self,
        completion: TxCompletion,
        block_ack: Option<HtBlockAckObservation>,
    ) -> FinishedTx<X> {
        use oer_ieee80211_port::TxStatus;
        let status = match (self.response, block_ack, completion.tx_status()) {
            (MacLegacyTxResponse::BlockAck, Some(_), _) => TxStatus::Success,
            (MacLegacyTxResponse::BlockAck, None, TxStatus::Success) => TxStatus::AckTimeout,
            (_, _, status) => status,
        };
        FinishedTx {
            active: self,
            status,
        }
    }
}

enum OrdinaryTxPhase {
    Published,
    AbortSettling,
}

/// Unique ordinary-MPDU descriptor and retry owner shared by protocol phases.
pub struct OrdinaryTxOwner<
    'slot,
    X: BackendTxProfile,
    P,
    E,
    T,
    const BUFFER_SIZE: usize,
    const LEGACY_ONLY: bool = false,
> {
    slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
    policy: WifiTxRuntimePolicy<X>,
    power: P,
    entropy: E,
    pub timer: T,
    active: Option<ActiveTx<X>>,
    last_outcome: Option<OrdinaryTxOutcome>,
}

/// The methods available during one exclusively borrowed ordinary TX phase.
pub enum OrdinaryTxAccess<
    'owner,
    'slot,
    X: BackendTxProfile,
    P,
    E,
    T,
    const N: usize,
    const LEGACY_ONLY: bool = false,
> {
    Idle(OrdinaryTxIdle<'owner, 'slot, X, P, E, T, N, LEGACY_ONLY>),
    Active(OrdinaryTxActive<'owner, 'slot, X, P, E, T, N, LEGACY_ONLY>),
    Unavailable(TxDmaState),
}

pub struct OrdinaryTxIdle<
    'owner,
    'slot,
    X: BackendTxProfile,
    P,
    E,
    T,
    const N: usize,
    const LEGACY_ONLY: bool = false,
> {
    owner: &'owner mut OrdinaryTxOwner<'slot, X, P, E, T, N, LEGACY_ONLY>,
}

pub struct OrdinaryTxActive<
    'owner,
    'slot,
    X: BackendTxProfile,
    P,
    E,
    T,
    const N: usize,
    const LEGACY_ONLY: bool = false,
> {
    owner: &'owner mut OrdinaryTxOwner<'slot, X, P, E, T, N, LEGACY_ONLY>,
}

/// An owned idle transmitter. Consuming this owner into the lower MAC fixes
/// queued mode: that core never exposes its nested classic transmitter.
pub struct OrdinaryTxIdleOwner<
    'slot,
    X: BackendTxProfile,
    P,
    E,
    T,
    const N: usize,
    const LEGACY_ONLY: bool = false,
> {
    pub(crate) owner: OrdinaryTxOwner<'slot, X, P, E, T, N, LEGACY_ONLY>,
}

impl<
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: Timer,
    const N: usize,
    const LEGACY_ONLY: bool,
> OrdinaryTxIdle<'_, '_, X, P, E, T, N, LEGACY_ONLY>
{
    pub fn buffer_mut(&mut self) -> &mut [u8; N] {
        self.owner.free_slot().into_buffer()
    }

    pub fn start<H: TxHardware>(
        self,
        hardware: &mut H,
        plan: OrdinaryTxPlan,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        self.owner.start_idle(hardware, plan)
    }

    /// Send management at the rate and budget selected above the driver.
    ///
    /// `management` replaces the exchange's initial rate and publication
    /// limit. All attempts keep that rate; no vendor ladder or control
    /// schedule participates. The caller supplies the plan for the encoded
    /// receiver: unicast or broadcast.
    pub fn start_management<H: TxHardware>(
        self,
        hardware: &mut H,
        mut plan: OrdinaryTxPlan,
        management: ManagementTxPlan,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        plan.exchange.initial_rate = TxPhyRate::Legacy(
            oer_esp32s31_ieee80211_mac::tx::LegacyRate::from_phy_rate(management.rate()),
        );
        plan.exchange.publication_limit = management.attempts();
        self.owner.start_idle_with_retry_rate_policy(
            hardware,
            plan,
            OrdinaryRetryRatePolicy::Management,
        )
    }

    pub fn start_with_retry_rate_policy<H: TxHardware>(
        self,
        hardware: &mut H,
        plan: OrdinaryTxPlan,
        retry: OrdinaryRetryRatePolicy,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        self.owner
            .start_idle_with_retry_rate_policy(hardware, plan, retry)
    }

    pub fn start_single_attempt<H: TxHardware>(
        self,
        hardware: &mut H,
        plan: OrdinaryTxPlan,
        attempt: SingleAttempt,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        self.owner
            .start_idle_single_attempt(hardware, plan, attempt)
    }
}

impl<
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: Timer,
    const N: usize,
    const LEGACY_ONLY: bool,
> OrdinaryTxActive<'_, '_, X, P, E, T, N, LEGACY_ONLY>
{
    pub fn service<H: TxHardware>(
        self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        self.owner.service_active(hardware, wake)
    }

    pub fn service_polling<H: TxHardware>(
        self,
        hardware: &mut H,
        poll_interval: oer_time::Duration,
    ) -> impl Future<Output = Result<WifiTxProgress, OrdinaryTxError>> {
        self.owner.service_active_polling(hardware, poll_interval)
    }
}

/// The owner's timer, for phases that wait on the owner's clock.
impl<X: BackendTxProfile, P, E, T: Clock, const BUFFER_SIZE: usize, const LEGACY_ONLY: bool> Clock
    for OrdinaryTxOwner<'_, X, P, E, T, BUFFER_SIZE, LEGACY_ONLY>
{
    fn now(&self) -> Instant {
        self.timer.now()
    }
}

impl<X: BackendTxProfile, P, E, T: Timer, const BUFFER_SIZE: usize, const LEGACY_ONLY: bool> Timer
    for OrdinaryTxOwner<'_, X, P, E, T, BUFFER_SIZE, LEGACY_ONLY>
{
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        self.timer.wait_until(deadline)
    }
}

/// Opaque logical completion state detached from idle physical TX resources.
///
/// There is intentionally no public constructor: only a real terminal owner
/// can produce this token, so a role handoff cannot fabricate an outcome.
pub struct OrdinaryTxParked {
    last_outcome: Option<OrdinaryTxOutcome>,
}

impl<'slot, X: BackendTxProfile, P, E, T, const BUFFER_SIZE: usize, const LEGACY_ONLY: bool>
    OrdinaryTxOwner<'slot, X, P, E, T, BUFFER_SIZE, LEGACY_ONLY>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: Timer,
{
    fn from_resources(resources: WifiTxResources<'slot, X, P, E, T, BUFFER_SIZE>) -> Self {
        let WifiTxResources {
            slot,
            policy,
            power,
            entropy,
            timer,
        } = resources;
        Self {
            slot,
            policy,
            power,
            entropy,
            timer,
            active: None,
            last_outcome: None,
        }
    }

    /// Rejoin idle physical resources with the exact opaque role-local state
    /// emitted by [`Self::try_park`].
    pub fn resume(
        resources: WifiTxResources<'slot, X, P, E, T, BUFFER_SIZE>,
        parked: OrdinaryTxParked,
    ) -> Self {
        let mut owner = Self::from_resources(resources);
        owner.last_outcome = parked.last_outcome;
        owner
    }

    pub const fn active(&self) -> bool {
        self.active.is_some()
    }

    /// Lend the current software phase for one encoding or service operation.
    pub fn access(&mut self) -> OrdinaryTxAccess<'_, 'slot, X, P, E, T, BUFFER_SIZE, LEGACY_ONLY> {
        if self.active.is_some() {
            OrdinaryTxAccess::Active(OrdinaryTxActive { owner: self })
        } else if self.slot_state() == TxDmaState::Free {
            OrdinaryTxAccess::Idle(OrdinaryTxIdle { owner: self })
        } else {
            OrdinaryTxAccess::Unavailable(self.slot_state())
        }
    }

    #[allow(
        clippy::result_large_err,
        reason = "an unavailable phase retains the entire transmitter"
    )]
    pub fn into_idle(
        self,
    ) -> Result<OrdinaryTxIdleOwner<'slot, X, P, E, T, BUFFER_SIZE, LEGACY_ONLY>, Self> {
        if self.active.is_none() && self.slot_state() == TxDmaState::Free {
            Ok(OrdinaryTxIdleOwner { owner: self })
        } else {
            Err(self)
        }
    }

    fn free_slot(&mut self) -> TxSlotFree<'_, BUFFER_SIZE> {
        match self.slot.as_mut().access() {
            TxSlotAccess::Free(free) => free,
            _ => panic!("ordinary transaction owns its free encoding phase"),
        }
    }

    /// Observe the bounded retry state retained by the active TX owner.
    ///
    /// `retry_bit_set` follows from a completed ACK-timeout re-publication:
    /// the owner increments that counter, writes the Retry bit and publishes
    /// again before restoring `active`.
    pub fn active_snapshot(
        &self,
    ) -> Result<Option<OrdinaryTxActiveSnapshot>, OrdinaryRetryError<TxPhyRate>> {
        self.active
            .as_ref()
            .map(|active| {
                Ok(OrdinaryTxActiveSnapshot {
                    counters: active.retry.counters(),
                    publications: active.retry.publications(),
                    current_rate: active.retry.current_rate()?,
                    retry_bit_set: active.retries.ack_timeouts != 0,
                    retries: active.retries,
                    protection: active.protection,
                })
            })
            .transpose()
    }

    /// Exact descriptor lifecycle state retained for ownership diagnostics.
    pub fn slot_state(&self) -> TxDmaState {
        self.slot.as_ref().get_ref().state()
    }

    /// Current hardware-visible descriptor ownership word.
    pub fn descriptor_word0(&self) -> oer_esp32s31_hal::layouts::WifiTxDmaDescriptorControl {
        self.slot.as_ref().get_ref().descriptor_word0()
    }

    /// Portable distinction between normal queue pressure and a quarantined
    /// descriptor that requires a new radio epoch.
    pub fn queue_state(&self) -> MacTxQueueState {
        if self.slot.as_ref().get_ref().state() == TxDmaState::ResetRequired {
            MacTxQueueState::ResetRequired
        } else if self.active.is_some() || self.slot.as_ref().get_ref().state() != TxDmaState::Free
        {
            MacTxQueueState::Backpressured
        } else {
            MacTxQueueState::Ready
        }
    }

    pub const fn policy(&self) -> &WifiTxRuntimePolicy<X> {
        &self.policy
    }

    pub fn policy_mut(&mut self) -> &mut WifiTxRuntimePolicy<X> {
        &mut self.policy
    }

    pub const fn power(&self) -> &P {
        &self.power
    }
    pub(crate) fn replace_power_profile(&mut self, power: P) {
        self.power = power;
    }

    /// Recover the phase-independent TX resources while DMA is idle.
    ///
    /// Returning `self` on failure preserves a live descriptor transaction;
    /// callers must drive it to a terminal outcome or reset the radio before
    /// attempting a station lifecycle transition again.
    #[allow(clippy::result_large_err)]
    pub fn try_into_resources(
        self,
    ) -> Result<WifiTxResources<'slot, X, P, E, T, BUFFER_SIZE>, Self> {
        self.try_park().map(|(resources, _)| resources)
    }

    /// Detach an idle descriptor owner without discarding its terminal
    /// observation. Physical resources and logical callback state become two
    /// distinct, uniquely owned capabilities.
    #[allow(clippy::result_large_err)]
    pub fn try_park(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, X, P, E, T, BUFFER_SIZE>,
            OrdinaryTxParked,
        ),
        Self,
    > {
        if self.queue_state() != MacTxQueueState::Ready {
            return Err(self);
        }
        let Self {
            slot,
            policy,
            power,
            entropy,
            timer,
            active: _,
            last_outcome,
        } = self;
        Ok((
            WifiTxResources {
                slot,
                policy,
                power,
                entropy,
                timer,
            },
            OrdinaryTxParked { last_outcome },
        ))
    }

    pub fn contention_publication(
        &mut self,
        role: VifRole,
        queue: LegacyTxQueue,
    ) -> (EdcaContentionParameters, u16) {
        let parameters = self.policy.contention_parameters(role, queue);
        let backoff = self
            .policy
            .select_backoff(role, queue, self.entropy.next_u32());
        (parameters, backoff)
    }

    pub fn record_retry_failure(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.policy.record_retry_failure(role, queue);
    }

    pub fn record_success(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.policy.record_success(role, queue);
    }

    pub fn reset_terminal_exchange(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.policy.reset_terminal_exchange(role, queue);
    }

    /// The next service deadline, including an in-progress hardware abort.
    pub fn next_deadline(&self) -> Option<oer_time::Instant> {
        self.active.as_ref().map(|active| active.deadline)
    }

    /// Wait for the next service deadline. A deadline already reached, and
    /// an idle owner, still yield to the executor once: the service loops
    /// poll again after this wait, and the time contract would otherwise end
    /// the wait at once and let them run without yielding.
    pub fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        let deadline = self.next_deadline().unwrap_or_else(|| self.timer.now());
        let reached = self.timer.now() >= deadline;
        async move {
            if reached {
                yield_once().await;
            }
            self.timer.wait_until(deadline).await;
        }
    }

    /// Submitted work for the current/last exchange, retained through retries
    /// and timeout detach; independent of terminal delivery status.
    pub fn work(&self) -> oer_ieee80211_port::MacTxWork {
        self.slot.as_ref().get_ref().work()
    }

    pub fn take_last_outcome(&mut self) -> Option<OrdinaryTxOutcome> {
        self.last_outcome.take()
    }

    /// Borrow the terminal outcome without consuming another owner's public
    /// observation of it.
    pub const fn last_outcome(&self) -> Option<OrdinaryTxOutcome> {
        self.last_outcome
    }

    fn start_idle<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        plan: OrdinaryTxPlan,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        self.start_idle_with_retry_rate_policy(hardware, plan, OrdinaryRetryRatePolicy::Normal)
    }

    /// Start one ordinary MPDU with an explicitly owned retry-rate policy.
    ///
    /// Protocols should use [`OrdinaryTxIdle::start`] unless they have a distinct,
    /// reviewed rate arena such as the standard ESP-NOW P2P profile. DMA,
    /// EDCA and terminal ownership remain identical to the normal path.
    fn start_idle_with_retry_rate_policy<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        plan: OrdinaryTxPlan,
        retry_rate_policy: OrdinaryRetryRatePolicy,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        self.start_transaction(hardware, plan, retry_rate_policy, None)
    }

    /// Start exactly one hardware publication of the encoded MPDU.
    ///
    /// This is the lower-MAC port's attempt: the owner publishes the MPDU
    /// once at `plan.exchange.initial_rate` with the caller's protection,
    /// and every completion, collision or timeout ends the transaction. No
    /// retry ladder, rate fallback, Retry-bit rewrite or BSS protection
    /// selection is applied, and `plan.exchange.publication_limit` is not
    /// consulted. The queue counts down the caller's `backoff_slots` rather
    /// than a draw from the owner's EDCA state: the contention window and
    /// its widening after failures belong to the caller's retry policy
    /// above the port. The owner's own EDCA state is left at its minimum.
    /// A power ceiling bounds the data and the protection control frame.
    fn start_idle_single_attempt<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        plan: OrdinaryTxPlan,
        attempt: SingleAttempt,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        let active = self.start_single_active(hardware, plan, attempt)?;
        Ok(self.settle(Step::Pending(active)))
    }

    /// [`OrdinaryTxIdle::start_single_attempt`], returning the published attempt
    /// instead of keeping it in the owner.
    fn start_single_active<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        mut plan: OrdinaryTxPlan,
        attempt: SingleAttempt,
    ) -> Result<ActiveTx<X>, OrdinaryTxError> {
        if attempt.backoff_slots > MAX_SINGLE_ATTEMPT_BACKOFF_SLOTS {
            return Err(OrdinaryTxError::Tx(TxError::Invalid));
        }
        plan.exchange.publication_limit = 1;
        self.start_active(
            hardware,
            plan,
            OrdinaryRetryRatePolicy::Normal,
            Some(attempt),
        )
    }

    /// Publish `slot`'s encoded MPDU as one single attempt on the queue of
    /// `plan.exchange.access_category`, then detach it from the owner.
    ///
    /// The publication is [`OrdinaryTxIdle::start_single_attempt`]'s; the owner keeps
    /// its own idle slot and no active transaction, so it can publish another
    /// queued attempt on another queue while this one is in flight. The
    /// hardware keeps a descriptor, a completion bank and timeout and
    /// collision state per ordinary queue (`pac/src/wifi/mac/tx/queue.rs`),
    /// so each queued attempt is serviced on its own queue. An idle owner is
    /// required: a classic transaction and queued attempts do not mix.
    #[allow(
        clippy::result_large_err,
        reason = "the refusal hands the caller's pinned slot back by value"
    )]
    pub(crate) fn start_queued_single_attempt<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        mut slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
        plan: OrdinaryTxPlan,
        attempt: SingleAttempt,
    ) -> Result<
        QueuedSingleAttempt<'slot, X, BUFFER_SIZE>,
        QueuedSingleAttemptRefused<'slot, BUFFER_SIZE>,
    > {
        assert!(
            self.active.is_none(),
            "queued mode retains an idle ordinary transmitter"
        );
        core::mem::swap(&mut self.slot, &mut slot);
        let result = self.start_single_active(hardware, plan, attempt);
        // `slot` is the owner's idle slot again; hand the caller's back.
        core::mem::swap(&mut self.slot, &mut slot);
        match result {
            Ok(mut active) => {
                active.external_contention = true;
                Ok(QueuedSingleAttempt { slot, active })
            }
            Err(error) => Err(QueuedSingleAttemptRefused { error, slot }),
        }
    }

    /// Consume one interrupt or deadline edge for a queued attempt.
    ///
    /// Several queues share one MAC interrupt, and its task-side events
    /// (`EVENT_TX_COMPLETE`, `EVENT_TX_TIMEOUT`) do not name
    /// a queue, so every queued attempt is serviced with the same edge and
    /// claims only its own queue's state: the completion bank, then its per-queue
    /// collision state, then (on a timeout edge or its
    /// expired publication deadline) the timeout abort. An edge its queue does
    /// not show leaves the attempt pending; unlike [`OrdinaryTxActive::service`], several
    /// simultaneous causes are not a fault, since they may belong to different
    /// queues. An expired deadline without the queue's timeout edge still
    /// quarantines the descriptor.
    ///
    /// The timeout abort forces the MAC-wide CCA until its detach releases
    /// it, so `may_begin_timeout_abort` must be `false` while another queue's
    /// abort settles; the attempt then stays pending with its timeout latched
    /// in the queue state until it is serviced again.
    ///
    /// On an error the descriptor and its slot stay in the returned quarantine;
    /// only a radio reset can recover it.
    pub(crate) fn service_queued_single_attempt<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        queued: QueuedSingleAttempt<'slot, X, BUFFER_SIZE>,
        wake: WifiTxWake,
        may_begin_timeout_abort: bool,
    ) -> Result<
        QueuedSingleAttemptProgress<'slot, X, BUFFER_SIZE>,
        QueuedSingleAttemptFailed<'slot, BUFFER_SIZE>,
    > {
        assert!(
            self.active.is_none(),
            "queued mode retains an idle ordinary transmitter"
        );
        let QueuedSingleAttempt { mut slot, active } = queued;
        core::mem::swap(&mut self.slot, &mut slot);
        let result = self.service_shared_edge(hardware, active, wake, may_begin_timeout_abort);
        core::mem::swap(&mut self.slot, &mut slot);
        match result {
            Ok(Step::Pending(active)) => {
                Ok(QueuedSingleAttemptProgress::Pending(QueuedSingleAttempt {
                    slot,
                    active,
                }))
            }
            Ok(Step::Complete(outcome)) => {
                Ok(QueuedSingleAttemptProgress::Complete { outcome, slot })
            }
            Err(error) => Err(QueuedSingleAttemptFailed {
                error,
                quarantine: QuarantinedSingleAttempt { _slot: slot },
            }),
        }
    }

    /// [`OrdinaryTxActive::service`] for an edge several queues share.
    fn service_shared_edge<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        active: ActiveTx<X>,
        wake: WifiTxWake,
        may_begin_timeout_abort: bool,
    ) -> Result<Step<X>, OrdinaryTxError> {
        use oer_esp32s31_ieee80211_mac::irq::EVENT_TX_TIMEOUT;

        if matches!(active.phase, OrdinaryTxPhase::AbortSettling) {
            return self.service_abort_settle(hardware, active);
        }
        let events = match wake {
            WifiTxWake::Interrupt { events } => events,
            WifiTxWake::Deadline => 0,
        };
        if let Some(event) = self.acknowledge_completion(hardware, &active)? {
            match event {
                CompletionObservation::Observed((completion, block_ack)) => {
                    self.slot
                        .as_mut()
                        .detach_completed(hardware, active.cookie)?;
                    let finished = active.finish(completion, block_ack);
                    return self.finish_completion(hardware, finished, completion, block_ack);
                }
                CompletionObservation::NoEffect => {}
            }
        }
        if self
            .slot
            .as_mut()
            .abort_collision(hardware, active.cookie)?
        {
            return self.finish_aborted_attempt(hardware, active, false);
        }
        let expired = self.timer.now() >= active.deadline;
        if (events & EVENT_TX_TIMEOUT != 0 || expired) && may_begin_timeout_abort {
            if self
                .slot
                .as_mut()
                .begin_timeout_abort(hardware, active.cookie)?
            {
                return self.start_abort_settle(active);
            }
            if expired {
                return self.reset_required(active, TxResetReason::ExecutorDeadline);
            }
        }
        Ok(Step::Pending(active))
    }

    /// The control frame of a single-attempt PPDU another owner publishes
    /// (an aggregate): the caller's protection at the BSS policy's control
    /// rate, its power codes bounded by `ceiling`, as
    /// [`OrdinaryTxIdle::start_single_attempt`] selects them for an MPDU.
    pub fn single_attempt_control_frame(
        &self,
        data: TxPhyRate,
        protection: SingleAttemptProtection,
        ceiling: Option<i8>,
    ) -> TxControlFrame {
        let decision = self.select_protection(
            Some(SingleAttempt::new(protection)),
            TxReceiver::Individual,
            data,
            0,
        );
        self.control_frame(data, decision, ceiling)
    }

    /// The calibrated power pair of `rate_code` bounded by `ceiling`, as
    /// [`OrdinaryTxIdle::start_single_attempt`] publishes it.
    pub fn single_attempt_power_pair(&self, rate_code: u8, ceiling: Option<i8>) -> WifiTxPowerPair {
        capped_power(self.power.power_pair(rate_code), ceiling)
    }

    fn start_transaction<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        plan: OrdinaryTxPlan,
        retry_rate_policy: OrdinaryRetryRatePolicy,
        single_attempt: Option<SingleAttempt>,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        let active = self.start_active(hardware, plan, retry_rate_policy, single_attempt)?;
        Ok(self.settle(Step::Pending(active)))
    }

    /// Publish a transaction's first attempt and return it, without keeping
    /// it in the owner.
    fn start_active<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        plan: OrdinaryTxPlan,
        retry_rate_policy: OrdinaryRetryRatePolicy,
        single_attempt: Option<SingleAttempt>,
    ) -> Result<ActiveTx<X>, OrdinaryTxError> {
        if LEGACY_ONLY && !matches!(plan.exchange.initial_rate, TxPhyRate::Legacy(_)) {
            return Err(OrdinaryTxError::UnsupportedPhyRate);
        }

        assert!(
            self.active.is_none(),
            "idle authority excludes an active ordinary transaction"
        );
        let hardware_frame_length = plan
            .frame_length
            .checked_add(plan.hardware_mic_length + TX_FCS_SIZE)
            .ok_or(OrdinaryTxError::BufferSizeOverflow)?;
        let transfer_length = TX_METADATA_SIZE
            .checked_add(hardware_frame_length)
            .ok_or(OrdinaryTxError::BufferSizeOverflow)?;
        let minimum_capacity = transfer_length
            .checked_add(3)
            .map(|length| length & !3)
            .ok_or(OrdinaryTxError::BufferSizeOverflow)?;
        let descriptor_capacity = plan.descriptor_capacity.unwrap_or(
            u32::try_from(minimum_capacity).map_err(|_| OrdinaryTxError::BufferSizeOverflow)?,
        );
        if usize::try_from(descriptor_capacity).map_err(|_| OrdinaryTxError::BufferSizeOverflow)?
            < transfer_length
            || descriptor_capacity as usize > BUFFER_SIZE
        {
            return Err(OrdinaryTxError::BufferSizeOverflow);
        }
        let hardware_frame_length = u32::try_from(hardware_frame_length)
            .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?;
        let (receiver, response) = {
            let buffer = self.free_slot().into_buffer();
            let address1: [u8; 6] = buffer[TX_METADATA_SIZE + 4..TX_METADATA_SIZE + 10]
                .try_into()
                .expect("an encoded MPDU carries Address 1");
            let receiver = TxReceiver::from_address1(&address1);
            let no_ack = single_attempt.is_some_and(|attempt| attempt.no_ack);
            let response = if receiver == TxReceiver::Group || no_ack {
                MacLegacyTxResponse::None
            } else if buffer[TX_METADATA_SIZE] == BLOCK_ACK_REQUEST_FRAME_CONTROL {
                MacLegacyTxResponse::BlockAck
            } else {
                MacLegacyTxResponse::Ack
            };
            (receiver, response)
        };
        if response == MacLegacyTxResponse::BlockAck
            && !matches!(plan.exchange.initial_rate, TxPhyRate::Legacy(_))
        {
            return Err(OrdinaryTxError::BlockAckRequestRate);
        }
        // Only the legacy program publishes the response field; an HT or HE
        // publication of a unicast frame would still wait for its ACK.
        if receiver == TxReceiver::Individual
            && response == MacLegacyTxResponse::None
            && !matches!(plan.exchange.initial_rate, TxPhyRate::Legacy(_))
        {
            return Err(OrdinaryTxError::Tx(TxError::Invalid));
        }
        let retry = X::MpduRetry::new(
            plan.interface.role(),
            LegacyTxQueue::from_access_category(plan.exchange.access_category),
            plan.exchange.initial_rate,
            retry_rate_policy,
            plan.exchange.publication_limit,
            if hardware_frame_length > u32::from(X::RTS_THRESHOLD_BYTES) {
                FrameClass::Long
            } else {
                FrameClass::Short
            },
        )?;
        {
            let buffer = self.free_slot().into_buffer();
            buffer[..4].copy_from_slice(&hardware_frame_length.to_le_bytes());
            buffer[4..TX_METADATA_SIZE].fill(0);
            buffer[TX_METADATA_SIZE + plan.frame_length
                ..TX_METADATA_SIZE + hardware_frame_length as usize]
                .fill(0);
        }

        let mut active = ActiveTx {
            external_contention: false,
            cookie: TxCookie(0),
            single_attempt,
            retry,
            frame_length: plan.frame_length,
            descriptor_capacity,
            transfer_length: u32::try_from(transfer_length)
                .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?,
            route: ActiveTxRoute::new(
                LegacyTxQueue::from_access_category(plan.exchange.access_category),
                plan.interface,
            ),
            hardware_mic_length: plan.hardware_mic_length,
            hardware_key_selector: plan.hardware_key_selector,
            scheduler_priority: plan.scheduler_priority,
            packet_priority: plan.packet_priority,
            priority_count: plan.priority_count,
            receiver,
            response,
            protection: TxProtectionDecision::UNPROTECTED,
            completion_timeout: plan.exchange.publication_timeout,
            deadline: oer_time::Instant::EPOCH,
            phase: OrdinaryTxPhase::Published,
            retries: OrdinaryTxRetryReport::default(),
        };
        self.free_slot().reset_work();
        self.publish_attempt(hardware, &mut active)?;
        self.last_outcome = None;
        Ok(active)
    }

    /// Consume one IRQ/deadline edge and retain or release DMA ownership.
    ///
    /// This call never waits. A timeout starts a retained abort-settle phase
    /// and returns `Pending`; the caller waits until `next_deadline`
    /// (or another event) before servicing again. An early wake cannot finish
    /// abort or release the descriptor.
    fn service_active<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        let active = self
            .active
            .take()
            .expect("active authority retains the ordinary transaction");
        let step = self.service_step(hardware, active, wake)?;
        Ok(self.settle(step))
    }

    fn service_step<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        active: ActiveTx<X>,
        wake: WifiTxWake,
    ) -> Result<Step<X>, OrdinaryTxError> {
        if matches!(active.phase, OrdinaryTxPhase::AbortSettling) {
            return self.service_abort_settle(hardware, active);
        }
        let interrupt_events = match wake {
            WifiTxWake::Interrupt { events } => events,
            WifiTxWake::Deadline => 0,
        };
        let tx_events = interrupt_events
            & (oer_esp32s31_ieee80211_mac::irq::EVENT_TX_COMPLETE
                | oer_esp32s31_ieee80211_mac::irq::EVENT_TX_TIMEOUT);
        if tx_events.count_ones() > 1 {
            return self
                .reset_required(active, TxResetReason::ConflictingInterruptEvents(tx_events));
        }

        let observation = self.acknowledge_completion(hardware, &active)?;
        let diagnostic = matches!(observation, Some(CompletionObservation::NoEffect));
        if let Some(event) = observation {
            match event {
                CompletionObservation::Observed((completion, block_ack)) => {
                    self.slot
                        .as_mut()
                        .detach_completed(hardware, active.cookie)?;
                    let finished = active.finish(completion, block_ack);
                    return self.finish_completion(hardware, finished, completion, block_ack);
                }
                CompletionObservation::NoEffect => {}
            }
        }

        use oer_esp32s31_ieee80211_mac::irq::{EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT};
        // A global HE BSS-color event is never evidence about this queue.
        // The PAC detaches only when its per-queue collision state is set.
        if self
            .slot
            .as_mut()
            .abort_collision(hardware, active.cookie)?
        {
            return self.finish_aborted_attempt(hardware, active, false);
        }
        if tx_events == EVENT_TX_COMPLETE && !diagnostic {
            return self.reset_required(active, TxResetReason::CompletionInterruptWithoutState);
        }
        let expired = self.timer.now() >= active.deadline;
        if tx_events == EVENT_TX_TIMEOUT || expired {
            if !self
                .slot
                .as_mut()
                .begin_timeout_abort(hardware, active.cookie)?
            {
                let reason = if expired {
                    TxResetReason::ExecutorDeadline
                } else {
                    TxResetReason::TimeoutInterruptWithoutState
                };
                return self.reset_required(active, reason);
            }
            return self.start_abort_settle(active);
        }

        Ok(Step::Pending(active))
    }

    /// Polling adapter used only before the production IRQ runner is active.
    ///
    /// It observes the same completion/timeout registers as the IRQ path and
    /// quarantines the descriptor when an executor deadline expires without a
    /// qualified hardware timeout edge.
    async fn service_active_polling<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        poll_interval: oer_time::Duration,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        let progress = self.poll_once(hardware)?;
        if progress == WifiTxProgress::Pending {
            let next_poll = self.timer.now().saturating_add(poll_interval);
            let deadline = match self.active.as_ref() {
                Some(active) if matches!(active.phase, OrdinaryTxPhase::AbortSettling) => {
                    active.deadline
                }
                _ => self.next_deadline().unwrap_or(next_poll).min(next_poll),
            };
            self.timer.wait_until(deadline).await;
        }
        Ok(progress)
    }

    /// Synchronous pre-IRQ observation using the same retained abort phase.
    fn poll_once<H: TxHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        let active = self
            .active
            .take()
            .expect("active authority retains the ordinary transaction");
        let step = self.poll_step(hardware, active)?;
        Ok(self.settle(step))
    }

    fn poll_step<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        active: ActiveTx<X>,
    ) -> Result<Step<X>, OrdinaryTxError> {
        if matches!(active.phase, OrdinaryTxPhase::AbortSettling) {
            return self.service_abort_settle(hardware, active);
        }
        if let Some(event) = self.acknowledge_completion(hardware, &active)? {
            match event {
                CompletionObservation::Observed((completion, block_ack)) => {
                    self.slot
                        .as_mut()
                        .detach_completed(hardware, active.cookie)?;
                    let finished = active.finish(completion, block_ack);
                    return self.finish_completion(hardware, finished, completion, block_ack);
                }
                CompletionObservation::NoEffect => {}
            }
        }
        if self
            .slot
            .as_mut()
            .begin_timeout_abort(hardware, active.cookie)?
        {
            return self.start_abort_settle(active);
        }
        if self.timer.now() >= active.deadline {
            return self.reset_required(active, TxResetReason::ExecutorDeadline);
        }
        Ok(Step::Pending(active))
    }

    /// Take the queue's completion; a BlockAckReq's completion carries the
    /// BlockAck it solicited, sampled before the edge is acknowledged.
    fn acknowledge_completion<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        active: &ActiveTx<X>,
    ) -> Result<Option<OrdinaryCompletion>, OrdinaryTxError> {
        if active.response != MacLegacyTxResponse::BlockAck {
            return Ok(self
                .slot
                .as_mut()
                .acknowledge_completion(hardware)?
                .map(|event| event.map(|completion| (completion, None))));
        }
        Ok(self
            .slot
            .as_mut()
            .acknowledge_block_ack_completion(hardware)?
            .map(|event| {
                event.map(|completion| {
                    (
                        completion.tx,
                        completion.valid_block_ack().map(|_| completion.block_ack),
                    )
                })
            }))
    }

    fn start_abort_settle(&mut self, mut active: ActiveTx<X>) -> Result<Step<X>, OrdinaryTxError> {
        // Start the interval after the hardware abort request has completed.
        let Some(deadline) = self.timer.now().checked_add(TX_ABORT_SETTLE) else {
            self.slot.as_mut().require_reset(active.cookie)?;
            return Err(OrdinaryTxError::DeadlineOverflow);
        };
        active.phase = OrdinaryTxPhase::AbortSettling;
        active.deadline = deadline;
        Ok(Step::Pending(active))
    }

    fn service_abort_settle<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        active: ActiveTx<X>,
    ) -> Result<Step<X>, OrdinaryTxError> {
        // Late completion and repeated IRQs cannot bypass the hardware settle
        // interval. Abort detach, not completion acknowledgement, owns release.
        if self.timer.now() < active.deadline {
            return Ok(Step::Pending(active));
        }
        self.slot
            .as_mut()
            .finish_timeout_abort(hardware, active.cookie)?;
        self.finish_aborted_attempt(hardware, active, true)
    }

    fn finish_completion<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        finished: FinishedTx<X>,
        completion: TxCompletion,
        block_ack: Option<HtBlockAckObservation>,
    ) -> Result<Step<X>, OrdinaryTxError> {
        let FinishedTx { mut active, status } = finished;
        let attempts = active.retry.publications();
        let final_rate = active.retry.current_rate()?;
        let decision = if active.single_attempt.is_some() {
            // One publication is the whole transaction: the retry state
            // never sees the completion, so it can never re-publish.
            if status == oer_ieee80211_port::TxStatus::Success {
                self.policy
                    .record_success(active.route.role(), active.route.queue());
            } else if !active.external_contention {
                active.retry.abort(&mut self.policy);
            }
            OrdinaryRetryDecision::Complete
        } else {
            active.retry.observe_completion(&mut self.policy, status)
        };
        match decision {
            OrdinaryRetryDecision::Complete => {
                let success = status == oer_ieee80211_port::TxStatus::Success;
                let report = OrdinaryTxReport {
                    status: MacTxStatus {
                        result: if success {
                            MacTxResult::Transmitted
                        } else {
                            MacTxResult::HardwareFailure(status)
                        },
                        attempts,
                        final_rate,
                        acknowledged: (active.receiver == TxReceiver::Individual
                            && active.response != MacLegacyTxResponse::None)
                            .then_some(success),
                        ack_snr_db: completion.ack_snr_sample(),
                        airtime_micros: None,
                    },
                    completion: Some(completion),
                    retries: active.retries,
                    protection: active.protection,
                    block_ack: if success { block_ack } else { None },
                };
                let outcome = if success {
                    OrdinaryTxOutcome::Success(OrdinaryTxCompleted {
                        completion,
                        status,
                        report,
                    })
                } else {
                    OrdinaryTxOutcome::HardwareFailure(OrdinaryTxCompleted {
                        completion,
                        status,
                        report,
                    })
                };
                Ok(Step::Complete(outcome))
            }
            OrdinaryRetryDecision::Retry { set_retry_bit } => {
                use oer_ieee80211_port::TxStatus;
                match status {
                    TxStatus::AckTimeout => {
                        active.retries.ack_timeouts = active.retries.ack_timeouts.saturating_add(1);
                    }
                    TxStatus::CtsTimeout => {
                        active.retries.cts_timeouts = active.retries.cts_timeouts.saturating_add(1);
                    }
                    TxStatus::Collision => {
                        active.retries.collisions = active.retries.collisions.saturating_add(1);
                    }
                    TxStatus::Success | TxStatus::TimedOut => {}
                }
                if set_retry_bit {
                    mark_retry(self.free_slot().into_buffer(), active.frame_length)?;
                }
                self.publish_attempt(hardware, &mut active)?;
                Ok(Step::Pending(active))
            }
        }
    }

    fn finish_aborted_attempt<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        mut active: ActiveTx<X>,
        timeout: bool,
    ) -> Result<Step<X>, OrdinaryTxError> {
        let attempts = active.retry.publications();
        let final_rate = active.retry.current_rate()?;
        if timeout {
            active.retry.abort(&mut self.policy);
            let report = OrdinaryTxReport {
                status: MacTxStatus {
                    result: MacTxResult::HardwareTimeout,
                    attempts,
                    final_rate,
                    acknowledged: (active.receiver == TxReceiver::Individual
                        && active.response != MacLegacyTxResponse::None)
                        .then_some(false),
                    ack_snr_db: None,
                    airtime_micros: None,
                },
                completion: None,
                retries: active.retries,
                protection: active.protection,
                block_ack: None,
            };
            let outcome = OrdinaryTxOutcome::HardwareTimeout(report);
            return Ok(Step::Complete(outcome));
        }

        let decision = if active.single_attempt.is_some() {
            if !active.external_contention {
                active.retry.abort(&mut self.policy);
            }
            OrdinaryCollisionDecision::Complete
        } else {
            active.retry.observe_collision(&mut self.policy)
        };
        match decision {
            OrdinaryCollisionDecision::Retry => {
                active.retries.collisions = active.retries.collisions.saturating_add(1);
                self.publish_attempt(hardware, &mut active)?;
                Ok(Step::Pending(active))
            }
            OrdinaryCollisionDecision::Complete => {
                let report = OrdinaryTxReport {
                    status: MacTxStatus {
                        result: MacTxResult::CollisionLimit,
                        attempts,
                        final_rate,
                        acknowledged: None,
                        ack_snr_db: None,
                        airtime_micros: None,
                    },
                    completion: None,
                    retries: active.retries,
                    protection: active.protection,
                    block_ack: None,
                };
                let outcome = OrdinaryTxOutcome::CollisionLimit(report);
                Ok(Step::Complete(outcome))
            }
        }
    }

    fn publish_attempt<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        active: &mut ActiveTx<X>,
    ) -> Result<(), OrdinaryTxError> {
        let deadline = self
            .timer
            .now()
            .checked_add(active.completion_timeout)
            .ok_or(OrdinaryTxError::DeadlineOverflow)?;
        let rate = active.retry.current_rate()?;
        let metadata = self.free_slot().into_buffer();
        let head = <&mut [u8; WifiTxMetadataHead::SIZE]>::try_from(
            &mut metadata[..WifiTxMetadataHead::SIZE],
        )
        .expect("TX metadata begins with its fixed head");
        // The HE S-MPDU bit selects the single-MPDU container geometry; the
        // low bits keep the MPDU+MIC+FCS length.
        let mut head = WifiTxMetadataHeadMut::new(head);
        let control = head
            .control()
            .with_he_smpdu(matches!(rate, TxPhyRate::He(_)));
        head.set_control(control);
        let cookie = self
            .free_slot()
            .reserve(active.descriptor_capacity, active.transfer_length)?;
        let protection = match self.submit_reserved_attempt(hardware, cookie, active) {
            Ok(protection) => protection,
            Err(error) => {
                self.slot.as_mut().cancel_reservation(cookie)?;
                return Err(error);
            }
        };
        active.protection = protection;
        active.cookie = cookie;
        active.deadline = deadline;
        Ok(())
    }

    /// Program one reserved publication and return its selected protection.
    fn submit_reserved_attempt<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
        active: &ActiveTx<X>,
    ) -> Result<TxProtectionDecision, OrdinaryTxError> {
        let queue = active.route.queue();
        let role = active.route.role();
        let rate = active.retry.current_rate()?;
        let contention = self.policy.contention_parameters(role, queue);
        let contention_window = match active.single_attempt {
            Some(attempt) => attempt.backoff_slots,
            None => self
                .policy
                .select_backoff(role, queue, self.entropy.next_u32()),
        };
        let ceiling = active
            .single_attempt
            .and_then(|attempt| attempt.power_ceiling_dbm);
        let psdu_length = u16::try_from(
            active
                .frame_length
                .checked_add(active.hardware_mic_length + TX_FCS_SIZE)
                .ok_or(OrdinaryTxError::BufferSizeOverflow)?,
        )
        .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?;
        match rate {
            TxPhyRate::Legacy(rate) => {
                let decision = if LEGACY_ONLY {
                    match active.single_attempt {
                        None => self.policy.protection().select_legacy(
                            rate,
                            active.receiver,
                            TxPsdu::Mpdu {
                                length: psdu_length.into(),
                            },
                        ),
                        Some(attempt) => TxProtectionDecision {
                            protection: match attempt.protection {
                                SingleAttemptProtection::None => TxProtection::None,
                                SingleAttemptProtection::RtsCts => TxProtection::RtsCts {
                                    rate: self
                                        .policy
                                        .protection()
                                        .control_rate(TxPhyRate::Legacy(rate)),
                                },
                                SingleAttemptProtection::CtsToSelf => TxProtection::CtsToSelf {
                                    rate: self
                                        .policy
                                        .protection()
                                        .control_rate(TxPhyRate::Legacy(rate)),
                                },
                            },
                            ..TxProtectionDecision::UNPROTECTED
                        },
                    }
                } else {
                    self.select_protection(
                        active.single_attempt,
                        active.receiver,
                        TxPhyRate::Legacy(rate),
                        psdu_length.into(),
                    )
                };
                let mut config = LegacyTxConfig::management(rate, psdu_length);
                config.control = self.control_frame(TxPhyRate::Legacy(rate), decision, ceiling);
                config.data_power =
                    capped_power(self.power.power_pair(rate.code()), ceiling).primary as u8;
                config.aifsn = contention.aifsn();
                config.contention_window = contention_window;
                config.scheduler_priority = active.scheduler_priority;
                config.pti = active.packet_priority;
                config.pti_count = active.priority_count;
                config.response = active.response;
                config.hardware_key_selector = active.hardware_key_selector;
                config.interface = active.route.mac_interface();
                self.slot
                    .as_mut()
                    .submit_legacy(hardware, cookie, queue, config)?;
                Ok(decision)
            }
            TxPhyRate::Ht(rate) if !LEGACY_ONLY => {
                let decision = self.select_protection(
                    active.single_attempt,
                    active.receiver,
                    TxPhyRate::Ht(rate),
                    psdu_length.into(),
                );
                let frame_length = u16::try_from(active.frame_length)
                    .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?;
                let mic_length = u8::try_from(active.hardware_mic_length)
                    .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?;
                let mut config = HtTxConfig::single_mpdu(rate, frame_length, mic_length)
                    .ok_or(OrdinaryTxError::BufferSizeOverflow)?;
                let data_power =
                    capped_power(self.power.power_pair(rate.power_lookup_code()), ceiling);
                config.data_power_primary = data_power.primary as u8;
                config.data_power_alternate = data_power.alternate as u8;
                config.control = self.control_frame(TxPhyRate::Ht(rate), decision, ceiling);
                config.protection_spacing = self.policy.ht_ampdu().protection_spacing();
                config.aifsn = contention.aifsn();
                config.contention_window = contention_window;
                config.scheduler_priority = active.scheduler_priority;
                config.pti = active.packet_priority;
                config.pti_count = active.priority_count;
                config.hardware_key_selector = active.hardware_key_selector;
                config.interface = active.route.mac_interface();
                self.slot
                    .as_mut()
                    .submit_ht(hardware, cookie, queue, config)?;
                Ok(decision)
            }
            TxPhyRate::He(rate) if !LEGACY_ONLY => {
                let mpdu_length = u16::try_from(
                    active
                        .frame_length
                        .checked_add(active.hardware_mic_length)
                        .ok_or(OrdinaryTxError::BufferSizeOverflow)?,
                )
                .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?;
                let mut config =
                    HeSmpduTxConfig::new(rate, self.policy.he_bss_color(), mpdu_length)
                        .ok_or(OrdinaryTxError::BufferSizeOverflow)?;
                let decision = self.select_protection(
                    active.single_attempt,
                    active.receiver,
                    TxPhyRate::He(rate),
                    config.apep_length().into(),
                );
                let data_power =
                    capped_power(self.power.power_pair(rate.power_lookup_code()), ceiling);
                config.data_power_primary = data_power.primary as u8;
                config.data_power_alternate = data_power.alternate as u8;
                config.control = self.control_frame(TxPhyRate::He(rate), decision, ceiling);
                config.aifsn = contention.aifsn();
                config.contention_window = contention_window;
                config.scheduler_priority = active.scheduler_priority;
                config.pti = active.packet_priority;
                config.pti_count = active.priority_count;
                config.hardware_key_selector = active.hardware_key_selector;
                config.interface = active.route.mac_interface();
                self.slot
                    .as_mut()
                    .submit_he_smpdu(hardware, cookie, queue, config)?;
                Ok(decision)
            }
            _ => Err(OrdinaryTxError::UnsupportedPhyRate),
        }
    }

    /// Select protection and its control frame for one PPDU which another
    /// owner (an aggregate path) publishes under this BSS policy.
    pub fn control_frame_for(&self, ppdu: ProtectedPpdu) -> (TxProtectionDecision, TxControlFrame) {
        let decision = self.policy.protection().select(ppdu);
        (decision, control_frame(&self.power, ppdu.rate, decision))
    }

    fn select_protection(
        &self,
        single_attempt: Option<SingleAttempt>,
        receiver: TxReceiver,
        rate: TxPhyRate,
        psdu_length: u32,
    ) -> TxProtectionDecision {
        if let Some(chosen) = single_attempt {
            let protection = match chosen.protection {
                SingleAttemptProtection::None => TxProtection::None,
                SingleAttemptProtection::RtsCts => TxProtection::RtsCts {
                    rate: self.policy.protection().control_rate(rate),
                },
                SingleAttemptProtection::CtsToSelf => TxProtection::CtsToSelf {
                    rate: self.policy.protection().control_rate(rate),
                },
            };
            return TxProtectionDecision {
                protection,
                ..TxProtectionDecision::UNPROTECTED
            };
        }
        self.policy.protection().select(ProtectedPpdu {
            rate,
            receiver,
            psdu: TxPsdu::Mpdu {
                length: psdu_length,
            },
        })
    }

    fn control_frame(
        &self,
        data: TxPhyRate,
        decision: TxProtectionDecision,
        ceiling: Option<i8>,
    ) -> TxControlFrame {
        capped_control_frame(&self.power, data, decision, ceiling)
    }

    /// Keep a step's result in the owner for the classic, owner-held path.
    fn settle(&mut self, step: Step<X>) -> WifiTxProgress {
        match step {
            Step::Pending(active) => {
                self.active = Some(active);
                WifiTxProgress::Pending
            }
            Step::Complete(outcome) => {
                self.last_outcome = Some(outcome);
                WifiTxProgress::Complete
            }
        }
    }

    fn reset_required(
        &mut self,
        active: ActiveTx<X>,
        reason: TxResetReason,
    ) -> Result<Step<X>, OrdinaryTxError> {
        self.slot.as_mut().require_reset(active.cookie)?;
        Err(OrdinaryTxError::RadioResetRequired(reason))
    }
}

/// What one step leaves: the attempt still published, or its outcome. The
/// queued path receives exactly this value, so no state is re-read later.
enum Step<X: BackendTxProfile> {
    Pending(ActiveTx<X>),
    Complete(OrdinaryTxOutcome),
}

/// Return to the executor once, then complete.
fn yield_once() -> impl Future<Output = ()> {
    let mut yielded = false;
    core::future::poll_fn(move |context| {
        if yielded {
            core::task::Poll::Ready(())
        } else {
            yielded = true;
            context.waker().wake_by_ref();
            core::task::Poll::Pending
        }
    })
}

impl<'slot, X: BackendTxProfile, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, const N: usize>
    OrdinaryTxOwner<'slot, X, P, E, T, N, false>
{
    pub fn new(resources: WifiTxResources<'slot, X, P, E, T, N>) -> Self {
        Self::from_resources(resources)
    }
}
impl<'slot, X: BackendTxProfile, P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, const N: usize>
    OrdinaryTxOwner<'slot, X, P, E, T, N, true>
{
    /// An IRQ-context transmitter whose publication path admits legacy rates
    /// only and excludes HE floating-point protection calculations.
    pub fn new_legacy(resources: WifiTxResources<'slot, X, P, E, T, N>) -> Self {
        Self::from_resources(resources)
    }
}
