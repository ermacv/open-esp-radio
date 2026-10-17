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
    fn mask_all_events(&mut self);
    fn enable_runtime_events(&mut self);
    fn mask_all_tx_aborts(&mut self);
    fn enable_runtime_tx_aborts(&mut self);
    fn mask_all_rx_aborts(&mut self);
    fn enable_runtime_rx_aborts(&mut self);
    fn order_device_accesses(&mut self);
    fn sample_events(&mut self) -> Self::EventSnapshot;
    fn acknowledge_events(&mut self, snapshot: Self::EventSnapshot);
}

/// Execute the complete activation while the platform CPU route is disabled.
///
/// `EVENT_ENABLE` remains zero while both abort fields and the stale affine
/// W1C transaction are updated. The reviewed event baseline is published only
/// after the exact sampled status has been consumed, and the final fence
/// precedes transfer of the hard-IRQ owner.
#[doc(hidden)]
pub fn execute_interrupt_activation<Port>(port: &mut Port, _plan: Ieee802154InterruptActivationPlan)
where
    Port: Ieee802154InterruptTransitionPort,
{
    port.mask_all_events();
    port.enable_runtime_tx_aborts();
    port.enable_runtime_rx_aborts();
    port.order_device_accesses();

    let stale = port.sample_events();
    port.acknowledge_events(stale);
    port.enable_runtime_events();
    port.order_device_accesses();
}

/// Execute the complete teardown after the platform CPU route is disabled.
///
/// The operation and both MAC timers are stopped before all three enable
/// fields are replaced with their closed zero images. One final affine W1C
/// sample is then consumed. Both ordering boundaries precede transfer back to
/// inactive setup ownership.
#[doc(hidden)]
pub fn execute_interrupt_deactivation<Port>(port: &mut Port)
where
    Port: Ieee802154InterruptTransitionPort,
{
    port.stop_operation();
    port.stop_timer0();
    port.stop_timer1();
    port.mask_all_events();
    port.mask_all_tx_aborts();
    port.mask_all_rx_aborts();
    port.order_device_accesses();

    let pending = port.sample_events();
    port.acknowledge_events(pending);
    port.order_device_accesses();
}
