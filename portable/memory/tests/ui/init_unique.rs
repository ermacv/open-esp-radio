#![no_std]
#![forbid(unsafe_code)]
use core::mem::MaybeUninit;

pub struct Pair { first: u8, second: u8 }
pub fn build(storage: &mut MaybeUninit<Pair>) -> &mut Pair {
    #[cfg(ui_reject)]
    return oer_memory::init_in_place!(storage, Pair { first: 1, first: 2, second: 3 }); // ui-error: E0415
    // ui-error@-1: E0062
    #[cfg(not(ui_reject))]
    oer_memory::init_in_place!(storage, Pair { first: 1, second: 2 })
}
