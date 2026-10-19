#![no_std]
use oer_radio_permit_pool::FifoPermitPool;
use oer_radio_port::{Mint, PermitPool, Permits};

pub enum Kind {}

pub async fn take<'id>(mint: Mint<'id, (Permits<Kind, 1, 1>, Permits<Kind, 1, 2>)>) {
    let (first, second) = mint.into_pools();
    let [mut seat] = first.seats;
    let [_, mut foreign] = second.seats;
    let pool = FifoPermitPool::new(first.supply, first.line);
    #[cfg(ui_reject)]
    let permit = pool.take(&mut foreign).await; // ui-error: E0308
    #[cfg(not(ui_reject))]
    let permit = pool.take(&mut seat).await;
    pool.release(permit);
}
