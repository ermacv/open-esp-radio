#![no_std]
use oer_protocols_ieee80211_contracts_port::{KeyTypes, PortCommands, Station};
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_testing_model::{ModelAssociation, ModelCommands, ModelKeys};
use oer_protocols_ieee80211_wire::address::IndividualAddress;

pub fn key<'id>(
    commands: &mut ModelCommands<'id, Id>,
    association: ModelAssociation<'id, Composition, Station>,
    slot: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::PairwiseSlot,
    peer: IndividualAddress,
) {
    #[cfg(ui_reject)]
    let _key = commands.install_pairwise(slot, peer, Ccmp128Key::new(&[1; 16])); // ui-error: E0061
    #[cfg(not(ui_reject))]
    let _key = commands.install_pairwise(&association, slot, peer, Ccmp128Key::new(&[1; 16]));
}

pub enum UiSlot {}
type Id = oer_protocols_radio_port::StaticId<UiSlot>;
type Composition = oer_protocols_ieee80211_testing_model::ModelBackend<Id>;
