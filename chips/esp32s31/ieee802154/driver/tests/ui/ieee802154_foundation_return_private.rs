#![no_std]
use oer_esp32s31_hal::ieee802154::Ieee802154Clocked;
use oer_espressif_ieee802154_engine::lifecycle::Ieee802154FoundationReturn;

pub fn a_return_phase_cannot_be_forged() {
    #[cfg(ui_reject)]
    let _ = Ieee802154FoundationReturn::<Ieee802154Clocked> { _backend: core::marker::PhantomData }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let _: Option<Ieee802154FoundationReturn<Ieee802154Clocked>> = None;
}
