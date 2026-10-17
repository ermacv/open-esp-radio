#![no_std]
use oer_ieee80211_lower_mac::association::{BlockAckSlot, BlockAckSlots, RxBlockAckPool};
use oer_radio_port::PoolPath;

/// A pool a backend would declare to claim a capacity it does not mint.
pub struct Declared;

#[cfg(ui_reject)]
impl RxBlockAckPool for Declared { // ui-error: E0277
    type Kind = u8;
    const LEN: usize = 8;
    type Slot<'id, Path: PoolPath> = BlockAckSlot<'id, u8, 2, Path>;
}

pub fn capacity() -> usize {
    <BlockAckSlots<u8, 2> as RxBlockAckPool>::LEN
}
