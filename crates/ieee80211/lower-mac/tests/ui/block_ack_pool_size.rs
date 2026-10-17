#![no_std]
use oer_ieee80211_lower_mac::association::{BlockAckSlot, BlockAckSlots, RxBlockAckPool};
use oer_radio_port::Root;

fn take<'id, P: RxBlockAckPool>(_: P::Slot<'id, Root>) {}

pub fn give<'id>(minted: BlockAckSlot<'id, u8, 2>) {
    #[cfg(ui_reject)]
    take::<BlockAckSlots<u8, 4>>(minted); // ui-error: E0308
    #[cfg(not(ui_reject))]
    take::<BlockAckSlots<u8, 2>>(minted);
}
