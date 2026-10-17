#![no_std]
#![forbid(unsafe_code)]

//! Radio-owned boundary between durable software backlog and physical TX.
//!
//! Network integrations may retain packets in different memory and ownership
//! models. Radio admission accepts opaque selected requests; packet-oriented
//! policy additionally uses affine Ethernet owners and batch materialization.
//! Selection precedes scarce SRAM admission without exposing a network stack,
//! executor or adapter queue to STA/AP policy.

use oer_memory::{
    DmaIndexReturn, PinnedDmaTxRadioLease, ReturningStableDmaBacking, StableDmaBacking,
    TaggedStableDmaBacking,
};
use oer_network_interface::NetworkInterfaceId;

pub mod airtime;
#[cfg(any(test, feature = "model"))]
pub mod memory;

mod queues;

pub use queues::TxQueues;

/// Two owners that must cross a materialization boundary atomically.
pub type FramePair<Frame> = (Frame, Frame);

/// Result of atomically materializing two software-owned frames.
pub type MaterializedPairResult<SoftwareFrame, PhysicalFrame> =
    Result<FramePair<PhysicalFrame>, FramePair<SoftwareFrame>>;

/// Diagnostic snapshot of the bounded physical materialization horizon.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MaterializationOwnershipSnapshot {
    pub free: usize,
    pub radio_owned: usize,
}

/// One affine software-owned Ethernet frame selected for radio service.
///
/// Implementations retain their original allocation until materialization or
/// drop. The radio may inspect bytes for peer/TID/lifecycle classification,
/// but physical DMA ownership is represented only by
/// [`SelectedBurstMaterializer::PhysicalFrame`].
pub trait SoftwareTxFrame {
    fn interface(&self) -> NetworkInterfaceId;

    fn ethernet(&self) -> &[u8];

    fn as_slice(&self) -> &[u8] {
        self.ethernet()
    }
}

/// A request selected for one logical interface's radio service.
///
/// The request may retain a complete Ethernet owner or identify deferred work.
/// It does not grant DMA ownership or require Ethernet bytes to exist yet.
/// The source owns validation of deferred request identity and lifetime.
pub trait TxRequest {
    fn interface(&self) -> NetworkInterfaceId;
}

impl<F: SoftwareTxFrame> TxRequest for F {
    fn interface(&self) -> NetworkInterfaceId {
        SoftwareTxFrame::interface(self)
    }
}

/// Final DMA-stable owner containing one materialized Ethernet frame.
///
/// The radio encoder needs both the logical Ethernet view and its offset in
/// the stable backing when it builds descriptor references. Keeping this
/// contract beside materialization avoids depending on one SRAM-pool lease
/// type while retaining exact, typed DMA geometry.
pub trait MaterializedTxFrame: StableDmaBacking {
    /// Upper bound for the Ethernet length of every owner of this type.
    /// Radio admission may use this before removing a frame from its source.
    const MAX_ETHERNET_LENGTH: usize;

    /// Storage bytes guaranteed by every owner of this type. Heterogeneous
    /// backings must report their smallest capacity, not their largest one.
    const MIN_STORAGE_CAPACITY: usize;

    fn ethernet(&self) -> &[u8];

    fn ethernet_offset(&self) -> usize;

    fn ethernet_length(&self) -> usize {
        self.ethernet().len()
    }

    fn as_slice(&self) -> &[u8] {
        self.ethernet()
    }

    fn storage_mut(&mut self) -> &mut [u8] {
        self.stable_dma_region().into_mut_slice()
    }
}

impl<R: DmaIndexReturn, const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    MaterializedTxFrame
    for TaggedStableDmaBacking<
        NetworkInterfaceId,
        ReturningStableDmaBacking<PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>, R>,
    >
{
    const MAX_ETHERNET_LENGTH: usize = FRAME_CAPACITY;
    const MIN_STORAGE_CAPACITY: usize = HEADROOM + FRAME_CAPACITY + TRAILER;

    fn ethernet(&self) -> &[u8] {
        core::ops::Deref::deref(self).ethernet()
    }

    fn ethernet_offset(&self) -> usize {
        core::ops::Deref::deref(self).ethernet_offset()
    }

    fn ethernet_length(&self) -> usize {
        core::ops::Deref::deref(self).ethernet_length()
    }
}

/// Synchronous source of final physical frame owners for one selected egress.
///
/// The source may construct a frame on demand or transfer one already prepared
/// in SRAM. Radio aggregation must not require a complete software Ethernet
/// representation. A failed take must preserve pending work, and must never
/// wait for another core or executor while holding physical credits.
pub trait PhysicalTxSource {
    type Frame: MaterializedTxFrame;

    /// Pending frames, including work which still needs a physical credit.
    fn pending_frames(&self) -> usize;

    fn try_take_physical(&self) -> Option<Self::Frame>;
}

impl<M: SelectedBurstMaterializer> PhysicalTxSource for M {
    type Frame = M::PhysicalFrame;

    fn pending_frames(&self) -> usize {
        self.queue_len()
    }

    fn try_take_physical(&self) -> Option<Self::Frame> {
        self.try_materialize_next()
    }
}

/// Narrow radio admission boundary shared by packet and deferred-work sources.
///
/// A failed materialization returns the exact request without consuming its
/// pending work. A successful operation supplies the first physical frame;
/// subsequent physical takes stay within the selected source's scope.
pub trait TxRequestSource: PhysicalTxSource {
    type Request: TxRequest;

    fn interface(&self) -> NetworkInterfaceId;

    fn try_materialize(&self, request: Self::Request) -> Result<Self::Frame, Self::Request>;

    fn materialization_capacity(&self) -> usize;

    fn ownership_snapshot(&self) -> MaterializationOwnershipSnapshot;
}

impl<M: SelectedBurstMaterializer> TxRequestSource for M {
    type Request = M::SoftwareFrame;

    fn interface(&self) -> NetworkInterfaceId {
        SelectedBurstMaterializer::interface(self)
    }

    fn try_materialize(&self, request: Self::Request) -> Result<Self::Frame, Self::Request> {
        SelectedBurstMaterializer::try_materialize(self, request)
    }

    fn materialization_capacity(&self) -> usize {
        SelectedBurstMaterializer::materialization_capacity(self)
    }

    fn ownership_snapshot(&self) -> MaterializationOwnershipSnapshot {
        SelectedBurstMaterializer::ownership_snapshot(self)
    }
}

/// Synchronous, batch-oriented admission from software ownership into the
/// fixed physical radio horizon.
///
/// Implementations must reserve every requested destination before consuming
/// any source owner. A failed operation returns or retains every source
/// unchanged. No method may wait for another executor/core while holding a
/// physical credit.
pub trait SelectedBurstMaterializer {
    type SoftwareFrame: SoftwareTxFrame;
    type PhysicalFrame: MaterializedTxFrame;

    fn interface(&self) -> NetworkInterfaceId;

    fn queue_len(&self) -> usize;

    fn try_take(&self) -> Option<Self::SoftwareFrame>;

    /// Optional direct access to queues classified before radio selection.
    /// FIFO-only compatibility adapters leave this unavailable.
    fn destination_queues(&self) -> Option<&dyn DestinationTxQueues<Frame = Self::SoftwareFrame>> {
        None
    }

    fn try_materialize(
        &self,
        frame: Self::SoftwareFrame,
    ) -> Result<Self::PhysicalFrame, Self::SoftwareFrame>;

    /// Reserve one physical credit before removing the next software owner.
    fn try_materialize_next(&self) -> Option<Self::PhysicalFrame>;

    fn materialization_capacity(&self) -> usize;

    fn ownership_snapshot(&self) -> MaterializationOwnershipSnapshot;

    /// Materialize one selected batch atomically with respect to source
    /// ownership. `destinations` must be empty on entry. On `false`, it stays
    /// empty and every occupied source remains in place.
    fn try_materialize_batch<const BATCH: usize>(
        &self,
        sources: &mut [Option<Self::SoftwareFrame>; BATCH],
        destinations: &mut [Option<Self::PhysicalFrame>; BATCH],
    ) -> bool;

    fn try_materialize_pair(
        &self,
        first: Self::SoftwareFrame,
        second: Self::SoftwareFrame,
    ) -> MaterializedPairResult<Self::SoftwareFrame, Self::PhysicalFrame> {
        let mut sources = [Some(first), Some(second)];
        let mut destinations = [const { None }; 2];
        if !self.try_materialize_batch(&mut sources, &mut destinations) {
            return Err((
                sources[0].take().expect("failed pair retains first owner"),
                sources[1].take().expect("failed pair retains second owner"),
            ));
        }
        Ok((
            destinations[0]
                .take()
                .expect("successful pair publishes first owner"),
            destinations[1]
                .take()
                .expect("successful pair publishes second owner"),
        ))
    }
}

/// Destination selection over software owners, before physical admission.
///
/// Destinations are Ethernet next hops, not association or security identities.
/// Radio policy must still validate association lifetime, power save and TID.
/// Implementations may schedule transport subflows inside one destination;
/// counts and readiness still cover all of its aggregate-eligible frames.
/// Current AP producers use TID 0; this interface does not assign additional
/// TIDs. A missing match leaves every other destination untouched.
pub trait DestinationTxQueues {
    type Frame: SoftwareTxFrame;

    /// Inspect the first destination strictly after `after` in address order,
    /// wrapping to the lowest address. None means the source is empty. The
    /// radio can merge this candidate with retained demand in the same order,
    /// without depending on either source's metadata slots or moving owners.
    fn next_head_after(&self, after: Option<[u8; 6]>) -> Option<([u8; 6], DestinationTxHead)>;
    /// Read one coherent next-packet length/backlog snapshot. Inspection must
    /// not dequeue, rotate transport flows, return credits or alter any wait.
    /// The implementation's queue lock, if any, ends before this call returns.
    /// This is demand metadata, not a reservation: link teardown, another
    /// consumer or later publication may change the next dequeue. The radio
    /// must revalidate the claimed frame before encoding or publication.
    fn head_for(&self, destination: [u8; 6]) -> Option<DestinationTxHead>;
    fn pending_for(&self, destination: [u8; 6]) -> usize {
        self.head_for(destination)
            .map_or(0, |head| head.pending_frames)
    }
    fn try_take_for(&self, destination: [u8; 6]) -> Option<Self::Frame>;
    /// Register and check, atomically with publication, that any destination
    /// has a frame: the wait of a radio that selects among destinations
    /// itself, whichever one becomes ready.
    fn poll_ready_any(&self, context: &mut core::task::Context<'_>) -> core::task::Poll<()>;
    /// Register and check the selected queue atomically with publication.
    fn poll_ready_for(
        &self,
        destination: [u8; 6],
        minimum: usize,
        context: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()>;
}

/// CPU-only demand metadata, with no packet address or ownership transfer.
/// Ethernet length includes its L2 header, not Wi-Fi/crypto/FCS expansion.
/// A radio cost estimate must supply that geometry and its current PHY rate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DestinationTxHead {
    pub ethernet_bytes: usize,
    /// Total packets across all transport flows of this destination.
    pub pending_frames: usize,
}
