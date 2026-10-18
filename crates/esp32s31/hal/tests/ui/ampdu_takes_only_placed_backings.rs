#![no_std]
use oer_esp32s31_hal::dma_static::PlacedTxLease;
use oer_esp32s31_hal::ieee80211::dma::tx_ampdu_storage::RetainedAmpduDma;
use oer_base_memory::PinnedDmaTxRadioLease;

// An aggregate publishes a retained backing's address without a check, so
// it takes only a lease of a pool placed in the DMA window: a lease of an
// ordinary pool is refused.
pub fn placed(
    owner: &mut RetainedAmpduDma<'_, PlacedTxLease<PinnedDmaTxRadioLease<'static, 64, 0, 0>>, 2, 0>,
    lease: PlacedTxLease<PinnedDmaTxRadioLease<'static, 64, 0, 0>>,
) {
    let _ = owner.push_backing(lease);
}

#[cfg(ui_reject)]
pub fn unplaced(
    owner: &mut RetainedAmpduDma<'_, PinnedDmaTxRadioLease<'static, 64, 0, 0>, 2, 0>,
    lease: PinnedDmaTxRadioLease<'static, 64, 0, 0>,
) {
    let _ = owner.push_backing(lease); // ui-error: E0599
}
