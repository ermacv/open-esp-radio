#![no_std]
use oer_chips_family_staged_runtime::adoption::{Binding, Prepared};

pub fn publish<B: Binding>(prepared: Prepared<B>) {
    #[cfg(ui_reject)]
    let _ready: oer_chips_family_staged_runtime::adoption::Runtime<B::Hardware, B::Controller, ()> = prepared; // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _prepared = prepared;
}
