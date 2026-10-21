#![no_std]
#![forbid(unsafe_code)]
mod core {} use ::core::mem::MaybeUninit;

pub struct Pair { pub first: u8, pub second: u8 }
pub fn build(storage: &mut MaybeUninit<Pair>) -> &mut Pair {
    #[cfg(ui_reject)]
    return oer_memory::init_in_place!(storage, Pair { first: 1 }); // ui-error: E0063
    #[cfg(not(ui_reject))]
    oer_memory::init_in_place!(storage, Pair { first: 1, second: 2 })
}
