#![no_std]
use core::mem::MaybeUninit;
use oer_portable_memory::Occupied;

pub fn vacated(place: &mut MaybeUninit<u32>) -> u32 {
    let occupied = Occupied::fill(place, 7);
    let (value, _) = occupied.vacate();
    #[cfg(ui_reject)]
    let _ = *occupied; // ui-error: E0382
    value
}
