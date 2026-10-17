#![no_std]
use oer_ieee80211_lower_mac::GatePool;
use oer_radio_port::{Mint, PoolBuilder, PoolPath, PoolSet, Pools};

pub struct Wrapped;
impl PoolSet for Wrapped {
    type Pools<'id, Path: PoolPath> = Pools<'id, (GatePool, (GatePool,)), Path>;
    fn build<'id, Path: PoolPath>(builder: PoolBuilder<'id, Path>) -> Self::Pools<'id, Path> {
        builder.into_pools::<(GatePool, (GatePool,))>()
    }
}

pub fn close<'id>(mint: Mint<'id, Wrapped>) {
    let ((first, mut issuer), ((nested, _),)) = mint.into_pools();
    #[cfg(ui_reject)]
    let closed = issuer.close(nested); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let closed = issuer.close(first);
    let _open = issuer.open(closed);
}
