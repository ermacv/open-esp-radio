#![no_std]
use oer_radio_permit_pool::FifoPermitPool;
use oer_radio_port::{Mint, Permits};

pub enum Kind {}

pub fn build<'id>(mint: Mint<'id, Permits<Kind, 2, 1>>) {
    let tokens = mint.into_pools();
    #[cfg(ui_reject)]
    {
        let [first, second] = tokens.supply.into_permits();
        let a = FifoPermitPool::new([first], tokens.line); // ui-error: E0308
        drop((a, second));
    }
    #[cfg(not(ui_reject))]
    let _pool = FifoPermitPool::new(tokens.supply, tokens.line);
}
