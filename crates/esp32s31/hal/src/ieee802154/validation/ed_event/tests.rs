use std::{collections::VecDeque, vec::Vec};

use super::{
    Ieee802154EdEventProbeBackend, Ieee802154EdEventProbeConfig, Ieee802154EdEventProbeStop,
    Ieee802154ObservedEventState, Ieee802154OperationRxAbortEnableObservation,
    Ieee802154RouteState, Ieee802154RxAbortReasonObservation, Ieee802154ValidationEdDurationState,
    Ieee802154ValidationEventEnableState, run_ieee802154_ed_event_probe,
};
use oer_esp32s31_pac::Ieee802154RxAbortReason;

const CLEAR: Ieee802154ObservedEventState = Ieee802154ObservedEventState::Clear;
const TIMER0: Ieee802154ObservedEventState = Ieee802154ObservedEventState::Timer0Only;
const ED_TIMER: Ieee802154ObservedEventState = Ieee802154ObservedEventState::EdDoneAndTimer0;
const RX_ABORT: Ieee802154ObservedEventState = Ieee802154ObservedEventState::RxAbortOnly;

const fn reset_route() -> Ieee802154RouteState {
    Ieee802154RouteState::ResetDetached
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    EventEnable,
    EnableEvents,
    DisableEvents,
    RxAbortEnable,
    EnableEdAbortReasons,
    DisableRxAbortReasons,
    Route,
    EventStatus,
    RxStatus,
    EdDuration,
    SetEdDurationEight,
    TimerValue,
    TimerThreshold(u32),
    StartTimer,
    StopTimer,
    StartEd,
    StopEd,
    WriteEdDone,
    WriteTimer,
}

struct FakeBackend {
    event_enable: Ieee802154ValidationEventEnableState,
    rx_abort_enable: Ieee802154OperationRxAbortEnableObservation,
    ed_duration: Ieee802154ValidationEdDurationState,
    status_reads: VecDeque<Ieee802154ObservedEventState>,
    route: Ieee802154RouteState,
    timer_running: bool,
    timer_value: u32,
    rx_abort_reason: Ieee802154RxAbortReasonObservation,
    operations: Vec<Operation>,
}

impl FakeBackend {
    fn new(status_reads: &[Ieee802154ObservedEventState]) -> Self {
        Self {
            event_enable: Ieee802154ValidationEventEnableState::AllMasked,
            rx_abort_enable: Ieee802154OperationRxAbortEnableObservation::AllMasked,
            ed_duration: Ieee802154ValidationEdDurationState::Other,
            status_reads: status_reads.iter().copied().collect(),
            route: reset_route(),
            timer_running: false,
            timer_value: 0,
            rx_abort_reason: Ieee802154RxAbortReason::EdStop.into(),
            operations: Vec::new(),
        }
    }

    fn assert_consumed(&self) {
        assert!(
            self.status_reads.is_empty(),
            "unused fake status reads: {:?}",
            self.status_reads
        );
    }
}

impl Ieee802154EdEventProbeBackend for FakeBackend {
    fn event_enable_state(&mut self) -> Ieee802154ValidationEventEnableState {
        self.operations.push(Operation::EventEnable);
        self.event_enable
    }

    fn enable_ed_timer_abort_events(&mut self) {
        self.operations.push(Operation::EnableEvents);
        self.event_enable = Ieee802154ValidationEventEnableState::EdDoneTimer0RxAbortOnly;
    }

    fn disable_all_events(&mut self) {
        self.operations.push(Operation::DisableEvents);
        self.event_enable = Ieee802154ValidationEventEnableState::AllMasked;
    }

    fn rx_abort_enable_state(&mut self) -> Ieee802154OperationRxAbortEnableObservation {
        self.operations.push(Operation::RxAbortEnable);
        self.rx_abort_enable
    }

    fn enable_ed_abort_reasons(&mut self) {
        self.operations.push(Operation::EnableEdAbortReasons);
        self.rx_abort_enable = Ieee802154OperationRxAbortEnableObservation::EdOperationReasonsOnly;
    }

    fn disable_all_rx_abort_reasons(&mut self) {
        self.operations.push(Operation::DisableRxAbortReasons);
        self.rx_abort_enable = Ieee802154OperationRxAbortEnableObservation::AllMasked;
    }

    fn interrupt_route_state(&mut self) -> Ieee802154RouteState {
        self.operations.push(Operation::Route);
        self.route
    }

    fn event_status_state(&mut self) -> Ieee802154ObservedEventState {
        self.operations.push(Operation::EventStatus);
        self.status_reads
            .pop_front()
            .expect("fake must provide every complete status sample")
    }

    fn rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation {
        self.operations.push(Operation::RxStatus);
        self.rx_abort_reason
    }

    fn ed_duration_state(&mut self) -> Ieee802154ValidationEdDurationState {
        self.operations.push(Operation::EdDuration);
        self.ed_duration
    }

    fn set_ed_duration_eight(&mut self) {
        self.operations.push(Operation::SetEdDurationEight);
        self.ed_duration = Ieee802154ValidationEdDurationState::ValidationEight;
    }

    fn timer0_value(&mut self) -> u32 {
        self.operations.push(Operation::TimerValue);
        if self.timer_running {
            self.timer_value = self.timer_value.wrapping_add(1);
        }
        self.timer_value
    }

    fn set_timer0_threshold(&mut self, threshold: u32) {
        self.operations.push(Operation::TimerThreshold(threshold));
    }

    fn start_timer0(&mut self) {
        self.operations.push(Operation::StartTimer);
        self.timer_running = true;
    }

    fn stop_timer0(&mut self) {
        self.operations.push(Operation::StopTimer);
        self.timer_running = false;
    }

    fn start_ed(&mut self) {
        self.operations.push(Operation::StartEd);
    }

    fn stop_operation(&mut self) {
        self.operations.push(Operation::StopEd);
    }

    fn write_ed_done_event(&mut self) {
        self.operations.push(Operation::WriteEdDone);
    }

    fn write_timer0_event(&mut self) {
        self.operations.push(Operation::WriteTimer);
    }
}

fn config(poll_limit: u32) -> Ieee802154EdEventProbeConfig {
    Ieee802154EdEventProbeConfig::new(poll_limit, 37).expect("nonzero bounded test configuration")
}

#[test]
fn complete_trace_selects_ed_done_and_retains_timer0() {
    let mut backend = FakeBackend::new(&[CLEAR, CLEAR, ED_TIMER, TIMER0, CLEAR, CLEAR, CLEAR]);

    let evidence = run_ieee802154_ed_event_probe(&mut backend, config(2));

    assert_eq!(evidence.stop, Ieee802154EdEventProbeStop::Complete);
    assert_eq!(
        evidence.event_enable_active,
        Ieee802154ValidationEventEnableState::EdDoneTimer0RxAbortOnly
    );
    assert_eq!(
        evidence.rx_abort_enable_active,
        Ieee802154OperationRxAbortEnableObservation::EdOperationReasonsOnly
    );
    assert_eq!(
        evidence.rx_abort_enable_after,
        Ieee802154OperationRxAbortEnableObservation::AllMasked
    );
    assert_eq!(
        evidence.ed_duration_active,
        Ieee802154ValidationEdDurationState::ValidationEight
    );
    assert_eq!(evidence.terminal_events, ED_TIMER);
    assert_eq!(evidence.after_ed_done_write_events, TIMER0);
    assert_eq!(evidence.after_timer0_write_events, CLEAR);
    assert_eq!(evidence.observed_events, ED_TIMER);
    assert_eq!(
        evidence.ed_duration_before,
        Ieee802154ValidationEdDurationState::Other
    );
    assert_eq!(
        evidence.ed_duration_after,
        Ieee802154ValidationEdDurationState::ValidationEight
    );
    assert!(!evidence.stop_command_issued);
    assert!(evidence.cleanup_clear);
    assert_eq!(evidence.rx_abort_reason, None);
    assert!(
        backend
            .operations
            .windows(2)
            .any(|operations| { operations == [Operation::StartTimer, Operation::StartEd] })
    );
    assert!(backend.operations.windows(4).any(|operations| {
        operations
            == [
                Operation::DisableEvents,
                Operation::EventEnable,
                Operation::DisableRxAbortReasons,
                Operation::RxAbortEnable,
            ]
    }));
    backend.assert_consumed();
}

#[test]
fn nonzero_entry_abort_mask_performs_no_mutating_operation() {
    let mut backend = FakeBackend::new(&[CLEAR]);
    backend.rx_abort_enable = Ieee802154OperationRxAbortEnableObservation::Unexpected;

    let evidence = run_ieee802154_ed_event_probe(&mut backend, config(1));

    assert_eq!(evidence.stop, Ieee802154EdEventProbeStop::UnsupportedSetup);
    assert_eq!(
        evidence.rx_abort_enable_before,
        Ieee802154OperationRxAbortEnableObservation::Unexpected
    );
    assert!(backend.operations.iter().all(|operation| matches!(
        operation,
        Operation::EventEnable
            | Operation::RxAbortEnable
            | Operation::Route
            | Operation::EventStatus
            | Operation::RxStatus
            | Operation::EdDuration
            | Operation::TimerValue
    )));
    backend.assert_consumed();
}

#[test]
fn selected_ed_done_write_mismatch_fails_closed_without_more_status_writes() {
    let mut backend = FakeBackend::new(&[CLEAR, CLEAR, ED_TIMER, ED_TIMER, ED_TIMER, ED_TIMER]);

    let evidence = run_ieee802154_ed_event_probe(&mut backend, config(1));

    assert_eq!(
        evidence.stop,
        Ieee802154EdEventProbeStop::SelectiveWriteMismatch
    );
    assert_eq!(evidence.after_ed_done_write_events, ED_TIMER);
    assert_eq!(evidence.final_events, ED_TIMER);
    assert!(!evidence.cleanup_clear);
    assert_eq!(
        backend
            .operations
            .iter()
            .filter(|operation| **operation == Operation::WriteEdDone)
            .count(),
        1
    );
    assert!(!backend.operations.contains(&Operation::WriteTimer));
    backend.assert_consumed();
}

#[test]
fn selected_timer_write_mismatch_never_repeats_either_selected_write() {
    let mut backend = FakeBackend::new(&[CLEAR, CLEAR, ED_TIMER, TIMER0, TIMER0, TIMER0, TIMER0]);

    let evidence = run_ieee802154_ed_event_probe(&mut backend, config(1));

    assert_eq!(
        evidence.stop,
        Ieee802154EdEventProbeStop::SelectiveWriteMismatch
    );
    assert_eq!(evidence.after_ed_done_write_events, TIMER0);
    assert_eq!(evidence.after_timer0_write_events, TIMER0);
    assert_eq!(evidence.final_events, TIMER0);
    assert!(!evidence.cleanup_clear);
    assert_eq!(
        backend
            .operations
            .iter()
            .filter(|operation| **operation == Operation::WriteEdDone)
            .count(),
        1
    );
    assert_eq!(
        backend
            .operations
            .iter()
            .filter(|operation| **operation == Operation::WriteTimer)
            .count(),
        1
    );
    backend.assert_consumed();
}

#[test]
fn pair_timeout_preserves_last_and_union_status_and_stops_ed() {
    let mut backend = FakeBackend::new(&[CLEAR, CLEAR, CLEAR, TIMER0, TIMER0, TIMER0]);

    let evidence = run_ieee802154_ed_event_probe(&mut backend, config(2));

    assert_eq!(evidence.stop, Ieee802154EdEventProbeStop::PairLatchTimeout);
    assert_eq!(evidence.terminal_events, TIMER0);
    assert_eq!(evidence.observed_events, TIMER0);
    assert_eq!(evidence.cleanup_pending_events, TIMER0);
    assert_eq!(evidence.final_events, TIMER0);
    assert!(evidence.stop_command_issued);
    assert!(!evidence.cleanup_clear);
    assert!(backend.operations.contains(&Operation::StopEd));
    backend.assert_consumed();
}

#[test]
fn rx_abort_reason_is_retained_and_never_acknowledged() {
    let mut backend = FakeBackend::new(&[CLEAR, CLEAR, RX_ABORT, RX_ABORT, RX_ABORT]);
    backend.rx_abort_reason = Ieee802154RxAbortReason::EdStop.into();

    let evidence = run_ieee802154_ed_event_probe(&mut backend, config(1));

    assert_eq!(evidence.stop, Ieee802154EdEventProbeStop::EdAborted);
    assert_eq!(evidence.terminal_events, RX_ABORT);
    assert_eq!(evidence.observed_events, RX_ABORT);
    assert_eq!(
        evidence.rx_abort_reason,
        Some(Ieee802154RxAbortReason::EdStop.into())
    );
    assert!(!backend.operations.contains(&Operation::WriteEdDone));
    assert!(!backend.operations.contains(&Operation::WriteTimer));
    assert!(!evidence.cleanup_clear);
    backend.assert_consumed();
}
