#![no_std]
use oer_ieee80211_lower_mac::GatePool;
use oer_radio_port::Mint;

pub fn close<'id>(mint: Mint<'id, (GatePool, GatePool)>) {
    let ((first, mut issuer), (second, _)) = mint.into_pools();
    #[cfg(ui_reject)]
    let closed = issuer.close(second); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let closed = issuer.close(first);
    let _open = issuer.open(closed);
}
