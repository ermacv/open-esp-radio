#![no_std]
use oer_esp32s31_hal::ieee802154::Ieee802154Clocked;
use oer_espressif_ieee802154_engine::lifecycle::{Ieee802154FoundationReturn, Ieee802154Lifecycle};
use oer_espressif_ieee802154_mac_values::{Ieee802154ResetPort, Ieee802154ResetReadback};

pub struct ForeignReset;
impl Ieee802154ResetPort for ForeignReset {
    fn set_ieee802154_mac_reset(&mut self, _: bool) {}
    fn set_ieee802154_apb_reset(&mut self, _: bool) {}
    fn ieee802154_reset_readback(&self) -> Ieee802154ResetReadback {
        Ieee802154ResetReadback { mac_reset_released: true, apb_reset_released: true }
    }
}

pub fn a_foreign_reset_cannot_prove_the_hal_phase(clocked: Ieee802154Clocked, mut foreign: ForeignReset) {
    #[cfg(ui_reject)]
    let _ = Ieee802154Lifecycle::clocked(clocked).reset_mac(&mut foreign); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _ = (clocked, &mut foreign);
}

pub fn resume_requires_the_prior_phase(clocked: Ieee802154Clocked) {
    #[cfg(ui_reject)]
    let _ = Ieee802154Lifecycle::resume(clocked); // ui-error: E0061
    #[cfg(not(ui_reject))]
    let _ = clocked;
}

pub fn a_return_phase_is_consumed_once(clocked: Ieee802154Clocked, phase: Ieee802154FoundationReturn<Ieee802154Clocked>) {
    let _ = Ieee802154Lifecycle::resume(clocked, phase);
    #[cfg(ui_reject)]
    let _again = phase; // ui-error: E0382
}
