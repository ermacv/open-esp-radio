#![no_std]
use oer_protocols_ieee80211_contracts_port::{TxSlot, TxSlotOf};
use oer_protocols_ieee80211_testing_model::ModelPort;
// The longest MPDU of a slot's body is one the port transmits: the engine's
// longest front (42) and the body, against the model's 2 304, which counts
// the MPDU without its MIC and FCS.
type Port = ModelPort<'static, 0, 0>;
pub const LONGEST: TxSlot<Port> = TxSlotOf::<'static, Port, 2_262, 4_096>::SLOT;
#[cfg(ui_reject)]
pub const LONGER: TxSlot<Port> = TxSlotOf::<'static, Port, 2_263, 4_096>::SLOT; // ui-error@outside: E0080: evaluation panicked: the longest MPDU of the body is one the port transmits
