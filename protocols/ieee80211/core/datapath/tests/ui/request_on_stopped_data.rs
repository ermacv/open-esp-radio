#![no_std]
use oer_protocols_ieee80211_core_datapath::engine::{
    ACCESS_POINT, Arenas, DATA_SINGLES, DataPlace, Engine, Singles,
};
use oer_protocols_ieee80211_core_datapath::numbers::TxKey;
use oer_protocols_ieee80211_testing_model::{MODEL_AGREEMENTS, MODEL_GROUPS, ModelPort};

pub type Station<'id, T> =
    Engine<'id, ModelPort<'id, 8, 0>, Arenas, T, 1, 8, 8, MODEL_GROUPS, MODEL_AGREEMENTS>;
pub type AccessPoint<'id, T> = Engine<
    'id,
    ModelPort<'id, 17, DATA_SINGLES>,
    Singles,
    T,
    2,
    8,
    17,
    MODEL_GROUPS,
    MODEL_AGREEMENTS,
>;

// A stopped engine's data offers no peer: a request needs the flowing data.
pub fn stopped<T: TxKey>(engine: &mut Station<'_, T>) {
    if let DataPlace::Stopped(data) = engine.data() {
        let _ = &data;
        #[cfg(ui_reject)]
        let _ = data.peer(ACCESS_POINT); // ui-error: E0599
    }
}
