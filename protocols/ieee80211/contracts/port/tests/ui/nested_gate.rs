#![no_std]
use oer_protocols_ieee80211_contracts_port::GatePool;
use oer_protocols_radio_port::{Mint, PoolBuilder, PoolPath, PoolSet, Pools};

pub struct Wrapped;
impl PoolSet for Wrapped {
    type Pools<'id, Path: PoolPath> = Pools<'id, (GatePool, (GatePool,)), Path>;
    fn build<'id, Path: PoolPath>(builder: PoolBuilder<'id, Path>) -> Self::Pools<'id, Path> {
        builder.into_pools::<(GatePool, (GatePool,))>()
    }
}

pub fn open<'id>(mint: Mint<'id, Wrapped>) {
    let ((first, issuer), ((nested, _),)) = mint.into_pools();
    #[cfg(ui_reject)]
    let open = issuer.open(nested); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let open = issuer.open(first);
    let _closed = issuer.close(open);
}
