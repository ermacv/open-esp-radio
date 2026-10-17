#![no_std]
use oer_ieee80211_mac::ccmp::Ccmp128Key;
use oer_ieee80211_lower_mac::{InterfacePhase, KeyTypes, LowerMacPhase, Station};
use oer_ieee80211_mac::address::IndividualAddress;
use oer_ieee80211_mac::security::Spp;
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelKeys, ModelPhase, ModelResources};
pub fn keys<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    resources: ModelResources<'id>,
    peer: IndividualAddress,
) {
    let [first, second] = resources.station_pairwise;
    let start = resources.station_association;
    let association = ModelKeys::<Station>::begin_association(start, Spp::Negotiated);
    let _first =
        phase
            .station()
            .install_pairwise_key(&association, first, peer, Ccmp128Key::new(&[1; 16]));
    #[cfg(ui_reject)]
    let second_association = ModelKeys::<Station>::begin_association(start, Spp::NotNegotiated); // ui-error: E0382
    #[cfg(not(ui_reject))]
    let second_association = association;
    let _second = phase.station().install_pairwise_key(
        &second_association,
        second,
        peer,
        Ccmp128Key::new(&[2; 16]),
    );
}
