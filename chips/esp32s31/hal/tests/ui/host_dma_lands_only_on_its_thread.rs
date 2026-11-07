// ui-requires: test-support
use core::cell::RefCell;
use oer_chips_esp32s31_hal::radio::ieee80211::dma::{
    rx_dma::model::RxWalkerModel, rx_ring::RxRingLive, rx_storage::RxDmaStorage, space::HostDma,
};

// The host model of the DMA engine writes an arena only on the thread that
// holds its references: a `HostDma` arena is not `Sync`, so no other thread
// lands a unit while the live ring and its views stay here.
pub fn land_elsewhere(
    storage: &'static RxDmaStorage<2, 16, 16, HostDma>,
    ring: &mut RxRingLive<'static, 2>,
) {
    let walker = RefCell::new(RxWalkerModel::new());
    let _ = storage.land_unit(&walker, &[1, 2, 3, 4]);
    #[cfg(ui_reject)]
    std::thread::spawn(move || storage.land_unit(&RefCell::new(RxWalkerModel::new()), &[9; 4]).is_ok()); // ui-error: E0277
    let _ = storage.reserved_frame(ring);
}
