#![no_std]
use oer_ieee80211_lower_mac::{AmpduPhase, LowerMacPhase, PhyRate};

pub fn check<'p, 'id: 'p, P, Q>(source: &mut P, destination: &mut Q, raw: PhyRate)
where
    P: LowerMacPhase<'p, 'id>,
    Q: LowerMacPhase<'p, 'id>,
    for<'v> P::Station<'v>: AmpduPhase<'p, 'id>,
    for<'v> Q::Station<'v>: AmpduPhase<'p, 'id>,
{
    let rate = source.rate(raw).unwrap();
    #[cfg(ui_reject)]
    {
        let _ = destination.no_ack(rate); // ui-error: E0308
        let _ = destination.station().ampdu_rate(rate); // ui-error: E0308
    }
    #[cfg(not(ui_reject))]
    {
        let _ = source.no_ack(rate);
        let _ = source.station().ampdu_rate(rate);
    }
}
