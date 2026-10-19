//! Direct Test Mode instances.
//!
//! One instance holds the DTM link state, the scheduler context, one
//! scheduler item, the TX header and packet and the private two-header
//! receive graph. DTM selects the memory-manager bypass, so it receives
//! outside the global lists: a successful receiver event returns one packet
//! through the private graph's two-slot rotation.
//!
//! An event lowers one positional image; after the item returns, finishing
//! the event recycles the item and, for a successful receiver event, rotates
//! the receive graph and reports the received packet.

#![forbid(unsafe_code)]

mod codec;

use core::{convert::Infallible, num::NonZeroU32};

use vcell::VolatileCell;

use crate::{
    dtm_event_image::{
        DtmPositionalEventWords, DtmRxHeaderTailProjection, DtmTxHeaderHeadProjection,
    },
    dtm_rx_result::{DtmRxResultProjection, DtmRxResultProjectionError},
    le_tx_packet::LeTxPacketPreparedLength,
    scheduler_pool::{
        SchedulerPoolBindError, SchedulerPoolError, SchedulerRoleInstance, SchedulerRoleKind,
        SchedulerRolePool, SchedulerRoleStorage, sealed,
    },
    sram_link::ControllerSramLinkAddress,
};

pub use codec::{
    BLUETOOTH_DTM_LINK_STATE_BYTES, BLUETOOTH_DTM_MAX_PACKET_CAPACITY,
    BLUETOOTH_DTM_RX_PACKET_BYTES, BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES,
    BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES, BLUETOOTH_DTM_TX_PACKET_BYTES, DtmBinding, DtmStorage,
};

use codec::{DtmRxRotationPlan, LeTestPduHeader};

/// Why a lower-layer selector cannot form a standard LE Test PDU header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmTxPacketPrepareError {
    /// The LE Test PDU Type field admits exactly the eight standard DTM types.
    UnsupportedPayloadType,
}

/// Snapshot supplied to one in-place positional event-word builder.
///
/// Both private links are sampled from this graph's current link-state words,
/// not reconstructed from an earlier binding or another graph. The builder is
/// invoked while the unique graph owner is consumed, so its output cannot be
/// applied later to a different owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmPositionalEventSeed {
    words: DtmPositionalEventWords,
    tx_header_head: DtmTxHeaderHeadProjection,
    rx_header_tail: DtmRxHeaderTailProjection,
}

impl DtmPositionalEventSeed {
    /// Return the current values of exactly the nineteen writable words.
    pub const fn words(self) -> DtmPositionalEventWords {
        self.words
    }

    /// Return the current graph-bound TX-header head projection.
    pub const fn tx_header_head_projection(self) -> DtmTxHeaderHeadProjection {
        self.tx_header_head
    }

    /// Return the current graph-bound RX-header tail projection.
    pub const fn rx_header_tail_projection(self) -> DtmRxHeaderTailProjection {
        self.rx_header_tail
    }
}

/// Why CPU-owned positional event words were not committed to the graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmPrepareError<BuildError = Infallible> {
    Pool(SchedulerPoolError),
    /// The operation does not follow the instance's preparation state.
    State,
    /// The upper builder rejected its semantic inputs before any graph write.
    Build(BuildError),
    /// The current private TX-head word contains the unbound zero link.
    CurrentTxHeadUnbound,
    /// The current private TX head does not name this graph's bound TX header.
    CurrentTxHeadIdentityMismatch,
    /// The selected TX header no longer names this graph's packet allocation.
    CurrentTxHeaderPacketBaseMismatch,
    /// The selected TX header no longer names this graph's LE Test PDU.
    CurrentTxHeaderPduTargetMismatch,
    /// The selected TX header lost the reviewed full-capacity allocation profile.
    CurrentTxHeaderAllocationExtentMismatch,
    /// The current private RX-tail word contains the unbound zero link.
    CurrentRxTailUnbound,
    /// The current private RX tail names neither of this graph's two RX headers.
    CurrentRxTailIdentityMismatch,
    /// The selected RX-tail header no longer names this graph's packet allocation.
    CurrentRxTailPacketMismatch,
    /// Link-state `+0x00` does not retain this graph's freshly sampled TX head.
    LinkStateTxHeadMismatch {
        /// Current private-chain projection required by this graph.
        expected: DtmTxHeaderHeadProjection,
        /// Candidate projection returned by the builder.
        observed: DtmTxHeaderHeadProjection,
    },
    /// Link-state `+0x08` does not retain this graph's freshly sampled RX tail.
    LinkStateRxTailMismatch {
        /// Current private-chain projection required by this graph.
        expected: DtmRxHeaderTailProjection,
        /// Candidate projection returned by the builder.
        observed: DtmRxHeaderTailProjection,
    },
    /// Scheduler-item `+0x08` no longer points to this graph's link-state.
    SchedulerItemLinkStateMismatch,
}

/// DTM terminal observation: hardware completion or a proven stopped sentinel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmSchedulerItemCompletionStatus {
    /// The item left the scheduler without executing: it was cancelled,
    /// deleted with its list or stopped.
    Aborted,
    /// The role-specific accounting path accepts positional status zero.
    Zero,
    /// Hardware reported a positional nonzero status.
    NonZero(NonZeroU32),
}

/// Why the two-slot receive rotation of a successful event was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmRxRotationError {
    /// The specialized path received a non-success scheduler status.
    CompletionStatusMismatch,
    /// The private RX head names neither bound header slot.
    RxHeadIdentityMismatch,
    /// The private RX tail names neither bound header slot.
    RxTailIdentityMismatch,
    /// The detached reserve does not name the other bound header slot.
    RxSwapIdentityMismatch,
    /// The initial detached reserve still carries a list or packet link.
    ReserveNotDetached,
    /// The initial packet-bearing header unexpectedly carries a backlink.
    InitialBacklinkUnexpected,
    /// A steady two-header chain unexpectedly retains a detached reserve.
    SwapReserveUnexpected,
    /// The steady predecessor still owns the packet.
    PredecessorPacketStillBound,
    /// The steady predecessor does not lead to the packet-bearing tail.
    PredecessorSuccessorMismatch,
    /// The steady predecessor was not returned by the prior event.
    PredecessorNotCompleted,
    /// The steady tail backlink does not name its exact predecessor.
    SuccessorBacklinkMismatch,
    /// The packet-bearing returned tail unexpectedly has a successor.
    ReturnedHasSuccessor,
    /// The returned tail does not retain this graph's sole RX packet.
    ReturnedPacketMismatch,
    /// Hardware left the packet result in its re-arm sentinel state.
    ReturnedResultNotProduced,
    /// Hardware left the positional auxiliary halfword in its re-arm sentinel state.
    ReturnedAuxiliaryNotProduced,
}

/// Proof, private to this module, that the DTM item executed and left the
/// scheduler. The receive graph is read only under it.
pub(super) struct DtmExecutedWitness(());

/// Preparation state of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum DtmState {
    Idle,
    Packet(LeTxPacketPreparedLength<BLUETOOTH_DTM_TX_PACKET_BYTES>),
    Event,
}

impl sealed::Sealed for DtmStorage {}

impl SchedulerRoleStorage for DtmStorage {
    const KIND: SchedulerRoleKind = SchedulerRoleKind::DirectTestMode;
    const ITEMS: usize = 1;
    const NUMBERS: usize = 1;
    const NEW: Self = Self::new();
    type Binding = DtmBinding;
    type State = DtmState;
    const INITIAL_STATE: DtmState = DtmState::Idle;

    fn bind(base: u32, number: u16) -> Result<DtmBinding, SchedulerPoolBindError> {
        DtmBinding::new(base, number)
    }

    fn item_words(&self, _item: usize) -> &[VolatileCell<u32>] {
        self.scheduler_item.words()
    }

    fn item_link(binding: &DtmBinding, _item: usize) -> ControllerSramLinkAddress {
        binding.scheduler_item_address()
    }

    fn reinitialize(&mut self, binding: &DtmBinding) -> DtmState {
        self.initialize_reviewed_allocation(binding);
        DtmState::Idle
    }

    fn admits(state: &DtmState, item: usize) -> bool {
        item == 0 && matches!(state, DtmState::Event)
    }
}

/// Pool of DTM instances.
pub type DtmPool<const N: usize> = SchedulerRolePool<DtmStorage, N>;

/// Why a DTM operation was refused. Nothing changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmError {
    Pool(SchedulerPoolError),
    /// The operation does not follow the instance's preparation state.
    State,
    Packet(DtmTxPacketPrepareError),
    /// The receive graph does not form the reviewed rotation; it is left
    /// unchanged.
    Rotation(DtmRxRotationError),
}

/// What a finished DTM event recorded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmEventResult {
    pub status: DtmSchedulerItemCompletionStatus,
    /// The packet returned by a successful receiver event.
    pub received: Option<Result<DtmRxResultProjection, DtmRxResultProjectionError>>,
}

impl<const N: usize> DtmPool<N> {
    /// Install one complete LE Test packet image. Bytes after the declared
    /// payload stay unchanged.
    pub fn prepare_tx_packet(
        &mut self,
        instance: &SchedulerRoleInstance,
        payload_type: u8,
        payload_length: u8,
        payload: &[u8; BLUETOOTH_DTM_MAX_PACKET_CAPACITY],
    ) -> Result<(), DtmError> {
        let cpu = self.cpu(instance).map_err(DtmError::Pool)?;
        if matches!(cpu.state, DtmState::Event) {
            return Err(DtmError::State);
        }
        let header = LeTestPduHeader::without_cte(payload_type).map_err(DtmError::Packet)?;
        let length = cpu
            .graph
            .prepare_tx_packet(header, &payload[..usize::from(payload_length)]);
        *cpu.state = DtmState::Packet(length);
        Ok(())
    }

    /// The prepared packet prefix and payload.
    pub fn prepared_packet_bytes(&self, instance: &SchedulerRoleInstance) -> Option<&[u8]> {
        let (graph, _, state) = self.shared(instance).ok()?;
        match *state {
            DtmState::Packet(length) => Some(graph.prepared_tx_packet_bytes(length)),
            _ => None,
        }
    }

    /// Build and commit one positional event image from the current words
    /// and private links. Every rejection leaves the instance unchanged; a
    /// transmitter event needs its packet prepared again afterwards.
    pub fn prepare_event<BuildError>(
        &mut self,
        instance: &SchedulerRoleInstance,
        build: impl FnOnce(DtmPositionalEventSeed) -> Result<DtmPositionalEventWords, BuildError>,
    ) -> Result<(), DtmPrepareError<BuildError>> {
        let cpu = self.cpu(instance).map_err(DtmPrepareError::Pool)?;
        if matches!(cpu.state, DtmState::Event) {
            return Err(DtmPrepareError::State);
        }
        let seed = cpu.graph.positional_event_seed(cpu.binding)?;
        let candidate = build(seed).map_err(DtmPrepareError::Build)?;
        cpu.graph
            .validate_and_commit_positional_event(cpu.binding, seed, candidate)?;
        *cpu.state = DtmState::Event;
        Ok(())
    }

    /// The positional words while the instance is quiescent.
    pub fn words(&self, instance: &SchedulerRoleInstance) -> Option<DtmPositionalEventWords> {
        let (graph, _, _) = self.shared(instance).ok()?;
        Some(graph.reviewed_event_words())
    }

    /// Recycle the item after it returned and, for a successful receiver
    /// event, rotate the receive graph.
    ///
    /// `receiver` selects the receive rotation. A refused rotation leaves the
    /// instance in its event state.
    pub fn finish_event(
        &mut self,
        instance: &SchedulerRoleInstance,
        receiver: bool,
    ) -> Result<DtmEventResult, DtmError> {
        let cpu = self.cpu(instance).map_err(DtmError::Pool)?;
        if !matches!(cpu.state, DtmState::Event) {
            return Err(DtmError::State);
        }
        let status = cpu
            .graph
            .observe_completion_status()
            .unwrap_or(DtmSchedulerItemCompletionStatus::Aborted);
        let executed = DtmExecutedWitness(());
        let plan: Option<DtmRxRotationPlan> =
            if receiver && status == DtmSchedulerItemCompletionStatus::Zero {
                Some(
                    cpu.graph
                        .validate_rx_rotation(cpu.binding, &executed, status)
                        .map_err(DtmError::Rotation)?,
                )
            } else {
                None
            };
        cpu.graph.commit_scheduler_recycle();
        let received = plan.and_then(|plan| {
            let received = plan.projection();
            cpu.graph.commit_rx_rotation(cpu.binding, plan);
            received
        });
        *cpu.state = DtmState::Idle;
        Ok(DtmEventResult { status, received })
    }
}

#[cfg(test)]
mod tests;
