use oer_memory::{Entered, Never};

#[cfg(not(ui_reject))]
pub fn proof() -> Option<Entered<Never>> {
    Entered::enter()
}

#[cfg(ui_reject)]
pub fn proof() -> Entered<Never> {
    Entered(core::marker::PhantomData) // ui-error: E0423
}
