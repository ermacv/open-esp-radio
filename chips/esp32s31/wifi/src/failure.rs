//! Facade-owned failures. Every source variant and observation is retained.
//! No backend, HAL, STA or RSN type appears in this public vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendFault {
    Core(LowerMacFault),
    Receive(ReceiveFault),
    Capacity,
    Clock,
    Retune(RetuneError),
    Abandoned,
    /// The data arenas quarantined themselves.
    Arena(ArenaFault),
    TuneNotRecorded,
    HardwareUnavailable,
    /// The lower-MAC core was not in the view a step needed.
    CoreViewUnavailable(CoreOperation),
    /// A hardware queue completed twice in one service.
    DuplicateCompletion {
        queue: u8,
    },
    /// The core removed more keys than were installed.
    RemovalOverflow,
    /// A queue completed with no MPDU published on it.
    CompletionWithoutPending {
        queue: u8,
    },
    /// A completed descriptor held no memory, or its place was gone.
    DescriptorMissing {
        queue: u8,
    },
    /// A completion returned memory its buffer had not published.
    ReturnRefused {
        queue: u8,
    },
    /// A delivered BlockAckReq came without its BlockAck report.
    BlockAckReportMissing {
        queue: u8,
    },
    /// An ended MPDU recorded no transmission.
    AirMissing {
        queue: u8,
    },
    /// An attempt's transmission or retry count overflowed.
    AttemptCounter {
        queue: u8,
    },
    /// An attempt's retry plan had no stage at its position.
    StageMissing {
        queue: u8,
    },
    /// A queue held more attempts than there are tickets.
    QueueFull {
        queue: u8,
    },
    /// The receiver would hold more units than the port allows.
    ReceiveOverflow,
    /// A received unit's MPDU lay outside the unit.
    ReceiveRange,
    /// A received buffer was dropped without its recycle.
    ReceiveLost,
    /// The data arenas' step completed an MPDU or removed a key.
    ArenaCoreContract,
    /// The installation's descriptors, chains, tickets or buffers were not
    /// all available.
    InstallationResources,
    /// The core refused the EDCA table.
    EdcaUnsupported,
    /// The regulatory domain did not permit the channel being started.
    ChannelNotPermitted,
    /// A key a command named was not installed, or no key place was free.
    KeySlotMissing(KeyOperation),
    /// Received units were returned or lent beyond the count held.
    ReceiveAccounting,
    /// A command the data port held at its barrier passed its bound.
    CommandExpired(PortCommandKind),
}
/// The lower-MAC operation a missing core view stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreOperation {
    Command,
    Tbtt,
    Arena,
    Publish,
}
/// The key operation that found no key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyOperation {
    Install,
    Remove,
    Replace,
    Select,
    Transmit,
}
/// The kind of a data-port command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortCommandKind {
    InstallPairwise,
    InstallGroup,
    ReplaceGroup,
    SelectGroup,
    RemovePairwise,
    RemoveGroup,
    Configure,
    CloseGate,
    SetEdca,
    SetBssColor,
}
/// Why the data arenas quarantined themselves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaFault {
    /// A descriptor chain refused a lifecycle or range edge.
    Chain(AggregateTxError),
    /// The lower MAC core is poisoned.
    Core(LowerMacFault),
    /// An operation disagreed with an arena's phase.
    State,
    /// The publication watchdog expired and no timeout abort could begin.
    Watchdog,
}
/// Why an A-MPDU descriptor chain refused or quarantined an edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateTxError {
    NotFree(TxDmaState),
    SlotCapacity,
    Stale,
    Completion(CompletionFault),
    FrameIndexOutOfRange { index: u8, count: u8 },
    BackingUnavailable { index: u8 },
    SlotCountOverflow { count: usize },
    InvalidStorageGeometry { slots: usize },
    TxProgramUnavailable { format: AggregateFormat },
    InvalidTriggerReservation { queue: usize, subframes: u8 },
    FrameTooLong,
    TooFewFrames,
    AggregateFull,
    Length(AggregateLengthError),
    AggregateConfigurationMismatch,
    QueueActive,
    TriggerMsduLengthUnavailable,
    TriggerBased(TriggerProgramError),
    DetachFailed,
    TimeoutNotPending,
    ResetRequired,
    DmaStorage(ChainStorageError),
}
/// The DMA state of a TX descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxDmaState {
    Free,
    Reserved,
    HardwareOwned,
    Completed,
    ResetRequired,
}
/// The PPDU format of an aggregate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateFormat {
    Ht,
    He,
}
/// Why an aggregate's length rule refused it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateLengthError {
    InvalidLimits,
    Empty,
    ZeroMpduLength,
    WindowFull,
    AggregateTooLong(u32),
}
/// Why a Trigger-based queue program was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerProgramError {
    TidNotEligible,
    LengthCountMismatch,
    InvalidMpduLength,
    QueuedBytesTooLarge,
}
/// Why an aggregate's descriptor chain refused an edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChainStorageError {
    Address,
    Count,
    GenerationExhausted,
    InvalidLength,
    StaleBacking,
    State,
    /// The chain's owner no longer held its arenas.
    Retired,
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
    KeyBank,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseCause {
    /// The stopped device still held a live or failed radio frontier.
    NotStopped,
    /// The MAC or its RX walker was still active; nothing was posted.
    StillActive(MaintenanceError),
    /// The radio actor never answered the leave request.
    LeaveUnanswered([u32; 4]),
    /// The leave completed into a poisoned radio domain.
    DomainPoisoned(RadioPoison),
    /// The stop's deadline passed before the datapath actor's task answered
    /// the release. The task keeps what it was handed; a later answer is
    /// never read, and nothing is reused.
    Deadline,
}
/// Why the station's lifecycle, or the datapath actor under it, ended
/// with a cause the image records before it resets. A restart of the
/// station never ends here: it reinstalls the port and builds the service
/// again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleFailure {
    /// The radio did not start on its first channel.
    StartRefused(oer_protocols_ieee80211_contracts_port::StartRefused),
    /// A stop proved nothing by its deadline: what was still out.
    StopUnconfirmed(oer_protocols_ieee80211_runtime::datapath::StopObserved),
    /// A retune's stop proved nothing.
    RetuneUnconfirmed(oer_protocols_ieee80211_contracts_port::StopUnconfirmed),
    /// The data port broke its contract: its named fault.
    Port(BackendFault),
    /// The datapath actor broke its own contract.
    Actor(oer_protocols_ieee80211_runtime::datapath::ActorViolation),
    /// The actor returned an owner the lifecycle did not send.
    Contract(oer_protocols_ieee80211_runtime::lifecycle::LifecycleViolation),
    /// The port did not reinstall after the station's restart.
    Reinstall(ReinstallCause),
    /// The datapath task's news came out of its order.
    Mailbox,
}
/// Why the port did not reinstall after the station's restart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReinstallCause {
    /// The stopped port still ran, faulted or held work.
    NotRetired,
    /// The TSF epoch repeated.
    SameEpoch,
    /// The port's channel named no Wi-Fi channel.
    ChannelUnset,
    /// The installation receipt was refused.
    Receipt,
    /// The retired driver kept its transmitter or a descriptor.
    Resources,
    /// The TSF epochs are used up.
    Epochs,
}
#[cfg(any(test, target_arch = "riscv32"))]
pub(crate) mod convert;
mod path;
#[cfg(test)]
mod tests;
