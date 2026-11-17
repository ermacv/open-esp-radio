#![no_std]
use oer_protocols_ieee80211_contracts_port::{DataLayout, DataRights, MintData, MintSeal};
// The data's rights are made only by an installation's mint: the seal its
// `mint` takes is made by the contract alone.
pub fn data<'id>(rights: DataRights<'id, 8, 2, 4, 0>) -> DataRights<'id, 8, 2, 4, 0> {
    #[cfg(ui_reject)]
    let rights = <DataLayout<8, 2, 4, 0> as MintData>::mint::<'id>(MintSeal(())); // ui-error: E0423
    rights
}
