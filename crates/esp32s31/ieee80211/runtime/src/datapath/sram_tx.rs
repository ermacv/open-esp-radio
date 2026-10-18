//! Fixed internal-SRAM execution pool for owned network TX packets.
//!
//! Long-lived packet ownership and queueing stay in the owned Xarxa adapter.
//! This module begins at physical admission: a selected packet is copied once
//! into a pinned DMA-visible slot and that slot remains owned by the radio
//! until terminal completion returns its index.

use core::{
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender, TrySendError},
};
use oer_memory::{
    DmaIndexReturn, PinnedDmaTxPool, PinnedDmaTxRadioLease, ReturningStableDmaBacking,
    TaggedStableDmaBacking,
};

use super::tx_performance::{TX_PERFORMANCE, TxPerformanceSample};
use oer_ieee80211_datapath::MaterializationOwnershipSnapshot;
use oer_ieee80211_datapath::SoftwareTxFrame;
use oer_network_interface::NetworkInterfaceId;

/// Permanently located storage for DMA-visible TX frames.
pub type PinnedTxPool<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

/// Synchronization state for one fixed physical SRAM pool.
pub struct PinnedTxResources<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    free: Channel<M, u8, QUEUE_DEPTH>,
    split: AtomicBool,
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedTxResources<M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            free: Channel::new(),
            split: AtomicBool::new(false),
        }
    }

    /// Bind the synchronization state to its one pinned DMA pool.
    pub fn split<'resources>(
        &'resources mut self,
        pool: Pin<&'resources mut PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>,
    ) -> PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        assert!(QUEUE_DEPTH > 0, "pinned TX pool must not be empty");
        assert!(
            QUEUE_DEPTH <= usize::from(u8::MAX) + 1,
            "pinned TX pool index must fit in u8"
        );
        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "pinned TX resources may only be split once"
        );
        for index in 0..QUEUE_DEPTH {
            self.free
                .try_send(index as u8)
                .expect("an empty free queue accepts every SRAM index");
        }
        let pool = Pin::into_ref(pool).get_ref();
        PinnedTxConsumer {
            free: self.free.sender(),
            free_claim: self.free.receiver(),
            pool,
        }
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> Default for PinnedTxResources<M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Sole radio-side allocator for the fixed physical TX pool.
pub struct PinnedTxConsumer<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    free: Sender<'resources, M, u8, QUEUE_DEPTH>,
    free_claim: Receiver<'resources, M, u8, QUEUE_DEPTH>,
    pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Clone
    for PinnedTxConsumer<'_, M, F, H, T, Q>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Copy
    for PinnedTxConsumer<'_, M, F, H, T, Q>
{
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub const fn for_interface(
        self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    {
        PinnedTxInterfaceConsumer {
            physical: self,
            interface,
        }
    }
}

/// Physical allocator narrowed to one logical network interface.
pub struct PinnedTxInterfaceConsumer<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    physical: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    interface: NetworkInterfaceId,
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Clone
    for PinnedTxInterfaceConsumer<'_, M, F, H, T, Q>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Copy
    for PinnedTxInterfaceConsumer<'_, M, F, H, T, Q>
{
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub const fn interface(self) -> NetworkInterfaceId {
        self.interface
    }

    fn return_reserved(&self, index: u8) {
        if let Err(TrySendError::Full(_)) = self.physical.free.try_send(index) {
            unreachable!("unused reservation returns its unique SRAM credit");
        }
    }

    fn promote_reserved<F: SoftwareTxFrame>(
        &self,
        index: u8,
        frame: F,
    ) -> PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        assert_eq!(
            frame.interface(),
            self.interface,
            "owned TX frame crossed its logical interface"
        );
        assert!(
            frame.ethernet().len() <= FRAME_CAPACITY,
            "owned network frame exceeds physical TX capacity"
        );

        let promotion_started = TxPerformanceSample::read();
        let credit_acquired = promotion_started;
        let length = frame.ethernet().len();
        let lease = self.physical.pool.claim_network(index);
        let destination_claimed = TxPerformanceSample::read();
        let publication_started = TxPerformanceSample::read();
        let mut copy = TxPerformanceSample::default();
        let (index, ()) = lease.publish(length, |destination| {
            let copy_started = TxPerformanceSample::read();
            destination.copy_from_slice(frame.ethernet());
            {
                copy = TxPerformanceSample::read().wrapping_delta_since(copy_started);
            }
        });
        let published = TxPerformanceSample::read();
        drop(frame);
        let source_released = TxPerformanceSample::read();
        let promoted = TaggedStableDmaBacking::new(
            self.interface,
            ReturningStableDmaBacking::new(
                self.physical.pool.claim_radio(index),
                PinnedTxReturn {
                    free: self.physical.free,
                },
            ),
        );
        TX_PERFORMANCE.record_promotion(
            length,
            promotion_started,
            credit_acquired,
            destination_claimed,
            copy,
            publication_started,
            published,
            source_released,
            TxPerformanceSample::read(),
        );
        promoted
    }

    pub fn try_materialize<F: SoftwareTxFrame>(
        &self,
        frame: F,
    ) -> Result<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>, F>
    {
        let Ok(index) = self.physical.free_claim.try_receive() else {
            {
                let now = TxPerformanceSample::read();
                TX_PERFORMANCE.record_promotion_no_credit(now, now);
            }
            return Err(frame);
        };
        Ok(self.promote_reserved(index, frame))
    }

    /// Reserve SRAM first, then remove one software owner.
    pub fn try_materialize_from<F: SoftwareTxFrame>(
        &self,
        next: impl FnOnce() -> Option<F>,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>> {
        let index = self.physical.free_claim.try_receive().ok()?;
        let Some(frame) = next() else {
            self.return_reserved(index);
            return None;
        };
        Some(self.promote_reserved(index, frame))
    }

    /// Reserve all occupied destinations before moving any source owner.
    pub fn try_materialize_batch<F: SoftwareTxFrame, const BATCH: usize>(
        &self,
        sources: &mut [Option<F>; BATCH],
        destinations: &mut [Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>;
                 BATCH],
    ) -> bool {
        assert!(
            destinations.iter().all(Option::is_none),
            "owned burst promotion requires empty destinations"
        );
        for source in sources.iter().flatten() {
            assert_eq!(source.interface(), self.interface);
            assert!(source.ethernet().len() <= FRAME_CAPACITY);
        }
        let count = sources.iter().flatten().count();
        let mut reserved = [None; BATCH];
        for slot in reserved.iter_mut().take(count) {
            let Ok(index) = self.physical.free_claim.try_receive() else {
                for index in reserved.iter_mut().filter_map(Option::take) {
                    self.return_reserved(index);
                }
                {
                    let now = TxPerformanceSample::read();
                    TX_PERFORMANCE.record_promotion_no_credit(now, now);
                }
                return false;
            };
            *slot = Some(index);
        }

        let mut next = 0;
        for (source, destination) in sources.iter_mut().zip(destinations.iter_mut()) {
            let Some(source) = source.take() else {
                continue;
            };
            let index = reserved[next]
                .take()
                .expect("one SRAM credit was reserved per source owner");
            next += 1;
            *destination = Some(self.promote_reserved(index, source));
        }
        debug_assert!(reserved.iter().all(Option::is_none));
        true
    }

    pub fn promotion_capacity(&self) -> usize {
        self.physical.free_claim.len()
    }

    pub fn ownership_snapshot(&self) -> MaterializationOwnershipSnapshot {
        let free = self.physical.free_claim.len();
        MaterializationOwnershipSnapshot {
            free,
            radio_owned: QUEUE_DEPTH.saturating_sub(free),
        }
    }
}

/// Queue-return capability paired with a physical DMA lease.
pub struct PinnedTxReturn<'resources, M: RawMutex, const QUEUE_DEPTH: usize> {
    free: Sender<'resources, M, u8, QUEUE_DEPTH>,
}

impl<M: RawMutex, const QUEUE_DEPTH: usize> DmaIndexReturn for PinnedTxReturn<'_, M, QUEUE_DEPTH> {
    fn return_index(&self, index: u8) {
        if let Err(TrySendError::Full(_)) = self.free.try_send(index) {
            unreachable!("radio completion returns its unique SRAM index");
        }
        TX_PERFORMANCE.record_radio_return();
    }
}

type PinnedTxBacking<
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = ReturningStableDmaBacking<
    PinnedDmaTxRadioLease<'resources, FRAME_CAPACITY, HEADROOM, TRAILER>,
    PinnedTxReturn<'resources, M, QUEUE_DEPTH>,
>;

/// One DMA-visible SRAM owner tagged with its logical interface.
pub type PinnedTxFrame<
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = TaggedStableDmaBacking<
    NetworkInterfaceId,
    PinnedTxBacking<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
>;
