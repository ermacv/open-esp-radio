#![no_std]
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    AmpduRate, Arena, DataPort, GateOpen, MpduStartSpacing, OnAir, PortRate, Ppdu, Protection,
    RetryPlan,
};
// An aggregate's rate is checked for one installation: another
// installation's goes to none of this port's publications.
pub fn publish<'a, 'b, P: DataPort<'a>>(
    port: &mut P,
    arena: Arena<'a>,
    own: AmpduRate<'a, P::Caps>,
    foreign: AmpduRate<'b, P::Caps>,
    alone: RetryPlan<PortRate<'a, P::Caps>>,
    spacing: MpduStartSpacing,
    gate: &GateOpen<'a>,
    now: Instant,
) -> OnAir<'a> {
    let _ = (&own, &foreign);
    #[cfg(ui_reject)]
    let rate = foreign; // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    let rate = own;
    let how = Ppdu::Aggregate { rate, spacing, protection: Protection::None, alone }; // ui-error: lifetime may not live long enough
    port.publish(arena, how, gate, now)
}
