#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{DataPort, PhyRate, PortRate};

// A rate checked in one turn does not outlive the port's borrow: the
// next mutable call of the port moves it on.
pub fn turn<'id, P: DataPort<'id>>(port: &mut P, raw: PhyRate, now: Instant) -> Option<PhyRate> {
    let rate = port.rate(raw)?;
    #[cfg(ui_reject)]
    let _ = port.begin_receive(1, now); // ui-error: E0502
    #[cfg(not(ui_reject))]
    let _ = now;
    Some(rate.get())
}

// Nor does it leave the borrow for good.
#[cfg(ui_reject)]
pub fn keep<'id, P: DataPort<'id>>(port: &P, raw: PhyRate) -> Option<PortRate<'static, P::Caps>> {
    port.rate(raw) // ui-error: lifetime may not live long enough
}
#[cfg(not(ui_reject))]
pub fn keep<'a, 'id, P: DataPort<'id>>(port: &'a P, raw: PhyRate) -> Option<PortRate<'a, P::Caps>> {
    port.rate(raw)
}
