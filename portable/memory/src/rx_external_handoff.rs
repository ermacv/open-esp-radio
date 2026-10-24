//! Pointer-free-to-users ownership of DMA buffers retained across tasks.
//!
//! A chip DMA owner creates an [`ExternalRxBuffer`] only after hardware has
//! released the matching descriptor. The bounded pool then transfers that
//! exact buffer from the radio/protocol owner to the network owner. No payload
//! bytes are copied. A shared return state makes the allocation eligible for
//! DMA reuse after its affine owner relinquishes access.

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    ptr::NonNull,
    sync::atomic::{AtomicU8, Ordering},
};

const SLOT_FREE: u8 = 0;
const SLOT_NETWORK: u8 = 1;
const SLOT_READY: u8 = 2;
const SLOT_RADIO: u8 = 3;
const SLOT_RELEASING: u8 = 4;

/// The DMA owner's return edge of one detachable buffer.
#[repr(transparent)]
pub struct ExternalRxReturn(AtomicU8);

#[allow(unsafe_code, reason = "zero is the Attached return state")]
// SAFETY: AtomicU8 is zeroable, and zero represents Attached.
unsafe impl bytemuck::Zeroable for ExternalRxReturn {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalRxReturnState {
    Attached,
    Detached,
    Returned,
}

impl ExternalRxReturn {
    pub const fn new() -> Self {
        Self(AtomicU8::new(0))
    }

    pub fn state(&self) -> ExternalRxReturnState {
        Self::decode(self.0.load(Ordering::Acquire))
    }

    #[expect(
        clippy::unreachable,
        reason = "internal invariant: the atomic holds only values `encode` produced"
    )]
    fn decode(state: u8) -> ExternalRxReturnState {
        match state {
            0 => ExternalRxReturnState::Attached,
            1 => ExternalRxReturnState::Detached,
            2 => ExternalRxReturnState::Returned,
            _ => unreachable!("external RX return state is always valid"),
        }
    }

    /// Detach the stable allocation after DMA has relinquished it.
    ///
    /// # Safety
    ///
    /// `pointer..pointer + capacity` is the stable allocation this state
    /// guards; DMA no longer accesses it while the state is not Attached.
    /// The first `length` bytes are initialized, and the returned buffer has
    /// exclusive CPU access until it is returned. The allocation must remain
    /// valid even if the buffer is forgotten.
    #[allow(
        unsafe_code,
        reason = "chip DMA storage establishes the stable detached-buffer proof"
    )]
    pub unsafe fn detach(
        &'static self,
        pointer: NonNull<u8>,
        length: usize,
        capacity: usize,
    ) -> Result<ExternalRxBuffer, ExternalRxReturnState> {
        assert!(length <= capacity, "external RX length exceeds capacity");
        self.0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .map_err(Self::decode)?;
        Ok(ExternalRxBuffer {
            pointer,
            length,
            capacity,
            ret: self,
            live: true,
        })
    }

    #[inline]
    pub fn reattach(&self) -> Result<(), ExternalRxReturnState> {
        match self.state() {
            ExternalRxReturnState::Attached => Ok(()),
            ExternalRxReturnState::Detached => Err(ExternalRxReturnState::Detached),
            ExternalRxReturnState::Returned => self
                .0
                .compare_exchange(2, 0, Ordering::AcqRel, Ordering::Acquire)
                .map(|_| ())
                .map_err(Self::decode),
        }
    }

    fn return_once(&self) {
        assert_eq!(
            self.0
                .compare_exchange(1, 2, Ordering::Release, Ordering::Acquire),
            Ok(1),
            "detached RX DMA buffer must be released exactly once"
        );
        #[cfg(test)]
        tests::observe_return();
    }
}

impl Default for ExternalRxReturn {
    fn default() -> Self {
        Self::new()
    }
}

/// One stable external receive buffer and its affine return edge.
pub struct ExternalRxBuffer {
    pointer: NonNull<u8>,
    length: usize,
    capacity: usize,
    ret: &'static ExternalRxReturn,
    live: bool,
}

impl ExternalRxBuffer {
    pub const fn length(&self) -> usize {
        self.length
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    fn frame(&self, offset: usize, length: usize) -> &[u8] {
        assert!(
            offset
                .checked_add(length)
                .is_some_and(|end| end <= self.length),
            "external RX view stays inside initialized bytes"
        );
        // SAFETY: the affine value retains exclusive CPU ownership and the
        // checked range lies inside its stable allocation.
        #[allow(unsafe_code, reason = "affine external buffer owns this range")]
        unsafe {
            core::slice::from_raw_parts(self.pointer.as_ptr().add(offset), length)
        }
    }

    fn frame_mut(&mut self, offset: usize, length: usize) -> &mut [u8] {
        assert!(
            offset
                .checked_add(length)
                .is_some_and(|end| end <= self.length),
            "external RX mutable view stays inside initialized bytes"
        );
        // SAFETY: `&mut self` borrows the sole affine owner and the range was
        // checked against initialized bytes.
        #[allow(unsafe_code, reason = "affine external buffer owns this range")]
        unsafe {
            core::slice::from_raw_parts_mut(self.pointer.as_ptr().add(offset), length)
        }
    }

    /// Start of `offset` and the bytes from there to the allocation end.
    fn raw_from(&self, offset: usize) -> (NonNull<u8>, usize) {
        assert!(
            offset <= self.length,
            "external RX adoption starts inside initialized bytes"
        );
        // SAFETY: `offset` lies inside the stable allocation checked above.
        #[allow(unsafe_code, reason = "affine external buffer owns this range")]
        let pointer = unsafe { self.pointer.add(offset) };
        (pointer, self.capacity - offset)
    }

    fn release_once(&mut self) {
        if !self.live {
            return;
        }
        self.live = false;
        self.ret.return_once();
    }
}

impl Drop for ExternalRxBuffer {
    fn drop(&mut self) {
        self.release_once();
    }
}

#[allow(
    unsafe_code,
    reason = "affine external buffer may cross executor cores"
)]
// SAFETY: detach promises stable, exclusively CPU-owned storage. The shared
// static return state is atomic, and the pool's release/acquire transitions
// transfer the affine buffer's access between cores.
unsafe impl Send for ExternalRxBuffer {}

struct ExternalRxHandoffSlot {
    /// Initialized exactly while `state` is not `SLOT_FREE`: a free slot's
    /// bytes are zero, which no `ExternalRxBuffer` is.
    buffer: UnsafeCell<MaybeUninit<ExternalRxBuffer>>,
    offset: UnsafeCell<usize>,
    length: UnsafeCell<usize>,
    state: AtomicU8,
}

#[allow(unsafe_code, reason = "a free slot is zero bytes")]
// SAFETY: state 0 is `SLOT_FREE`, whose binding is uninitialized;
// offset and length are zero integers.
unsafe impl bytemuck::Zeroable for ExternalRxHandoffSlot {}

impl Drop for ExternalRxHandoffSlot {
    fn drop(&mut self) {
        if *self.state.get_mut() != SLOT_FREE {
            #[allow(unsafe_code, reason = "a non-free slot holds its binding")]
            // SAFETY: every non-free state holds an initialized binding, and
            // `&mut self` excludes every lease.
            unsafe {
                self.buffer.get_mut().assume_init_drop();
            }
        }
    }
}

impl ExternalRxHandoffSlot {
    const fn new() -> Self {
        Self {
            buffer: UnsafeCell::new(MaybeUninit::uninit()),
            offset: UnsafeCell::new(0),
            length: UnsafeCell::new(0),
            state: AtomicU8::new(SLOT_FREE),
        }
    }

    fn try_claim_radio(&self, buffer: ExternalRxBuffer) -> Result<(), ExternalRxBuffer> {
        if self
            .state
            .compare_exchange(SLOT_FREE, SLOT_RADIO, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(buffer);
        }
        // SAFETY: SLOT_RADIO is exclusively owned by this successful claim;
        // no consumer can observe the binding before the Release publication.
        #[allow(unsafe_code, reason = "slot state exclusively owns binding mutation")]
        unsafe {
            let buffer = (*self.buffer.get()).write(buffer);
            *self.offset.get() = 0;
            *self.length.get() = buffer.length();
        }
        Ok(())
    }

    fn publish(&self, owner: u8, offset: usize, length: usize) {
        let initialized = self.initialized_length();
        assert!(
            offset
                .checked_add(length)
                .is_some_and(|end| end <= initialized),
            "published external RX range stays initialized"
        );
        // SAFETY: the current affine owner is the only stage allowed to alter
        // the published view before its Release transition.
        #[allow(unsafe_code, reason = "slot owner exclusively mutates range metadata")]
        unsafe {
            *self.offset.get() = offset;
            *self.length.get() = length;
        }
        assert_eq!(
            self.state
                .compare_exchange(owner, SLOT_READY, Ordering::Release, Ordering::Acquire,),
            Ok(owner),
            "only the current external RX owner may publish"
        );
    }

    fn claim_network(&self) {
        assert_eq!(
            self.state.compare_exchange(
                SLOT_READY,
                SLOT_NETWORK,
                Ordering::Acquire,
                Ordering::Relaxed,
            ),
            Ok(SLOT_READY),
            "ready index did not name an external RX slot"
        );
    }

    fn initialized_length(&self) -> usize {
        // SAFETY: every non-free state contains an initialized binding. The
        // state transition which published it supplies the acquire ordering.
        #[allow(unsafe_code, reason = "non-free slot retains its external binding")]
        unsafe {
            (*self.buffer.get()).assume_init_ref().length()
        }
    }

    fn frame(&self) -> &[u8] {
        // SAFETY: a live lease uniquely owns the non-free slot state.
        #[allow(unsafe_code, reason = "live lease retains external slot ownership")]
        unsafe {
            let offset = *self.offset.get();
            let length = *self.length.get();
            (*self.buffer.get()).assume_init_ref().frame(offset, length)
        }
    }

    fn with_frame<R>(&self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        // SAFETY: the caller's affine lease requires unique mutable access,
        // proving exclusive ownership of the current non-free allocation.
        #[allow(unsafe_code, reason = "affine lease uniquely owns external bytes")]
        unsafe {
            let offset = *self.offset.get();
            let length = *self.length.get();
            let buffer = (*self.buffer.get()).assume_init_mut();
            f(buffer.frame_mut(offset, length))
        }
    }

    fn adoption(&self, index: u8) -> ExternalRxAdoption {
        // SAFETY: the network lease being consumed uniquely owns this
        // non-free slot and its range metadata.
        #[allow(unsafe_code, reason = "live lease retains external slot ownership")]
        let (offset, length, buffer) = unsafe {
            (
                *self.offset.get(),
                *self.length.get(),
                (*self.buffer.get()).assume_init_ref(),
            )
        };
        let (data, available) = buffer.raw_from(offset);
        ExternalRxAdoption {
            index,
            data,
            length,
            available,
        }
    }

    fn release(&self, owner: u8) {
        assert_eq!(
            self.state.compare_exchange(
                owner,
                SLOT_RELEASING,
                Ordering::Acquire,
                Ordering::Acquire,
            ),
            Ok(owner),
            "only the current external RX owner may release"
        );
        // SAFETY: the successful exchange to SLOT_RELEASING grants this call
        // exclusive access to the binding cells and excludes a new producer
        // until the old binding has been removed. The affine buffer's Drop
        // then publishes its separate DMA-release state.
        #[allow(unsafe_code, reason = "free slot exclusively owns binding removal")]
        let buffer = unsafe {
            *self.offset.get() = 0;
            *self.length.get() = 0;
            (*self.buffer.get()).assume_init_read()
        };
        // The DMA allocation must become releasable before this handoff slot
        // is advertised as a new radio credit. Publishing FREE first creates
        // a cross-core window in which a new detached buffer can claim this
        // slot while the old descriptor buffer is still DETACHED, violating
        // the pool's upper bound on retained physical DMA credits.
        drop(buffer);
        self.state.store(SLOT_FREE, Ordering::Release);
    }
}

#[allow(
    unsafe_code,
    reason = "slot state machine is the external pool Sync boundary"
)]
// SAFETY: atomic slot ownership serializes every access to UnsafeCell fields;
// Release/Acquire publishes both the binding and its bytes across cores.
unsafe impl Sync for ExternalRxHandoffSlot {}

/// Bounded index pool for DMA buffers retained above the descriptor ring.
pub struct ExternalRxHandoffPool<const FRAME_CAPACITY: usize, const SLOT_COUNT: usize> {
    slots: [ExternalRxHandoffSlot; SLOT_COUNT],
}

#[allow(unsafe_code, reason = "the free pool is zero bytes")]
// SAFETY: an array of free slots.
unsafe impl<const FRAME_CAPACITY: usize, const SLOT_COUNT: usize> bytemuck::Zeroable
    for ExternalRxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>
{
}

impl<const FRAME_CAPACITY: usize, const SLOT_COUNT: usize>
    ExternalRxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>
{
    pub const fn new() -> Self {
        Self {
            slots: [const { ExternalRxHandoffSlot::new() }; SLOT_COUNT],
        }
    }

    pub fn try_claim_radio(
        &self,
        buffer: ExternalRxBuffer,
        start: usize,
    ) -> Result<ExternalRxRadioLease<'_, FRAME_CAPACITY>, ExternalRxBuffer> {
        assert!(
            buffer.length() <= FRAME_CAPACITY,
            "external RX frame exceeds the handoff profile"
        );
        if SLOT_COUNT == 0 || SLOT_COUNT > usize::from(u8::MAX) + 1 {
            return Err(buffer);
        }
        let mut buffer = buffer;
        for distance in 0..SLOT_COUNT {
            let index = (start + distance) % SLOT_COUNT;
            let Some(slot) = self.slots.get(index) else {
                continue;
            };
            match slot.try_claim_radio(buffer) {
                Ok(()) => {
                    return Ok(ExternalRxRadioLease {
                        slot,
                        index: index as u8,
                        live: true,
                    });
                }
                Err(returned) => buffer = returned,
            }
        }
        Err(buffer)
    }

    #[expect(
        clippy::expect_used,
        reason = "documented panic of the claims: the index comes from this pool's own channels, so a foreign one is a driver defect"
    )]
    fn slot_of(&self, index: u8) -> &ExternalRxHandoffSlot {
        self.slots
            .get(usize::from(index))
            .expect("external RX index belongs to its pool")
    }

    pub fn claim_network(&self, index: u8) -> ExternalRxNetworkLease<'_, FRAME_CAPACITY> {
        let slot = self.slot_of(index);
        slot.claim_network();
        ExternalRxNetworkLease {
            slot,
            index,
            live: true,
        }
    }

    /// Return the storage of an adopted network slot to its DMA owner.
    ///
    /// # Safety
    ///
    /// `index` must come from [`ExternalRxNetworkLease::into_adoption`] on
    /// this pool, be released exactly once per adoption, and nothing may
    /// access the adopted bytes after this call.
    #[allow(
        unsafe_code,
        reason = "an adopter returns the slot it took out of its affine lease"
    )]
    pub unsafe fn release_adopted(&self, index: u8) {
        self.slot_of(index).release(SLOT_NETWORK);
    }

    pub fn claimed_slots(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.state.load(Ordering::Acquire) != SLOT_FREE)
            .count()
    }

    pub fn network_slots(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.state.load(Ordering::Acquire) == SLOT_NETWORK)
            .count()
    }
}

impl<const FRAME_CAPACITY: usize, const SLOT_COUNT: usize> Default
    for ExternalRxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>
{
    fn default() -> Self {
        Self::new()
    }
}

pub struct ExternalRxRadioLease<'pool, const FRAME_CAPACITY: usize> {
    slot: &'pool ExternalRxHandoffSlot,
    index: u8,
    live: bool,
}

impl<'pool, const FRAME_CAPACITY: usize> ExternalRxRadioLease<'pool, FRAME_CAPACITY> {
    pub const fn index(&self) -> usize {
        self.index as usize
    }

    pub fn frame(&self) -> &[u8] {
        self.slot.frame()
    }

    pub fn with_frame<R>(&mut self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        self.slot.with_frame(f)
    }

    /// Publish the final protocol-owned view for a later network claim.
    ///
    /// The radio/protocol owner remains affine until this transition. Unlike
    /// an eager `RADIO -> READY -> NETWORK` handoff, this does not fabricate a
    /// network owner while Core0 is still decoding and transforming the MPDU.
    pub fn republish(mut self, offset: usize, length: usize) -> u8 {
        self.slot.publish(SLOT_RADIO, offset, length);
        self.live = false;
        self.index
    }
}

impl<const FRAME_CAPACITY: usize> Drop for ExternalRxRadioLease<'_, FRAME_CAPACITY> {
    fn drop(&mut self) {
        if self.live {
            self.slot.release(SLOT_RADIO);
        }
    }
}

pub struct ExternalRxNetworkLease<'pool, const FRAME_CAPACITY: usize> {
    slot: &'pool ExternalRxHandoffSlot,
    index: u8,
    live: bool,
}

impl<const FRAME_CAPACITY: usize> ExternalRxNetworkLease<'_, FRAME_CAPACITY> {
    pub const fn index(&self) -> usize {
        self.index as usize
    }

    pub fn frame(&self) -> &[u8] {
        self.slot.frame()
    }

    pub fn with_frame<R>(&mut self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        self.slot.with_frame(f)
    }

    pub fn republish(mut self, offset: usize, length: usize) -> u8 {
        self.slot.publish(SLOT_NETWORK, offset, length);
        self.live = false;
        self.index
    }

    pub fn release(mut self) -> u8 {
        self.slot.release(SLOT_NETWORK);
        self.live = false;
        self.index
    }

    /// Hand the slot's storage to an owner that holds it by raw pointer, such
    /// as a network stack packet. The slot stays network-owned until
    /// [`ExternalRxHandoffPool::release_adopted`] returns it.
    pub fn into_adoption(mut self) -> ExternalRxAdoption {
        self.live = false;
        self.slot.adoption(self.index)
    }
}

/// Storage of a network slot taken out of its affine lease.
#[derive(Debug, Eq, PartialEq)]
pub struct ExternalRxAdoption {
    /// Slot index to pass to [`ExternalRxHandoffPool::release_adopted`].
    pub index: u8,
    /// First byte of the published view.
    pub data: NonNull<u8>,
    /// Bytes of the published view.
    pub length: usize,
    /// Bytes from `data` to the end of the stable allocation, which the
    /// adopter owns in full until release.
    pub available: usize,
}

impl<const FRAME_CAPACITY: usize> Drop for ExternalRxNetworkLease<'_, FRAME_CAPACITY> {
    fn drop(&mut self) {
        if self.live {
            self.slot.release(SLOT_NETWORK);
        }
    }
}

#[cfg(test)]
mod tests;
