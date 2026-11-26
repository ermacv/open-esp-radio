#![no_std]
use oer_protocols_ieee80211_core_datapath::engine::{
    Arenas, DataPlace, Engine, NamePlace, Peer, PeerPlace, Singles,
};
use oer_protocols_ieee80211_core_datapath::numbers::TxKey;
use oer_protocols_ieee80211_testing_model::{MODEL_CHAIN_SLOTS, MODEL_AGREEMENTS, MODEL_GROUPS, AccessPointModel, ModelPort, StationModel};

pub type Station<'id, T> =
    Engine<'id, ModelPort<'id, StationModel<8>>, Arenas, T, 1, 8, 8, MODEL_GROUPS, MODEL_AGREEMENTS, MODEL_CHAIN_SLOTS, 7>;
pub type AccessPoint<'id, T> = Engine<
    'id,
    ModelPort<'id, AccessPointModel<17, 2>>,
    Singles,
    T,
    2,
    8,
    17,
    MODEL_GROUPS,
    MODEL_AGREEMENTS,
    MODEL_CHAIN_SLOTS,
    7,
>;

// An access point's data goes as singles: its named TID offers no transmit
// agreement.
pub fn agreement<T: TxKey>(engine: &mut AccessPoint<'_, T>) {
    if let DataPlace::Flowing(mut data) = engine.data()
        && let PeerPlace::Linked(mut peer) = data.peer(Peer::new(0).unwrap())
        && let NamePlace::Named(named) = peer.naming()
    {
        let _ = &named;
        #[cfg(ui_reject)]
        named.tx_agreement(); // ui-error: E0599
    }
}
