#![no_std]
use oer_esp32s31_hal::ieee802154::mac::Ieee802154InterruptOwner;
use oer_espressif_ieee802154_mac_values::{Ieee802154EventMask, Ieee802154EventObservation};

pub fn acknowledge_once(owner: &mut Ieee802154InterruptOwner) {
    let snapshot = owner.events();
    let _observed = snapshot.observation();
    owner.clear_events(snapshot, Ieee802154EventMask::NAMED);
    #[cfg(ui_reject)]
    owner.clear_events(snapshot, Ieee802154EventMask::NAMED); // ui-error: E0382
}

pub fn observation_grants_no_write(owner: &mut Ieee802154InterruptOwner) {
    let observation = Ieee802154EventObservation::from_named(Ieee802154EventMask::NAMED);
    #[cfg(ui_reject)]
    owner.clear_events(observation, Ieee802154EventMask::NAMED); // ui-error: E0308
    let _ = (owner, observation);
}
