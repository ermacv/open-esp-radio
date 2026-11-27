#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    DataPort, FreeQueue, GateOpen, Layout, LongIn, LongOnAir, LongPpdu, OpenLong, PortRate,
    Protection, RetryPlan, RetryStage,
};
// A rate is checked for one installation: another installation's goes to
// none of this port's publications.
pub fn publish<'p, 'a, 'b, P: DataPort<'a>, M: LongIn<<P::Layout as Layout>::Longs>, const S: usize>(
    port: &mut P,
    open: OpenLong<'p, 'a, M, S>,
    queue: FreeQueue<'_, 'a, ()>,
    own: PortRate<'a, P::Caps>,
    foreign: PortRate<'b, P::Caps>,
    gate: &GateOpen<'a>,
    now: Instant,
) -> LongOnAir<'p, 'a, M, S> {
    let _ = (&own, &foreign);
    #[cfg(ui_reject)]
    let rate = foreign; // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    let rate = own;
    let plan = RetryPlan::new(RetryStage {
        rate,
        transmissions: core::num::NonZeroU8::MIN,
        protection: Protection::None,
    });
    open.publish(queue, port, LongPpdu::Single(plan), gate, now) // ui-error: lifetime may not live long enough
}
