#![no_std]
use oer_radio_port::{Child, Mint, PoolBuilder, PoolPath, PoolSet, Pools, Root, Slot, Slots};

pub enum Kind {}
pub struct Wrapped;

impl PoolSet for Wrapped {
    type Pools<'id, Path: PoolPath> = Pools<'id, (Slots<Kind, 1>, (Slots<Kind, 1>,)), Path>;
    fn build<'id, Path: PoolPath>(builder: PoolBuilder<'id, Path>) -> Self::Pools<'id, Path> {
        builder.into_pools::<(Slots<Kind, 1>, (Slots<Kind, 1>,))>()
    }
}

pub fn select<'id>(mint: Mint<'id, Wrapped>) -> Slot<'id, Kind, Child<Root, 0>> {
    let ([first], ([nested],)) = mint.into_pools();
    #[cfg(ui_reject)]
    return nested; // ui-error: E0308
    #[cfg(not(ui_reject))]
    first
}
