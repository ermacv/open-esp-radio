use super::*;
use std::{collections::VecDeque, vec::Vec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FakeError {
    Injected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    RouteRead,
    MaskRead,
    RxAbortMaskRead,
    Channel(u8),
    CcaMode(Ieee802154CcaMode),
    CcaThreshold(i8),
    Duration(u16),
    EnableOperationEvents,
    EnableOperationRxAborts,
    MaskOperationEvents,
    MaskOperationRxAborts,
    Fence,
    EdStart,
    EventStatus(Ieee802154OperationEventObservation),
    AcknowledgePendingEvents(Ieee802154OperationEventObservation),
    RxAbortReason(Ieee802154RxAbortReasonObservation),
    EdRss(i8),
    CcaBusy(bool),
}

struct FakeBackend {
    operations: Vec<Operation>,
    route_detached: bool,
    mask: Ieee802154OperationEventMaskState,
    rx_abort_mask: Ieee802154OperationRxAbortMaskState,
    event_status: Ieee802154OperationEventObservation,
    pending_at_acknowledge: Option<Ieee802154OperationEventObservation>,
    samples: VecDeque<Ieee802154OperationEventObservation>,
    rx_abort_reason: Ieee802154RxAbortReasonObservation,
    polling: bool,
    rss_code: i8,
    cca_busy: bool,
    fail_on_call: Option<usize>,
    calls: usize,
}

impl FakeBackend {
    fn new(samples: impl IntoIterator<Item = Ieee802154OperationEventObservation>) -> Self {
        Self {
            operations: Vec::new(),
            route_detached: true,
            mask: Ieee802154OperationEventMaskState::AllMasked,
            rx_abort_mask: Ieee802154OperationRxAbortMaskState::AllMasked,
            event_status: Ieee802154OperationEventObservation::default(),
            pending_at_acknowledge: None,
            samples: samples.into_iter().collect(),
            rx_abort_reason: Ieee802154RxAbortReason::EdAbort.into(),
            polling: false,
            rss_code: -42,
            cca_busy: false,
            fail_on_call: None,
            calls: 0,
        }
    }

    fn before_call(&mut self) -> Result<(), FakeError> {
        let call = self.calls;
        self.calls += 1;
        if self.fail_on_call == Some(call) {
            Err(FakeError::Injected)
        } else {
            Ok(())
        }
    }
}

impl Ieee802154PolledOperationBackend for FakeBackend {
    type Error = FakeError;

    fn set_channel(&mut self, channel: Ieee802154Channel) -> Result<(), Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::Channel(channel.number()));
        Ok(())
    }

    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) -> Result<(), Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::CcaMode(mode));
        Ok(())
    }

    fn set_cca_threshold_code(&mut self, threshold: i8) -> Result<(), Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::CcaThreshold(threshold));
        Ok(())
    }

    fn set_ed_duration(&mut self, duration: u16) -> Result<(), Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::Duration(duration));
        Ok(())
    }

    fn cpu_interrupt_route_is_detached(&mut self) -> Result<bool, Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::RouteRead);
        Ok(self.route_detached)
    }

    fn operation_event_mask_state(
        &mut self,
    ) -> Result<Ieee802154OperationEventMaskState, Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::MaskRead);
        Ok(self.mask)
    }

    fn operation_rx_abort_mask_state(
        &mut self,
    ) -> Result<Ieee802154OperationRxAbortMaskState, Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::RxAbortMaskRead);
        Ok(self.rx_abort_mask)
    }

    fn enable_ed_done_and_rx_abort(&mut self) -> Result<(), Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::EnableOperationEvents);
        self.mask = Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly;
        Ok(())
    }

    fn enable_ed_operation_rx_abort_reasons(&mut self) -> Result<(), Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::EnableOperationRxAborts);
        self.rx_abort_mask = Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly;
        Ok(())
    }

    fn mask_ed_done_and_rx_abort(&mut self) -> Result<(), Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::MaskOperationEvents);
        self.mask = Ieee802154OperationEventMaskState::AllMasked;
        self.polling = false;
        Ok(())
    }

    fn mask_ed_operation_rx_abort_reasons(&mut self) -> Result<(), Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::MaskOperationRxAborts);
        self.rx_abort_mask = Ieee802154OperationRxAbortMaskState::AllMasked;
        Ok(())
    }

    fn order_device_accesses(&mut self) -> Result<(), Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::Fence);
        Ok(())
    }

    fn request_ed_start(&mut self) -> Result<(), Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::EdStart);
        self.polling = true;
        Ok(())
    }

    fn sample_event_status(&mut self) -> Result<Ieee802154OperationEventObservation, Self::Error> {
        self.before_call()?;
        if self.polling && self.event_status.is_clear() {
            self.event_status = self.samples.pop_front().unwrap_or_default();
        }
        self.operations
            .push(Operation::EventStatus(self.event_status));
        Ok(self.event_status)
    }

    fn acknowledge_pending_events(
        &mut self,
    ) -> Result<Ieee802154OperationEventObservation, Self::Error> {
        self.before_call()?;
        let acknowledged = self.pending_at_acknowledge.unwrap_or(self.event_status);
        self.operations
            .push(Operation::AcknowledgePendingEvents(acknowledged));
        self.event_status = Ieee802154OperationEventObservation::default();
        self.polling = false;
        Ok(acknowledged)
    }

    fn sample_rx_abort_reason(
        &mut self,
    ) -> Result<Ieee802154RxAbortReasonObservation, Self::Error> {
        self.before_call()?;
        self.operations
            .push(Operation::RxAbortReason(self.rx_abort_reason));
        Ok(self.rx_abort_reason)
    }

    fn sample_ed_rss_code(&mut self) -> Result<i8, Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::EdRss(self.rss_code));
        Ok(self.rss_code)
    }

    fn sample_cca_busy(&mut self) -> Result<bool, Self::Error> {
        self.before_call()?;
        self.operations.push(Operation::CcaBusy(self.cca_busy));
        Ok(self.cca_busy)
    }
}

fn channel() -> Ieee802154Channel {
    Ieee802154Channel::new(20).unwrap()
}

fn budget(samples: u32) -> Ieee802154OperationPollBudget {
    Ieee802154OperationPollBudget::new(samples).unwrap()
}

fn event(event: Ieee802154Event) -> Ieee802154OperationEventObservation {
    Ieee802154OperationEventObservation::event(event)
}

fn events(first: Ieee802154Event, second: Ieee802154Event) -> Ieee802154OperationEventObservation {
    Ieee802154OperationEventObservation::events(first.mask().union(second.mask()))
}

fn owner(backend: FakeBackend) -> Ieee802154PolledOperationOwner<FakeBackend> {
    Ieee802154PolledOperationOwner::from_semantic_backend(backend)
}

fn start(
    backend: FakeBackend,
    operation: Ieee802154PolledOperation,
    samples: u32,
) -> Ieee802154PolledOperationActive<FakeBackend> {
    owner(backend)
        .prepare(operation, budget(samples))
        .unwrap_or_else(|_| panic!("prepare must succeed"))
        .start()
        .unwrap_or_else(|_| panic!("start must succeed"))
}

#[test]
fn zero_poll_budget_is_rejected_before_ownership() {
    assert_eq!(Ieee802154OperationPollBudget::new(0), None);
    assert_eq!(budget(3).samples(), 3);
}

#[test]
fn cca_prepare_and_start_use_duration_eight_and_exact_detached_window() {
    let operation = Ieee802154PolledOperation::clear_channel_assessment(
        channel(),
        Ieee802154CcaMode::CarrierOrEnergyDetection,
        -71,
    );
    let active = start(FakeBackend::new([]), operation, 2);

    assert_eq!(
        active.backend.operations,
        [
            Operation::RouteRead,
            Operation::MaskRead,
            Operation::RxAbortMaskRead,
            Operation::Channel(20),
            Operation::CcaMode(Ieee802154CcaMode::CarrierOrEnergyDetection),
            Operation::CcaThreshold(-71),
            Operation::Duration(IEEE802154_CCA_ED_DURATION),
            Operation::Fence,
            Operation::RouteRead,
            Operation::MaskRead,
            Operation::RxAbortMaskRead,
            Operation::RouteRead,
            Operation::MaskRead,
            Operation::RxAbortMaskRead,
            Operation::EnableOperationEvents,
            Operation::EnableOperationRxAborts,
            Operation::Fence,
            Operation::RouteRead,
            Operation::MaskRead,
            Operation::RxAbortMaskRead,
            Operation::EventStatus(Ieee802154OperationEventObservation::default()),
            Operation::EdStart,
        ]
    );
    assert_eq!(
        active.backend.mask,
        Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly
    );
    assert_eq!(
        active.backend.rx_abort_mask,
        Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly
    );
}

#[test]
fn energy_detection_completion_recovers_in_proved_order() {
    let operation = Ieee802154PolledOperation::energy_detection(channel(), 27);
    let active = start(
        FakeBackend::new([event(Ieee802154Event::EdDone)]),
        operation,
        2,
    );
    let completed = match active.poll() {
        Ieee802154PolledOperationPoll::Completed(completed) => completed,
        _ => panic!("lone ED_DONE must complete"),
    };

    assert_eq!(completed.evidence().operation(), operation);
    assert_eq!(
        completed.evidence().result(),
        Ieee802154PolledOperationResult::EnergyDetection { rss_code: -42 }
    );
    assert_eq!(completed.evidence().polls(), 1);
    assert_eq!(
        completed.backend.mask,
        Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly
    );
    assert_eq!(
        completed.backend.rx_abort_mask,
        Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly
    );

    let recovered = completed
        .recover()
        .unwrap_or_else(|_| panic!("exact ED_DONE acknowledgement must recover"));
    assert_eq!(
        recovered.evidence().result(),
        Ieee802154PolledOperationResult::EnergyDetection { rss_code: -42 }
    );
    assert_eq!(
        recovered.owner.backend.mask,
        Ieee802154OperationEventMaskState::AllMasked
    );
    assert_eq!(
        recovered.owner.backend.rx_abort_mask,
        Ieee802154OperationRxAbortMaskState::AllMasked
    );
    assert!(recovered.owner.backend.operations.ends_with(&[
        Operation::EventStatus(event(Ieee802154Event::EdDone)),
        Operation::EdRss(-42),
        Operation::RouteRead,
        Operation::MaskRead,
        Operation::RxAbortMaskRead,
        Operation::AcknowledgePendingEvents(event(Ieee802154Event::EdDone)),
        Operation::MaskOperationEvents,
        Operation::MaskOperationRxAborts,
        Operation::Fence,
        Operation::MaskRead,
        Operation::RxAbortMaskRead,
        Operation::RouteRead,
    ]));
}

#[test]
fn cca_completion_samples_busy_without_reading_rss() {
    let mut backend = FakeBackend::new([event(Ieee802154Event::EdDone)]);
    backend.cca_busy = true;
    let active = start(
        backend,
        Ieee802154PolledOperation::clear_channel_assessment(
            channel(),
            Ieee802154CcaMode::EnergyDetection,
            -64,
        ),
        1,
    );
    let completed = match active.poll() {
        Ieee802154PolledOperationPoll::Completed(completed) => completed,
        _ => panic!("ED_DONE must produce CCA evidence"),
    };

    assert_eq!(
        completed.evidence().result(),
        Ieee802154PolledOperationResult::ClearChannelAssessment { busy: true }
    );
    assert!(
        completed
            .backend
            .operations
            .contains(&Operation::CcaBusy(true))
    );
    assert!(
        !completed
            .backend
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::EdRss(_)))
    );
}

#[test]
fn recovered_owner_supports_back_to_back_serialized_ed() {
    let operation = Ieee802154PolledOperation::energy_detection(channel(), 8);
    let backend = FakeBackend::new([
        event(Ieee802154Event::EdDone),
        event(Ieee802154Event::EdDone),
    ]);
    let completed = match start(backend, operation, 1).poll() {
        Ieee802154PolledOperationPoll::Completed(completed) => completed,
        _ => panic!("first operation must complete"),
    };
    let recovered = completed
        .recover()
        .unwrap_or_else(|_| panic!("first operation must recover"));
    let active = recovered
        .into_owner()
        .prepare(operation, budget(1))
        .unwrap_or_else(|_| panic!("recovered owner must prepare again"))
        .start()
        .unwrap_or_else(|_| panic!("recovered owner must start again"));
    let completed = match active.poll() {
        Ieee802154PolledOperationPoll::Completed(completed) => completed,
        _ => panic!("second operation must complete"),
    };
    let recovered = completed
        .recover()
        .unwrap_or_else(|_| panic!("second operation must recover"));

    assert_eq!(
        recovered
            .owner
            .backend
            .operations
            .iter()
            .filter(|operation| matches!(operation, Operation::EdStart))
            .count(),
        2
    );
    assert_eq!(
        recovered
            .owner
            .backend
            .operations
            .iter()
            .filter(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
            .count(),
        2
    );
}

#[test]
fn pending_then_timeout_uses_exact_budget_and_remasks_without_stop_claim() {
    let operation = Ieee802154PolledOperation::energy_detection(channel(), 9);
    let active = start(FakeBackend::new([]), operation, 2);
    let active = match active.poll() {
        Ieee802154PolledOperationPoll::Pending(active) => active,
        _ => panic!("first empty sample must remain pending"),
    };
    let timeout = match active.poll() {
        Ieee802154PolledOperationPoll::Timeout(timeout) => timeout,
        _ => panic!("second empty sample must exhaust the budget"),
    };

    assert_eq!(timeout.operation(), operation);
    assert_eq!(timeout.polls(), 2);
    assert_eq!(
        timeout.backend.mask,
        Ieee802154OperationEventMaskState::AllMasked
    );
    assert_eq!(
        timeout.backend.rx_abort_mask,
        Ieee802154OperationRxAbortMaskState::AllMasked
    );
    assert_eq!(
        timeout
            .backend
            .operations
            .iter()
            .filter(|operation| matches!(operation, Operation::EdStart))
            .count(),
        1
    );
    assert!(
        !timeout
            .backend
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
    );
}

#[test]
fn stale_status_is_contained_before_ed_start_without_acknowledgement() {
    let mut backend = FakeBackend::new([]);
    backend.event_status = event(Ieee802154Event::Timer0Overflow);
    let prepared = owner(backend)
        .prepare(
            Ieee802154PolledOperation::energy_detection(channel(), 1),
            budget(1),
        )
        .unwrap_or_else(|_| panic!("request fields may prepare before status gate"));
    let quarantine = match prepared.start() {
        Err(quarantine) => quarantine,
        Ok(_) => panic!("stale status must block ED_START"),
    };

    assert_eq!(
        quarantine.reason(),
        &Ieee802154OperationQuarantineReason::StaleEventStatus {
            observed: event(Ieee802154Event::Timer0Overflow),
        }
    );
    assert!(!quarantine.backend.operations.contains(&Operation::EdStart));
    assert!(
        !quarantine
            .backend
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
    );
    assert_eq!(
        quarantine.backend.mask,
        Ieee802154OperationEventMaskState::AllMasked
    );
}

#[test]
fn attached_route_is_rejected_before_any_request_write() {
    let mut backend = FakeBackend::new([]);
    backend.route_detached = false;
    let quarantine = match owner(backend).prepare(
        Ieee802154PolledOperation::energy_detection(channel(), 1),
        budget(1),
    ) {
        Err(quarantine) => quarantine,
        Ok(_) => panic!("attached route must fail closed"),
    };

    assert_eq!(
        quarantine.reason(),
        &Ieee802154OperationQuarantineReason::CpuInterruptRouteAttached {
            stage: Ieee802154OperationStage::Prepare,
        }
    );
    assert_eq!(quarantine.backend.operations, [Operation::RouteRead]);
}

#[test]
fn unexpected_event_or_abort_enable_is_rejected_before_request_writes() {
    let operation = Ieee802154PolledOperation::energy_detection(channel(), 1);
    let mut backend = FakeBackend::new([]);
    backend.mask = Ieee802154OperationEventMaskState::Unexpected;
    let quarantine = match owner(backend).prepare(operation, budget(1)) {
        Err(quarantine) => quarantine,
        Ok(_) => panic!("unexpected event enable must fail closed"),
    };
    assert_eq!(
        quarantine.reason(),
        &Ieee802154OperationQuarantineReason::UnexpectedEventMask {
            stage: Ieee802154OperationStage::Prepare,
            observed: Ieee802154OperationEventMaskState::Unexpected,
        }
    );
    assert_eq!(
        quarantine.backend.operations,
        [Operation::RouteRead, Operation::MaskRead]
    );

    let mut backend = FakeBackend::new([]);
    backend.rx_abort_mask = Ieee802154OperationRxAbortMaskState::Unexpected;
    let quarantine = match owner(backend).prepare(operation, budget(1)) {
        Err(quarantine) => quarantine,
        Ok(_) => panic!("unexpected receive-abort enable must fail closed"),
    };
    assert_eq!(
        quarantine.reason(),
        &Ieee802154OperationQuarantineReason::UnexpectedRxAbortMask {
            stage: Ieee802154OperationStage::Prepare,
            observed: Ieee802154OperationRxAbortMaskState::Unexpected,
        }
    );
    assert_eq!(
        quarantine.backend.operations,
        [
            Operation::RouteRead,
            Operation::MaskRead,
            Operation::RxAbortMaskRead,
        ]
    );
}

#[test]
fn conflicting_terminal_events_are_remasked_and_quarantined() {
    let active = start(
        FakeBackend::new([events(Ieee802154Event::EdDone, Ieee802154Event::RxAbort)]),
        Ieee802154PolledOperation::energy_detection(channel(), 1),
        1,
    );
    let quarantine = match active.poll() {
        Ieee802154PolledOperationPoll::Quarantined(quarantine) => quarantine,
        _ => panic!("conflicting terminal sample must quarantine"),
    };

    assert_eq!(
        quarantine.reason(),
        &Ieee802154OperationQuarantineReason::ConflictingTerminalEvents {
            observed: events(Ieee802154Event::EdDone, Ieee802154Event::RxAbort),
        }
    );
    assert_eq!(
        quarantine.backend.mask,
        Ieee802154OperationEventMaskState::AllMasked
    );
    assert_eq!(
        quarantine.backend.rx_abort_mask,
        Ieee802154OperationRxAbortMaskState::AllMasked
    );
    assert!(
        !quarantine
            .backend
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
    );
}

#[test]
fn rx_abort_retains_full_evidence_and_never_acknowledges() {
    let event_status = events(Ieee802154Event::RxAbort, Ieee802154Event::Timer0Overflow);
    let mut backend = FakeBackend::new([event_status]);
    backend.rx_abort_reason = Ieee802154RxAbortReason::EdCoexistenceReject.into();
    let active = start(
        backend,
        Ieee802154PolledOperation::energy_detection(channel(), 1),
        1,
    );
    let aborted = match active.poll() {
        Ieee802154PolledOperationPoll::Aborted(aborted) => aborted,
        _ => panic!("RX_ABORT must end the polled request"),
    };

    assert_eq!(
        aborted.evidence().operation(),
        Ieee802154PolledOperation::energy_detection(channel(), 1)
    );
    assert_eq!(aborted.evidence().event_status(), event_status);
    assert_eq!(
        aborted.evidence().rx_abort_reason(),
        Ieee802154RxAbortReasonObservation::Named(Ieee802154RxAbortReason::EdCoexistenceReject)
    );
    assert_eq!(aborted.evidence().polls(), 1);
    assert_eq!(
        aborted.backend.mask,
        Ieee802154OperationEventMaskState::AllMasked
    );
    assert!(
        !aborted
            .backend
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
    );
    assert!(
        aborted
            .backend
            .operations
            .windows(2)
            .any(|operations| operations
                == [
                    Operation::EventStatus(event_status),
                    Operation::RxAbortReason(Ieee802154RxAbortReasonObservation::Named(
                        Ieee802154RxAbortReason::EdCoexistenceReject,
                    )),
                ])
    );
}

#[test]
fn unrelated_terminal_status_is_contained_without_acknowledgement() {
    let active = start(
        FakeBackend::new([event(Ieee802154Event::Timer0Overflow)]),
        Ieee802154PolledOperation::energy_detection(channel(), 1),
        1,
    );
    let quarantine = match active.poll() {
        Ieee802154PolledOperationPoll::Quarantined(quarantine) => quarantine,
        _ => panic!("unrelated terminal status must quarantine"),
    };

    assert_eq!(
        quarantine.reason(),
        &Ieee802154OperationQuarantineReason::UnexpectedTerminalStatus {
            observed: event(Ieee802154Event::Timer0Overflow),
        }
    );
    assert!(
        !quarantine
            .backend
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
    );
}

#[test]
fn acknowledged_snapshot_must_be_exactly_lone_ed_done() {
    for acknowledged in [
        Ieee802154OperationEventObservation::default(),
        event(Ieee802154Event::Timer0Overflow),
        events(Ieee802154Event::Timer0Overflow, Ieee802154Event::EdDone),
        events(Ieee802154Event::RxAbort, Ieee802154Event::EdDone),
        Ieee802154OperationEventObservation::unclassified(),
    ] {
        let mut backend = FakeBackend::new([event(Ieee802154Event::EdDone)]);
        backend.pending_at_acknowledge = Some(acknowledged);
        let completed = match start(
            backend,
            Ieee802154PolledOperation::energy_detection(channel(), 1),
            1,
        )
        .poll()
        {
            Ieee802154PolledOperationPoll::Completed(completed) => completed,
            _ => panic!("lone ED_DONE poll sample must complete before recovery"),
        };
        let quarantine = match completed.recover() {
            Err(quarantine) => quarantine,
            Ok(_) => panic!("non-ED_DONE acknowledgement must block reuse"),
        };

        assert_eq!(
            quarantine.reason(),
            &Ieee802154OperationQuarantineReason::UnexpectedAcknowledgedEvents {
                observed: acknowledged,
            }
        );
        assert!(quarantine.backend.operations.windows(2).any(|operations| {
            operations
                == [
                    Operation::AcknowledgePendingEvents(acknowledged),
                    Operation::MaskOperationEvents,
                ]
        }));
        assert_eq!(
            quarantine.backend.mask,
            Ieee802154OperationEventMaskState::AllMasked
        );
    }
}

#[test]
fn recovery_rechecks_active_window_before_acknowledgement() {
    let active = start(
        FakeBackend::new([event(Ieee802154Event::EdDone)]),
        Ieee802154PolledOperation::energy_detection(channel(), 1),
        1,
    );
    let mut completed = match active.poll() {
        Ieee802154PolledOperationPoll::Completed(completed) => completed,
        _ => panic!("lone ED_DONE must complete"),
    };
    completed.backend.mask = Ieee802154OperationEventMaskState::Unexpected;
    let quarantine = match completed.recover() {
        Err(quarantine) => quarantine,
        Ok(_) => panic!("changed event window must block recovery"),
    };

    assert_eq!(
        quarantine.reason(),
        &Ieee802154OperationQuarantineReason::UnexpectedEventMask {
            stage: Ieee802154OperationStage::AcknowledgeTerminalEvent,
            observed: Ieee802154OperationEventMaskState::Unexpected,
        }
    );
    assert!(
        !quarantine
            .backend
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::AcknowledgePendingEvents(_)))
    );
    assert_eq!(
        quarantine.backend.mask,
        Ieee802154OperationEventMaskState::AllMasked
    );
}

#[test]
fn acknowledgement_backend_failure_contains_and_never_recovers() {
    let active = start(
        FakeBackend::new([event(Ieee802154Event::EdDone)]),
        Ieee802154PolledOperation::energy_detection(channel(), 1),
        1,
    );
    let mut completed = match active.poll() {
        Ieee802154PolledOperationPoll::Completed(completed) => completed,
        _ => panic!("lone ED_DONE must complete"),
    };
    completed.backend.fail_on_call = Some(completed.backend.calls + 3);
    let quarantine = match completed.recover() {
        Err(quarantine) => quarantine,
        Ok(_) => panic!("failed acknowledgement must not recover"),
    };

    assert!(matches!(
        quarantine.reason(),
        Ieee802154OperationQuarantineReason::Backend {
            stage: Ieee802154OperationStage::AcknowledgeTerminalEvent,
            error: FakeError::Injected,
        }
    ));
    assert_eq!(
        quarantine.backend.mask,
        Ieee802154OperationEventMaskState::AllMasked
    );
    assert_eq!(
        quarantine.backend.rx_abort_mask,
        Ieee802154OperationRxAbortMaskState::AllMasked
    );
}

#[test]
fn backend_failure_during_start_is_cleaned_and_quarantined() {
    let operation = Ieee802154PolledOperation::energy_detection(channel(), 1);
    let prepared = owner(FakeBackend::new([]))
        .prepare(operation, budget(1))
        .unwrap_or_else(|_| panic!("prepare must succeed"));
    let mut prepared = prepared;
    prepared.backend.fail_on_call = Some(prepared.backend.calls + 10);
    let quarantine = match prepared.start() {
        Err(quarantine) => quarantine,
        Ok(_) => panic!("injected command failure must quarantine"),
    };

    assert!(matches!(
        quarantine.reason(),
        Ieee802154OperationQuarantineReason::Backend {
            stage: Ieee802154OperationStage::StartCommand,
            error: FakeError::Injected,
        }
    ));
    assert_eq!(
        quarantine.backend.mask,
        Ieee802154OperationEventMaskState::AllMasked
    );
    assert_eq!(
        quarantine.backend.rx_abort_mask,
        Ieee802154OperationRxAbortMaskState::AllMasked
    );
}
