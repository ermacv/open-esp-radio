#![no_std]
use oer_espressif_interrupt_table::Controller;
use oer_espressif_staged_runtime::Adopted;

pub fn publish<H, C: Controller, M>(hardware: H, mapping: M, interrupts: Adopted<C>) {
    #[cfg(ui_reject)]
    let _ready = oer_espressif_staged_runtime::adoption::Runtime { hardware, mapping, interrupts }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let _unpublished = (hardware, mapping, interrupts);
}
