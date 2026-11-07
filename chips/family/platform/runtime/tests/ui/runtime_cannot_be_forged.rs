#![no_std]
use oer_chips_family_platform_interrupt_table::Controller;
use oer_chips_family_platform_runtime::Adopted;

pub fn publish<H, C: Controller, M>(hardware: H, mapping: M, interrupts: Adopted<C>) {
    #[cfg(ui_reject)]
    let _ready = oer_chips_family_platform_runtime::adoption::Runtime { hardware, mapping, interrupts }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let _unpublished = (hardware, mapping, interrupts);
}
