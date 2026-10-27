//! The coexistence entries of the vendor comparison, over the session's
//! radio domain: each lends the domain to the production core or timer
//! sequence and compares the registers it leaves.

use oer_esp32s31_hal::coex::{CoexPolicyTimer, CoexTimerBank};
use oer_esp32s31_radio_coex::{
    CoexClockSelector, CoexCore, CoexDomainPorts, CoexError, CoexEventId,
};

use crate::session;

fn with_timer(index: u32, operation: impl FnOnce(&mut CoexTimerBank<'_>, CoexPolicyTimer)) {
    let Some(timer) = u8::try_from(index).ok().and_then(CoexPolicyTimer::new) else {
        return;
    };
    let _ = session::with(|session| operation(&mut session.radio_mut().coex_timer_bank(), timer));
}

pub fn enable_timer(index: u32) {
    with_timer(index, |bank, timer| bank.enable(timer));
}

pub fn disable_timer(index: u32) {
    with_timer(index, |bank, timer| bank.disable(timer));
}

pub fn force_timer(index: u32) {
    with_timer(index, |bank, timer| bank.force(timer));
}

pub fn unforce_timer(index: u32) {
    with_timer(index, |bank, timer| bank.unforce(timer));
}

/// One core release; `None` when the session did not start.
pub fn core_release(event: CoexEventId) -> Option<Result<(), CoexError>> {
    let mut core = CoexCore::new();
    session::with(|session| {
        let ports = CoexDomainPorts::new(session.radio_mut());
        let (mut timer, _) = ports.ports();
        core.release(&mut timer, event).map(|_| ())
    })
}

/// The timer clock, as `coex_hw_timer_freq_set(selector, divisor)`
/// programs it; a selector outside the four one-hot values, or a session
/// that did not start, is rejected.
pub fn set_timer_clock(selector: u32, divisor: u32) -> bool {
    let selector = match selector {
        1 => CoexClockSelector::Selector1,
        2 => CoexClockSelector::Selector2,
        4 => CoexClockSelector::Selector4,
        8 => CoexClockSelector::Selector8,
        _ => return false,
    };
    session::with(|session| {
        CoexDomainPorts::new(session.radio_mut()).set_timer_clock(selector, divisor)
    })
    .unwrap_or(false)
}
