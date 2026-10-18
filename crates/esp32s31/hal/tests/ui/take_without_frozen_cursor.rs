#![no_std]
use oer_esp32s31_hal::ieee80211::dma::rx_ring::{RxFrozenCursor, RxRingLive};
use oer_esp32s31_hal::ieee80211::dma::rx_storage::RxDmaStorage;

// A unit or a descriptor is taken only through a cursor frozen on the ring:
// the take is bounded by its LAST inclusive. No bare descriptor limit and no
// missing cursor takes one.
pub fn take<const COUNT: usize, const BUFFER: usize, const STORAGE: usize>(
    storage: &RxDmaStorage<COUNT, BUFFER, STORAGE>,
    ring: &mut RxRingLive<'_, COUNT>,
    cursor: RxFrozenCursor,
) {
    let _ = storage.take_completed_unit(ring, cursor);
    #[cfg(ui_reject)]
    let _ = storage.take_completed_unit(ring); // ui-error: E0061
    #[cfg(ui_reject)]
    let _ = storage.take_completed_unit(ring, 1); // ui-error: E0308
    let _ = storage.take_completed(ring, cursor, 0);
    #[cfg(ui_reject)]
    let _ = storage.take_completed(ring, 0); // ui-error: E0061
}
