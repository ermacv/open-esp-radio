#![no_std]

pub struct Owner;

pub fn transfer(owner: Owner) -> Owner {
    let moved = owner;
    #[cfg(ui_reject)]
    return owner; // ui-error: E0382
    #[cfg(not(ui_reject))]
    moved
}
