#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    CloseGate, CommandPlace, DataPort, GateOpen, InCommand,
};
// The close-gate command takes this installation's open gate: another
// installation's is no command of this port.
pub fn close<'a, 'b, P: DataPort<'a>>(
    port: &mut P,
    place: CommandPlace<'a>,
    own: GateOpen<'a>,
    foreign: GateOpen<'b>,
    now: Instant,
) -> InCommand<'a, CloseGate<'a>> {
    let _ = (&own, &foreign);
    #[cfg(ui_reject)]
    let gate = foreign; // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    let gate = own;
    port.begin_close(place, CloseGate(gate), now) // ui-error: lifetime may not live long enough
}
