#![no_std]
use oer_portable_memory::arena::*;
use oer_portable_memory::{DmaLayout, DmaRef, DmaWindow, zeroed::ZeroedStatic};

pub struct Window;

impl DmaWindow for Window {
    const RANGE: core::ops::Range<u32> = 0x4000_0000..0x4010_0000;
}

/// The placement, outside the code that forbids `unsafe`, as a HAL's is.
pub struct Placed<T>(pub ZeroedStatic<T>);

// SAFETY: `claim` gives nothing.
unsafe impl<T: DmaLayout> PlacedStatic<T> for Placed<T> {
    fn claim(&'static self) -> Option<(&'static mut T, DmaRef<T>)> {
        None
    }
}

#[macro_export]
macro_rules! place {
    (static $name:ident: $ty:ty = zeroed in $section:literal;) => {
        static $name: $crate::Placed<$ty> = $crate::Placed(ZeroedStatic::new());
    };
}

/// An image's code: it forbids `unsafe`. The macro's `unsafe impl Brand`
/// builds here; one written by hand does not.
#[forbid(unsafe_code)]
pub mod image {
    use super::*;

    oer_portable_memory::arena! {
        pub Rx: window Window, count 4, size 64, floor 8,
            placed by place as Placed in ".dma.bss.ui_forbid";
    }

    pub type RxArena = Arena<Rx, 256>;

    pub enum Mine {}

    #[cfg(ui_reject)]
    unsafe impl Brand for Mine { // ui-error: implementation of an `unsafe` trait
        type Window = Window;
        const COUNT: usize = 4;
        const SIZE: usize = 64;
        const FLOOR: usize = 8;
        type Static = ();
        fn place() -> &'static () {
            &()
        }
    }
}
