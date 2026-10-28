#![no_std]
use oer_esp32s31_hal::memory::dma_static::PlacedTxLease;
use oer_memory::{PinnedDmaTxRadioLease, StableDmaBacking};

type Lease = PinnedDmaTxRadioLease<'static, 64, 0, 0>;

pub fn write_bytes(placed: &mut PlacedTxLease<Lease>) {
    placed.stable_dma_region().as_mut_slice().fill(0);
}

#[cfg(ui_reject)]
pub fn replace_backing(placed: &mut PlacedTxLease<Lease>, foreign: Lease) {
    **placed = foreign; // ui-error: E0594
}
