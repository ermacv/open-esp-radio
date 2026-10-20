#![no_std]
use oer_radio_permit_pool::FifoPermitPool;
use oer_radio_port::{Mint, PermitPool, Permits};

pub enum Kind {}

pub async fn release<'id>(mint: Mint<'id, (Permits<Kind, 1, 1>, Permits<Kind, 1, 1>)>) {
    let (first, second) = mint.into_pools();
    let [mut seat] = second.seats;
    let a = FifoPermitPool::new(first.supply, first.line);
    let b = FifoPermitPool::new(second.supply, second.line);
    let permit = b.take(&mut seat).await;
    #[cfg(ui_reject)]
    a.release(permit); // ui-error: E0308
    #[cfg(not(ui_reject))]
    b.release(permit);
}
