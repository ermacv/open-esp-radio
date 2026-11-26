#![no_std]
use oer_protocols_ieee80211_core_datapath::engine::{Arenas, DataPlace, Engine, GatePlace};
use oer_protocols_ieee80211_core_datapath::numbers::TxKey;
use oer_protocols_ieee80211_testing_model::{MODEL_CHAIN_SLOTS, MODEL_AGREEMENTS, MODEL_GROUPS, ModelPort, StationModel};

pub type Station<'id, T> =
    Engine<'id, ModelPort<'id, StationModel<8>>, Arenas, T, 1, 8, 8, MODEL_GROUPS, MODEL_AGREEMENTS, MODEL_CHAIN_SLOTS, 7>;

// An open gate offers no closing while a receive agreement's command runs:
// the controller asks again after the agreement's end.
pub fn close<T: TxKey>(engine: &mut Station<'_, T>) {
    if let DataPlace::Flowing(mut data) = engine.data()
        && let GatePlace::OpenAgreeing(gate) = data.gate()
    {
        let _ = &gate;
        #[cfg(ui_reject)]
        gate.close(); // ui-error: E0599
    }
}
