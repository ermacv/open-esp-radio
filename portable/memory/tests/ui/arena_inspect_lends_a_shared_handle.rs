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
        placed by place as Placed in ".dma.bss.ui_inspect";
}

pub type RxArena = Arena<Rx, 512>;

/// A borrowed release lends a shared handle for the call: its bytes read.
pub fn read(arena: &RxArena, quiet: &Quiet<'_, Rx>) -> usize {
    arena.inspect(quiet, |handle| arena.bytes(handle).len())
}

/// Its bytes are not written through it.
#[cfg(ui_reject)]
pub fn write(arena: &RxArena, quiet: &Quiet<'_, Rx>) {
    arena.inspect(quiet, |handle| arena.bytes_mut(handle).fill(0)); // ui-error: E0308
}

/// It does not outlive the call.
#[cfg(ui_reject)]
pub fn keep<'a>(arena: &RxArena, quiet: &Quiet<'_, Rx>, kept: &mut Option<&'a Handle<Rx>>) {
    arena.inspect(quiet, |handle| *kept = Some(handle)); // ui-error: E0521
}
