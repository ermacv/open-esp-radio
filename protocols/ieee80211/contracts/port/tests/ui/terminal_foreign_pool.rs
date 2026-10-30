#![no_std]
use oer_ieee80211_contracts_port::{Completed, TerminalAccess, TerminalParts};
use oer_radio_port::{Child, Root, Slot};
pub fn unpack<'id, C, K, M, O>(
    terminal: Completed<C, Slot<'id, K, Child<Root<oer_radio_port::LocalId<'id>>, 0>>, M, O>,
    own: &mut TerminalAccess<'id, K, Child<Root<oer_radio_port::LocalId<'id>>, 0>>,
    foreign: &mut TerminalAccess<'id, K, Child<Root<oer_radio_port::LocalId<'id>>, 1>>,
) -> TerminalParts<C, Slot<'id, K, Child<Root<oer_radio_port::LocalId<'id>>, 0>>, M, O> {
    #[cfg(ui_reject)]
    return terminal.into_parts(foreign); // ui-error: E0308
    #[cfg(not(ui_reject))]
    terminal.into_parts(own)
}
