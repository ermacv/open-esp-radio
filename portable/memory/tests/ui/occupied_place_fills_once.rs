#![no_std]
use core::mem::MaybeUninit;
use oer_portable_memory::Occupied;

pub fn twice(place: &mut MaybeUninit<u32>) -> u32 {
    let first = Occupied::fill(&mut *place, 7);
    #[cfg(ui_reject)]
    let _second = Occupied::fill(&mut *place, 8); // ui-error: E0499
    *first
}
