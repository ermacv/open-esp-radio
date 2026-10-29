#![no_std]
use oer_esp32s31_hal::memory::dma_static::PlacedTxLease;
use oer_esp32s31_hal::ieee80211::dma::tx_ampdu_storage::{RetainedAmpduDma, RetainedDmaBacking};
use oer_memory::PinnedDmaTxRadioLease;

pub fn bind(
    owner: &mut RetainedAmpduDma<'_, PlacedTxLease<PinnedDmaTxRadioLease<'static, 64, 0, 0>>, 2, 0>,
    backing: RetainedDmaBacking,
    address: usize,
) {
    // A rejected image returns the identity, so the same lease can be corrected.
    let backing = match owner.commit_backing_descriptor(backing, address, 64, 0) {
        Ok(()) => return,
        Err((backing, _)) => backing,
    };
    let _ = owner.commit_backing_descriptor(backing, address, 64, 8);
    #[cfg(ui_reject)]
    let _ = owner.commit_backing_descriptor(backing, address, 64, 8); // ui-error: E0382
}
