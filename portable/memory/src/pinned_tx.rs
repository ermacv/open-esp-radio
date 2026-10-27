//! Permanently located TX storage and its finite ownership transitions.
//!
//! Pinning is a type boundary: only the affine [`PinnedDmaTxPoolHandle`]
//! issues leases. Slot indices still follow the caller's free/ready queues:
//! an out-of-range index or a claim in the wrong state panics, rather than
//! lending aliased bytes.
//! Publication lengths must fit the frame capacity before the writer runs.

use core::{
    cell::UnsafeCell,
    marker::PhantomPinned,
    ops::{Deref, DerefMut},
    pin::Pin,
    ptr,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

use crate::{StableDmaBacking, StableDmaRegion};

const SLOT_FREE: u8 = 0;
const SLOT_NETWORK: u8 = 1;
const SLOT_READY: u8 = 2;
const SLOT_RADIO: u8 = 3;
/// The Ethernet frame of `length` bytes behind the headroom.
#[expect(
    clippy::indexing_slicing,
    reason = "internal invariant: length is checked against FRAME_CAPACITY at publish, and storage holds HEADROOM + FRAME_CAPACITY + TRAILER"
)]
fn frame<const HEADROOM: usize>(storage: &[u8], length: usize) -> &[u8] {
    &storage[HEADROOM..HEADROOM + length]
}

/// The mutable Ethernet frame of `length` bytes behind the headroom.
#[expect(
    clippy::indexing_slicing,
    reason = "internal invariant: length is checked against FRAME_CAPACITY at publish, and storage holds HEADROOM + FRAME_CAPACITY + TRAILER"
)]
fn frame_mut<const HEADROOM: usize>(storage: &mut [u8], length: usize) -> &mut [u8] {
    &mut storage[HEADROOM..HEADROOM + length]
}

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

    fn claim(&self, from: u8, to: u8) {
        let observed = self
            .state
            .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire);
        assert!(observed == Ok(from), "pinned TX slot ownership transition");
    }

    fn publish_ready(&self, length: usize) {
        self.length.store(length, Ordering::Relaxed);
        let observed = self.state.compare_exchange(
            SLOT_NETWORK,
            SLOT_READY,
            Ordering::Release,
            Ordering::Acquire,
        );
        assert!(
            observed == Ok(SLOT_NETWORK),
            "only the network lease may publish a pinned TX slot"
        );
    }

    fn release_radio(&self) {
        self.length.store(0, Ordering::Relaxed);
        self.claim(SLOT_RADIO, SLOT_FREE);
    }

    fn length(&self) -> usize {
        self.length.load(Ordering::Acquire)
    }

    #[allow(
        unsafe_code,
        reason = "TX slot exposes its contiguous storage to a typed lease"
    )]
    fn storage(&self) -> &[u8] {
        // SAFETY: the caller holds the unique state-specific lease. The three
        // byte-aligned repr(C) fields form one padding-free allocation.
        unsafe {
            let bytes = &*self.bytes.get();
            debug_assert!(core::mem::size_of_val(bytes) == HEADROOM + FRAME_CAPACITY + TRAILER);
            core::slice::from_raw_parts(
                ptr::addr_of!(bytes.headroom).cast::<u8>(),
                HEADROOM + FRAME_CAPACITY + TRAILER,
            )
        }
    }

    fn storage_mut_ptr(&self) -> *mut u8 {
        self.bytes.get().cast::<u8>()
    }

    const fn storage_capacity(&self) -> usize {
        HEADROOM + FRAME_CAPACITY + TRAILER
    }
}

// SAFETY: all access to the UnsafeCell is gated by the atomic state machine
// and by non-Clone state-specific leases.
#[allow(unsafe_code, reason = "TX slot state machine is its Sync boundary")]
unsafe impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> Sync
    for PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>
{
}

/// Permanently located TX allocations exposed to radio DMA.
///
/// Its zero bytes are the free pool `new` builds: every slot free and empty,
/// with no prepare hook (coherent memory). Use a pool only through
/// [`Self::pin_static`], which consumes the permanent allocation into a
/// [`PinnedDmaTxPoolHandle`]. An unpinned pool has no claim operations.
pub struct PinnedDmaTxPool<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    slots: [PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>; QUEUE_DEPTH],
    /// The cache maintenance before a DMA reader receives a buffer; `None`
    /// is coherent memory (`coherent_dma_read`), the zero value.
    prepare_for_dma_read: Option<fn(&mut [u8])>,
    _pin: PhantomPinned,
}

fn coherent_dma_read(_: &mut [u8]) {}

#[allow(
    unsafe_code,
    reason = "Zeroable derive cannot express the higher-ranked optional prepare hook"
)]
// SAFETY: free slots, a `None` prepare hook (the null niche of
// `Option<fn>`, coherent memory) and a zero-sized pin marker.
unsafe impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> bytemuck::Zeroable for PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            slots: [const { PinnedTxSlot::new() }; QUEUE_DEPTH],
            prepare_for_dma_read: None,
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
    ) -> PinnedDmaTxPoolHandle<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        Self::pin_static_with_dma_read_prepare(storage, coherent_dma_read)
    }

    /// Pin a pool which needs target-specific cache maintenance before a DMA
    /// reader receives ownership.
    pub fn pin_static_with_dma_read_prepare(
        storage: &'static mut Self,
        prepare_for_dma_read: fn(&mut [u8]),
    ) -> PinnedDmaTxPoolHandle<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        storage.prepare_for_dma_read = Some(prepare_for_dma_read);
        PinnedDmaTxPoolHandle {
            pool: Pin::static_mut(storage).into_ref(),
        }
    }
}

/// Affine authority to claim slots in one permanently pinned TX pool.
///
/// Only [`PinnedDmaTxPool::pin_static`] and its cache-maintenance variant
/// construct this handle, consuming exclusive static storage. It exposes
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
> {
    pool: Pin<&'static PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>,
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedDmaTxPoolHandle<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    /// The slot `index` names.
    #[expect(
        clippy::expect_used,
        reason = "documented panic of the claims: the index comes from this pool's own channels, so a foreign one is a driver defect"
    )]
    fn slot_of(&self, index: u8) -> &'static PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER> {
        self.pool
            .get_ref()
            .slots
            .get(usize::from(index))
            .expect("pinned TX index belongs to this pool")
    }

    /// Claim a free slot for a unique network writer.
    ///
    /// # Panics
    /// The index must belong to this pool and name a free slot.
    pub fn claim_network(
        &self,
        index: u8,
    ) -> PinnedDmaTxNetworkLease<'static, FRAME_CAPACITY, HEADROOM, TRAILER> {
        let slot = self.slot_of(index);
        slot.claim(SLOT_FREE, SLOT_NETWORK);
        PinnedDmaTxNetworkLease {
            slot,
            index,
            live: true,
        }
    }

    /// Claim a slot whose network writer has published its complete frame.
    ///
    /// # Panics
    /// The index must belong to this pool and name a ready slot.
    pub fn claim_radio(
        &self,
        index: u8,
    ) -> PinnedDmaTxRadioLease<'static, FRAME_CAPACITY, HEADROOM, TRAILER> {
        let slot = self.slot_of(index);
        slot.claim(SLOT_READY, SLOT_RADIO);
        PinnedDmaTxRadioLease {
            slot,
            index,
            live: true,
            prepare_for_dma_read: self.pool.prepare_for_dma_read.unwrap_or(coherent_dma_read),
        }
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
> Default for PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
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
    /// # Panics
    /// `length` must fit `FRAME_CAPACITY`. The writer may itself panic; on
    /// unwind the unpublished network lease returns its slot. A corrupted
    /// ownership state is fatal.
    pub fn publish<R>(mut self, length: usize, write: impl FnOnce(&mut [u8]) -> R) -> (u8, R) {
        assert!(length <= FRAME_CAPACITY, "TX frame exceeds slot capacity");
        // SAFETY: this non-Clone Network lease is the unique SLOT_NETWORK
        // owner. No radio lease can exist before `publish_ready` below.
        #[allow(unsafe_code, reason = "network lease uniquely initializes its TX slot")]
        let storage = unsafe {
            core::slice::from_raw_parts_mut(
                self.slot.storage_mut_ptr(),
                self.slot.storage_capacity(),
            )
        };
        let result = write(frame_mut::<HEADROOM>(storage, length));
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
    /// # Panics
    /// As with [`Self::publish`], an oversized length or corrupted ownership
    /// state is fatal. A callback's ordinary `Err` returns the live lease.
    pub fn try_publish<R, E>(
        mut self,
        length: usize,
        write: impl FnOnce(&mut [u8]) -> Result<R, E>,
    ) -> Result<(u8, R), (Self, E)> {
        assert!(length <= FRAME_CAPACITY, "TX frame exceeds slot capacity");
        // SAFETY: this non-Clone Network lease is the unique SLOT_NETWORK
        // owner. No radio lease can exist before `publish_ready` below.
        #[allow(unsafe_code, reason = "network lease uniquely initializes its TX slot")]
        let storage = unsafe {
            core::slice::from_raw_parts_mut(
                self.slot.storage_mut_ptr(),
                self.slot.storage_capacity(),
            )
        };
        let result = match write(frame_mut::<HEADROOM>(storage, length)) {
            Ok(result) => result,
            Err(error) => return Err((self, error)),
        };
        self.slot.publish_ready(length);
        self.live = false;
        Ok((self.index, result))
    }

    pub fn release(mut self) -> u8 {
        self.slot.claim(SLOT_NETWORK, SLOT_FREE);
        self.live = false;
        self.index
    }
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> Drop
    for PinnedDmaTxNetworkLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    fn drop(&mut self) {
        if self.live {
            self.slot.claim(SLOT_NETWORK, SLOT_FREE);
        }
    }
}

/// Unique radio/DMA owner for one ready TX allocation.
pub struct PinnedDmaTxRadioLease<
    'pool,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
> {
    slot: &'pool PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>,
    index: u8,
    live: bool,
    prepare_for_dma_read: fn(&mut [u8]),
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
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
        let length = self.ethernet_length();
        frame::<HEADROOM>(self.slot.storage(), length)
    }

    pub fn as_slice(&self) -> &[u8] {
        self.ethernet()
    }

    pub fn ethernet_mut(&mut self) -> &mut [u8] {
        let length = self.ethernet_length();
        frame_mut::<HEADROOM>(self.storage_mut(), length)
    }

    pub fn storage_mut(&mut self) -> &mut [u8] {
        // SAFETY: `&mut self` borrows the unique non-Clone SLOT_RADIO lease.
        #[allow(unsafe_code, reason = "radio lease uniquely owns its pinned TX slot")]
        unsafe {
            core::slice::from_raw_parts_mut(
                self.slot.storage_mut_ptr(),
                self.slot.storage_capacity(),
            )
        }
    }

    pub const fn trailer_capacity(&self) -> usize {
        TRAILER
    }

    pub fn release(mut self) -> u8 {
        self.slot.release_radio();
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
unsafe impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    StableDmaBacking for PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_> {
        // SAFETY: the Radio lease exclusively retains this pinned allocation.
        #[allow(unsafe_code, reason = "radio lease retains the pinned allocation")]
        unsafe {
            StableDmaRegion::new(self.storage_mut())
        }
    }

    fn prepare_for_dma_read(&mut self) {
        let prepare = self.prepare_for_dma_read;
        prepare(self.storage_mut());
    }
}

/// Stable DMA lease which yields its pool index when explicitly released.
pub trait IndexedStableDmaLease: StableDmaBacking {
    fn release_index(self) -> u8;
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> IndexedStableDmaLease
    for PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    fn release_index(self) -> u8 {
        self.release()
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
    backing: Option<B>,
    returner: R,
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
    pub const fn new(backing: B, returner: R) -> Self {
        Self {
            backing: Some(backing),
            returner,
        }
    }
}

impl<B: IndexedStableDmaLease, R: DmaIndexReturn> ReturningStableDmaBacking<B, R> {
    #[expect(
        clippy::expect_used,
        reason = "internal invariant: the backing is taken only by the drop that ends the value"
    )]
    fn live_backing(&self) -> &B {
        self.backing
            .as_ref()
            .expect("returning DMA backing remains live until drop")
    }

    #[expect(
        clippy::expect_used,
        reason = "internal invariant: the backing is taken only by the drop that ends the value"
    )]
    fn live_backing_mut(&mut self) -> &mut B {
        self.backing
            .as_mut()
            .expect("returning DMA backing remains live until drop")
    }
}

impl<B: IndexedStableDmaLease, R: DmaIndexReturn> Deref for ReturningStableDmaBacking<B, R> {
    type Target = B;

    fn deref(&self) -> &Self::Target {
        self.live_backing()
    }
}

impl<B: IndexedStableDmaLease, R: DmaIndexReturn> DerefMut for ReturningStableDmaBacking<B, R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.live_backing_mut()
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
        self.deref_mut().stable_dma_region()
    }

    fn prepare_for_dma_read(&mut self) {
        self.deref_mut().prepare_for_dma_read();
    }
}

impl<B: IndexedStableDmaLease, R: DmaIndexReturn> Drop for ReturningStableDmaBacking<B, R> {
    fn drop(&mut self) {
        if let Some(backing) = self.backing.take() {
            let index = backing.release_index();
            self.returner.return_index(index);
        }
    }
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> Drop
    for PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    fn drop(&mut self) {
        if self.live {
            self.slot.release_radio();
        }
    }
}

#[cfg(test)]
mod tests;
