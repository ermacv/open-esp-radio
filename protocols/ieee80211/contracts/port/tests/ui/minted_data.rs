#![no_std]
use oer_protocols_ieee80211_contracts_port::{
    InterfaceLayout, Layout, LayoutRights, MintSeal, Station,
};
type Alone = (InterfaceLayout<Station, 8, 1, 2, 4>,);
// A layout's rights are made only by an installation's mint: the seal its
// `mint` takes is made by the contract alone.
pub fn data<'id>(rights: LayoutRights<'id, Alone>) -> LayoutRights<'id, Alone> {
    #[cfg(ui_reject)]
    let rights = <Alone as Layout>::mint::<'id>(MintSeal(())); // ui-error: E0423
    rights
}
