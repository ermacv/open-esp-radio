//! Executor-neutral DATAPATH contracts shared by ESP32-S31 Wi-Fi roles.
//!
//! DATAPATH coordinates finite LMAC work without owning AP or STA semantics.
//! Role crates consume these value-only contracts while executor adapters own
//! waiting, queues and task placement.

pub mod lifecycle;

/// Result of one bounded RX bottom-half pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathRxProgress {
    /// The durable completion frontier was drained within this pass.
    Drained,
    /// Completed descriptors remain but the role's independent staging is full.
    ///
    /// Descriptor ownership stays with the completed ring until a capacity
    /// wake allows the same head unit to be staged.
    StageCapacityBlocked,
    /// The finite poll budget ended with another completed DMA unit pending.
    BudgetExhausted,
    /// Upper staging was saturated, but bulk units were deliberately dropped
    /// and their descriptors recycled instead of blocking hardware.
    UpperLayerBlockedButDroppable,
    /// A decoded frame is retained while the final network RX queue is full.
    ///
    /// This is not permission to stop the independent DMA frontier: an
    /// executor adapter must also wake for new RX completions so the role can
    /// use any remaining staging credits to republish descriptors.
    NetworkBackpressured,
    /// A republished terminal descriptor needs another hardware observation.
    ProbePending,
    /// Ordered protocol work is retained until the active physical TX owner
    /// reaches its terminal edge.
    ///
    /// This is not a descriptor-writeback probe and must not manufacture a
    /// software RX repost. New hardware completions may still wake the DMA
    /// producer; after TX completion the role's software-work predicate makes
    /// the retained head runnable without another MAC interrupt.
    ProtocolBlockedByTx,
    /// Descriptors were recycled while the RX interrupt source was masked.
    ///
    /// No completed descriptor or terminal writeback was observed at the end
    /// of the bounded pass. The adapter must nevertheless retain ownership of
    /// the current drain epoch until one later observation proves that the
    /// recycled descriptors did not complete behind the owned interrupt edge.
    RecycledAppendPending,
}

/// Monotonic physical RX work completed by one DMA owner.
///
/// The executor uses a before/after delta to select a bounded continuation
/// window. Keeping units and bytes together distinguishes a low-packet-rate
/// full-MTU stream from a high-packet-rate small-frame stream without parsing
/// protocol headers in the role-neutral scheduler.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DatapathRxWorkCounters {
    pub completed_units: u64,
    pub staged_bytes: u64,
}

impl DatapathRxWorkCounters {
    pub const fn saturating_sub(self, earlier: Self) -> Self {
        Self {
            completed_units: self.completed_units.saturating_sub(earlier.completed_units),
            staged_bytes: self.staged_bytes.saturating_sub(earlier.staged_bytes),
        }
    }
}

/// Coherent scheduler facts supplied to one role-control step.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DatapathControlContext {
    /// A network lease is waiting behind this control boundary.
    pub network_tx_pending: bool,
    /// The finite runner must restore active control state before teardown.
    pub stop_pending: bool,
}

impl DatapathControlContext {
    pub const IDLE: Self = Self {
        network_tx_pending: false,
        stop_pending: false,
    };

    pub const STOPPING: Self = Self {
        network_tx_pending: false,
        stop_pending: true,
    };
}

/// Result of one finite role-control transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathControlProgress<E> {
    Idle,
    More,
    TxPending,
    Exit(E),
}

/// Result of one finite role shutdown transition at an idle TX boundary.
///
/// Roles with no protocol shutdown work use `Stopped` immediately. A role
/// such as an access point may publish a final management transaction and
/// return `TxPending`; DATAPATH drives that transaction to a terminal edge before
/// invoking shutdown again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathStopProgress {
    More,
    TxPending,
    Stopped,
}
