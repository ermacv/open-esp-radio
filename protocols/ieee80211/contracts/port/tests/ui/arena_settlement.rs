#![no_std]
use oer_protocols_ieee80211_contracts_port::arena::TxArena;

pub fn settled<'p, 'id: 'p, A: TxArena<'p, 'id>>(port: &mut A) -> bool {
    #[cfg(ui_reject)]
    {
        use oer_protocols_ieee80211_contracts_port::arena::{ArenaId, FrameMask, Settlement};
        let settlement = Settlement { arena: ArenaId::First, frames: 1, keep: FrameMask::EMPTY, retransmission: false }; // ui-error: E0451
        return port.settle(settlement, drop).is_ok();
    }
    #[cfg(not(ui_reject))]
    port.on_air().is_some()
}
