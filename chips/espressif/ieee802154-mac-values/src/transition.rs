//! Interrupt activation and teardown, ordered once for every chip.

use crate::Ieee802154InterruptActivationPlan;

/// Execution port of the interrupt activation and teardown transactions,
/// implemented by each chip PAC over its raw owners and by host ordering
/// models.
///
/// The event snapshot is an associated affine type. An executor can therefore
/// acknowledge only the value returned by `sample_events`; no integer event
/// image can cross this boundary.
#[doc(hidden)]
pub trait Ieee802154InterruptTransitionPort {
    type EventSnapshot;

    fn stop_operation(&mut self);
    fn stop_timer0(&mut self);
    fn stop_timer1(&mut self);
    fn enable_all_events(&mut self);
    fn mask_all_events(&mut self);
    fn mask_all_tx_aborts(&mut self);
    fn enable_runtime_tx_aborts(&mut self);
    fn mask_all_rx_aborts(&mut self);
    fn enable_runtime_rx_aborts(&mut self);
    fn order_device_accesses(&mut self);
    fn sample_events(&mut self) -> Self::EventSnapshot;
    fn acknowledge_events(&mut self, snapshot: Self::EventSnapshot);
}

/// Arm the interrupt side for its binding: the first part of the
/// activation, with every event left masked.
///
/// With no handler bound, enable events to expose retained timer pending,
/// sample it, mask events and acknowledge that saved affine W1C sample.
/// Both abort fields take their runtime baseline; the final fence precedes
/// transfer of the interrupt owner. The task publishes runtime events after
/// binding, or when the polling engine enables them.
/// SOURCE: hw8-20261026-radio-local-05, ieee-timers.json, PR #689.
#[doc(hidden)]
pub fn execute_interrupt_arming<Port>(port: &mut Port, _plan: Ieee802154InterruptActivationPlan)
where
    Port: Ieee802154InterruptTransitionPort,
{
    port.enable_runtime_tx_aborts();
    port.enable_runtime_rx_aborts();
    port.enable_all_events();
    port.order_device_accesses();

    let stale = port.sample_events();
    port.mask_all_events();
    port.acknowledge_events(stale);
    port.order_device_accesses();
}

/// Execute the complete teardown of a polled epoch.
///
/// Stop the operation and both MAC timers, sample before masking, then close
/// all three enable fields and acknowledge the saved affine W1C sample.
/// Masked timer status reads zero even when pending remains latched.
/// Both ordering boundaries precede transfer back to inactive setup ownership.
/// SOURCE: hw8-20261026-radio-local-05, ieee-timers.json, PR #689.
#[doc(hidden)]
pub fn execute_interrupt_deactivation<Port>(port: &mut Port)
where
    Port: Ieee802154InterruptTransitionPort,
{
    port.stop_operation();
    port.stop_timer0();
    port.stop_timer1();
    port.order_device_accesses();
    let pending = port.sample_events();
    port.mask_all_events();
    port.mask_all_tx_aborts();
    port.mask_all_rx_aborts();
    port.acknowledge_events(pending);
    port.order_device_accesses();
}
