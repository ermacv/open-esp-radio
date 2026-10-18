use std::{collections::VecDeque, vec, vec::Vec};

use super::{
    Ieee802154EventStatusProbeBackend, Ieee802154EventStatusProbeConfig,
    Ieee802154EventStatusProbeIsolation, Ieee802154EventStatusProbeStop,
    Ieee802154ObservedEventState, Ieee802154RouteState, Ieee802154ValidationEventEnableState,
    run_ieee802154_event_status_probe,
};

const CLEAR: Ieee802154ObservedEventState = Ieee802154ObservedEventState::Clear;
const TIMER0: Ieee802154ObservedEventState = Ieee802154ObservedEventState::Timer0Only;
const TIMER1: Ieee802154ObservedEventState = Ieee802154ObservedEventState::Timer1Only;
const TIMERS: Ieee802154ObservedEventState = Ieee802154ObservedEventState::Timer0AndTimer1;
const UNEXPECTED: Ieee802154ObservedEventState = Ieee802154ObservedEventState::UnexpectedNamed;
const MASKED: Ieee802154ValidationEventEnableState =
    Ieee802154ValidationEventEnableState::AllMasked;
const TIMER_PAIR: Ieee802154ValidationEventEnableState =
    Ieee802154ValidationEventEnableState::TimerPairOnly;
const UNEXPECTED_ENABLE: Ieee802154ValidationEventEnableState =
    Ieee802154ValidationEventEnableState::Unexpected;

const fn reset_route() -> Ieee802154RouteState {
    Ieee802154RouteState::ResetDetached
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    EventEnable,
    EnableTimerEvents,
    DisableAllEvents,
    RouteState,
    EventStatus,
    Timer0Value,
    Timer1Value,
    Thresholds(u32),
    StartTimer0,
    StopTimer0,
    StartTimer1,
    StopTimer1,
    WriteTimer0,
    WriteTimer1,
}

struct FakeBackend {
    event_enable: Ieee802154ValidationEventEnableState,
    enabled_writeback: Ieee802154ValidationEventEnableState,
    disabled_writeback: Ieee802154ValidationEventEnableState,
    route_reads: VecDeque<Ieee802154RouteState>,
    route_default: Ieee802154RouteState,
    status_reads: VecDeque<Ieee802154ObservedEventState>,
    timer0_advances: bool,
    timer1_advances: bool,
    timer0_value: u32,
    timer1_value: u32,
    operations: Vec<Operation>,
}

impl FakeBackend {
    fn new(
        event_enable: Ieee802154ValidationEventEnableState,
        status_reads: &[Ieee802154ObservedEventState],
    ) -> Self {
        Self {
            event_enable,
            enabled_writeback: TIMER_PAIR,
            disabled_writeback: MASKED,
            route_reads: VecDeque::new(),
            route_default: reset_route(),
            status_reads: status_reads.iter().copied().collect(),
            timer0_advances: true,
            timer1_advances: true,
            timer0_value: 0,
            timer1_value: 0,
            operations: Vec::new(),
        }
    }

    fn with_inactive_timers(mut self) -> Self {
        self.timer0_advances = false;
        self.timer1_advances = false;
        self
    }

    fn with_inactive_timer0(mut self) -> Self {
        self.timer0_advances = false;
        self
    }

    fn with_inactive_timer1(mut self) -> Self {
        self.timer1_advances = false;
        self
    }

    fn with_enabled_writeback(mut self, writeback: Ieee802154ValidationEventEnableState) -> Self {
        self.enabled_writeback = writeback;
        self
    }

    fn with_disabled_writeback(mut self, writeback: Ieee802154ValidationEventEnableState) -> Self {
        self.disabled_writeback = writeback;
        self
    }

    fn with_routes(mut self, routes: &[Ieee802154RouteState]) -> Self {
        self.route_reads = routes.iter().copied().collect();
        self
    }

    fn assert_consumed(&self) {
        assert!(
            self.status_reads.is_empty(),
            "unused fake status reads: {:?}",
            self.status_reads
        );
    }
}

impl Ieee802154EventStatusProbeBackend for FakeBackend {
    fn event_enable_state(&mut self) -> Ieee802154ValidationEventEnableState {
        self.operations.push(Operation::EventEnable);
        self.event_enable
    }

    fn enable_timer_events(&mut self) {
        self.operations.push(Operation::EnableTimerEvents);
        self.event_enable = self.enabled_writeback;
    }

    fn disable_all_events(&mut self) {
        self.operations.push(Operation::DisableAllEvents);
        self.event_enable = self.disabled_writeback;
    }

    fn interrupt_route_state(&mut self) -> Ieee802154RouteState {
        self.operations.push(Operation::RouteState);
        self.route_reads.pop_front().unwrap_or(self.route_default)
    }

    fn event_status_state(&mut self) -> Ieee802154ObservedEventState {
        self.operations.push(Operation::EventStatus);
        self.status_reads
            .pop_front()
            .expect("fake must provide every requested semantic status sample")
    }

    fn timer0_value(&mut self) -> u32 {
        self.operations.push(Operation::Timer0Value);
        self.timer0_value
    }

    fn timer1_value(&mut self) -> u32 {
        self.operations.push(Operation::Timer1Value);
        self.timer1_value
    }

    fn set_timer_thresholds(&mut self, threshold: u32) {
        self.operations.push(Operation::Thresholds(threshold));
    }

    fn start_timer0(&mut self) {
        self.operations.push(Operation::StartTimer0);
        if self.timer0_advances {
            self.timer0_value = self.timer0_value.wrapping_add(1);
        }
    }

    fn stop_timer0(&mut self) {
        self.operations.push(Operation::StopTimer0);
    }

    fn start_timer1(&mut self) {
        self.operations.push(Operation::StartTimer1);
        if self.timer1_advances {
            self.timer1_value = self.timer1_value.wrapping_add(1);
        }
    }

    fn stop_timer1(&mut self) {
        self.operations.push(Operation::StopTimer1);
    }

    fn write_timer0_event(&mut self) {
        self.operations.push(Operation::WriteTimer0);
    }

    fn write_timer1_event(&mut self) {
        self.operations.push(Operation::WriteTimer1);
    }
}

fn config(poll_limit: u32) -> Ieee802154EventStatusProbeConfig {
    Ieee802154EventStatusProbeConfig::new(poll_limit, 37).expect("nonzero finite test config")
}

#[test]
fn config_rejects_degenerate_inputs_and_preserves_bounds() {
    assert_eq!(Ieee802154EventStatusProbeConfig::new(0, 1), None);
    assert_eq!(Ieee802154EventStatusProbeConfig::new(1, 0), None);
    assert_eq!(
        Ieee802154EventStatusProbeConfig::new(
            Ieee802154EventStatusProbeConfig::MAX_POLL_LIMIT + 1,
            1,
        ),
        None
    );
    assert_eq!(
        Ieee802154EventStatusProbeConfig::new(
            1,
            Ieee802154EventStatusProbeConfig::MAX_TIMER_THRESHOLD + 1,
        ),
        None
    );
    let maximum = Ieee802154EventStatusProbeConfig::new(
        Ieee802154EventStatusProbeConfig::MAX_POLL_LIMIT,
        Ieee802154EventStatusProbeConfig::MAX_TIMER_THRESHOLD,
    )
    .expect("wire maxima are supported");
    assert_eq!(
        maximum.poll_limit(),
        Ieee802154EventStatusProbeConfig::MAX_POLL_LIMIT
    );
    assert_eq!(
        maximum.timer_threshold(),
        Ieee802154EventStatusProbeConfig::MAX_TIMER_THRESHOLD
    );
}

#[test]
fn reset_isolation_capability_is_process_lifetime_unique() {
    {
        let _isolation = Ieee802154EventStatusProbeIsolation::claim_for_reset_isolated_image()
            .expect("first validation transaction owns reset isolation");
        assert!(Ieee802154EventStatusProbeIsolation::claim_for_reset_isolated_image().is_none());
    }
    assert!(Ieee802154EventStatusProbeIsolation::claim_for_reset_isolated_image().is_none());
}

#[test]
fn complete_trace_orders_detached_route_enabled_stimulus_and_masked_cleanup() {
    let mut backend = FakeBackend::new(
        MASKED,
        &[
            CLEAR, CLEAR, TIMERS, TIMER1, CLEAR, TIMER0, TIMERS, TIMER1, CLEAR, CLEAR,
        ],
    );

    let evidence = run_ieee802154_event_status_probe(&mut backend, config(2));

    assert_eq!(evidence.stop, Ieee802154EventStatusProbeStop::Complete);
    assert_eq!(evidence.event_enable_before, MASKED);
    assert_eq!(evidence.event_enable_active, TIMER_PAIR);
    assert_eq!(evidence.event_enable_after, MASKED);
    assert_eq!(evidence.post_enable_events, CLEAR);
    assert_eq!(evidence.timer0_value_before_start, 0);
    assert_eq!(evidence.timer0_value_min, 0);
    assert_eq!(evidence.timer0_value_max, 1);
    assert_eq!(evidence.timer0_value_after_stop, 1);
    assert_eq!(evidence.timer1_value_before_start, 0);
    assert_eq!(evidence.timer1_value_min, 0);
    assert_eq!(evidence.timer1_value_max, 1);
    assert_eq!(evidence.timer1_value_after_stop, 1);
    assert_eq!(evidence.reset_events, CLEAR);
    assert_eq!(evidence.dual_observed_events, TIMERS);
    assert_eq!(evidence.dual_latched_events, TIMERS);
    assert_eq!(evidence.after_timer0_ack_events, TIMER1);
    assert_eq!(evidence.after_timer1_ack_events, CLEAR);
    assert_eq!(evidence.distinct_snapshot_events, TIMER0);
    assert_eq!(evidence.distinct_before_ack_events, TIMERS);
    assert_eq!(evidence.distinct_after_ack_events, TIMER1);
    assert_eq!(evidence.cleanup_pending_events, CLEAR);
    assert_eq!(evidence.final_events, CLEAR);
    assert_eq!(
        backend.operations,
        vec![
            Operation::EventEnable,
            Operation::RouteState,
            Operation::EventStatus,
            Operation::Thresholds(37),
            Operation::EnableTimerEvents,
            Operation::EventEnable,
            Operation::EventStatus,
            Operation::RouteState,
            Operation::Timer0Value,
            Operation::Timer1Value,
            Operation::StartTimer0,
            Operation::StartTimer1,
            Operation::Timer0Value,
            Operation::Timer1Value,
            Operation::EventStatus,
            Operation::StopTimer0,
            Operation::StopTimer1,
            Operation::Timer0Value,
            Operation::Timer1Value,
            Operation::WriteTimer0,
            Operation::EventStatus,
            Operation::WriteTimer1,
            Operation::EventStatus,
            Operation::Thresholds(37),
            Operation::StartTimer0,
            Operation::EventStatus,
            Operation::StopTimer0,
            Operation::StartTimer1,
            Operation::EventStatus,
            Operation::StopTimer1,
            Operation::WriteTimer0,
            Operation::EventStatus,
            Operation::StopTimer0,
            Operation::StopTimer1,
            Operation::DisableAllEvents,
            Operation::EventEnable,
            Operation::EventStatus,
            Operation::WriteTimer0,
            Operation::WriteTimer1,
            Operation::EventStatus,
            Operation::RouteState,
        ]
    );
    backend.assert_consumed();
}

#[test]
fn unsupported_setup_never_programs_or_writes_the_mac() {
    let mut backend = FakeBackend::new(UNEXPECTED_ENABLE, &[UNEXPECTED]);
    let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::UnsupportedSetup
    );
    assert_eq!(evidence.final_events, UNEXPECTED);
    assert_eq!(
        backend.operations,
        [
            Operation::EventEnable,
            Operation::EventStatus,
            Operation::EventEnable,
        ]
    );
    backend.assert_consumed();
}

#[test]
fn reset_not_clear_returns_without_any_register_write() {
    let mut backend = FakeBackend::new(MASKED, &[TIMER0, TIMER0]);
    let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

    assert_eq!(evidence.stop, Ieee802154EventStatusProbeStop::ResetNotClear);
    assert_eq!(evidence.reset_events, TIMER0);
    assert_eq!(evidence.final_events, TIMER0);
    assert!(backend.operations.iter().all(|operation| {
        !matches!(
            operation,
            Operation::Thresholds(_)
                | Operation::EnableTimerEvents
                | Operation::DisableAllEvents
                | Operation::StartTimer0
                | Operation::StartTimer1
                | Operation::StopTimer0
                | Operation::StopTimer1
                | Operation::WriteTimer0
                | Operation::WriteTimer1
        )
    }));
    backend.assert_consumed();
}

#[test]
fn active_route_at_entry_returns_without_any_register_write() {
    let active = Ieee802154RouteState::DestinationAssigned;
    let mut backend = FakeBackend::new(MASKED, &[CLEAR]).with_routes(&[active]);

    let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::RouteNotQuiesced
    );
    assert!(backend.operations.iter().all(|operation| {
        matches!(
            operation,
            Operation::EventEnable | Operation::EventStatus | Operation::RouteState
        )
    }));
    backend.assert_consumed();
}

#[test]
fn enable_readback_mismatch_restores_zero_before_any_start_or_status_write() {
    let mut backend =
        FakeBackend::new(MASKED, &[CLEAR, TIMER0, CLEAR]).with_enabled_writeback(UNEXPECTED_ENABLE);

    let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::EventEnableReadbackMismatch
    );
    assert_eq!(evidence.event_enable_active, UNEXPECTED_ENABLE);
    assert_eq!(evidence.event_enable_after, MASKED);
    assert!(!backend.operations.contains(&Operation::StartTimer0));
    assert!(!backend.operations.contains(&Operation::StartTimer1));
    assert!(!backend.operations.contains(&Operation::WriteTimer0));
    assert!(!backend.operations.contains(&Operation::WriteTimer1));
    backend.assert_consumed();
}

#[test]
fn status_revealed_by_enable_is_not_counted_as_fresh_stimulus() {
    let mut backend = FakeBackend::new(MASKED, &[CLEAR, TIMER0, CLEAR, CLEAR]);

    let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::PostEnableStatusNotClear
    );
    assert_eq!(evidence.post_enable_events, TIMER0);
    assert_eq!(evidence.dual_observed_events, CLEAR);
    assert!(!backend.operations.contains(&Operation::StartTimer0));
    assert!(!backend.operations.contains(&Operation::StartTimer1));
    let disable = backend
        .operations
        .iter()
        .position(|operation| *operation == Operation::DisableAllEvents)
        .expect("cleanup masks delivery");
    let write = backend
        .operations
        .iter()
        .position(|operation| *operation == Operation::WriteTimer0)
        .expect("retained revealed bit is cleaned best-effort");
    assert!(disable < write);
    backend.assert_consumed();
}

#[test]
fn route_change_while_enabled_restores_mask_before_any_timer_start() {
    let active = Ieee802154RouteState::PassLevelConfigured;
    let mut backend = FakeBackend::new(MASKED, &[CLEAR, CLEAR, TIMER0, CLEAR]).with_routes(&[
        reset_route(),
        active,
        reset_route(),
    ]);

    let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::RouteNotQuiesced
    );
    assert_eq!(evidence.event_enable_after, MASKED);
    assert!(!backend.operations.contains(&Operation::StartTimer0));
    assert!(!backend.operations.contains(&Operation::StartTimer1));
    assert!(!backend.operations.contains(&Operation::WriteTimer0));
    assert!(!backend.operations.contains(&Operation::WriteTimer1));
    backend.assert_consumed();
}

#[test]
fn dual_latch_timeout_retains_the_last_bounded_sample() {
    let mut backend = FakeBackend::new(MASKED, &[CLEAR, CLEAR, CLEAR, TIMER0, TIMER0, CLEAR]);
    let evidence = run_ieee802154_event_status_probe(&mut backend, config(2));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::DualLatchTimeout
    );
    assert_eq!(evidence.dual_latched_events, TIMER0);
    assert_eq!(evidence.dual_observed_events, TIMER0);
    assert_eq!(evidence.cleanup_pending_events, TIMER0);
    assert_eq!(evidence.final_events, CLEAR);
    backend.assert_consumed();
}

#[test]
fn dual_wait_union_retains_a_transient_separately_from_the_last_sample() {
    let mut backend = FakeBackend::new(MASKED, &[CLEAR, CLEAR, TIMER0, CLEAR, CLEAR, CLEAR]);
    let evidence = run_ieee802154_event_status_probe(&mut backend, config(2));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::DualLatchTimeout
    );
    assert_eq!(evidence.dual_observed_events, TIMER0);
    assert_eq!(evidence.dual_latched_events, CLEAR);
    assert_eq!(evidence.cleanup_pending_events, CLEAR);
    assert_eq!(evidence.final_events, CLEAR);
    assert!(backend.operations.contains(&Operation::WriteTimer0));
    assert!(!backend.operations.contains(&Operation::WriteTimer1));
    backend.assert_consumed();
}

#[test]
fn inactive_timer_counters_are_distinguished_from_masked_status() {
    let mut backend = FakeBackend::new(MASKED, &[CLEAR; 6]).with_inactive_timers();
    let evidence = run_ieee802154_event_status_probe(&mut backend, config(2));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::TimerActivityTimeout
    );
    assert_eq!(
        evidence.timer0_value_before_start,
        evidence.timer0_value_min
    );
    assert_eq!(
        evidence.timer1_value_before_start,
        evidence.timer1_value_max
    );
    assert_eq!(evidence.final_events, CLEAR);
    backend.assert_consumed();
}

#[test]
fn either_inactive_timer_is_fail_closed_with_individual_extrema() {
    let mut timer0_inactive = FakeBackend::new(MASKED, &[CLEAR; 6]).with_inactive_timer0();
    let evidence = run_ieee802154_event_status_probe(&mut timer0_inactive, config(2));
    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::TimerActivityTimeout
    );
    assert_eq!(evidence.timer0_value_min, evidence.timer0_value_max);
    assert!(evidence.timer1_value_min < evidence.timer1_value_max);
    timer0_inactive.assert_consumed();

    let mut timer1_inactive = FakeBackend::new(MASKED, &[CLEAR; 6]).with_inactive_timer1();
    let evidence = run_ieee802154_event_status_probe(&mut timer1_inactive, config(2));
    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::TimerActivityTimeout
    );
    assert!(evidence.timer0_value_min < evidence.timer0_value_max);
    assert_eq!(evidence.timer1_value_min, evidence.timer1_value_max);
    timer1_inactive.assert_consumed();
}

#[test]
fn first_selected_write_mismatch_is_fail_closed() {
    let mut backend = FakeBackend::new(MASKED, &[CLEAR, CLEAR, TIMERS, TIMERS, TIMERS, CLEAR]);
    let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch
    );
    assert_eq!(evidence.after_timer0_ack_events, TIMERS);
    assert_eq!(evidence.final_events, CLEAR);
    backend.assert_consumed();
}

#[test]
fn second_selected_write_mismatch_is_fail_closed() {
    let mut backend = FakeBackend::new(
        MASKED,
        &[CLEAR, CLEAR, TIMERS, TIMER1, TIMER1, TIMER1, CLEAR],
    );
    let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch
    );
    assert_eq!(evidence.after_timer1_ack_events, TIMER1);
    assert_eq!(evidence.final_events, CLEAR);
    backend.assert_consumed();
}

#[test]
fn distinct_first_latch_timeout_retains_the_last_sample() {
    let mut backend = FakeBackend::new(
        MASKED,
        &[
            CLEAR, CLEAR, TIMERS, TIMER1, CLEAR, CLEAR, CLEAR, CLEAR, CLEAR,
        ],
    );
    let evidence = run_ieee802154_event_status_probe(&mut backend, config(2));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::DistinctFirstLatchTimeout
    );
    assert_eq!(evidence.distinct_snapshot_events, CLEAR);
    assert_eq!(evidence.final_events, CLEAR);
    backend.assert_consumed();
}

#[test]
fn distinct_second_latch_timeout_retains_timer_zero() {
    let mut backend = FakeBackend::new(
        MASKED,
        &[
            CLEAR, CLEAR, TIMERS, TIMER1, CLEAR, TIMER0, TIMER0, TIMER0, TIMER0, CLEAR,
        ],
    );
    let evidence = run_ieee802154_event_status_probe(&mut backend, config(2));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::DistinctSecondLatchTimeout
    );
    assert_eq!(evidence.distinct_before_ack_events, TIMER0);
    assert_eq!(evidence.final_events, CLEAR);
    backend.assert_consumed();
}

#[test]
fn distinct_selected_write_mismatch_is_fail_closed() {
    let mut backend = FakeBackend::new(
        MASKED,
        &[
            CLEAR, CLEAR, TIMERS, TIMER1, CLEAR, TIMER0, TIMERS, TIMERS, TIMERS, CLEAR,
        ],
    );
    let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch
    );
    assert_eq!(evidence.distinct_after_ack_events, TIMERS);
    assert_eq!(evidence.final_events, CLEAR);
    backend.assert_consumed();
}

#[test]
fn failed_mask_restore_skips_selected_cleanup_and_overrides_the_probe_stop() {
    let mut backend = FakeBackend::new(MASKED, &[CLEAR; 5]).with_disabled_writeback(TIMER_PAIR);
    let evidence = run_ieee802154_event_status_probe(&mut backend, config(1));

    assert_eq!(
        evidence.stop,
        Ieee802154EventStatusProbeStop::CleanupNotClear
    );
    assert_eq!(evidence.event_enable_after, TIMER_PAIR);
    assert!(!backend.operations.contains(&Operation::WriteTimer0));
    assert!(!backend.operations.contains(&Operation::WriteTimer1));
    backend.assert_consumed();
}
