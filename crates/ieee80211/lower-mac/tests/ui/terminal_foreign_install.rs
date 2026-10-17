#![no_std]
use oer_ieee80211_lower_mac::{Completed, TerminalAccess, TerminalParts};
use oer_radio_port::Slot;
pub fn unpack<'id, 'foreign, C, K, M, O>(
    terminal: Completed<C, Slot<'id, K>, M, O>,
    own: &mut TerminalAccess<'id, K>,
    foreign: &mut TerminalAccess<'foreign, K>,
) -> TerminalParts<C, Slot<'id, K>, M, O> {
    #[cfg(ui_reject)]
    return terminal.into_parts(foreign); // ui-error: null lifetime may not live long enough // ui-error: null lifetime may not live long enough
    #[cfg(not(ui_reject))]
    terminal.into_parts(own)
}
