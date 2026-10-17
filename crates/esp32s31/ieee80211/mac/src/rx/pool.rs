//! Bounded descriptor-retaining receive ownership.
//!
//! The production physical ring has 96 buffers and upper handoff admits at most 32.
//! A completed buffer moves `Dma -> Radio -> Network -> Released`; only the
//! ring owner can turn the final state back into `Dma`. Thus upper processing
//! never copies an MPDU and cannot consume the 64-descriptor capacity retained
//! in the radio ownership domain. That capacity includes completed descriptors
//! awaiting a software drain and is not itself an armed-credit guarantee.

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::rx::{RxPhyInfo, decode_normalized_rx_metadata};
use oer_esp32s31_ieee80211_dma::{
    rx_ring::{RxRingError, RxSegment},
    rx_storage::{RxDmaCompletedUnit, RxDmaDetachedUnit},
};
use oer_ieee80211_softmac::MacRxMetadata;
use oer_memory::{ExternalRxHandoffPool, ExternalRxRadioLease};

pub const VENDOR_LARGE_RX_SLOT_COUNT: usize = 32;
pub const VENDOR_LARGE_RX_PAYLOAD_CAPACITY: usize = 1_700;

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
    /// Admission must leave at least one ordinary credit in both domains.
    InvalidCreditReserve {
        reserved: usize,
        pool_slots: usize,
        queue_depth: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxInPlaceEthernetError {
    PayloadOutsideFrame,
    InsufficientHeadroom,
}

/// Result after a completed DMA unit has either transferred its original
/// buffer or remained in the observed prefix for immediate discard reclaim.
pub enum RxDmaDeferredStageUnitOutcome<'pool, const SLOTS: usize, const CAPACITY: usize> {
    Staged(NetworkRxFrame<'pool, SLOTS, CAPACITY>),
    Discarded(RxStageError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StagedMetadata {
    descriptor_address: u32,
    descriptor_word0: u32,
    next_descriptor_address: u32,
    length: usize,
}

/// Fixed token storage for descriptor-backed upper ownership.
#[derive(bytemuck::Zeroable)]
pub struct RxStagePool<const SLOTS: usize, const CAPACITY: usize> {
    external: ExternalRxHandoffPool<CAPACITY, SLOTS>,
    next_claim: AtomicUsize,
}

impl<const SLOTS: usize, const CAPACITY: usize> RxStagePool<SLOTS, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            external: ExternalRxHandoffPool::new(),
            next_claim: AtomicUsize::new(0),
        }
    }

    oer_esp32s31_ieee80211_dma::place_rx_hot_path! {
    /// Transfer one completed single-descriptor DMA buffer to the upper RX
    /// owner without copying payload bytes.
    ///
    /// Empty, oversized and chained units remain in the ring's observed
    /// prefix and follow the caller's frozen-LAST discard/reclaim path. A
    /// valid unit remains detached until its final network token is dropped;
    /// the ring owner later rearms only the contiguous released prefix.
    #[inline(never)]
    pub fn stage_dma_unit_deferred_bounded<
        'pool,
        'storage,
        'owner,
        'ring,
        const COUNT: usize,
        const BUFFER_SIZE: usize,
        const STORAGE_SIZE: usize,
    >(
        &'pool self,
        completed: RxDmaCompletedUnit<
            'storage,
            'owner,
            'ring,
            COUNT,
            BUFFER_SIZE,
            STORAGE_SIZE,
        >,
        maximum_length: usize,
    ) -> Result<RxDmaDeferredStageUnitOutcome<'pool, SLOTS, CAPACITY>, RxStageTransactionError>
    where
        'storage: 'static,
    {
        let length = completed.total_length();
        let discard = if completed.descriptor_count() != 1 {
            Some(RxStageError::Chained)
        } else if length == 0 {
            Some(RxStageError::Empty)
        } else if length > CAPACITY.min(maximum_length) {
            Some(RxStageError::TooLong)
        } else {
            None
        };
        if let Some(error) = discard {
            completed.retain_for_deferred_recycle();
            return Ok(RxDmaDeferredStageUnitOutcome::Discarded(error));
        }

        let detached = completed
            .detach_single()
            .map_err(RxStageTransactionError::Ring)?;
        self.stage_detached_unit(detached, maximum_length)
            .map(RxDmaDeferredStageUnitOutcome::Staged)
    }}

    oer_esp32s31_ieee80211_dma::place_rx_hot_path! {
    /// Admit an allocation detached from its completed descriptor.
    ///
    /// The descriptor remains observed while this affine allocation is owned
    /// above DMA. Returning the network lease marks the allocation released;
    /// the sole ring owner can then rearm the exact descriptor/buffer pair.
    #[inline(never)]
    pub fn stage_detached_unit(
        &self,
        detached: RxDmaDetachedUnit,
        maximum_length: usize,
    ) -> Result<NetworkRxFrame<'_, SLOTS, CAPACITY>, RxStageTransactionError> {
        let length = detached.length();
        if length == 0 {
            return Err(RxStageTransactionError::Stage(RxStageError::Empty));
        }
        if length > CAPACITY.min(maximum_length) {
            return Err(RxStageTransactionError::Stage(RxStageError::TooLong));
        }
        let metadata = detached_metadata(&detached);
        let start = self.next_claim.load(Ordering::Relaxed);
        let lease = match self.external.try_claim_radio(detached.into_buffer(), start) {
            Ok(lease) => lease,
            Err(buffer) => {
                drop(buffer);
                return Err(RxStageTransactionError::Stage(RxStageError::Exhausted));
            }
        };
        let next = if lease.index() + 1 == SLOTS {
            0
        } else {
            lease.index() + 1
        };
        self.next_claim.store(next, Ordering::Relaxed);
        Ok(NetworkRxFrame {
            lease,
            _slots: PhantomData,
            metadata,
            times: None,
        })
    }}

    pub fn claimed_slots(&self) -> u32 {
        self.external.claimed_slots() as u32
    }

    /// Number of original DMA buffers which can still transfer to upper
    /// ownership without consuming the radio-reserved half-ring.
    pub fn available_slots(&self) -> usize {
        SLOTS.saturating_sub(self.claimed_slots() as usize)
    }

    pub fn network_slots(&self) -> u32 {
        self.external.network_slots() as u32
    }

    pub const fn external_handoff_pool(&self) -> &ExternalRxHandoffPool<CAPACITY, SLOTS> {
        &self.external
    }
}

impl<const SLOTS: usize, const CAPACITY: usize> Default for RxStagePool<SLOTS, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

fn detached_metadata(detached: &RxDmaDetachedUnit) -> StagedMetadata {
    StagedMetadata {
        descriptor_address: detached.descriptor_address(),
        descriptor_word0: detached.descriptor_word0(),
        next_descriptor_address: 0,
        length: detached.length(),
    }
}

/// Unique upper-layer owner of one original receive DMA buffer.
pub struct NetworkRxFrame<'pool, const SLOTS: usize, const CAPACITY: usize> {
    lease: ExternalRxRadioLease<'pool, CAPACITY>,
    _slots: PhantomData<[(); SLOTS]>,
    metadata: StagedMetadata,
    times: Option<RxTimes>,
}

/// When the image saw one received frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxTimes {
    /// The monotonic time of the frame's first executor-visible handoff.
    pub handoff: oer_time::Instant,
    /// The frame's receive timestamp: the raw MAC local time the hardware
    /// wrote into its RX-control prefix (`rx::decode_rx_local_timestamp`).
    pub stamp: Option<u32>,
}

impl<const SLOTS: usize, const CAPACITY: usize> NetworkRxFrame<'_, SLOTS, CAPACITY> {
    /// Stable index of this unique lease in its integration-owned pool.
    pub fn slot(&self) -> usize {
        self.lease.index()
    }

    pub fn segment(&self) -> RxSegment<'_> {
        RxSegment {
            descriptor_address: self.metadata.descriptor_address,
            descriptor_word0: self.metadata.descriptor_word0,
            buffer: self.lease.frame(),
            next_descriptor_address: self.metadata.next_descriptor_address,
        }
    }

    /// Record the frame's first handoff; a later handoff keeps it.
    pub fn mark_handoff(&mut self, times: RxTimes) {
        if self.times.is_none() {
            self.times = Some(times);
        }
    }

    /// The times of the first handoff, once marked.
    pub const fn times(&self) -> Option<RxTimes> {
        self.times
    }

    pub fn normalized_metadata(&self) -> Option<MacRxMetadata<RxPhyInfo>> {
        decode_normalized_rx_metadata(self.segment().buffer)
    }

    pub const fn length(&self) -> usize {
        self.metadata.length
    }

    /// Replace the dead 802.11/CCMP/LLC prefix with an Ethernet header and
    /// publish a view of the same DMA buffer to the network-device consumer.
    #[inline(always)]
    pub fn publish_ethernet_in_place(
        self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload_offset: usize,
        payload_length: usize,
    ) -> Result<u8, (Self, RxInPlaceEthernetError)> {
        let Some(payload_end) = payload_offset.checked_add(payload_length) else {
            return Err((self, RxInPlaceEthernetError::PayloadOutsideFrame));
        };
        if payload_end > self.metadata.length {
            return Err((self, RxInPlaceEthernetError::PayloadOutsideFrame));
        }
        let Some(frame_offset) = payload_offset.checked_sub(14) else {
            return Err((self, RxInPlaceEthernetError::InsufficientHeadroom));
        };
        let ethernet_length = 14 + payload_length;
        let NetworkRxFrame {
            mut lease,
            _slots: _,
            metadata: _,
            times: _,
        } = self;
        lease.with_frame(|raw| {
            raw[frame_offset..frame_offset + 6].copy_from_slice(&destination);
            raw[frame_offset + 6..frame_offset + 12].copy_from_slice(&source);
            raw[frame_offset + 12..frame_offset + 14].copy_from_slice(&ether_type.to_be_bytes());
        });
        Ok(lease.republish(frame_offset, ethernet_length))
    }
}
