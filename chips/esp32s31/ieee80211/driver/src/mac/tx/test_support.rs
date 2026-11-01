//! Checked semantic completion fixtures for TX unit tests.

use super::Box;
use super::{CompletionObservation, TxBacking, TxCompletion, TxCookie};
use oer_esp32s31_hal::memory::dma_static::{Placed, PlacedTxPool};
use oer_memory::PinnedDmaTxPool;

/// A host-placed transmit pool of `N` buffers of `B` bytes, as a facade
/// places one in the DMA window; the model allocation is never freed.
pub fn model_pool<const B: usize, const N: usize>() -> Option<&'static PlacedTxPool<B, 0, 0, N>> {
    let storage = Box::leak(Box::new(PinnedDmaTxPool::new()));
    let pool = PlacedTxPool::pin(Placed::host(storage)).ok()?;
    Some(Box::leak(Box::new(pool)))
}

/// Buffer `index` of `pool`, as the radio holds it.
pub fn pool_backing<const B: usize, const N: usize>(
    pool: &'static PlacedTxPool<B, 0, 0, N>,
    index: u8,
) -> Option<TxBacking<B>> {
    pool.claim_network(index)
        .ok()?
        .publish(oer_memory::FrameLen::constant::<0>(), |_| {});
    pool.claim_radio(index).ok()
}

/// One model buffer of `B` bytes, from a pool of its own.
pub fn model_backing<const B: usize>() -> Option<TxBacking<B>> {
    pool_backing(model_pool::<B, 1>()?, 0)
}

pub fn model_completion(cookie: TxCookie, status: u8, detail: u8) -> TxCompletion {
    match TxCompletion::new_model(cookie, status, detail).expect("valid model observation") {
        CompletionObservation::Observed(completion) => completion,
        CompletionObservation::NoEffect => panic!("fixture needs an observed completion"),
    }
}
