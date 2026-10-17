#![no_std]
use oer_esp32s31_ieee80211_dma::ring_index::RingIndex;

pub fn unchecked(raw: usize) -> Option<RingIndex<4>> {
    #[cfg(ui_reject)]
    { Some(RingIndex(raw)) } // ui-error: E0423
    #[cfg(not(ui_reject))]
    { RingIndex::checked(raw) }
}

fn four_descriptors(_index: RingIndex<4>) {}

pub fn foreign_bound(index: RingIndex<8>, matching: RingIndex<4>) {
    #[cfg(ui_reject)]
    four_descriptors(index); // ui-error: E0308
    #[cfg(not(ui_reject))]
    four_descriptors(matching);
}
