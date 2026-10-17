use std::vec::Vec;

use super::{
    Ieee802154InterruptActivationPlan, Ieee802154InterruptTransitionPort,
    Ieee802154ObservedEventState, Ieee802154RxStateCode, Ieee802154StateSnapshot,
    Ieee802154TxStateCode, execute_interrupt_activation, execute_interrupt_deactivation,
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
    let zero_rx = Ieee802154RxStateCode::for_validation(0).expect("three-bit state");
    let zero_tx = Ieee802154TxStateCode::for_validation(0).expect("four-bit state");
    let sfd = Ieee802154RxStateCode::for_validation(1).expect("three-bit state");
    let after_sfd = Ieee802154RxStateCode::for_validation(2).expect("three-bit state");

    assert!(Ieee802154StateSnapshot::new(zero_rx, zero_tx).all_codes_zero());
    assert!(sfd.is_receive_sfd());
    assert!(!sfd.is_after_receive_sfd());
    assert!(after_sfd.is_after_receive_sfd());
    assert_eq!(after_sfd.value(), 2);
    assert_eq!(zero_tx.value(), 0);
    assert_eq!(
        Ieee802154RxStateCode::for_validation(Ieee802154RxStateCode::MAX + 1),
        None
    );
    assert_eq!(
        Ieee802154TxStateCode::for_validation(Ieee802154TxStateCode::MAX + 1),
        None
    );
}

#[test]
fn nonzero_state_code_fails_only_the_numeric_zero_predicate() {
    let rx = Ieee802154RxStateCode::for_validation(0).expect("three-bit state");
    let tx = Ieee802154TxStateCode::for_validation(1).expect("four-bit state");
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
    MaskEvents,
    EnableRuntimeEvents,
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
    operations: Vec<InterruptTransitionOperation>,
}

impl Ieee802154InterruptTransitionPort for RecordingInterruptTransitionPort {
    type EventSnapshot = RecordingEventSnapshot;

    fn stop_operation(&mut self) {
        self.operations
            .push(InterruptTransitionOperation::StopOperation);
    }

    fn stop_timer0(&mut self) {
        self.operations
            .push(InterruptTransitionOperation::StopTimer0);
    }

    fn stop_timer1(&mut self) {
        self.operations
            .push(InterruptTransitionOperation::StopTimer1);
    }

    fn mask_all_events(&mut self) {
        self.operations
            .push(InterruptTransitionOperation::MaskEvents);
    }

    fn enable_runtime_events(&mut self) {
        self.operations
            .push(InterruptTransitionOperation::EnableRuntimeEvents);
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
        self.operations
            .push(InterruptTransitionOperation::SampleStaleEvents(
                self.event_identity,
            ));
        RecordingEventSnapshot(self.event_identity)
    }

    fn acknowledge_events(&mut self, snapshot: Self::EventSnapshot) {
        self.operations
            .push(InterruptTransitionOperation::AcknowledgeStaleEvents(
                snapshot.0,
            ));
    }
}

#[test]
fn production_activation_executor_masks_then_acks_the_exact_affine_sample() {
    let mut port = RecordingInterruptTransitionPort {
        event_identity: 0xa5,
        operations: Vec::new(),
    };
    execute_interrupt_activation(
        &mut port,
        Ieee802154InterruptActivationPlan::SOURCE_CONFIRMED_BASELINE,
    );

    assert_eq!(
        port.operations,
        [
            InterruptTransitionOperation::MaskEvents,
            InterruptTransitionOperation::EnableRuntimeTxAborts,
            InterruptTransitionOperation::EnableRuntimeRxAborts,
            InterruptTransitionOperation::OrderDeviceAccesses,
            InterruptTransitionOperation::SampleStaleEvents(0xa5),
            InterruptTransitionOperation::AcknowledgeStaleEvents(0xa5),
            InterruptTransitionOperation::EnableRuntimeEvents,
            InterruptTransitionOperation::OrderDeviceAccesses,
        ]
    );
}

#[test]
fn production_deactivation_stops_every_engine_before_final_affine_ack() {
    let mut port = RecordingInterruptTransitionPort {
        event_identity: 0x3c,
        operations: Vec::new(),
    };
    execute_interrupt_deactivation(&mut port);

    assert_eq!(
        port.operations,
        [
            InterruptTransitionOperation::StopOperation,
            InterruptTransitionOperation::StopTimer0,
            InterruptTransitionOperation::StopTimer1,
            InterruptTransitionOperation::MaskEvents,
            InterruptTransitionOperation::MaskTxAborts,
            InterruptTransitionOperation::MaskRxAborts,
            InterruptTransitionOperation::OrderDeviceAccesses,
            InterruptTransitionOperation::SampleStaleEvents(0x3c),
            InterruptTransitionOperation::AcknowledgeStaleEvents(0x3c),
            InterruptTransitionOperation::OrderDeviceAccesses,
        ]
    );
}
