#![no_std]
use oer_protocols_ieee80211_core_datapath::engine::{
    Arenas, DataPlace, Engine, NamePlace, Peer, PeerPlace,
};
use oer_protocols_ieee80211_core_datapath::numbers::TxKey;
use oer_protocols_ieee80211_testing_model::{MODEL_AGREEMENTS, MODEL_GROUPS, ModelPort, StationModel};

pub type Station<'id, T> =
    Engine<'id, ModelPort<'id, StationModel<8>>, Arenas, T, 1, 8, 8, MODEL_GROUPS, MODEL_AGREEMENTS, 7>;

// A transmit agreement begins on the TID whose named number was told: a
// linked peer, a TID not named and a TID whose number is not told yet offer
// none.
pub fn agreement<T: TxKey>(engine: &mut Station<'_, T>) {
    if let DataPlace::Flowing(mut data) = engine.data()
        && let PeerPlace::Linked(mut peer) = data.peer(Peer::new(0).unwrap())
    {
        #[cfg(ui_reject)]
        peer.tx_agreement(); // ui-error: E0599
        match peer.naming() {
            NamePlace::Unnamed(unnamed) => {
                let _ = &unnamed;
                #[cfg(ui_reject)]
                unnamed.tx_agreement(); // ui-error: E0599
            }
            NamePlace::Draining(draining) => {
                let _ = &draining;
                #[cfg(ui_reject)]
                draining.tx_agreement(); // ui-error: E0599
            }
            NamePlace::Named(_) => {}
        }
    }
}
