#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    Arena, DataPort, GateOpen, OnAir, PortRate, Ppdu, Protection, RetryPlan, RetryStage,
};
// A rate is checked for one installation: another installation's goes to
// none of this port's publications.
pub fn publish<'a, 'b, P: DataPort<'a>>(
    port: &mut P,
    arena: Arena<'a>,
    own: PortRate<'a, P::Caps>,
    foreign: PortRate<'b, P::Caps>,
    gate: &GateOpen<'a>,
    now: Instant,
) -> OnAir<'a> {
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
    port.publish(arena, Ppdu::Single(plan), gate, now) // ui-error: lifetime may not live long enough
}
