#![no_std]
#![forbid(unsafe_code)]
use core::mem::MaybeUninit;

mod sealed {
    pub struct Token { value: u8 }
    pub fn init(storage: &mut MaybeUninit<Token>) -> &mut Token {
        oer_memory::init_in_place!(storage, Token { value: 1 })
    }
    use core::mem::MaybeUninit;
}

pub fn build(storage: &mut MaybeUninit<sealed::Token>) -> &mut sealed::Token {
    #[cfg(ui_reject)]
    return oer_memory::init_in_place!(storage, sealed::Token {
        value: 1, // ui-error: E0616
    });
    #[cfg(not(ui_reject))]
    sealed::init(storage)
}
