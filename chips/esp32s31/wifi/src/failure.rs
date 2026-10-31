//! Facade-owned failures. Every source variant and observation is retained.
//! No backend, HAL, STA or RSN type appears in this public vocabulary.
use oer_ieee80211_runtime::station::{BackendContractViolation, PendingTerminal, Request};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationFailure {
    PolicyDeadlineOverflow,
    /// A station request arrived without its kind's admission.
    CommandAdmission(Request),
    RxBlockAckCapacity,
    RxBlockAckEncode,
    EnableRefused(BackendFault),
    Backend(BackendFault),
    Submission(BackendFault),
    Contract(BackendContractViolation),
    DrainedWithOutstanding {
        cause: BackendFault,
        missing: PendingTerminal,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendFault {
    Core(LowerMacFault),
    Receive(ReceiveFault),
    Deadline,
    Capacity,
    Clock,
    Retune(RetuneError),
    Abandoned,
    UnexpectedAggregate,
    TuneNotRecorded,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowerMacFault {
    Tx(OrdinaryTxError),
    RxBlockAckReadback,
    Key(CryptoKeyError),
    RxBlockAck(RxBlockAckError),
    NoRxBlockAckBank,
    Abandoned,
    KeyUseMismatch,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryTxError {
    BufferSizeOverflow,
    UnsupportedPhyRate,
    DeadlineOverflow,
    Tx(TxError),
    RadioResetRequired(TxResetReason),
    BlockAckRequestRate,
    InvalidRetryHeader,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxError {
    Invalid,
    QueueActive,
    Stale,
    DetachFailed,
    TimeoutNotPending,
    ResetRequired,
    Completion(CompletionFault),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionFault {
    SecurityKeyError,
    InvalidStatus { status: u8 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxResetReason {
    CompletionInterruptWithoutState,
    TimeoutInterruptWithoutState,
    ConflictingInterruptEvents(u32),
    ExecutorDeadline,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CryptoKeyError {
    InvalidGroupKeyId,
    InvalidAccessPointAssociationId,
    Occupied,
    HardwareRejected,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxBlockAckError {
    HardwareIndex(u8),
    MulticastPeer,
    Tid(u8),
    Window(u16),
    HardwareReadbackMismatch,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiveFault {
    StopRequested,
    Prepare(RxRingError),
    LineRefused,
    Start(RxFrontierError),
    Service(RxStageTransactionError),
    ForeignExecutor,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxRingError {
    Empty,
    Count,
    Address,
    DescriptorOwnerAddress,
    Size,
    Overflow,
    OutstandingLeases,
    BufferOwnershipConflict,
    ReloadPending,
    WalkerStartUnconfirmed,
    WalkerStopUnconfirmed,
    Corrupt,
    ResetRequired,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxFrontierError {
    AlreadyStarted,
    OwnerUnavailable,
    Ring(RxRingError),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxStageError {
    Exhausted,
    Empty,
    TooLong,
    Chained,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxStageTransactionError {
    Stage(RxStageError),
    Ring(RxRingError),
    InvalidCreditReserve {
        reserved: u64,
        pool_slots: u64,
        queue_depth: u64,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetuneError {
    NotStopped(MaintenanceError),
    Radio(RadioFailure),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceError {
    MacActive { state: u8 },
    RxWalkerEnabled,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioFailure {
    Refused,
    ClientFailed,
    Poisoned(RadioPoison),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioPoison {
    pub origin: RadioOperation,
    pub cause: RadioPoisonCause,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioOperation {
    TrackingTick,
    Prepare,
    CloseRf,
    Request {
        client: RadioClient,
        request: RadioRequest,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioRequest {
    Join,
    Track,
    Retune,
    Leave,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioClient {
    Wifi,
    Bluetooth,
    Ieee802154,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioPoisonCause {
    TrackingRefused(PhyDomainError),
    Tracking(PhyFault),
    CalibrationTracking(CalibrationTrackingFault),
    Channel(PhyFault),
    Rf(PhyFault),
    Registration(RegistrationFault),
    Domain(PhyDomainError),
    CommonPower(CommonRadioPowerError),
    ModemClock(ModemClockError),
    Rollback,
    Controller,
    Custody,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFault {
    EdgeTimedOut,
    CapabilityUnavailable,
    HardwareInvariant,
    RfOperationLimit,
    UnexpectedBinding,
    RegistrationEpochMismatch,
    DeadlineOverflow,
    Sequence,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalibrationTrackingFault {
    Step(PhyFault),
    Radio(CalibrationTrackingFailure),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalibrationTrackingFailure {
    PbusClearTimedOut,
    Dcode,
    RxGain,
    Channel,
    TxDcPwdet,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationFault {
    Step(PhyFault),
    Lowering {
        stage: PhyRegisterStage,
        error: PhyRegisterBindingError,
    },
    Radio(RegistrationFailure),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationFailure {
    I2cMasterResetTimedOut { index: u8, samples: u16 },
    Rf,
    Baseband,
    Temperature,
    FinalI2cDeadlineExceeded,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterStage {
    Mmio,
    Delay,
    I2cMasterResetSample,
    Rf,
    Baseband,
    Temperature,
    FinalI2cRead,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyDomainError {
    NotRegistered,
    AlreadyRegistered,
    TrackingPending,
    NoTrackingPending,
    Poisoned,
    RfClosed,
    RfOpen,
    EpochMismatch,
    ClientsActive,
    Clock(ModemClockError),
    Acquire(PhyClientAcquireError),
    Time(PhyTrackTimeError),
    ClientAbsent(RadioClient),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTrackTimeError {
    DeadlineOverflow {
        class: PhyPllTrackClass,
    },
    TimeReversed {
        class: PhyPllTrackClass,
        previous_micros: u64,
        now_micros: u64,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyClientAcquireError {
    TrackingTime(PhyTrackTimeError),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPllTrackClass {
    Wifi,
    BluetoothIeee802154,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommonRadioPowerError {
    AlreadyEntered,
    Power(PowerError),
    Restore(PowerRestoreCheckpoint),
    Release(PlatformClock),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemClockError {
    AlreadyEnabled,
    NotEnabled,
    Rejected,
    Poisoned,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerError {
    pub checkpoint: PowerCheckpoint,
    pub expected: bool,
    pub observed: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerCheckpoint {
    Mpll,
    ResetReleased,
    HpActiveIcg,
    ModemBusClock,
    HpActiveClockMap,
    SharedClockMap,
    Reference160m,
    ModemClockSource,
    PhyClocks,
    I2cSource,
    I2cClock,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerRestoreCheckpoint {
    ModemSyscon,
    ModemSourceClocks,
    ModemRegisterBusClock,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformClock {
    Pll160m,
    AnalogI2cMaster,
    Mpll,
    ModemCoexistence,
    ModemLowPowerTimer,
}
/// A station that ended for uninstall whose installation fail-stopped
/// instead of retiring: the slot stays quarantined and the radio is not
/// released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetirementFailure {
    /// The station's own failure, when it ended with one.
    pub failure: Option<StationFailure>,
    /// Why the ordinary retirement did not complete.
    pub cause: RetirementCause,
    /// The P2 owner confirmed the hardware stopped before quarantine.
    pub hardware_stopped: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetirementCause {
    Cancelled,
    FailStopRequested,
    Construction,
    BodyAlreadyIssued,
    Deadline,
    StopUnconfirmed,
    InvalidState(SlotObservation),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlotObservation {
    pub phase: u8,
    pub storage: SlotStorage,
    pub invocation_present: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotStorage {
    Empty,
    Registered,
    Vacant,
    Unavailable,
}
/// A retired installation whose Wi-Fi radio release did not complete:
/// every owner the release did not return stays quarantined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleaseFailure {
    /// The station's own failure, when it ended with one.
    pub failure: Option<StationFailure>,
    pub cause: ReleaseCause,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseCause {
    /// The radio driver task never answered the stop handoff; the
    /// observations are its request kind, slot, epoch and sequence.
    HandoffLost([u32; 4]),
    /// The joined slot refused retirement.
    Retire(RetirementCause),
    /// The stopped device still held a live or failed radio frontier.
    NotStopped,
    /// The MAC or its RX walker was still active; nothing was posted.
    StillActive(MaintenanceError),
    /// The radio actor never answered the leave request.
    LeaveUnanswered([u32; 4]),
    /// The leave completed into a poisoned radio domain.
    DomainPoisoned(RadioPoison),
}
#[cfg(any(test, target_arch = "riscv32"))]
pub(crate) mod convert;
#[cfg(test)]
mod tests;
