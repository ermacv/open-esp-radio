use oer_esp32s31_ieee80211_mac::rx::{
    RxObservedMask, RxRingError, RxRingHalted, RxRingLive, RxRingStopped, RxRingTopologySnapshot,
};

/// Hardware-valid phase retained by the finite RX frontier owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxFrontierPhase {
    Halted,
    Prepared,
    Live,
    Vacant,
}

pub(super) enum RxFrontierState<'storage, const COUNT: usize> {
    Halted(RxRingHalted<'storage, COUNT>),
    Prepared(RxRingStopped<'storage, COUNT>),
    Live(RxRingLive<'storage, COUNT>),
    Vacant,
}

impl<const COUNT: usize> RxFrontierState<'_, COUNT> {
    pub(super) const fn phase(&self) -> RxFrontierPhase {
        match self {
            Self::Halted(_) => RxFrontierPhase::Halted,
            Self::Prepared(_) => RxFrontierPhase::Prepared,
            Self::Live(_) => RxFrontierPhase::Live,
            Self::Vacant => RxFrontierPhase::Vacant,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxFrontierError {
    AlreadyStarted,
    OwnerUnavailable,
    Ring(RxRingError),
}

/// Complete owner return when a finite frontier cannot be promoted into the
/// connected live ring.
pub struct RxFrontierIntoLiveFailure<'storage, const COUNT: usize, const DMA_BUFFER_SIZE: usize> {
    pub owner: ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
    pub error: RxFrontierError,
}

/// Decision made while observing one completed descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxFrontierDirective {
    Continue,
    /// End this bounded service pass after recycling every descriptor already
    /// observed by it. The live RX epoch remains owned by this phase.
    Pause,
    /// Retain the terminal descriptor for a typed transfer into another
    /// protocol phase. This is not a scheduling/batching directive.
    Stop,
}

/// Scheduler edge required after one finite RX ownership observation.
///
/// The vendor `datapathProcessRxSucDataAll` consumes one PP event and remains in
/// its descriptor loop while refreshing the hardware LAST frontier. Rust may
/// cooperatively yield while waiting for a safe ownership proof, but it must
/// not turn that yield into a requirement for another RX interrupt.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RxFrontierContinuation {
    #[default]
    AwaitInterrupt,
    ProbePending,
}

/// Read-only scheduler state retained before a live RX owner is halted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxFrontierSchedulerSnapshot {
    pub recycle_start: usize,
    pub accepted_tail: usize,
    pub observed_mask: RxObservedMask,
    pub topology: RxRingTopologySnapshot,
}

/// Finite progress returned by one descriptor service transaction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxFrontierProgress {
    pub completed: u32,
    pub stopped: bool,
}

/// Progress of one complete descriptor-frontier service transaction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxFrontierServiceProgress {
    pub completed_descriptors: u32,
    pub recycled_descriptors: u32,
    pub reload_pending: bool,
    pub service_probe_pending: bool,
}

impl From<RxRingError> for RxFrontierError {
    fn from(error: RxRingError) -> Self {
        Self::Ring(error)
    }
}

/// Unique RX-ring owner shared by all finite role phases.
pub struct ReceiveFrontier<'storage, const COUNT: usize, const DMA_BUFFER_SIZE: usize> {
    pub(super) state: RxFrontierState<'storage, COUNT>,
}
