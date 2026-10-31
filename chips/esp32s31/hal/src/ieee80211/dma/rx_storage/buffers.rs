use super::*;
use crate::ieee80211::dma::rx_ring::{ReadOnly, Rearm, ScopedBuffers};
use core::{cell::UnsafeCell, marker::PhantomData, ptr::NonNull};
use oer_memory::{ExternalRxBuffer, ExternalRxReturn, ExternalRxReturnState};

/// One aligned DMA-visible buffer and its affine return state.
#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub struct RxDmaBuffer<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>(
    UnsafeCell<[u8; STORAGE_SIZE]>,
    ExternalRxReturn,
);

impl<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE> {
    pub(super) const fn check_geometry() {
        const {
            assert!(
                BUFFER_SIZE != 0 && STORAGE_SIZE >= BUFFER_SIZE,
                "RX DMA capacity is nonzero and fits its backing"
            );
        }
    }

    pub(super) const fn new() -> Self {
        Self::check_geometry();
        Self(UnsafeCell::new([0; STORAGE_SIZE]), ExternalRxReturn::new())
    }

    pub(super) fn cpu_owned_mut(&mut self) -> &mut [u8; BUFFER_SIZE] {
        Self::check_geometry();
        self.0
            .get_mut()
            .get_mut(..BUFFER_SIZE)
            .expect("RX DMA storage always contains its declared buffer prefix")
            .try_into()
            .expect("the selected RX DMA prefix has the exact declared size")
    }

    /// Borrow bytes while the retained completion excludes descriptor recycle.
    #[allow(unsafe_code, reason = "completed grant proves CPU read ownership")]
    fn completed(&self) -> &[u8; BUFFER_SIZE] {
        // SAFETY: scoped storage selects the completion's allocation. Its ring
        // loan excludes DMA writes and recycle for the returned lifetime;
        // the type guarantees a prefix of exactly this size.
        unsafe { &*self.0.get().cast::<[u8; BUFFER_SIZE]>() }
    }

    fn prepare_for_recycle(&self) -> Result<(), RxRingError> {
        match self.1.state() {
            ExternalRxReturnState::Attached => {}
            ExternalRxReturnState::Detached
            | ExternalRxReturnState::Returned
            | ExternalRxReturnState::Invalid { .. } => {
                return Err(RxRingError::BufferOwnershipConflict);
            }
        }
        Ok(())
    }

    /// Normalize an observed buffer before a role-neutral frontier republishes
    /// its descriptor.
    ///
    /// A staged role may hand a live ring to a finite scan/join frontier after
    /// every upper lease has returned but before the next staged service pass
    /// recycled the released prefix. The descriptor remains observed and is
    /// therefore safe to republish, but its allocation still carries the
    /// durable `Returned` state. Normalize that state in the same transaction;
    /// otherwise the descriptor becomes DMA-visible while a later detach sees
    /// the stale release and rejects a real completion as `BufferOwnershipConflict`.
    fn prepare_observed_for_recycle(&self) -> Result<(), RxRingError> {
        self.1
            .reattach()
            .map_err(|_| RxRingError::BufferOwnershipConflict)?;
        self.prepare_for_recycle()
    }

    fn detach_with(&'static self, length: usize) -> Result<ExternalRxBuffer, RxRingError> {
        if length == 0 || length > BUFFER_SIZE {
            return Err(RxRingError::Size);
        }
        let pointer = NonNull::new(self.0.get().cast::<u8>()).expect("RX DMA buffer is non-null");
        #[allow(
            unsafe_code,
            reason = "completed descriptor detaches stable DMA buffer ownership"
        )]
        // SAFETY: the completed-unit owner proved that DMA released this
        // buffer. The buffer is part of stable arena storage and the state
        // transition prevents recycle until the affine buffer is returned.
        // Its whole `[u8; STORAGE_SIZE]` cell is initialized for its life:
        // `new` builds it from `[0; STORAGE_SIZE]`, zeroed placement
        // (`Zeroable`) writes zeros, and DMA only stores bytes into it, so
        // every byte of `BUFFER_SIZE <= STORAGE_SIZE` is initialized.
        unsafe { self.1.detach(pointer, length, BUFFER_SIZE) }
            .map_err(|_| RxRingError::BufferOwnershipConflict)
    }

    pub(super) fn is_released(&self) -> bool {
        self.1.state() == ExternalRxReturnState::Returned
    }

    pub(super) fn state(&self) -> ExternalRxReturnState {
        self.1.state()
    }

    fn prepare_for_stopped_ring(&self) -> Result<(), RxRingError> {
        self.1
            .reattach()
            .map_err(|_| RxRingError::BufferOwnershipConflict)?;
        self.prepare_for_recycle()
    }
}

#[allow(
    unsafe_code,
    reason = "detached-buffer state machine serializes cross-core access"
)]
// SAFETY: the atomic detached-buffer state serializes every cross-core
// access to the byte cell; only the current state owner reads or writes it.
unsafe impl<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> Sync
    for RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>
{
}

/// Only these constructors can pair an installation with its byte storage.
pub(crate) struct BoundBuffers<Source, R> {
    source: Source,
    ring: R,
}
impl<Source, R> BoundBuffers<Source, R> {
    pub(crate) fn into_parts(self) -> (Source, R) {
        (self.source, self.ring)
    }
}

pub(crate) struct InitialRing<'a, const N: usize> {
    pub(crate) table: &'a RxDescriptorTable<N>,
    pub(crate) descriptor_table: RxDescriptorAddresses,
    pub(crate) buffer_addresses: &'a RxDmaBufferAddresses<N>,
    pub(crate) reserved_buffer: DmaWindowAddress,
    pub(crate) arena: &'a RxArenaWords,
}
pub(crate) struct CompletedRead<'loan> {
    index: usize,
    _loan: PhantomData<&'loan ()>,
}
impl CompletedRead<'_> {
    pub(crate) fn index(&self) -> usize {
        self.index
    }
}

impl<'s, const N: usize, const B: usize, const S: usize, D: DmaSpace>
    BoundBuffers<&'s RxDmaStorage<N, B, S, D>, InitialRing<'s, N>>
{
    pub(super) fn initial(
        storage: &'s RxDmaStorage<N, B, S, D>,
        descriptor_table: RxDescriptorAddresses,
        reserved_buffer: DmaWindowAddress,
    ) -> Self {
        Self {
            source: storage,
            ring: InitialRing {
                table: &storage.descriptors,
                descriptor_table,
                buffer_addresses: &storage.buffer_addresses,
                reserved_buffer,
                arena: &storage.arena,
            },
        }
    }
}
impl<'s, 'loan, 'r, const N: usize, const B: usize, const S: usize, D: DmaSpace>
    BoundBuffers<&'s RxDmaStorage<N, B, S, D>, &'loan mut RxRingLive<'r, N>>
{
    pub(crate) fn live(
        storage: &'s RxDmaStorage<N, B, S, D>,
        ring: &'loan mut RxRingLive<'r, N>,
    ) -> Result<Self, RxRingError> {
        storage.validate_live_ring(ring)?;
        Ok(Self {
            source: storage,
            ring,
        })
    }
    pub(crate) fn live_ring(&self) -> &RxRingLive<'r, N> {
        self.ring
    }
    pub(super) fn live_ring_mut(&mut self) -> &mut RxRingLive<'r, N> {
        self.ring
    }
    pub(super) fn credit(credit: RxDmaRecycleUnit<'s, 'loan, 'r, N, B, S, D>) -> Self {
        Self {
            source: credit.storage,
            ring: credit.ring,
        }
    }
    pub(super) fn unit(unit: &'loan mut RxDmaCompletedUnit<'s, '_, 'r, N, B, S, D>) -> Self {
        Self {
            source: unit.storage,
            ring: unit
                .ring
                .as_deref_mut()
                .expect("completed unit retains ring loan"),
        }
    }
}
impl<'s, const N: usize, const B: usize, const S: usize, D: DmaSpace>
    BoundBuffers<&'s RxDmaStorage<N, B, S, D>, RxRingHalted<'s, N>>
{
    pub(crate) fn halted(
        storage: &'s RxDmaStorage<N, B, S, D>,
        ring: RxRingHalted<'s, N>,
    ) -> Result<Self, (RxRingHalted<'s, N>, RxRingError)> {
        if !ring.belongs_to_arena(&storage.arena) {
            return Err((ring, RxRingError::DescriptorOwnerAddress));
        }
        Ok(Self {
            source: storage,
            ring,
        })
    }
}
impl<'loan, Source> BoundBuffers<Source, CompletedRead<'loan>> {
    /// # Safety
    /// The retained owner must exclude DMA and detached writers from the
    /// selected descriptor and backing for the entire loan. A cached single
    /// buffer must be that descriptor's validated bound allocation.
    #[allow(
        unsafe_code,
        reason = "retained completed owners establish the CPU read loan"
    )]
    pub(super) unsafe fn completed<T: ?Sized>(
        source: Source,
        index: usize,
        _owner: &'loan T,
    ) -> Self {
        Self {
            source,
            ring: CompletedRead {
                index,
                _loan: PhantomData,
            },
        }
    }
}

impl<'id, 's, const N: usize, const B: usize, const S: usize, D: DmaSpace>
    ScopedBuffers<'id, &'s RxDmaStorage<N, B, S, D>>
{
    fn buffer<const RESERVED: bool>(
        &self,
        index: usize,
    ) -> Result<&'s RxDmaBuffer<B, S>, RxRingError> {
        // RESERVED is selected by the ring's grant type, not tested per frame.
        if RESERVED {
            Ok(&self.source().reserved_buffer)
        } else {
            self.source()
                .buffer_for_descriptor(index)
                .ok_or(RxRingError::Count)
        }
    }
    pub(crate) fn prepare_for_recycle<const RESERVED: bool>(
        &self,
        grant: RecycleGrant<'id, '_, Rearm, RESERVED>,
    ) -> Result<(), RxRingError> {
        self.buffer::<RESERVED>(grant.index())?
            .prepare_for_recycle()
    }
    pub(crate) fn prepare_observed_for_recycle(
        &self,
        grant: RecycleGrant<'id, '_>,
    ) -> Result<(), RxRingError> {
        self.buffer::<false>(grant.index())?
            .prepare_observed_for_recycle()
    }
    pub(crate) fn prepare_for_stopped_ring<const RESERVED: bool>(
        &self,
        grant: RecycleGrant<'id, '_, Rearm, RESERVED>,
    ) -> Result<(), RxRingError> {
        self.buffer::<RESERVED>(grant.index())?
            .prepare_for_stopped_ring()
    }
    pub(crate) fn completed_bytes<'bytes>(
        &self,
        grant: RecycleGrant<'id, 'bytes, ReadOnly>,
    ) -> Result<&'bytes [u8; B], RxRingError>
    where
        's: 'bytes,
    {
        Ok(self.buffer::<false>(grant.index())?.completed())
    }
}
// Single-descriptor completion already cached its validated allocation when
// taken. Preserve that path's pointer and avoid another lookup or admission.
impl<'id, 's, const B: usize, const S: usize> ScopedBuffers<'id, &'s RxDmaBuffer<B, S>> {
    pub(super) fn completed_bytes<'bytes>(
        &self,
        _grant: RecycleGrant<'id, 'bytes, ReadOnly>,
    ) -> &'bytes [u8; B]
    where
        's: 'bytes,
    {
        self.source().completed()
    }
}
impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize, S: DmaSpace>
    RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>
{
    /// The DMA address of `buffer`, one of this arena's buffers, as an
    /// integer for comparisons.
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn buffer_dma_address(
        &self,
        buffer: &RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>,
    ) -> Result<u32, RxDmaStorageError> {
        S::address(self, buffer.0.get()).ok_or(RxDmaStorageError::AddressWidth)
    }
    fn detach_buffer(
        &'static self,
        length: usize,
        index: usize,
    ) -> Result<ExternalRxBuffer, RxRingError> {
        let buffer = self.buffer_by_id(index).ok_or(RxRingError::Count)?;
        buffer.detach_with(length)
    }
}

// Detachment consumes the retained completed owner; callers outside this
// byte core cannot detach a buffer by a freely chosen physical index.
impl<
    'storage,
    'owner,
    'ring,
    const COUNT: usize,
    const BUFFER_SIZE: usize,
    const STORAGE_SIZE: usize,
    S: DmaSpace,
> RxDmaCompletedUnit<'storage, 'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE, S>
{
    /// Detach one complete hardware buffer for zero-copy upper ownership.
    ///
    /// The descriptor remains observed and CPU-owned in the ring. Before the
    /// buffer is returned, the descriptor's owner bit is cleared and `mmio`
    /// fences, so a walker that ever reached it would stop on it instead of
    /// writing the leased buffer (the second layer next to the reserved
    /// descriptor, [`RxRingLive`]). Dropping the returned buffer marks it
    /// released; only a later ring-owner service may rearm the descriptor,
    /// which sets the owner bit again.
    pub fn detach_single<M: RxDma>(mut self, mmio: &mut M) -> Result<RxDmaDetachedUnit, RxRingError>
    where
        'storage: 'static,
    {
        if self.unit.descriptor_count() != 1 {
            return Err(RxRingError::Count);
        }
        // A single descriptor's length always fits its own length field.
        let descriptor_word0 = self.unit.staged_word0().ok_or(RxRingError::Size)?;
        let index = self.unit.head_index();
        let buffer_id = self
            .storage
            .descriptor_buffer_id(index)
            .ok_or(RxRingError::Count)?;
        let detached_word = self
            .ring
            .as_deref_mut()
            .expect("completed unit retains ring loan")
            .detached_word(index)?;
        let buffer = self
            .storage
            .detach_buffer(self.unit.total_length(), buffer_id)?;
        self.ring
            .as_deref_mut()
            .expect("completed unit retains ring loan")
            .publish_detached(mmio, index, detached_word);
        self.requires_recycle = false;
        Ok(RxDmaDetachedUnit {
            buffer,
            descriptor_address: self.unit.descriptor_address(),
            descriptor_word0,
            length: self.unit.total_length(),
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
#[path = "host_dma.rs"]
pub(super) mod host_dma;

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::sync::atomic::AtomicU8;
    use std::boxed::Box;
    #[test]
    fn rx_buffer_keeps_its_dma_field_layout() {
        use core::mem::{align_of, offset_of, size_of};

        fn check<const B: usize, const S: usize>() {
            let alignment = align_of::<RxDmaBuffer<B, S>>();
            let old_fields = size_of::<UnsafeCell<[u8; S]>>() + size_of::<AtomicU8>();
            assert_eq!(
                size_of::<RxDmaBuffer<B, S>>(),
                old_fields.next_multiple_of(alignment),
            );
            assert_eq!(offset_of!(RxDmaBuffer<B, S>, 0), 0);
            assert_eq!(offset_of!(RxDmaBuffer<B, S>, 1), size_of::<[u8; S]>());
            assert_eq!(align_of::<ExternalRxReturn>(), align_of::<AtomicU8>());
        }

        check::<16, 16>();
        check::<16, 17>();
        check::<1700, 1700>();
    }

    #[test]
    fn rx_buffer_keeps_detach_and_recycle_error_conditions() {
        let buffer = &*Box::leak(Box::new(RxDmaBuffer::<16, 16>::new()));
        for length in [0, 17] {
            assert_eq!(buffer.detach_with(length).err(), Some(RxRingError::Size));
        }
        assert_eq!(buffer.state(), ExternalRxReturnState::Attached);
        let detached = buffer.detach_with(16).unwrap();
        assert_eq!(buffer.detach_with(0).err(), Some(RxRingError::Size));
        assert_eq!(
            buffer.detach_with(16).err(),
            Some(RxRingError::BufferOwnershipConflict),
        );
        assert_eq!(
            buffer.prepare_for_recycle(),
            Err(RxRingError::BufferOwnershipConflict)
        );
        assert_eq!(
            buffer.prepare_observed_for_recycle(),
            Err(RxRingError::BufferOwnershipConflict)
        );
        assert_eq!(
            buffer.prepare_for_stopped_ring(),
            Err(RxRingError::BufferOwnershipConflict)
        );
        drop(detached);
        assert_eq!(buffer.state(), ExternalRxReturnState::Returned);
        assert_eq!(
            buffer.prepare_for_recycle(),
            Err(RxRingError::BufferOwnershipConflict)
        );
        assert_eq!(buffer.prepare_observed_for_recycle(), Ok(()));
        assert_eq!(buffer.state(), ExternalRxReturnState::Attached);
        drop(buffer.detach_with(16).unwrap());
        assert_eq!(buffer.prepare_for_stopped_ring(), Ok(()));
        assert_eq!(buffer.state(), ExternalRxReturnState::Attached);
    }
}
