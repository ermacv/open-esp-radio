#![no_std]
use oer_memory::{PinnedDmaTxPool, PinnedDmaTxRadioLease};

type Pool = PinnedDmaTxPool<32, 8, 4, 1>;

pub fn claim(storage: &'static mut Pool) -> PinnedDmaTxRadioLease<'static, 32, 8, 4> {
    #[cfg(not(ui_reject))]
    let pool = {
        let pool = Pool::pin_static(storage);
        pool.claim_network(0).publish(0, |_| ());
        pool
    };
    #[cfg(ui_reject)]
    let pool = core::pin::Pin::static_mut(storage);
    pool.claim_radio(0) // ui-error: E0599
}
