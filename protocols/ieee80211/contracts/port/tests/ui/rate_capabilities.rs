#![no_std]
use oer_protocols_ieee80211_contracts_port::{DataPort, PhyRate, PortArena};

pub fn check<'id, P, Q>(source: &mut P, destination: &mut Q, raw: PhyRate)
where
    P: DataPort<'id>,
    Q: DataPort<'id>,
{
    let rate = source.rate(raw).unwrap().get();
    let arena = source.arena();
    let rate = arena.rate(rate).unwrap();
    #[cfg(ui_reject)]
    {
        let _ = destination.arena().ampdu_rate(rate); // ui-error: E0308
    }
    #[cfg(not(ui_reject))]
    {
        let _ = destination;
        let _ = arena.ampdu_rate(rate);
    }
}
