#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    AggregatePpdu, AmpduRate, DataPort, FreeQueue, GateOpen, Layout, LongIn, LongOnAir, LongPpdu,
    OpenLong, PortRate, Protection, RetryPlan,
};
// An aggregate's rate is checked for one installation: another
// installation's goes to none of this port's publications.
pub fn publish<'p, 'a, 'b, P: DataPort<'a>, M: LongIn<<P::Layout as Layout>::Longs>, const S: usize>(
    port: &mut P,
    open: OpenLong<'p, 'a, M, S>,
    queue: FreeQueue<'_, 'a, ()>,
    own: AmpduRate<'a, P::Caps>,
    foreign: AmpduRate<'b, P::Caps>,
    alone: RetryPlan<PortRate<'a, P::Caps>>,
    spacing: oer_protocols_ieee80211_wire::ht::MinimumMpduStartSpacing,
    gate: &GateOpen<'a>,
    now: Instant,
) -> LongOnAir<'p, 'a, M, S> {
    let _ = (&own, &foreign);
    #[cfg(ui_reject)]
    let rate = foreign; // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    let rate = own;
    let how = LongPpdu::Aggregate(AggregatePpdu { rate, spacing, protection: Protection::None, alone }); // ui-error: lifetime may not live long enough
    open.publish(queue, port, how, gate, now)
}
