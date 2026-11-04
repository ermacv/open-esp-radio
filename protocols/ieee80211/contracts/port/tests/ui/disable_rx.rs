#![no_std]
use oer_protocols_ieee80211_contracts_port::{LowerMacTerminal, PortTypes};

pub fn disabled_return<'id, P: PortTypes<'id>>(
    frame: P::RxBuffer,
    meta: P::RxMeta,
    terminal: LowerMacTerminal<'id, P>,
) -> LowerMacTerminal<'id, P> {
    #[cfg(ui_reject)]
    return LowerMacTerminal::Received { frame, meta }; // ui-error: E0599
    #[cfg(not(ui_reject))]
    terminal
}
