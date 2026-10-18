//! Compiled vendor-comparison entry points over an isolated HAL owner.
//!
//! Each call splits the validation radio root, borrows the coexistence timer
//! bank through the arbiter lease and runs the production core or timer
//! sequence against it.

use oer_esp32s31_hal::{
    coex::{CoexPolicyTimer, CoexTimerBank},
    root::RadioHardware,
    shared_radio::SharedRadioLease,
};

use crate::{
    CoexArbiterPorts, CoexClient, CoexClientRequest, CoexCore, CoexError, CoexEventId, CoexPti,
    CoexTimerIndex,
};

/// Borrow the timer bank of an isolated validation radio for one call.
fn with_bank<R>(operation: impl FnOnce(CoexTimerBank<'_>) -> R) -> R {
    with_lease(|lease| operation(lease.coex_timer_bank()))
}

/// Borrow the lease of an isolated validation radio for one call. Its
/// priority table is the arbiter's cold table.
fn with_lease<R>(operation: impl FnOnce(&mut SharedRadioLease<'_, ()>) -> R) -> R {
    let (radio, _partitions) = RadioHardware::for_validation().into_concurrent(());
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a fresh validation arbiter grants its lease"));
    operation(&mut lease)
}

fn with_timer(index: u32, operation: impl FnOnce(&mut CoexTimerBank<'_>, CoexPolicyTimer)) {
    let Some(timer) = CoexPolicyTimer::new(index as u8) else {
        return;
    };
    with_bank(|mut bank| operation(&mut bank, timer));
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

/// Execute one complete timer program. The caller supplies only the chip
/// clock environment and typed values.
// CAPABILITY: coex-timer-validation-bridge
pub fn program_timer(
    real_chip: bool,
    index: CoexTimerIndex,
    client: CoexClient,
    pti: CoexPti,
    latency: u32,
    duration: u32,
) -> Result<(), CoexError> {
    with_lease(|lease| {
        let ports = CoexArbiterPorts::with_real_chip(lease, real_chip);
        let (mut timer, mut clock) = ports.ports();
        crate::program_timer(
            &mut timer, &mut clock, index, client, pti, latency, duration,
        )
    })
}

/// Execute one enabled core request. The boolean selects Bluetooth (`false`)
/// or Wi-Fi (`true`).
pub fn core_request(
    real_chip: bool,
    wifi: bool,
    request: CoexClientRequest,
) -> Result<(), CoexError> {
    let mut core = CoexCore::new();
    core.enable();
    with_lease(|lease| {
        let ports = CoexArbiterPorts::with_real_chip(lease, real_chip);
        let (mut timer, mut clock) = ports.ports();
        if wifi {
            core.request_wifi(&mut timer, &mut clock, request)
                .map(|_| ())
        } else {
            core.request_bluetooth(&mut timer, &mut clock, request)
                .map(|_| ())
        }
    })
}

/// Execute one core release.
pub fn core_release(event: CoexEventId) -> Result<(), CoexError> {
    let mut core = CoexCore::new();
    with_lease(|lease| {
        let ports = CoexArbiterPorts::new(lease);
        let (mut timer, _) = ports.ports();
        core.release(&mut timer, event)
    })
    .map(|_| ())
}

/// Program the timer clock as `coex_hw_timer_freq_set(selector, divisor)`
/// does. A selector outside the four one-hot values is rejected.
pub fn set_timer_clock(selector: u32, divisor: u32) -> bool {
    let selector = match selector {
        1 => crate::CoexClockSelector::Selector1,
        2 => crate::CoexClockSelector::Selector2,
        4 => crate::CoexClockSelector::Selector4,
        8 => crate::CoexClockSelector::Selector8,
        _ => return false,
    };
    with_lease(|lease| CoexArbiterPorts::new(lease).set_timer_clock(selector, divisor))
}
