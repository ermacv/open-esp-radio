#![no_std]
use oer_protocols_ieee80211_contracts_port::{HEADROOM, MIC_ROOM, TxSlot, TxSlotOf};
use oer_protocols_ieee80211_testing_model::{MODEL_TX_BEHIND, MODEL_TX_BODY, MODEL_TX_FRONT, ModelPort, StationModel};
// A slot holds the port's octets in front, the headroom, the body, its MIC's
// room and the port's octets behind: one octet less does not build.
type Port = ModelPort<'static, StationModel>;
const EXACT: usize = MODEL_TX_FRONT + HEADROOM + MODEL_TX_BODY + MIC_ROOM + MODEL_TX_BEHIND;
pub const SLOT: TxSlot<Port> = TxSlotOf::<'static, Port, MODEL_TX_BODY, EXACT>::SLOT;
#[cfg(ui_reject)]
pub const SHORT: TxSlot<Port> = TxSlotOf::<'static, Port, MODEL_TX_BODY, { EXACT - 1 }>::SLOT; // ui-error@outside: E0080: evaluation panicked: a slot holds the port's octets, the headroom, the body, its MIC and the port's octets behind
