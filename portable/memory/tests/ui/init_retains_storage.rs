#![no_std]
#![forbid(unsafe_code)]
use core::mem::MaybeUninit;

pub struct Pair { first: u8, second: u8 }
pub fn build(storage: &mut MaybeUninit<Pair>) -> &mut Pair {
    let value = oer_memory::init_in_place!(storage, Pair { first: 1, second: 2 });
    #[cfg(ui_reject)]
    let value: &'static mut Pair = value; // ui-error: lifetime may not live long enough
    value
}
