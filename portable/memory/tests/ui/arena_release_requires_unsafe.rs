#![no_std]
use oer_portable_memory::arena::*;
use oer_portable_memory::{DmaWindow, zeroed::ZeroedStatic};

pub struct Window;

impl DmaWindow for Window {
    const RANGE: core::ops::Range<u32> = 0x4000_0000..0x4010_0000;
}

macro_rules! place {
    (static $name:ident: $ty:ty = zeroed in $section:literal;) => {
        static $name: ZeroedStatic<$ty> = ZeroedStatic::new();
    };
}

oer_portable_memory::arena! {
    pub Rx: window Window, count 4, size 64, front 8, back 4,
        placed by place as ZeroedStatic in ".dma.bss.ui_rx";
}

oer_portable_memory::arena! {
    pub Tx: window Window, count 4, size 64, front 0, back 0,
        placed by place as ZeroedStatic in ".dma.bss.ui_tx";
}

pub struct Quiet;

impl ReturnNotify for Quiet {
    fn notify() {}
}

pub type RxArena = Arena<Rx, 256>;
pub type TxArena = Arena<Tx, 256>;

pub fn release(lent: Lent<Rx>) -> Option<Released<Rx>> {
    #[cfg(ui_reject)]
    return Some(Released::assume(lent, Extent::FULL)); // ui-error: E0133
    #[cfg(not(ui_reject))]
    {
        drop(lent);
        None
    }
}
