// ui-requires: test-support
use core::cell::RefCell;
use oer_chips_esp32s31_hal::radio::ieee80211::dma::{
    rx_dma::model::RxWalkerModel, rx_ring::RxRingLive, rx_storage::RxDmaStorage, space::HostDma,
};

// Two writers of one host arena run on one thread, one after the other: no
// two threads write it at once, through `land_unit` or `dma_write`.
pub fn two_writers(
    storage: &RxDmaStorage<2, 16, 16, HostDma>,
    ring: &RxRingLive<'_, 2>,
    address: u32,
) {
    let _ = storage.land_unit(&RefCell::new(RxWalkerModel::new()), &[1; 4]);
    let _ = storage.dma_write(ring, address, &[2; 4]);
    std::thread::scope(|scope| {
        #[cfg(ui_reject)]
        scope.spawn(|| storage.land_unit(&RefCell::new(RxWalkerModel::new()), &[3; 4]).is_ok()); // ui-error: E0277
        #[cfg(ui_reject)]
        scope.spawn(|| storage.dma_write(ring, address, &[4; 4]).is_ok()); // ui-error: E0277
        let _ = scope;
    });
}
