#![no_std]
use oer_ieee80211_contracts_security::Ccmp128Key;
use oer_ieee80211_contracts_port::{InterfacePhase, KeyTypes, LowerMacPhase, Station};
use oer_ieee80211_wire::address::IndividualAddress;
use oer_ieee80211_wire::security::Spp;
use oer_ieee80211_testing_model::{ModelKeys, ModelPhase, ModelResources};
pub fn keys<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id, oer_ieee80211_testing_model::ModelCaps, Composition>,
    resources: ModelResources<'id, Composition>,
    peer: IndividualAddress,
) {
    let [first, second] = resources.station_pairwise;
    let start = resources.station_association;
    let association = ModelKeys::<'_, Composition, Station>::begin_association(start, Spp::Negotiated);
    let _first =
        phase
            .station()
            .install_pairwise_key(&association, first, peer, Ccmp128Key::new(&[1; 16]));
    #[cfg(ui_reject)]
    let second_association = ModelKeys::<'_, Composition, Station>::begin_association(start, Spp::NotNegotiated); // ui-error: E0382
    #[cfg(not(ui_reject))]
    let second_association = association;
    let _second = phase.station().install_pairwise_key(
        &second_association,
        second,
        peer,
        Ccmp128Key::new(&[2; 16]),
    );
}

pub enum UiSlot {}
type Composition = oer_ieee80211_testing_model::ModelBackend<oer_radio_port::StaticId<UiSlot>>;
