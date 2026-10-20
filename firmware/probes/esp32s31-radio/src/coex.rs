//! The coexistence entries of the vendor comparison, over the radio domain
//! of the probe image's root.
//!
//! Each call splits a fresh probe root, lends its domain to the production
//! core or timer sequence and compares the registers it leaves.

use oer_esp32s31_hal::{
    coex::{CoexPolicyTimer, CoexTimerBank},
    domain::RadioDomain,
    root::RadioHardware,
};
use oer_esp32s31_radio_coex::{
    CoexClient, CoexClientRequest, CoexClockSelector, CoexCore, CoexDomainPorts, CoexError,
    CoexEventId, CoexPti, CoexTimerIndex,
};

/// The radio domain of a fresh probe root, for one call.
fn with_domain<R>(
    operation: impl FnOnce(&mut RadioDomain<oer_espressif_esp_hal_radio::EspHal>) -> R,
) -> R {
    let (mut radio, _partitions) =
        RadioHardware::for_probe().split::<oer_espressif_esp_hal_radio::EspHal, _>(());
    operation(&mut radio)
}

fn with_timer(index: u32, operation: impl FnOnce(&mut CoexTimerBank<'_>, CoexPolicyTimer)) {
    let Some(timer) = u8::try_from(index).ok().and_then(CoexPolicyTimer::new) else {
        return;
    };
    with_domain(|radio| operation(&mut radio.coex_timer_bank(), timer));
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

/// One complete timer program; the caller supplies the chip clock
/// environment and typed values.
pub fn program_timer(
    real_chip: bool,
    index: CoexTimerIndex,
    client: CoexClient,
    pti: CoexPti,
    latency: u32,
    duration: u32,
) -> Result<(), CoexError> {
    with_domain(|radio| {
        let ports = CoexDomainPorts::with_real_chip(radio, real_chip);
        let (mut timer, mut clock) = ports.ports();
        oer_esp32s31_radio_coex::program_timer(
            &mut timer, &mut clock, index, client, pti, latency, duration,
        )
    })
}

/// One enabled core request: Bluetooth (`wifi` false) or Wi-Fi.
pub fn core_request(
    real_chip: bool,
    wifi: bool,
    request: CoexClientRequest,
) -> Result<(), CoexError> {
    let mut core = CoexCore::new();
    core.enable();
    with_domain(|radio| {
        let ports = CoexDomainPorts::with_real_chip(radio, real_chip);
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

/// One core release.
pub fn core_release(event: CoexEventId) -> Result<(), CoexError> {
    let mut core = CoexCore::new();
    with_domain(|radio| {
        let ports = CoexDomainPorts::new(radio);
        let (mut timer, _) = ports.ports();
        core.release(&mut timer, event).map(|_| ())
    })
}

/// The timer clock, as `coex_hw_timer_freq_set(selector, divisor)`
/// programs it; a selector outside the four one-hot values is rejected.
pub fn set_timer_clock(selector: u32, divisor: u32) -> bool {
    let selector = match selector {
        1 => CoexClockSelector::Selector1,
        2 => CoexClockSelector::Selector2,
        4 => CoexClockSelector::Selector4,
        8 => CoexClockSelector::Selector8,
        _ => return false,
    };
    with_domain(|radio| CoexDomainPorts::new(radio).set_timer_clock(selector, divisor))
}
