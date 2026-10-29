// SPDX-License-Identifier: Apache-2.0
//! Family-owned interrupt startup, activation policy and sampled teardown.

/// Discard every pre-bind event through consuming HAL phases, then release the IRQ owner.
/// No sample is taken and no event is enabled. Issued stops do not prove producer drain.
/// SOURCE: HW8_S31_IEEE_MASKED_STATUS_20261026; HW26_S31_IEEE_ABORT_20261027.
pub fn arm<P: oer_espressif_ieee802154_mac_values::Ieee802154InterruptStartup>(
    setup: P::Setup,
    task: P::Task,
) -> P::Output {
    let phase = P::mask_events(setup, task);
    let phase = P::runtime_tx_aborts(phase);
    let phase = P::runtime_rx_aborts(phase);
    let phase = P::order_before_stop(phase);
    let phase = P::stop_operation(phase);
    let phase = P::stop_timer0(phase);
    let phase = P::stop_timer1(phase);
    let phase = P::order_before_discard(phase);
    let phase = P::discard_startup_events(phase);
    P::finish(phase)
}

/// Stop, sample before masking, then consume the saved W1C sample before returning setup ownership.
/// SOURCE: hw8-20261026-radio-local-05, ieee-timers.json, PR #689.
pub fn deactivate<P: oer_espressif_ieee802154_mac_values::Ieee802154InterruptTeardown>(
    interrupt: P::Interrupt,
    task: P::Task,
) -> P::Output {
    let phase = P::stop_operation(interrupt, task);
    let phase = P::stop_timer0(phase);
    let phase = P::stop_timer1(phase);
    let phase = P::order_before_sample(phase);
    let phase = P::sample_events(phase);
    let phase = P::mask_events(phase);
    let phase = P::mask_tx_aborts(phase);
    let phase = P::mask_rx_aborts(phase);
    let phase = P::acknowledge_events(phase);
    P::finish(phase)
}

/// The same ten runtime events used for first activation and later sessions.
pub const RUNTIME_EVENTS: oer_espressif_ieee802154_mac_values::Ieee802154EventMask =
    oer_espressif_ieee802154_mac_values::Ieee802154EventMask::HANDLED_BASELINE_NO_TIMER0;

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::disallowed_macros)]
mod tests;
