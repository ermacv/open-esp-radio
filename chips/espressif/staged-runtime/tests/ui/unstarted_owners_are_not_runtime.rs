#![no_std]
use oer_espressif_boot::adoption::{Owners, Port};
use oer_espressif_interrupt_table::Controller;

pub fn publish<P: Port, C: Controller>(owners: Owners<P>) {
    #[cfg(ui_reject)]
    let _ready: oer_espressif_staged_runtime::adoption::Runtime<P::Output, C, ()> = owners; // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _unstarted = owners;
}
