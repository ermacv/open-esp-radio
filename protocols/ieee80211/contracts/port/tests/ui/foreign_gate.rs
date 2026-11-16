#![no_std]
use oer_protocols_ieee80211_contracts_port::GatePool;
use oer_protocols_radio_port::Mint;

pub fn open<'id>(mint: Mint<'id, (GatePool, GatePool)>) {
    let ((first, issuer), (second, _)) = mint.into_pools();
    #[cfg(ui_reject)]
    let open = issuer.open(second); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let open = issuer.open(first);
    let _closed = issuer.close(open);
}
