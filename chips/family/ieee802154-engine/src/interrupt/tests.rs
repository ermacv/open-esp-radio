// SPDX-License-Identifier: Apache-2.0
use std::vec::Vec;

use oer_espressif_ieee802154_mac_values::{
    Ieee802154InterruptStartup, Ieee802154InterruptTeardown,
};
/// Execution port of the interrupt activation and teardown transactions,
/// implemented by each chip PAC over its raw owners and by host ordering
/// models.
///
/// The event snapshot is an associated affine type. An executor can therefore
/// acknowledge only the value returned by `sample_events`; no integer event
/// image can cross this boundary.
#[doc(hidden)]
trait Ieee802154InterruptTransitionPort {
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

/// Startup discard available only while the task still owns the unbound IRQ
/// part. Every event before binding is stale and cannot be serviced.
#[doc(hidden)]
trait Ieee802154InterruptInitializationPort: Ieee802154InterruptTransitionPort {
    fn discard_startup_events(&mut self);
}

#[derive(Debug, Eq, PartialEq)]
enum InterruptTransitionOperation {
    StopOperation,
    StopTimer0,
    StopTimer1,
    EnableEvents,
    DiscardStartupEvents,
    MaskEvents,
    MaskTxAborts,
    EnableRuntimeTxAborts,
    MaskRxAborts,
    EnableRuntimeRxAborts,
    OrderDeviceAccesses,
    SampleStaleEvents(u8),
    AcknowledgeStaleEvents(u8),
}

#[derive(Debug, Eq, PartialEq)]
struct RecordingEventSnapshot(u8);

struct RecordingInterruptTransitionPort {
    event_identity: u8,
    events_enabled: bool,
    pending: bool,
    expire_on_stop: bool,
    operations: Vec<InterruptTransitionOperation>,
    line_was_high: bool,
    handler_calls: usize,
    bound: bool,
}

impl Ieee802154InterruptTransitionPort for RecordingInterruptTransitionPort {
    type EventSnapshot = RecordingEventSnapshot;

    fn stop_operation(&mut self) {
        self.operations
            .push(InterruptTransitionOperation::StopOperation);
    }

    fn stop_timer0(&mut self) {
        self.pending |= self.expire_on_stop;
        self.operations
            .push(InterruptTransitionOperation::StopTimer0);
    }

    fn stop_timer1(&mut self) {
        self.pending |= self.expire_on_stop;
        self.operations
            .push(InterruptTransitionOperation::StopTimer1);
    }

    fn enable_all_events(&mut self) {
        self.events_enabled = true;
        self.observe_line();
        self.operations
            .push(InterruptTransitionOperation::EnableEvents);
    }

    fn mask_all_events(&mut self) {
        self.events_enabled = false;
        self.operations
            .push(InterruptTransitionOperation::MaskEvents);
    }

    fn mask_all_tx_aborts(&mut self) {
        self.operations
            .push(InterruptTransitionOperation::MaskTxAborts);
    }

    fn enable_runtime_tx_aborts(&mut self) {
        self.operations
            .push(InterruptTransitionOperation::EnableRuntimeTxAborts);
    }

    fn mask_all_rx_aborts(&mut self) {
        self.operations
            .push(InterruptTransitionOperation::MaskRxAborts);
    }

    fn enable_runtime_rx_aborts(&mut self) {
        self.operations
            .push(InterruptTransitionOperation::EnableRuntimeRxAborts);
    }

    fn order_device_accesses(&mut self) {
        self.observe_line();
        self.operations
            .push(InterruptTransitionOperation::OrderDeviceAccesses);
    }

    fn sample_events(&mut self) -> Self::EventSnapshot {
        // Model the measured timer read: masking hides, but retains, pending.
        let visible = if self.events_enabled && self.pending {
            self.event_identity
        } else {
            0
        };
        self.operations
            .push(InterruptTransitionOperation::SampleStaleEvents(visible));
        RecordingEventSnapshot(visible)
    }

    fn acknowledge_events(&mut self, snapshot: Self::EventSnapshot) {
        if snapshot.0 == self.event_identity {
            self.pending = false;
        }
        self.operations
            .push(InterruptTransitionOperation::AcknowledgeStaleEvents(
                snapshot.0,
            ));
    }
}

#[test]
fn production_deactivation_stops_every_engine_before_final_affine_ack() {
    let mut port = RecordingInterruptTransitionPort {
        event_identity: 0x3c,
        events_enabled: true,
        pending: false,
        expire_on_stop: true,
        operations: Vec::new(),
        line_was_high: false,
        handler_calls: 0,
        bound: false,
    };
    port = super::deactivate::<RecordingTeardown>(port, ());

    assert_eq!(
        port.operations,
        [
            InterruptTransitionOperation::StopOperation,
            InterruptTransitionOperation::StopTimer0,
            InterruptTransitionOperation::StopTimer1,
            InterruptTransitionOperation::OrderDeviceAccesses,
            InterruptTransitionOperation::SampleStaleEvents(0x3c),
            InterruptTransitionOperation::MaskEvents,
            InterruptTransitionOperation::MaskTxAborts,
            InterruptTransitionOperation::MaskRxAborts,
            InterruptTransitionOperation::AcknowledgeStaleEvents(0x3c),
            InterruptTransitionOperation::OrderDeviceAccesses,
        ]
    );
    assert!(!port.events_enabled);
    port.enable_all_events();
    assert_eq!(port.sample_events(), RecordingEventSnapshot(0));
}

#[test]
fn masked_zero_cannot_clear_pending_but_a_saved_positive_sample_can() {
    let mut port = RecordingInterruptTransitionPort {
        event_identity: 1,
        events_enabled: true,
        pending: true,
        expire_on_stop: false,
        operations: Vec::new(),
        line_was_high: false,
        handler_calls: 0,
        bound: false,
    };
    let saved = port.sample_events();
    port.mask_all_events();
    let hidden = port.sample_events();
    assert_eq!(hidden, RecordingEventSnapshot(0));
    port.acknowledge_events(hidden);
    port.enable_all_events();
    assert_eq!(port.sample_events(), RecordingEventSnapshot(1));
    port.mask_all_events();
    port.acknowledge_events(saved);
    port.enable_all_events();
    assert_eq!(port.sample_events(), RecordingEventSnapshot(0));
}

impl Ieee802154InterruptInitializationPort for RecordingInterruptTransitionPort {
    fn discard_startup_events(&mut self) {
        assert!(!self.events_enabled && !self.bound);
        self.pending = false;
        self.operations
            .push(InterruptTransitionOperation::DiscardStartupEvents);
    }
}

impl RecordingInterruptTransitionPort {
    fn line_high(&self) -> bool {
        self.events_enabled && self.pending
    }
    fn observe_line(&mut self) {
        self.line_was_high |= self.line_high();
        // Emulate CPU admission at every observable boundary, rather than
        // waiting for the test to explicitly poll after initialization.
        let _ = self.service_interrupt();
    }
    fn service_interrupt(&mut self) -> bool {
        if !self.bound || !self.line_high() {
            return false;
        }
        self.handler_calls += 1;
        self.pending = false;
        true
    }
}

#[test]
fn pre_bind_discard_consumes_hidden_timer1_without_delivery_or_storm() {
    let mut port = RecordingInterruptTransitionPort {
        event_identity: 1,
        events_enabled: false,
        pending: true,
        expire_on_stop: true,
        operations: Vec::new(),
        line_was_high: false,
        handler_calls: 0,
        bound: false,
    };
    // Measured masking hides timer1 pending; a fresh zero sample cannot ack it.
    assert_eq!(port.sample_events(), RecordingEventSnapshot(0));
    assert!(port.pending);
    port.operations.clear();
    port = super::arm::<RecordingStartup>(port, ());
    assert_eq!(
        port.operations,
        [
            InterruptTransitionOperation::MaskEvents,
            InterruptTransitionOperation::EnableRuntimeTxAborts,
            InterruptTransitionOperation::EnableRuntimeRxAborts,
            InterruptTransitionOperation::OrderDeviceAccesses,
            InterruptTransitionOperation::StopOperation,
            InterruptTransitionOperation::StopTimer0,
            InterruptTransitionOperation::StopTimer1,
            InterruptTransitionOperation::OrderDeviceAccesses,
            InterruptTransitionOperation::DiscardStartupEvents,
            InterruptTransitionOperation::OrderDeviceAccesses,
        ]
    );
    assert!(!port.pending && !port.line_high() && !port.line_was_high);
    assert_eq!(port.handler_calls, 0);
    assert!(!port.service_interrupt());

    // Bind only after discard. Enabling the handled set cannot deliver the
    // inherited pending, so a newly installed callback receives no stale IRQ.
    port.bound = true;
    port.events_enabled = true;
    for _ in 0..32 {
        assert!(!port.service_interrupt());
    }
    assert!(!port.line_high());
    assert_eq!(
        port.handler_calls, 0,
        "neither stale callback nor IRQ storm"
    );
    // A fresh deadline delivers once and then leaves the line low.
    port.pending = true;
    assert!(port.service_interrupt());
    assert!(!port.service_interrupt());
    assert!(!port.line_high());
    assert_eq!(port.handler_calls, 1);
}

struct RecordingStartup;
impl Ieee802154InterruptStartup for RecordingStartup {
    type Task = ();
    type Setup = RecordingInterruptTransitionPort;
    type Masked = RecordingInterruptTransitionPort;
    type TxAborts = RecordingInterruptTransitionPort;
    type RxAborts = RecordingInterruptTransitionPort;
    type Ordered = RecordingInterruptTransitionPort;
    type Stopped = RecordingInterruptTransitionPort;
    type Timer0Stopped = RecordingInterruptTransitionPort;
    type TimersStopped = RecordingInterruptTransitionPort;
    type DiscardReady = RecordingInterruptTransitionPort;
    type Discarded = RecordingInterruptTransitionPort;
    type Output = RecordingInterruptTransitionPort;
    fn mask_events(mut port: Self::Setup, _: ()) -> Self::Masked {
        port.mask_all_events();
        port
    }
    fn runtime_tx_aborts(mut port: Self::Masked) -> Self::TxAborts {
        port.enable_runtime_tx_aborts();
        port
    }
    fn runtime_rx_aborts(mut port: Self::TxAborts) -> Self::RxAborts {
        port.enable_runtime_rx_aborts();
        port
    }
    fn order_before_stop(mut port: Self::RxAborts) -> Self::Ordered {
        port.order_device_accesses();
        port
    }
    fn stop_operation(mut port: Self::Ordered) -> Self::Stopped {
        port.stop_operation();
        port
    }
    fn stop_timer0(mut port: Self::Stopped) -> Self::Timer0Stopped {
        port.stop_timer0();
        port
    }
    fn stop_timer1(mut port: Self::Timer0Stopped) -> Self::TimersStopped {
        port.stop_timer1();
        port
    }
    fn order_before_discard(mut port: Self::TimersStopped) -> Self::DiscardReady {
        port.order_device_accesses();
        port
    }
    fn discard_startup_events(mut port: Self::DiscardReady) -> Self::Discarded {
        port.discard_startup_events();
        port
    }
    fn finish(mut port: Self::Discarded) -> Self::Output {
        port.order_device_accesses();
        port
    }
}
struct RecordingTeardown;
impl Ieee802154InterruptTeardown for RecordingTeardown {
    type Task = ();
    type Interrupt = RecordingInterruptTransitionPort;
    type Stopped = RecordingInterruptTransitionPort;
    type Timer0Stopped = RecordingInterruptTransitionPort;
    type TimersStopped = RecordingInterruptTransitionPort;
    type SampleReady = RecordingInterruptTransitionPort;
    type Sampled = (RecordingInterruptTransitionPort, RecordingEventSnapshot);
    type Masked = (RecordingInterruptTransitionPort, RecordingEventSnapshot);
    type TxMasked = (RecordingInterruptTransitionPort, RecordingEventSnapshot);
    type AllMasked = (RecordingInterruptTransitionPort, RecordingEventSnapshot);
    type Acknowledged = RecordingInterruptTransitionPort;
    type Output = RecordingInterruptTransitionPort;
    fn stop_operation(mut port: Self::Interrupt, _: ()) -> Self::Stopped {
        port.stop_operation();
        port
    }
    fn stop_timer0(mut port: Self::Stopped) -> Self::Timer0Stopped {
        port.stop_timer0();
        port
    }
    fn stop_timer1(mut port: Self::Timer0Stopped) -> Self::TimersStopped {
        port.stop_timer1();
        port
    }
    fn order_before_sample(mut port: Self::TimersStopped) -> Self::SampleReady {
        port.order_device_accesses();
        port
    }
    fn sample_events(mut port: Self::SampleReady) -> Self::Sampled {
        let sample = port.sample_events();
        (port, sample)
    }
    fn mask_events((mut port, sample): Self::Sampled) -> Self::Masked {
        port.mask_all_events();
        (port, sample)
    }
    fn mask_tx_aborts((mut port, sample): Self::Masked) -> Self::TxMasked {
        port.mask_all_tx_aborts();
        (port, sample)
    }
    fn mask_rx_aborts((mut port, sample): Self::TxMasked) -> Self::AllMasked {
        port.mask_all_rx_aborts();
        (port, sample)
    }
    fn acknowledge_events((mut port, sample): Self::AllMasked) -> Self::Acknowledged {
        port.acknowledge_events(sample);
        port
    }
    fn finish(mut port: Self::Acknowledged) -> Self::Output {
        port.order_device_accesses();
        port
    }
}

#[test]
fn first_and_later_activation_use_exactly_the_ten_serviced_events() {
    use oer_espressif_ieee802154_mac_values::{Ieee802154Event as E, Ieee802154EventMask as M};
    let expected = M::NONE
        .with(E::TxDone)
        .with(E::RxDone)
        .with(E::AckTxDone)
        .with(E::AckRxDone)
        .with(E::RxAbort)
        .with(E::TxAbort)
        .with(E::EdDone)
        .with(E::Timer1Overflow)
        .with(E::TxSfdDone)
        .with(E::RxSfdDone);
    assert_eq!(super::RUNTIME_EVENTS, expected);
    assert!(!super::RUNTIME_EVENTS.contains(E::Timer0Overflow));
    assert!(!super::RUNTIME_EVENTS.contains(E::ClockCountMatch));
}
