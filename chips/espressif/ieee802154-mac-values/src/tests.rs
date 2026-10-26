use std::vec::Vec;

use super::{
    Ieee802154InterruptActivationPlan, Ieee802154InterruptTransitionPort,
    Ieee802154ObservedEventState, Ieee802154RxStateCode, Ieee802154StateSnapshot,
    Ieee802154TxStateCode, execute_interrupt_arming, execute_interrupt_deactivation,
};

#[test]
fn semantic_event_union_never_loses_ed_done_or_rx_abort_presence() {
    use Ieee802154ObservedEventState as State;

    assert_eq!(
        State::EdDoneAndRxAbortWithOther.union(State::RxAbortOnly),
        State::EdDoneAndRxAbortWithOther
    );
    assert_eq!(
        State::RxAbortWithOther.union(State::EdDoneWithOther),
        State::EdDoneAndRxAbortWithOther
    );
}

#[test]
fn state_codes_are_bounded_and_expose_only_reviewed_predicates() {
    let zero_rx = Ieee802154RxStateCode::for_host_test(0).expect("three-bit state");
    let zero_tx = Ieee802154TxStateCode::for_host_test(0).expect("four-bit state");
    let sfd = Ieee802154RxStateCode::for_host_test(1).expect("three-bit state");
    let after_sfd = Ieee802154RxStateCode::for_host_test(2).expect("three-bit state");

    assert!(Ieee802154StateSnapshot::new(zero_rx, zero_tx).all_codes_zero());
    assert!(sfd.is_receive_sfd());
    assert!(!sfd.is_after_receive_sfd());
    assert!(after_sfd.is_after_receive_sfd());
    assert_eq!(after_sfd.value(), 2);
    assert_eq!(zero_tx.value(), 0);
    assert_eq!(
        Ieee802154RxStateCode::for_host_test(Ieee802154RxStateCode::MAX + 1),
        None
    );
    assert_eq!(
        Ieee802154TxStateCode::for_host_test(Ieee802154TxStateCode::MAX + 1),
        None
    );
}

#[test]
fn nonzero_state_code_fails_only_the_numeric_zero_predicate() {
    let rx = Ieee802154RxStateCode::for_host_test(0).expect("three-bit state");
    let tx = Ieee802154TxStateCode::for_host_test(1).expect("four-bit state");
    let snapshot = Ieee802154StateSnapshot::new(rx, tx);

    assert!(!snapshot.all_codes_zero());
    assert_eq!(snapshot.rx().value(), 0);
    assert_eq!(snapshot.tx().value(), 1);
}

#[derive(Debug, Eq, PartialEq)]
enum InterruptTransitionOperation {
    StopOperation,
    StopTimer0,
    StopTimer1,
    EnableEvents,
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
fn arming_acks_the_exact_affine_sample_and_leaves_every_event_masked() {
    let mut port = RecordingInterruptTransitionPort {
        event_identity: 0x5a,
        events_enabled: false,
        pending: true,
        expire_on_stop: false,
        operations: Vec::new(),
    };
    execute_interrupt_arming(
        &mut port,
        Ieee802154InterruptActivationPlan::SOURCE_CONFIRMED_BASELINE,
    );

    assert_eq!(
        port.operations,
        [
            InterruptTransitionOperation::EnableRuntimeTxAborts,
            InterruptTransitionOperation::EnableRuntimeRxAborts,
            InterruptTransitionOperation::EnableEvents,
            InterruptTransitionOperation::OrderDeviceAccesses,
            InterruptTransitionOperation::SampleStaleEvents(0x5a),
            InterruptTransitionOperation::MaskEvents,
            InterruptTransitionOperation::AcknowledgeStaleEvents(0x5a),
            InterruptTransitionOperation::OrderDeviceAccesses,
        ]
    );
    assert!(!port.events_enabled);
    port.enable_all_events();
    assert_eq!(port.sample_events(), RecordingEventSnapshot(0));
}

#[test]
fn production_deactivation_stops_every_engine_before_final_affine_ack() {
    let mut port = RecordingInterruptTransitionPort {
        event_identity: 0x3c,
        events_enabled: true,
        pending: false,
        expire_on_stop: true,
        operations: Vec::new(),
    };
    execute_interrupt_deactivation(&mut port);

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

#[test]
fn a_security_payload_offset_is_made_only_inside_seven_bits() {
    use super::Ieee802154SecurityPayloadOffset as Offset;

    assert_eq!(Offset::new(Offset::MAX).map(Offset::value), Some(0x7f));
    assert_eq!(Offset::new(0x80), None);
    assert_eq!(Offset::new(u8::MAX), None);
}
