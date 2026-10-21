#![no_std]
#![forbid(unsafe_code)]
use core::mem::MaybeUninit;

pub struct Token;
pub struct Pair { first: Token, second: Token }
pub fn build(storage: &mut MaybeUninit<Pair>, first: Token, second: Token) -> &mut Pair {
    #[cfg(ui_reject)]
    let second = first;
    oer_memory::init_in_place!(storage, Pair {
        first: first, // ui-error: E0382
        second: second,
    })
}
