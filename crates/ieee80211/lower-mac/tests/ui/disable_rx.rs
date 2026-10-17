#![no_std]
use oer_ieee80211_lower_mac::{LowerMacTerminal, PhaseTypes};

pub fn disabled_return<'id, P: PhaseTypes<'id>>(
    frame: P::RxBuffer,
    meta: P::RxMeta,
    terminal: LowerMacTerminal<'id, P>,
) -> LowerMacTerminal<'id, P> {
    #[cfg(ui_reject)]
    return LowerMacTerminal::Received { frame, meta }; // ui-error: E0599
    #[cfg(not(ui_reject))]
    terminal
}
