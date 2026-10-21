#![no_std]
#![forbid(unsafe_code)]
use core::mem::MaybeUninit;

pub struct One { value: u8 }
pub fn build(storage: &mut MaybeUninit<One>, pointer: *const u8) -> &mut One {
    #[cfg(ui_reject)]
    return oer_memory::init_in_place!(storage, One { value: *pointer }); // ui-error: E0133
    #[cfg(not(ui_reject))]
    oer_memory::init_in_place!(storage, One { value: 7 })
}
