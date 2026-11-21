use oer_chips_esp32s31_hal::{
    system::cores::Released,
    system::platform::{
        Hart1Controller, Hart1Grant, Hart1Routes, HartPending, InterruptController, InterruptGrant,
    },
};
use oer_chips_family_platform_interrupt_table::{NoBindings, Router};

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
    bank: Hart1Routes,
    clic: oer_chips_esp32s31_pac::clic::Clic,
    pending: HartPending<1>,
) -> Hart1Grant {
    InterruptGrant::new(bank, clic, None, pending) // ui-error: E0624
}

/// Only hart 1's proof makes hart 1's controller.
pub fn on_its_hart(grant: Hart1Grant, hart: oer_portable_riscv::OnHart<1>) -> Hart1Controller {
    grant.into_controller_with(hart)
}

/// Another hart's proof does not: hart 0 would reach its own CLIC through
/// hart 1's parts.
#[cfg(ui_reject)]
pub fn on_another_hart(grant: Hart1Grant, hart: oer_portable_riscv::OnHart<0>) -> Hart1Controller {
    grant.into_controller_with(hart) // ui-error: E0308
}

/// A hart's controller, and the router over it, never leave the hart.
pub fn stays_local() {
    send::<Hart1Grant>();
    #[cfg(ui_reject)]
    send::<Hart1Controller>(); // ui-error: E0277
    #[cfg(ui_reject)]
    send::<Router<InterruptController, 0, NoBindings>>(); // ui-error: E0277
}
