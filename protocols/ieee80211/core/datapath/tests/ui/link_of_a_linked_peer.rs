#![no_std]
use oer_protocols_ieee80211_core_datapath::engine::{
    ACCESS_POINT, Arenas, DataPlace, Engine, PeerPlace, Singles,
};
use oer_protocols_ieee80211_core_datapath::numbers::TxKey;
use oer_protocols_ieee80211_testing_model::{MODEL_AGREEMENTS, MODEL_GROUPS, AccessPointModel, ModelPort, StationModel};

pub type Station<'id, T> =
    Engine<'id, ModelPort<'id, StationModel<8>>, Arenas, T, 1, 8, 8, MODEL_GROUPS, MODEL_AGREEMENTS, 7>;
pub type AccessPoint<'id, T> = Engine<
    'id,
    ModelPort<'id, AccessPointModel<17>>,
    Singles,
    T,
    2,
    8,
    17,
    MODEL_GROUPS,
    MODEL_AGREEMENTS,
    7,
>;

// A linked peer offers no link: its free place does.
pub fn linked<T: TxKey>(engine: &mut Station<'_, T>) {
    if let DataPlace::Flowing(mut data) = engine.data()
        && let PeerPlace::Linked(peer) = data.peer(ACCESS_POINT)
    {
        let _ = &peer;
        #[cfg(ui_reject)]
        peer.link(); // ui-error: E0599
    }
}
