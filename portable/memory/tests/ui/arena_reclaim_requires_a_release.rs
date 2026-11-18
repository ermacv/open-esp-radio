#![no_std]
use oer_portable_memory::arena::*;
use oer_portable_memory::{DmaLayout, DmaRef, DmaWindow, zeroed::ZeroedStatic};

pub struct Window;

impl DmaWindow for Window {
    const RANGE: core::ops::Range<u32> = 0x4000_0000..0x4010_0000;
}

/// A placement that never hands its value out: the cases build no arena.
pub struct Placed<T>(pub ZeroedStatic<T>);

// SAFETY: `claim` gives nothing.
unsafe impl<T: DmaLayout> PlacedStatic<T> for Placed<T> {
    fn claim(&'static self) -> Option<(&'static mut T, DmaRef<T>)> {
        None
    }
}

macro_rules! place {
    (static $name:ident: $ty:ty = zeroed in $section:literal;) => {
        static $name: Placed<$ty> = Placed(ZeroedStatic::new());
    };
}

oer_portable_memory::arena! {
    pub Rx: window Window, count 4, size 64, floor 8,
        placed by place as Placed in ".dma.bss.ui_rx";
}

oer_portable_memory::arena! {
    pub Tx: window Window, count 4, size 64, floor 0,
        placed by place as Placed in ".dma.bss.ui_tx";
}

pub struct Quiet;

impl ReturnNotify for Quiet {
    fn notify() {}
}

pub type RxArena = Arena<Rx, 256>;
pub type TxArena = Arena<Tx, 256>;

pub fn reclaim(arena: RxArena, lent: Lent<Rx>, released: Released<Rx>) -> Handle<Rx> {
    #[cfg(ui_reject)]
    let _ = arena.reclaim(lent); // ui-error: E0308
    #[cfg(not(ui_reject))]
    drop(lent);
    arena.reclaim(released)
}
