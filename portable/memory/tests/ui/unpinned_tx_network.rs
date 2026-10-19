#![no_std]
use oer_memory::{PinnedDmaTxNetworkLease, PinnedDmaTxPool};

type Pool = PinnedDmaTxPool<32, 8, 4, 1>;

pub fn claim(storage: &'static mut Pool) -> PinnedDmaTxNetworkLease<'static, 32, 8, 4> {
    #[cfg(not(ui_reject))]
    let pool = Pool::pin_static(storage);
    #[cfg(ui_reject)]
    let pool = Pool::new();
    pool.claim_network(0) // ui-error: E0599
}
