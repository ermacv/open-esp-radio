#![no_std]
use oer_protocols_ieee80211_contracts_port::{
    Buffers, ChainOf, First, InterfaceLayout, LongChain, RadioLayout, Second, Station,
};
// A chain reaches its own buffer by its type alone: a long chain by its
// marker in its layout's set, so a marker the set has not reaches nothing;
// and no long chain reaches a queue's chain of one's buffer, there being
// no number to address it by.
type OneLong = RadioLayout<(First,), 16, (InterfaceLayout<Station, 1, 0, 0>,)>;
pub fn reach<'id>(
    first: &LongChain<'id, First, 16>,
    second: &LongChain<'id, Second, 16>,
    buffers: &mut Buffers<u8, (First,)>,
) -> u8 {
    let _ = second;
    #[cfg(ui_reject)]
    let _other = *ChainOf::<'id, OneLong>::buffer(second, buffers); // ui-error: E0277
    *ChainOf::<'id, OneLong>::buffer(first, buffers)
}
