use crate::ieee80211::dma::{
    rx_dma::RxDma,
    rx_ring::{with_bound_buffers, RecycleGrant, Rearm, RxRingLive, RxRingHalted, RxRingError},
    rx_storage::{RxDmaBuffer, RxDmaStorage, buffers::BoundBuffers},
};
fn half(storage: &RxDmaStorage<4, 16, 16>, ring: &mut RxRingLive<'_, 4>, mmio: &mut impl RxDma) -> Result<(), RxRingError> {
    with_bound_buffers(BoundBuffers::live(storage, ring)?, |ring, buffers| {
        let mut held: Option<RecycleGrant<'_, '_>> = None;
        let result = ring.recycle_completed_half_owned(mmio, |grant| {
            #[cfg(escape_half)]
            { held = Some(grant); Ok(()) }
            #[cfg(not(escape_half))]
            { buffers.prepare_for_recycle(grant) }
        });
        drop(held);
        result.map(|_| ())
    })
}
fn prefix(storage: &RxDmaStorage<4, 16, 16>, ring: &mut RxRingLive<'_, 4>, mmio: &mut impl RxDma) -> Result<(), RxRingError> {
    with_bound_buffers(BoundBuffers::live(storage, ring)?, |ring, buffers| {
        let mut held: Option<RecycleGrant<'_, '_>> = None;
        let result = ring.recycle_completed_prefix_owned::<1, _, _>(mmio, |grant| {
            #[cfg(escape_prefix)]
            { held = Some(grant); Ok(()) }
            #[cfg(not(escape_prefix))]
            { buffers.prepare_for_recycle(grant) }
        });
        drop(held);
        result.map(|_| ())
    })
}
fn unit(storage: &RxDmaStorage<4, 16, 16>, ring: &mut RxRingLive<'_, 4>, mmio: &mut impl RxDma) -> Result<(), RxRingError> {
    with_bound_buffers(BoundBuffers::live(storage, ring)?, |ring, buffers| {
        let mut held: Option<RecycleGrant<'_, '_>> = None;
        let result = ring.recycle_completed_unit_owned(mmio, 1, |grant| {
            #[cfg(escape_unit)]
            { held = Some(grant); Ok(()) }
            #[cfg(not(escape_unit))]
            { buffers.prepare_for_recycle(grant) }
        });
        drop(held);
        result.map(|_| ())
    })
}
fn released(storage: &RxDmaStorage<4, 16, 16>, ring: &mut RxRingLive<'_, 4>, mmio: &mut impl RxDma) -> Result<(), RxRingError> {
    with_bound_buffers(BoundBuffers::live(storage, ring)?, |ring, buffers| {
        let mut held: Option<RecycleGrant<'_, '_>> = None;
        let result = ring.recycle_released_terminal_prefix_owned(mmio, 1, |grant| {
            #[cfg(escape_released)]
            { held = Some(grant); Ok(()) }
            #[cfg(not(escape_released))]
            { buffers.prepare_for_recycle(grant) }
        });
        drop(held);
        result.map(|_| ())
    })
}
fn proven(storage: &RxDmaStorage<4, 16, 16>, ring: &mut RxRingLive<'_, 4>, mmio: &mut impl RxDma) -> Result<(), RxRingError> {
    with_bound_buffers(BoundBuffers::live(storage, ring)?, |ring, buffers| {
        let mut held: Option<RecycleGrant<'_, '_>> = None;
        let result = ring.recycle_proven_unit_owned(mmio, 1, |grant| {
            #[cfg(escape_proven)]
            { held = Some(grant); Ok(()) }
            #[cfg(not(escape_proven))]
            { buffers.prepare_for_recycle(grant) }
        });
        drop(held);
        result.map(|_| ())
    })
}
fn prepare<'s>(storage: &'s RxDmaStorage<4, 16, 16>, ring: RxRingHalted<'s, 4>, mmio: &mut impl RxDma) -> Result<(), RxRingError> {
    let binding = BoundBuffers::halted(storage, ring).map_err(|(_, error)| error)?;
    with_bound_buffers(binding, |ring, buffers| {
        let mut held: Option<RecycleGrant<'_, '_>> = None;
        let mut reserved: Option<RecycleGrant<'_, '_, Rearm, true>> = None;
        let result = ring.prepare_owned(mmio, 16, |grant| {
            #[cfg(escape_prepare)]
            { held = Some(grant); Ok(()) }
            #[cfg(not(escape_prepare))]
            { buffers.prepare_for_stopped_ring(grant) }
        }, |grant| {
            #[cfg(escape_reserved)]
            { reserved = Some(grant); Ok(()) }
            #[cfg(not(escape_reserved))]
            { buffers.prepare_for_stopped_ring(grant) }
        });
        drop((held, reserved));
        result.map(|_| ()).map_err(|(_, error)| error)
    })
}
fn foreign(first: &RxDmaStorage<4, 16, 16>, first_ring: &mut RxRingLive<'_, 4>,
           second: &RxDmaStorage<4, 16, 16>, second_ring: &mut RxRingLive<'_, 4>, mmio: &mut impl RxDma) -> Result<(), RxRingError> {
    with_bound_buffers(BoundBuffers::live(first, first_ring)?, |_first_ring, first_buffers| {
        with_bound_buffers(BoundBuffers::live(second, second_ring)?, |second_ring, second_buffers| {
            second_ring.recycle_completed_prefix_owned::<1, _, _>(mmio, |grant| {
                #[cfg(foreign_ring)]
                { first_buffers.prepare_for_recycle(grant) }
                #[cfg(not(foreign_ring))]
                { second_buffers.prepare_for_recycle(grant) }
            }).map(|_| ())
        })
    })
}
#[cfg(forge_grant)]
fn forge() { let _: RecycleGrant<'_, '_> = RecycleGrant::new(0); }
#[cfg(raw_bytes)]
fn raw(buffer: &RxDmaBuffer<16, 16>) { let _ = buffer.prepare_for_recycle(); }
fn affine(storage: &RxDmaStorage<4, 16, 16>, ring: &mut RxRingLive<'_, 4>, mmio: &mut impl RxDma) -> Result<(), RxRingError> {
    with_bound_buffers(BoundBuffers::live(storage, ring)?, |ring, buffers| {
        ring.recycle_completed_prefix_owned::<1, _, _>(mmio, |grant| {
            buffers.prepare_for_recycle(grant)?;
            #[cfg(duplicate_grant)]
            buffers.prepare_for_recycle(grant)?;
            Ok(())
        }).map(|_| ())
    })
}

// Const proofs keep grants and scoped hot-path receivers pointer/index sized.
const _: () = assert!(core::mem::size_of::<RecycleGrant<'static, 'static>>() == core::mem::size_of::<usize>());
const _: () = assert!(core::mem::size_of::<crate::ieee80211::dma::rx_ring::ScopedRing<'static, &mut RxRingLive<'static, 4>>>() == core::mem::size_of::<&mut RxRingLive<'static, 4>>());
fn read_authority<'id, 'loan, 'storage: 'loan>(
    ring: crate::ieee80211::dma::rx_ring::ScopedRing<'id, crate::ieee80211::dma::rx_storage::buffers::CompletedRead<'loan>>,
    buffers: crate::ieee80211::dma::rx_ring::ScopedBuffers<'id, &'storage RxDmaStorage<4, 16, 16>>,
) {
    #[cfg(read_rearm)]
    let _ = buffers.prepare_for_recycle(ring.read_grant());
    #[cfg(not(read_rearm))]
    let _ = buffers.completed_bytes(ring.read_grant());
}

#[cfg(raw_completed)]
fn raw_read(buffer: &RxDmaBuffer<16, 16>) { let _ = buffer.completed(); }
#[cfg(raw_detach)]
fn raw_detach(storage: &'static RxDmaStorage<4, 16, 16>) { let _ = storage.detach_buffer(4, 0); }
