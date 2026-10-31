use oer_esp32s31_hal::{
    cores::Released,
    platform::{Hart1Controller, Hart1Grant, InterruptController, InterruptGrant},
};
use oer_espressif_interrupt_table::Router;

fn send<T: Send>() {}
fn keep(_: Hart1Grant) {}

/// A released hart's grant moves out once and is not copied.
pub fn hand_over(released: &mut Released) {
    if let Some(grant) = released.take_interrupts() {
        keep(grant);
        #[cfg(ui_reject)]
        keep(grant); // ui-error: E0382
    }
}

/// Only the root pairs a hart's bank with its own CLIC parts.
#[cfg(ui_reject)]
pub fn forge(
    bank: oer_esp32s31_pac::interrupt_core::InterruptCore1,
    clic: oer_esp32s31_pac::clic::Clic,
) -> Hart1Grant {
    InterruptGrant::new(bank, clic, None) // ui-error: E0624
}

/// A hart's controller, and the router over it, never leave the hart.
pub fn stays_local() {
    send::<Hart1Grant>();
    #[cfg(ui_reject)]
    send::<Hart1Controller>(); // ui-error: E0277
    #[cfg(ui_reject)]
    send::<Router<InterruptController>>(); // ui-error: E0277
}
