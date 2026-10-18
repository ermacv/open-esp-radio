#![no_std]
use oer_esp32s31_ieee80211_form::scope::commands::{ClosingGate, GateBarrier, GatePath, PhaseReceiver};
use oer_ieee80211_lower_mac::{GateOpen, Ieee80211Capabilities};
#[cfg(ui_reject)]
type Receipt<'id> = GateOpen<'id, GatePath>;
#[cfg(not(ui_reject))]
type Receipt<'id> = ClosingGate<'id>;
pub fn reply<'id, Caps: Ieee80211Capabilities>(
    receiver: &mut PhaseReceiver<'_, 'id, Caps, (), (), (), (), 4, 9>,
    receipt: Receipt<'id>,
) -> GateBarrier<'id> {
    receiver.gate_barrier(receipt) // ui-error: E0308
}
