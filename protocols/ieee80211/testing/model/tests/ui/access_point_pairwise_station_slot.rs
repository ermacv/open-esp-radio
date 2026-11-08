#![no_std]
use oer_protocols_ieee80211_contracts_port::{AccessPoint, AccessPointCommands, KeyTypes};
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_testing_model::{ModelBackend, ModelCommands, ModelKeys, ModelResources};
use oer_protocols_ieee80211_wire::{address::IndividualAddress, security::Spp};

pub enum UiSlot {}
type Id = oer_protocols_radio_port::StaticId<UiSlot>;
type Composition = ModelBackend<Id>;

pub fn install<'id>(commands: &mut ModelCommands<'id, Id>, resources: ModelResources<'id, Composition>, peer: IndividualAddress) {
    let association = ModelKeys::<'_, Composition, AccessPoint>::begin_association(resources.access_point.association, Spp::NotNegotiated);
    #[cfg(not(ui_reject))]
    let [slot, _] = resources.access_point.pairwise;
    #[cfg(ui_reject)]
    let [slot, _] = resources.station.pairwise;
    let _install = commands.install_access_point_pairwise(&association, slot, peer, Ccmp128Key::new(&[1; 16])); // ui-error: E0308
}
