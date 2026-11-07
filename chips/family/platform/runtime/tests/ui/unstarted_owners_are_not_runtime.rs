#![no_std]
use oer_chips_family_boot::adoption::{Owners, Port};
use oer_chips_family_platform_interrupt_table::Controller;

pub fn publish<P: Port, C: Controller>(owners: Owners<P>) {
    #[cfg(ui_reject)]
    let _ready: oer_chips_family_platform_runtime::adoption::Runtime<P::Output, C, ()> = owners; // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _unstarted = owners;
}
