#![no_std]
use oer_protocols_ieee80211_contracts_port::{KeyTypes, PortCommands, Station};
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_testing_model::{ModelCommands, ModelKeys, ModelResources};
use oer_protocols_ieee80211_wire::address::IndividualAddress;
use oer_protocols_ieee80211_wire::security::Spp;

pub fn keys<'id>(
    commands: &mut ModelCommands<'id, Id>,
    resources: ModelResources<'id, Composition>,
    peer: IndividualAddress,
) {
    let [first, second] = resources.station.pairwise;
    let start = resources.station.association;
    let association = ModelKeys::<'_, Composition, Station>::begin_association(start, Spp::Negotiated);
    let _first = commands.install_pairwise(&association, first, peer, Ccmp128Key::new(&[1; 16]));
    #[cfg(ui_reject)]
    let second_association = ModelKeys::<'_, Composition, Station>::begin_association(start, Spp::NotNegotiated); // ui-error: E0382
    #[cfg(not(ui_reject))]
    let second_association = association;
    let _second =
        commands.install_pairwise(&second_association, second, peer, Ccmp128Key::new(&[2; 16]));
}

pub enum UiSlot {}
type Id = oer_protocols_radio_port::StaticId<UiSlot>;
type Composition = oer_protocols_ieee80211_testing_model::ModelBackend<Id>;
