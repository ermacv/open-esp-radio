#![no_std]
use oer_protocols_ieee80211_contracts_port::{TxSlot, TxSlotOf};
use oer_protocols_ieee80211_testing_model::{MODEL_PLACES, MODEL_TX_BODY, MODEL_TX_SLOT, ModelPort, StationModel};
// The body lies on a word: a port's front and the headroom are whole words.
// The model's front of 12 builds; a front of 14 leaves the body at 58.
type Port = ModelPort<'static, StationModel>;
pub const SLOT: TxSlot<Port> = TxSlotOf::<'static, Port, MODEL_TX_BODY, MODEL_TX_SLOT>::SLOT;
#[cfg(ui_reject)]
type Off = ModelPort<'static, StationModel, MODEL_PLACES, 14>;
#[cfg(ui_reject)]
pub const OFF: TxSlot<Off> = TxSlotOf::<'static, Off, MODEL_TX_BODY, MODEL_TX_SLOT>::SLOT; // ui-error@outside: E0080: evaluation panicked: the body lies on a word: the port's front and the headroom are whole words
