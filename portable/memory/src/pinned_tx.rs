//! Permanently located TX storage and its finite ownership transitions.
//!
//! Pinning is a type boundary: only the affine [`PinnedDmaTxPoolHandle`]
//! issues leases. Slot indices still follow the caller's free/ready queues:
//! an out-of-range index or a claim in the wrong state returns an error.
//! Publication lengths must fit the frame capacity before the writer runs.

use core::{
    cell::UnsafeCell,
    marker::{PhantomData, PhantomPinned},
    ops::{Deref, DerefMut},
    pin::Pin,
    ptr,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

use crate::{FrameLen, PoolError, StableDmaBacking, StableDmaRegion};

const SLOT_FREE: u8 = 0;
const SLOT_NETWORK: u8 = 1;
const SLOT_READY: u8 = 2;
const SLOT_RADIO: u8 = 3;
#[repr(C)]
#[derive(bytemuck::Zeroable)]
struct PinnedTxBytes<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> {
    headroom: [u8; HEADROOM],
    ethernet: [u8; FRAME_CAPACITY],
    trailer: [u8; TRAILER],
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    PinnedTxBytes<FRAME_CAPACITY, HEADROOM, TRAILER>
{
    const fn new() -> Self {
        Self {
            headroom: [0; HEADROOM],
            ethernet: [0; FRAME_CAPACITY],
            trailer: [0; TRAILER],
        }
    }
}

#[repr(C, align(16))]
#[derive(bytemuck::Zeroable)]
struct PinnedTxSlot<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> {
    bytes: UnsafeCell<PinnedTxBytes<FRAME_CAPACITY, HEADROOM, TRAILER>>,
    length: AtomicUsize,
    state: AtomicU8,
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>
{
    const fn new() -> Self {
        Self {
            bytes: UnsafeCell::new(PinnedTxBytes::new()),
            length: AtomicUsize::new(0),
            state: AtomicU8::new(SLOT_FREE),
        }
    }

    fn claim(&self, index: u8, from: u8, to: u8) -> Result<(), PoolError> {
        self.state
            .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|observed| PoolError::State {
                index,
                expected: from,
                observed,
            })
    }

    // Only the affine network lease can publish or release NETWORK; only the
    // affine radio lease can release RADIO. Neither state can be stolen.
    fn publish_ready(&self, length: FrameLen<FRAME_CAPACITY>) {
        self.length.store(length.get(), Ordering::Relaxed);
        self.state.store(SLOT_READY, Ordering::Release);
    }

    fn release(&self) {
        self.length.store(0, Ordering::Relaxed);
        self.state.store(SLOT_FREE, Ordering::Release);
    }

    fn length(&self) -> usize {
        self.length.load(Ordering::Acquire)
    }

    #[allow(
        unsafe_code,
        reason = "TX slot exposes its contiguous storage to a typed lease"
    )]
    fn ethernet(&self) -> &[u8] {
        // SAFETY: the caller holds the unique state-specific lease. The three
        // byte-aligned repr(C) fields form one padding-free allocation.
        unsafe {
            let bytes = &*self.bytes.get();
            core::slice::from_raw_parts(ptr::addr_of!(bytes.ethernet).cast::<u8>(), self.length())
        }
    }

    fn storage_mut_ptr(&self) -> *mut u8 {
        self.bytes.get().cast::<u8>()
    }

    const fn storage_capacity(&self) -> usize {
        core::mem::size_of::<PinnedTxBytes<FRAME_CAPACITY, HEADROOM, TRAILER>>()
    }
}

// SAFETY: all access to the UnsafeCell is gated by the atomic state machine
// and by non-Clone state-specific leases.
#[allow(unsafe_code, reason = "TX slot state machine is its Sync boundary")]
unsafe impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> Sync
    for PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>
{
}

/// The cache maintenance a pool's bytes need before a DMA reader receives
/// them, chosen as the pool's type: [`Coherent`] memory needs none.
///
/// A target whose pool lies in cached memory (a PSRAM TX buffer) names its
/// write-back here; the lease calls it at the final descriptor-publication
/// edge ([`StableDmaBacking::prepare_for_dma_read`]), never through a
/// function pointer.
pub trait DmaReadPrepare: 'static {
    /// Make CPU writes to `bytes` visible to a DMA reader.
    fn prepare_for_dma_read(bytes: &mut [u8]);
}

/// Memory the DMA reader sees coherently (internal SRAM): nothing to do.
pub enum Coherent {}

impl DmaReadPrepare for Coherent {
    fn prepare_for_dma_read(_: &mut [u8]) {}
}

/// Permanently located TX allocations exposed to radio DMA.
///
/// Its zero bytes are the free pool `new` builds: every slot free and empty.
/// `P` is the cache maintenance before a DMA reader receives a buffer. Use a
/// pool only through [`Self::pin_static`], which consumes the permanent
/// allocation into a [`PinnedDmaTxPoolHandle`]. An unpinned pool has no claim
/// operations.
#[derive(bytemuck::Zeroable)]
#[zeroable(bound = "")]
pub struct PinnedDmaTxPool<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    P: DmaReadPrepare = Coherent,
> {
    slots: [PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>; QUEUE_DEPTH],
    _prepare: PhantomData<fn() -> P>,
    _pin: PhantomPinned,
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    P: DmaReadPrepare,
> PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH, P>
{
    pub const fn new() -> Self {
        Self {
            slots: [const { PinnedTxSlot::new() }; QUEUE_DEPTH],
            _prepare: PhantomData,
            _pin: PhantomPinned,
        }
    }

    /// Locations of the initialized byte regions, before permanent pinning.
    /// These observations grant no access or DMA lifetime authority.
    pub fn dma_storage_locations(&self) -> [(*const u8, usize); QUEUE_DEPTH] {
        self.slots.each_ref().map(|slot| {
            (
                slot.bytes.get().cast::<u8>().cast_const(),
                slot.storage_capacity(),
            )
        })
    }

    /// Consume permanent storage into its sole, non-cloneable claim handle.
    /// Dropping or forgetting the handle never frees or unpins the storage.
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> PinnedDmaTxPoolHandle<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH, P> {
        PinnedDmaTxPoolHandle {
            pool: Pin::static_mut(storage).into_ref(),
        }
    }
}

/// Affine authority to claim slots in one permanently pinned TX pool.
///
/// Only [`PinnedDmaTxPool::pin_static`] constructs this handle, consuming exclusive static storage. It exposes
/// neither that storage nor another handle. Individual leases retain the
/// static allocation and the existing atomic slot protocol; a slot index is
/// not a lease. Their lifetime does not depend on the handle's location.
/// The handle's destruction never resets slots, releases DMA owners or makes
/// the allocation available for a second initialization. A forgotten lease
/// therefore reserves its allocation even after this handle is gone.
pub struct PinnedDmaTxPoolHandle<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    P: DmaReadPrepare = Coherent,
> {
    pool: Pin<&'static PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH, P>>,
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    P: DmaReadPrepare,
> PinnedDmaTxPoolHandle<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH, P>
{
    fn slot_of(
        &self,
        index: u8,
    ) -> Result<&'static PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>, PoolError> {
        self.pool
            .get_ref()
            .slots
            .get(usize::from(index))
            .ok_or(PoolError::Index {
                index,
                slots: QUEUE_DEPTH,
            })
    }

    /// Claim a free slot for a unique network writer.
    pub fn claim_network(
        &self,
        index: u8,
    ) -> Result<PinnedDmaTxNetworkLease<'static, FRAME_CAPACITY, HEADROOM, TRAILER>, PoolError>
    {
        let slot = self.slot_of(index)?;
        slot.claim(index, SLOT_FREE, SLOT_NETWORK)?;
        Ok(PinnedDmaTxNetworkLease {
            slot,
            index,
            live: true,
        })
    }

    /// Claim a slot whose network writer published its complete frame.
    pub fn claim_radio(
        &self,
        index: u8,
    ) -> Result<PinnedDmaTxRadioLease<'static, FRAME_CAPACITY, HEADROOM, TRAILER, P>, PoolError>
    {
        let slot = self.slot_of(index)?;
        slot.claim(index, SLOT_READY, SLOT_RADIO)?;
        Ok(PinnedDmaTxRadioLease {
            slot,
            index,
            live: true,
            _prepare: PhantomData,
        })
    }

    /// Number of slots retained by a network, ready-queue or radio stage.
    ///
    /// This is observation only and grants no right to recover a quarantined
    /// radio slot. A platform reset path needs a separate ownership proof
    /// before such a transition can be added.
    pub fn claimed_slots(&self) -> usize {
        self.pool
            .slots
            .iter()
            .filter(|slot| slot.state.load(Ordering::Acquire) != SLOT_FREE)
            .count()
    }
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    P: DmaReadPrepare,
> Default for PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH, P>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Unique network writer for one free TX slot.
pub struct PinnedDmaTxNetworkLease<
    'pool,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
> {
    slot: &'pool PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>,
    index: u8,
    live: bool,
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    PinnedDmaTxNetworkLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    /// Write and publish one frame, consuming its network lease.
    ///
    /// The checked length bounds the writer's slice. On callback unwind the
    /// unpublished network lease returns its slot.
    pub fn publish<R>(
        mut self,
        length: FrameLen<FRAME_CAPACITY>,
        write: impl FnOnce(&mut [u8]) -> R,
    ) -> (u8, R) {
        // SAFETY: this non-Clone Network lease is the unique SLOT_NETWORK
        // owner. No radio lease can exist before `publish_ready` below.
        #[allow(unsafe_code, reason = "network lease uniquely initializes its TX slot")]
        let storage = unsafe {
            core::slice::from_raw_parts_mut(
                self.slot.storage_mut_ptr().wrapping_add(HEADROOM),
                length.get(),
            )
        };
        let result = write(storage);
        self.slot.publish_ready(length);
        self.live = false;
        (self.index, result)
    }

    /// Writes one frame transactionally and publishes it only on success.
    ///
    /// The error branch returns the still-live network lease. Bytes written by
    /// a failed callback remain private to that lease and can never be
    /// observed by a radio owner.
    ///
    /// A callback's ordinary `Err` returns the live lease.
    pub fn try_publish<R, E>(
        mut self,
        length: FrameLen<FRAME_CAPACITY>,
        write: impl FnOnce(&mut [u8]) -> Result<R, E>,
    ) -> Result<(u8, R), (Self, E)> {
        // SAFETY: this non-Clone Network lease is the unique SLOT_NETWORK
        // owner. No radio lease can exist before `publish_ready` below.
        #[allow(unsafe_code, reason = "network lease uniquely initializes its TX slot")]
        let storage = unsafe {
            core::slice::from_raw_parts_mut(
                self.slot.storage_mut_ptr().wrapping_add(HEADROOM),
                length.get(),
            )
        };
        let result = match write(storage) {
            Ok(result) => result,
            Err(error) => return Err((self, error)),
        };
        self.slot.publish_ready(length);
        self.live = false;
        Ok((self.index, result))
    }

    pub fn release(mut self) -> u8 {
        self.slot.release();
        self.live = false;
        self.index
    }
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> Drop
    for PinnedDmaTxNetworkLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    fn drop(&mut self) {
        if self.live {
            self.slot.release();
        }
    }
}

/// Unique radio/DMA owner for one ready TX allocation.
pub struct PinnedDmaTxRadioLease<
    'pool,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    P: DmaReadPrepare = Coherent,
> {
    slot: &'pool PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>,
    index: u8,
    live: bool,
    _prepare: PhantomData<fn() -> P>,
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize, P: DmaReadPrepare>
    PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER, P>
{
    pub const fn ethernet_offset(&self) -> usize {
        HEADROOM
    }

    pub fn ethernet_length(&self) -> usize {
        self.slot.length()
    }

    pub fn len(&self) -> usize {
        self.ethernet_length()
    }

    pub fn is_empty(&self) -> bool {
        self.ethernet_length() == 0
    }

    pub fn ethernet(&self) -> &[u8] {
        self.slot.ethernet()
    }

    pub fn as_slice(&self) -> &[u8] {
        self.ethernet()
    }

    pub fn ethernet_mut(&mut self) -> &mut [u8] {
        self.view_mut(HEADROOM, self.ethernet_length())
    }

    pub fn storage_mut(&mut self) -> &mut [u8] {
        self.view_mut(0, self.slot.storage_capacity())
    }

    fn view_mut(&mut self, offset: usize, length: usize) -> &mut [u8] {
        // SAFETY: `&mut self` borrows the unique non-Clone SLOT_RADIO lease.
        #[allow(unsafe_code, reason = "radio lease uniquely owns its pinned TX slot")]
        unsafe {
            core::slice::from_raw_parts_mut(
                self.slot.storage_mut_ptr().wrapping_add(offset),
                length,
            )
        }
    }

    pub const fn trailer_capacity(&self) -> usize {
        TRAILER
    }

    pub fn release(mut self) -> u8 {
        self.slot.release();
        self.live = false;
        self.index
    }
}

#[allow(
    unsafe_code,
    reason = "radio lease proves the stable DMA backing contract"
)]
// SAFETY: claims exist only on PinnedDmaTxPoolHandle, whose private constructor
// consumes &'static mut pool storage and pins it permanently. Neither a local
// pool nor an independently pinned borrow can issue a lease. The handle
// exposes no storage recovery or reinitialization, and dropping it does not
// reclaim the allocation. This type proof is independent of guard assertions.
// A successfully claimed allocation therefore outlives even a shortened 'pool
// borrow. Forgetting this non-Clone lease leaves the slot in SLOT_RADIO: neither
// the pool nor another lease can reclaim it, and its backing remains allocated.
unsafe impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    P: DmaReadPrepare,
> StableDmaBacking for PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER, P>
{
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_> {
        // SAFETY: the Radio lease exclusively retains this pinned allocation.
        #[allow(unsafe_code, reason = "radio lease retains the pinned allocation")]
        unsafe {
            StableDmaRegion::new(self.storage_mut())
        }
    }

    fn prepare_for_dma_read(&mut self) {
        P::prepare_for_dma_read(self.storage_mut());
    }
}

/// Stable DMA lease whose destructor releases its slot before its index is returned.
/// `index` must identify that exact slot for the entire lease lifetime.
pub trait IndexedStableDmaLease: StableDmaBacking {
    fn index(&self) -> u8;
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize, P: DmaReadPrepare>
    IndexedStableDmaLease for PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER, P>
{
    fn index(&self) -> u8 {
        self.index
    }
}

/// Safe integration callback which returns a released pool index to its queue.
pub trait DmaIndexReturn {
    fn return_index(&self, index: u8);
}

/// Stable backing plus the integration-specific queue return capability.
///
/// Dropping this value first releases the backing lease and only then returns
/// the pool index to another producer. Any hardware access must already be
/// quiescent; this wrapper does not stop DMA or establish completion.
pub struct ReturningStableDmaBacking<B: IndexedStableDmaLease, R: DmaIndexReturn> {
    // Fields drop in declaration order: release storage before publishing its index.
    backing: B,
    _returner: ReturnIndex<R>,
}

/// Immutable CPU metadata paired with one stable DMA owner.
///
/// The tag is never part of the hardware-visible allocation. This wrapper is
/// used when several logical producers share one physical DMA pool and the
/// eventual consumer must select the correct protocol encoder.
pub struct TaggedStableDmaBacking<T, B> {
    tag: T,
    backing: B,
}

impl<T, B> TaggedStableDmaBacking<T, B> {
    pub const fn new(tag: T, backing: B) -> Self {
        Self { tag, backing }
    }

    pub const fn tag(&self) -> &T {
        &self.tag
    }
}

impl<T, B> Deref for TaggedStableDmaBacking<T, B> {
    type Target = B;

    fn deref(&self) -> &Self::Target {
        &self.backing
    }
}

impl<T, B> DerefMut for TaggedStableDmaBacking<T, B> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.backing
    }
}

#[allow(
    unsafe_code,
    reason = "metadata wrapper retains the same audited stable DMA owner"
)]
// SAFETY: immutable CPU metadata cannot move, release, or alias the wrapped
// stable backing. The exact allocation remains retained by `backing`. Forgetting
// this wrapper forgets both fields, so the backing's leak guarantee is preserved
// even when the CPU-only tag contains a shorter-lived borrow.
unsafe impl<T, B: StableDmaBacking> StableDmaBacking for TaggedStableDmaBacking<T, B> {
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_> {
        self.backing.stable_dma_region()
    }

    fn prepare_for_dma_read(&mut self) {
        self.backing.prepare_for_dma_read();
    }
}

impl<B: IndexedStableDmaLease, R: DmaIndexReturn> ReturningStableDmaBacking<B, R> {
    pub fn new(backing: B, returner: R) -> Self {
        Self {
            _returner: ReturnIndex {
                index: backing.index(),
                returner,
            },
            backing,
        }
    }
}

impl<B: IndexedStableDmaLease, R: DmaIndexReturn> Deref for ReturningStableDmaBacking<B, R> {
    type Target = B;

    fn deref(&self) -> &Self::Target {
        &self.backing
    }
}

#[allow(
    unsafe_code,
    reason = "wrapper retains the same audited stable DMA owner"
)]
// SAFETY: the wrapper retains the exact stable backing and exposes no
// operation which can release it before this owner is dropped. Forgetting the
// wrapper leaks `backing` without invoking the returner: the allocation stays
// reserved by B's contract even after any borrow inside R ends.
unsafe impl<B: IndexedStableDmaLease, R: DmaIndexReturn> StableDmaBacking
    for ReturningStableDmaBacking<B, R>
{
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_> {
        self.backing.stable_dma_region()
    }

    fn prepare_for_dma_read(&mut self) {
        self.backing.prepare_for_dma_read();
    }
}

struct ReturnIndex<R: DmaIndexReturn> {
    index: u8,
    returner: R,
}

impl<R: DmaIndexReturn> Drop for ReturnIndex<R> {
    fn drop(&mut self) {
        self.returner.return_index(self.index);
    }
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize, P: DmaReadPrepare>
    Drop for PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER, P>
{
    fn drop(&mut self) {
        if self.live {
            self.slot.release();
        }
    }
}

#[cfg(test)]
mod tests;
