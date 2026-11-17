#![no_std]
use oer_protocols_ieee80211_core_datapath::engine::{
    ACCESS_POINT, Arenas, DataPlace, Engine, KeyChange, KeyPlace, PeerPlace,
};
use oer_protocols_ieee80211_core_datapath::numbers::TxKey;
use oer_protocols_ieee80211_testing_model::{MODEL_AGREEMENTS, MODEL_GROUPS, ModelPort};

pub type Station<'id, T> =
    Engine<'id, ModelPort<'id, 8, 0>, Arenas, T, 1, 8, 8, MODEL_GROUPS, MODEL_AGREEMENTS>;

// A key's place offers its change only while no command runs: not while a
// key changes, nor while the gate closes.
pub fn changing<T: TxKey>(engine: &mut Station<'_, T>, change: KeyChange<T>) {
    if let DataPlace::Flowing(mut data) = engine.data()
        && let PeerPlace::Linked(mut peer) = data.peer(ACCESS_POINT)
    {
        match peer.key() {
            KeyPlace::Changing(key) => {
                let _ = (&key, &change);
                #[cfg(ui_reject)]
                key.change(change); // ui-error: E0599
            }
            KeyPlace::Busy(key) => {
                let _ = &key;
                #[cfg(ui_reject)]
                key.change(change); // ui-error: E0599
            }
            KeyPlace::Idle(_) => {}
        }
    }
}
