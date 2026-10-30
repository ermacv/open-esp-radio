#![no_std]
use oer_ieee80211_contracts_security::Ccmp128Key;
use oer_ieee80211_port::{InterfacePhase, KeyTypes, LowerMacPhase, Station};
use oer_ieee80211_wire::address::IndividualAddress;
use oer_ieee80211_model::{ModelAssociation, ModelKeys, ModelPhase};
pub fn key<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id, oer_ieee80211_model::ModelCaps, Composition>,
    association: ModelAssociation<'id, Composition, Station>,
    slot: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::PairwiseSlot,
    peer: IndividualAddress,
) {
    #[cfg(ui_reject)]
    let _key = phase
        .station()
        .install_pairwise_key(slot, peer, Ccmp128Key::new(&[1; 16])); // ui-error: E0061
    #[cfg(not(ui_reject))]
    let _key =
        phase
            .station()
            .install_pairwise_key(&association, slot, peer, Ccmp128Key::new(&[1; 16]));
}

pub enum UiSlot {}
type Composition = oer_ieee80211_model::ModelBackend<oer_radio_port::StaticId<UiSlot>>;
