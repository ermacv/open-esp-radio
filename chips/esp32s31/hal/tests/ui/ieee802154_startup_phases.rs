#![no_std]
use oer_esp32s31_hal::ieee802154::{
    mac::{Ieee802154ArmedTaskOwner, Ieee802154InterruptOwner,
        Ieee802154InterruptSetupOwner, Ieee802154StartupPort, Ieee802154TaskOwner},
    polled::Ieee802154MacOwners,
};
use oer_espressif_ieee802154_mac_values::{Ieee802154EventMask, Ieee802154InterruptStartup};
use oer_espressif_interrupt_table::{Bound, Controller, Entry, Router, Silent};

pub fn setup_is_not_bindable<const H: u8, C: Controller, I: Entry<Part = Ieee802154InterruptOwner>>(
    router: &mut Router<C, H>, token: I, setup: Ieee802154InterruptSetupOwner,
) {
    #[cfg(ui_reject)]
    let _ = router.bind(token, setup); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _ = (router, token, setup);
}

pub fn masked_is_not_discarded(setup: Ieee802154InterruptSetupOwner, task: Ieee802154TaskOwner) {
    let masked = Ieee802154StartupPort::mask_events(setup, task);
    #[cfg(ui_reject)]
    let _ = Ieee802154StartupPort::finish(masked); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _ = masked;
}

pub fn no_early_parts(setup: Ieee802154InterruptSetupOwner, task: Ieee802154TaskOwner) {
    let masked = Ieee802154StartupPort::mask_events(setup, task);
    #[cfg(ui_reject)]
    let _ = masked.into_parts(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = masked;
}

pub struct OtherIrq;
impl oer_mmio::InterruptSource for OtherIrq { const SOURCE: u16 = 3; }

pub fn foreign_bound<I: Entry<Part = OtherIrq>>(
    startup: Ieee802154ArmedTaskOwner, bound: &Bound<I>,
) {
    #[cfg(ui_reject)]
    let _ = startup.activate(bound, Ieee802154EventMask::HANDLED_BASELINE_NO_TIMER0); // ui-error: E0271
    #[cfg(not(ui_reject))]
    let _ = (startup, bound, Ieee802154EventMask::NONE);
}

pub fn foreign_silent(startup: Ieee802154ArmedTaskOwner, irq: Ieee802154InterruptOwner, silent: Silent<OtherIrq>) {
    #[cfg(ui_reject)]
    let _ = Ieee802154MacOwners::new(startup, irq, silent); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _ = (startup, irq, silent);
}

pub fn valid_poll(startup: Ieee802154ArmedTaskOwner, irq: Ieee802154InterruptOwner, silent: Silent<Ieee802154InterruptOwner>) {
    let _ = Ieee802154MacOwners::new(startup, irq, silent);
}

pub fn no_poll_without_silent(startup: Ieee802154ArmedTaskOwner, irq: Ieee802154InterruptOwner) {
    #[cfg(ui_reject)]
    let _ = Ieee802154MacOwners::new(startup, irq); // ui-error: E0061
    #[cfg(not(ui_reject))]
    let _ = (startup, irq);
}
