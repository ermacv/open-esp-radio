#![no_std]
use oer_protocols_ieee80211_contracts_port::{PortKey, Running, Starting};
pub fn run<'id>(key: &PortKey<'id>, starting: Starting<'id>) -> Running<'id> {
    #[cfg(ui_reject)]
    let running = Running { brand: core::marker::PhantomData }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let running = key.started(starting);
    running
}
