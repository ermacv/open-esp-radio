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
    pub Rx: window Window, count 4, size 128, floor 64,
        placed by place as Placed in ".dma.bss.ui_floor";
}

pub type RxArena = Arena<Rx, 512>;

/// The floor of a held buffer is copied out.
pub fn held(arena: &RxArena, handle: &Handle<Rx>) -> [u8; 64] {
    arena.floor(handle).copy()
}

/// A lent buffer is the hardware's: there is no handle to ask its floor by.
#[cfg(ui_reject)]
pub fn lent(arena: &RxArena, lent: &Lent<Rx>) -> [u8; 64] {
    arena.floor(lent).copy() // ui-error: E0308
}
