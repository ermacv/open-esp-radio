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
/// Mask every event before installing the abort baseline. No event is
/// enabled or acknowledged here. Initialization consumes retained startup
/// pending after binding, or after obtaining the polling route's authority.
/// SOURCE: hw8-20261026-radio-local-05, ieee-timers.json, PR #689.
#[doc(hidden)]
pub fn execute_interrupt_arming<Port>(port: &mut Port, _plan: Ieee802154InterruptActivationPlan)
where
    Port: Ieee802154InterruptTransitionPort,
{
    port.mask_all_events();
    port.enable_runtime_tx_aborts();
    port.enable_runtime_rx_aborts();
    port.order_device_accesses();
}

/// Startup operations available only in a typed post-bind initialization or
/// in a pair holding the silent-route proof. Unsupported events stay unacked.
#[doc(hidden)]
pub trait Ieee802154InterruptInitializationPort: Ieee802154InterruptTransitionPort {
    type Fault;
    fn enable_handled_events(&mut self);
    fn acknowledge_startup_events(
        &mut self,
        snapshot: Self::EventSnapshot,
    ) -> Result<(), Self::Fault>;
}

/// Stop startup producers, expose only handled lanes, sample once, mask and
/// acknowledge that exact sample. The caller releases operational ownership
/// only after success. Timer0 is sampled here too, then remains masked until
/// its per-transmission ACK wait. No operation callback can consume old pending.
#[doc(hidden)]
pub fn execute_interrupt_initialization<Port: Ieee802154InterruptInitializationPort>(
    port: &mut Port,
) -> Result<(), Port::Fault> {
    port.stop_operation();
    port.stop_timer0();
    port.stop_timer1();
    port.order_device_accesses();
    port.enable_handled_events();
    port.order_device_accesses();
    let stale = port.sample_events();
    port.mask_all_events();
    let result = port.acknowledge_startup_events(stale);
    port.order_device_accesses();
    result
}

/// Issue the teardown commands and selected acknowledgement of a polled epoch.
///
/// Stop the operation and both MAC timers, sample before masking, then close
/// all three enable fields and acknowledge the saved affine W1C sample.
/// Masked timer status reads zero even when pending remains latched.
/// Both ordering boundaries precede transfer back to inactive setup ownership.
/// Issued commands and saved-sample acknowledgement do not prove producer drain.
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
