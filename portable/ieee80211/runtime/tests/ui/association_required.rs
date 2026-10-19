#![no_std]
use oer_ieee80211_mac::ccmp::Ccmp128Key;
use oer_ieee80211_lower_mac::{InterfacePhase, KeyTypes, LowerMacPhase, Station};
use oer_ieee80211_mac::address::IndividualAddress;
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelAssociation, ModelKeys, ModelPhase};
pub fn key<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    association: ModelAssociation<'id, Station>,
    slot: <ModelKeys<'id, Station> as KeyTypes<'id>>::PairwiseSlot,
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
