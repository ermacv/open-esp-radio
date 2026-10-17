#![no_std]
use oer_ieee80211_lower_mac::{Completed, TerminalAccess, TerminalParts};
use oer_radio_port::Slot;
pub fn unpack<'id, C, K, M, O>(
    terminal: Completed<C, Slot<'id, K>, M, O>,
    access: &mut TerminalAccess<'id, K>,
) -> TerminalParts<C, Slot<'id, K>, M, O> {
    #[cfg(ui_reject)]
    let access = &mut TerminalAccess { proof: core::marker::PhantomData }; // ui-error: E0451
    terminal.into_parts(access)
}
