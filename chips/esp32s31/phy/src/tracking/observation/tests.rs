use super::*;

#[test]
fn poll_stamp_includes_the_completed_body_and_preserves_the_ready_endpoint() {
    use oer_time::{Clock, Duration};
    let clock = oer_time_virtual::VirtualClock::<1>::new();
    let started = clock.now();
    clock.advance(Duration::from_micros(10)).expect("time fits");
    let deadline = clock.now();
    clock.advance(Duration::from_micros(35)).expect("time fits");
    let resumed = clock.now();
    // Work continues after readiness and until final counter accounting.
    clock.advance(Duration::from_micros(35)).expect("time fits");
    let mut timing = EngineTiming::default();
    let finished = timing.poll_finished(
        &clock,
        started,
        true,
        Some((deadline, resumed)),
        Some(Duration::from_micros(47)),
        None,
    );
    assert_eq!(finished, clock.now());
    assert_eq!(
        (
            timing.polls,
            timing.pending,
            timing.resumes,
            timing.ready_edges
        ),
        (1, 1, 1, 1)
    );
    assert_eq!((timing.total_micros, timing.maximum_micros), (80, 80));
    assert_eq!(
        (timing.lateness_micros, timing.maximum_lateness_micros),
        (35, 35)
    );
    assert_eq!(timing.maximum_ready_micros, 47);
    assert!(!timing.invalid);
}

#[test]
fn coarse_rx_observation_contains_only_phase_boundaries() {
    for operation in [
        Operation::RxGainControlPhase,
        Operation::RxGainDcPhase,
        Operation::RxGainPublishPhase,
    ] {
        assert!(operation.is_rx_gain_region(), "{operation:?}");
    }
}

#[test]
fn calibration_names_remain_correlated_with_the_poll_and_atom_they_measure() {
    let mut execution = RxGainExecution::default();
    let mut timing = EngineTiming::default();
    for (action, poll, atom) in [(0x95, 48, 40), (24, 44, 41), (21, 48, 39)] {
        let mut sample = EngineTiming::default();
        sample.maximum_micros = poll;
        sample.maximum_ready_micros = atom;
        timing.merge(&sample);
        execution.observe_engine(&timing, action);
    }
    assert_eq!(execution.maximum_engine_micros, [48, 41]);
    assert_eq!(execution.maximum_engine_actions, [0x95, 24]);
    assert_eq!(
        calibration::name(false, execution.maximum_engine_actions[0]),
        "DcIq.Configure"
    );
    assert_eq!(
        calibration::name(false, execution.maximum_engine_actions[1]),
        "DcIq.AwaitReadinessEdge"
    );
}

#[test]
fn action_statistics_merge_preserves_counters_and_marks_overflow() {
    let mut total = EngineTiming::default();
    let action = EngineTiming {
        polls: 3,
        pending: 2,
        ready_edges: 1,
        total_micros: 45,
        maximum_micros: 25,
        maximum_ready_micros: 20,
        ..EngineTiming::default()
    };
    total.merge(&action);
    total.merge(&action);
    assert_eq!(
        (
            total.polls,
            total.pending,
            total.ready_edges,
            total.total_micros
        ),
        (6, 4, 2, 90)
    );
    assert_eq!((total.maximum_micros, total.maximum_ready_micros), (25, 20));
    assert!(!total.invalid);
    total.merge(&EngineTiming {
        polls: u16::MAX,
        ..EngineTiming::default()
    });
    assert!(total.invalid, "an incomplete aggregate cannot qualify");
}

#[test]
fn rx_execution_summary_is_single_and_scoped_to_the_active_child() {
    let execution = RxGainExecution {
        maximum_parts_micros: [0; 3],
        maximum_parts_actions: [0; 3],
        maximum_engine_micros: [0; 2],
        maximum_engine_actions: [0; 2],
        i2c_poll_micros: [[0; 2]; 2],
        i2c_samples: [0; 2],
        quality: None,
        minimum_searches: 121,
        minimum_operations: 191,
        outer_operations: 58,
        settle_1us: 700,
        settle_2us: 1,
        settle_10us: 121,
    };
    let mut outside = Recorder::default();
    outside.observe_rx_gain_execution(execution);
    assert!(outside.report().invalid);

    let mut recorder = Recorder::default();
    recorder.observe(Operation::RxGain, Event::Started, 1);
    recorder.observe_rx_gain_execution(execution);
    recorder.observe(Operation::RxGain, Event::Completed, 2);
    assert_eq!(recorder.report().rx_gain_execution, Some(execution));
    assert!(!recorder.report().invalid);
    recorder.observe_rx_gain_execution(execution);
    assert!(recorder.report().invalid);
}

#[test]
fn temperature_outcome_is_single_and_scoped_to_the_active_child() {
    let outcome = crate::analog::temperature::PhyTemperatureOutcome {
        temperature: 61,
        sensor_index: 2,
        next_dac: 15,
    };
    let mut outside = Recorder::default();
    outside.observe_temperature(outcome);
    assert!(outside.report().invalid);

    let mut recorder = Recorder::default();
    recorder.observe(Operation::Temperature, Event::Started, 1);
    recorder.observe_temperature(outcome);
    recorder.observe(Operation::Temperature, Event::Completed, 2);
    assert_eq!(recorder.report().temperature, Some(outcome));
    assert!(!recorder.report().invalid);
    recorder.observe_temperature(outcome);
    assert!(recorder.report().invalid);
}

#[test]
fn nested_work_keeps_parent_inclusive_time_and_failed_attempts() {
    let mut recorder = Recorder::default();
    recorder.observe(Operation::Calibration, Event::Started, 100);
    recorder.observe(Operation::Dcode, Event::Started, 110);
    recorder.observe(Operation::Dcode, Event::Completed, 140);
    recorder.observe(Operation::RxGain, Event::Started, 145);
    recorder.observe(Operation::RxGain, Event::Failed, 160);
    recorder.observe(Operation::Calibration, Event::Failed, 170);
    let report = recorder.report();
    assert!(!report.invalid);
    assert_eq!(
        report.timing(Operation::Calibration),
        Timing {
            started: 1,
            failed: 1,
            elapsed_micros: 70,
            maximum_micros: 70,
            ..Timing::default()
        }
    );
    assert_eq!(report.timing(Operation::Dcode).elapsed_micros, 30);
    assert_eq!(report.timing(Operation::RxGain).failed, 1);
    assert_eq!(report.timing(Operation::Temperature), Timing::default());
}

#[test]
fn interrupted_work_does_not_become_completed_or_acquire_an_invented_duration() {
    let mut recorder = Recorder::default();
    recorder.observe(Operation::TxDcPwdet, Event::Started, 10);
    assert_eq!(
        recorder.report().timing(Operation::TxDcPwdet),
        Timing {
            started: 1,
            ..Timing::default()
        }
    );
}

#[test]
fn repetitions_accumulate_but_clock_reversal_and_overflow_invalidate() {
    let mut recorder = Recorder::default();
    for (start, end) in [(10, 20), (30, 45)] {
        recorder.observe(Operation::ForceTxRx, Event::Started, start);
        recorder.observe(Operation::ForceTxRx, Event::Completed, end);
    }
    assert_eq!(
        recorder.report().timing(Operation::ForceTxRx),
        Timing {
            started: 2,
            completed: 2,
            failed: 0,
            elapsed_micros: 25,
            maximum_micros: 15,
        }
    );
    recorder.observe(Operation::Temperature, Event::Started, 50);
    recorder.observe(Operation::Temperature, Event::Completed, 49);
    assert!(recorder.report().invalid);
    assert_eq!(
        recorder
            .report()
            .timing(Operation::Temperature)
            .elapsed_micros,
        0
    );
    let mut recorder = Recorder::default();
    recorder.observe(Operation::Dcode, Event::Started, 0);
    recorder.observe(Operation::Dcode, Event::Completed, u64::from(u32::MAX) + 1);
    assert!(recorder.report().invalid);
}

#[test]
fn duplicate_and_unmatched_observations_fail_closed() {
    let mut recorder = Recorder::default();
    recorder.observe(Operation::Rfpll, Event::Started, 10);
    recorder.observe(Operation::Rfpll, Event::Started, 20);
    recorder.observe(Operation::Rfpll, Event::Completed, 30);
    assert!(recorder.report().invalid);
    assert_eq!(
        recorder.report().timing(Operation::Rfpll).elapsed_micros,
        20
    );
    let mut recorder = Recorder::default();
    recorder.observe(Operation::Rfpll, Event::Completed, 30);
    assert!(recorder.report().invalid);
    assert_eq!(recorder.report().timing(Operation::Rfpll).completed, 0);
}

#[test]
fn polls_separate_suspended_time_without_changing_future_progress() {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct WakeCounter(AtomicUsize);
    impl std::task::Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let counter = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let waker = Waker::from(counter.clone());
    let mut cx = Context::from_waker(&waker);
    let mut attempts = 0;
    let future = core::future::poll_fn(|cx| {
        attempts += 1;
        if attempts == 1 {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(42)
        }
    });
    let mut recorder = Recorder::default();
    let operation = Operation::RxGain;
    recorder.observe(operation, Event::Started, 100);
    let mut times = [110, 120, 200, 205].into_iter();
    {
        let mut observed = core::pin::pin!(observe_polls(future, |event| {
            recorder.observe(operation, event, times.next().unwrap());
        }));
        assert!(observed.as_mut().poll(&mut cx).is_pending());
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
        assert_eq!(observed.as_mut().poll(&mut cx), Poll::Ready(42));
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
    }
    recorder.observe(operation, Event::Completed, 210);
    let report = recorder.report();
    assert!(!report.invalid);
    assert_eq!(report.timing(operation).elapsed_micros, 110);
    assert_eq!(
        report.poll_timing(operation),
        Some(PollTiming {
            polls: 2,
            pending: 1,
            suspended_micros: 80,
            maximum_suspension_micros: 80,
            elapsed_micros: 15,
            maximum_micros: 10,
        })
    );
    assert_eq!(attempts, 2);
}

#[test]
fn malformed_poll_intervals_and_cancellation_are_not_success() {
    for events in [
        &[Event::PollPending][..],
        &[Event::PollStarted, Event::PollStarted],
        &[Event::PollStarted, Event::Completed],
    ] {
        let mut recorder = Recorder::default();
        recorder.observe(Operation::Dcode, Event::Started, 0);
        for (now, event) in events.iter().enumerate() {
            recorder.observe(Operation::Dcode, *event, now as u64 + 1);
        }
        assert!(recorder.report().invalid);
    }
    let mut recorder = Recorder::default();
    recorder.observe(Operation::Dcode, Event::Started, 0);
    recorder.observe(Operation::Dcode, Event::PollStarted, 1);
    recorder.observe(Operation::Dcode, Event::PollPending, 2);
    // Dropping a pending operation preserves a closed poll, but not completion.
    assert_eq!(
        recorder
            .report()
            .poll_timing(Operation::Dcode)
            .unwrap()
            .polls,
        1
    );
    assert_eq!(recorder.report().timing(Operation::Dcode).completed, 0);
}

#[test]
fn cancellation_drops_a_pinned_child_once_without_fabricating_completion() {
    use core::{
        cell::Cell,
        future::Future,
        marker::PhantomPinned,
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    struct Child<'a> {
        polled: Cell<bool>,
        drops: &'a Cell<u8>,
        _pin: PhantomPinned,
    }
    impl Future for Child<'_> {
        type Output = ();
        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            self.as_ref().get_ref().polled.set(true);
            Poll::Pending
        }
    }
    impl Drop for Child<'_> {
        fn drop(&mut self) {
            assert!(self.polled.get());
            self.drops.set(self.drops.get() + 1);
        }
    }
    let drops = Cell::new(0);
    let mut events = std::vec::Vec::new();
    {
        let child = Child {
            polled: Cell::new(false),
            drops: &drops,
            _pin: PhantomPinned,
        };
        let mut observed = core::pin::pin!(observe_polls(child, |event| events.push(event)));
        assert!(
            observed
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert_eq!(drops.get(), 0);
    }
    assert_eq!(drops.get(), 1);
    assert_eq!(events, [Event::PollStarted, Event::PollPending]);
}

#[test]
fn ready_cannot_be_polled_again_and_pending_cannot_be_terminal() {
    for terminal in [Event::PollReady, Event::PollPending] {
        let mut recorder = Recorder::default();
        recorder.observe(Operation::Dcode, Event::Started, 0);
        recorder.observe(Operation::Dcode, Event::PollStarted, 1);
        recorder.observe(Operation::Dcode, terminal, 2);
        recorder.observe(
            Operation::Dcode,
            if terminal == Event::PollReady {
                Event::PollStarted
            } else {
                Event::Completed
            },
            3,
        );
        assert!(recorder.report().invalid);
    }
}

#[test]
fn gaps_accumulate_across_pending_polls_but_not_between_invocations() {
    let mut recorder = Recorder::default();
    for offset in [0, 1000] {
        for (event, time) in [
            (Event::Started, 0),
            (Event::PollStarted, 1),
            (Event::PollPending, 4),
            (Event::PollStarted, 14),
            (Event::PollPending, 16),
            (Event::PollStarted, 36),
            (Event::PollReady, 40),
            (Event::Completed, 41),
        ] {
            recorder.observe(Operation::RxGain, event, offset + time);
        }
    }
    let report = recorder.report();
    assert!(!report.invalid);
    let polls = report.poll_timing(Operation::RxGain).unwrap();
    assert_eq!(polls.polls, 6);
    assert_eq!(polls.pending, 4);
    assert_eq!(polls.elapsed_micros, 18);
    assert_eq!(polls.suspended_micros, 60);
    assert_eq!(polls.maximum_suspension_micros, 20);
}

#[test]
fn tx_waits_and_sar_samples_require_an_active_tx_calibration() {
    use wait::{Kind, tx::Scope};
    let mut recorder = Recorder::default();
    recorder.observe_tx_sar_ready(true);
    assert!(recorder.report().invalid);
    let mut recorder = Recorder::default();
    recorder.observe_tx_wait(
        Scope::Tone,
        Kind::Settle,
        wait::Event::Started {
            requested_micros: 1,
        },
    );
    assert!(recorder.report().invalid);
    let mut recorder = Recorder::default();
    recorder.observe(Operation::TxDcPwdet, Event::Started, 0);
    recorder.observe_tx_wait(
        Scope::Tone,
        Kind::Settle,
        wait::Event::Started {
            requested_micros: 1,
        },
    );
    assert!(recorder.report().invalid); // In-flight evidence is incomplete.
    recorder.observe_tx_wait(
        Scope::Tone,
        Kind::Settle,
        wait::Event::Completed {
            elapsed_micros: 3,
            lateness_micros: 2,
        },
    );
    recorder.observe_tx_sar_ready(true);
    recorder.observe(Operation::TxDcPwdet, Event::Completed, 10);
    assert!(!recorder.report().invalid);
    assert_eq!(recorder.report().tx_waits.tone.elapsed_micros, 3);
    assert_eq!(recorder.report().tx_waits.sar_ready, 1);
    assert_eq!(recorder.report().dcode_waits, wait::Report::default());
    recorder.observe_tx_sar_ready(false);
    assert!(recorder.report().invalid);
}

#[test]
fn rfpll_terminal_detail_is_scoped_to_one_active_operation() {
    use crate::tracking::rfpll::{Observation, thermal};
    let value = Observation {
        sample_age_micros: Some(30),
        request: thermal::Request {
            current_temperature: 10,
            reference_temperature: 10,
            current_channel: 13,
            threshold_override: None,
        },
        outcome: thermal::Outcome {
            reference_temperature: 10,
            correction: None,
        },
    };
    let mut outside = Recorder::default();
    outside.observe_rfpll(value);
    assert!(outside.report().invalid);
    let mut recorder = Recorder::default();
    recorder.observe(Operation::Rfpll, Event::Started, 1);
    recorder.observe_rfpll(value);
    recorder.observe(Operation::Rfpll, Event::Completed, 2);
    let report = recorder.report();
    assert!(!report.invalid);
    assert_eq!(report.rfpll, Some(value));
    recorder.observe_rfpll(value);
    assert!(recorder.report().invalid);
    let mut duplicate = Recorder::default();
    duplicate.observe(Operation::Rfpll, Event::Started, 1);
    duplicate.observe_rfpll(value);
    duplicate.observe_rfpll(value);
    assert!(duplicate.report().invalid);
}

/// The grant-protect request and the Wi-Fi baseband-off window are timed as
/// held intervals: their longest hold is the RF time tracking takes from the
/// other protocols, separate from the operations nested inside them.
#[test]
fn held_rf_intervals_record_their_longest_hold() {
    let mut recorder = Recorder::default();
    for (start, end) in [(100, 1_100), (2_000, 2_400)] {
        recorder.observe(Operation::GrantProtectCommon, Event::Started, start);
        recorder.observe(Operation::WifiBasebandOff, Event::Started, start + 10);
        recorder.observe(Operation::WifiBasebandOff, Event::Completed, end - 10);
        recorder.observe(Operation::GrantProtectCommon, Event::Completed, end);
    }
    let report = recorder.report();
    assert!(!report.invalid);
    let grant = report.timing(Operation::GrantProtectCommon);
    assert_eq!((grant.completed, grant.maximum_micros), (2, 1_000));
    assert_eq!(grant.elapsed_micros, 1_400);
    let baseband = report.timing(Operation::WifiBasebandOff);
    assert_eq!((baseband.completed, baseband.maximum_micros), (2, 980));
}

/// A release without a set is a malformed observation.
#[test]
fn a_release_without_a_set_invalidates_the_report() {
    let mut recorder = Recorder::default();
    recorder.observe(Operation::GrantProtectCommon, Event::Completed, 10);
    assert!(recorder.report().invalid);
}

#[test]
fn preparation_trace_reports_truncation_without_losing_the_failure() {
    use oer_time::Clock;
    let clock = oer_time_virtual::VirtualClock::<1>::new();
    let mut trace = ReadinessTrace::default();
    trace.command_started(clock.now());
    for index in 0..21 {
        clock
            .advance(oer_time::Duration::from_micros(1))
            .expect("time fits");
        trace.checked(clock.now(), index == 0, true, None);
    }
    trace.checked(clock.now(), false, false, Some((0, 19)));
    assert_eq!(trace.not_ready_samples().len(), 16);
    assert!(!trace.complete());
    assert_eq!(trace.failure_busy, Some(0));
    assert_eq!(trace.failure_elapsed_micros, 21);
    assert_eq!(trace.failure_lateness_micros, 19);
}

#[test]
fn rfpll_command_maxima_keep_the_breakdown_of_the_same_poll() {
    use super::{RfpllCommand, RfpllCommandSample, RfpllEngineTrace};
    use oer_time::{Clock, Duration};
    use oer_time_virtual::VirtualClock;
    let clock = VirtualClock::<1>::new();
    let mut trace = RfpllEngineTrace::default();
    for (command, poll, publication, readiness) in [
        (RfpllCommand::CapLow, 45, 12, 8),
        (RfpllCommand::CapLow, 40, 20, 9),
        (RfpllCommand::Status, 30, 10, 5),
    ] {
        let start = clock.now();
        clock
            .advance(Duration::from_micros(poll))
            .expect("time fits");
        let mut timing = super::EngineTiming::default();
        timing.poll_finished(
            &clock,
            start,
            true,
            None,
            None,
            Some((
                &mut trace,
                RfpllCommandSample {
                    command,
                    publication_micros: publication,
                    readiness_micros: readiness,
                    invalid: false,
                },
            )),
        );
    }
    let cap = trace.commands[RfpllCommand::CapLow as usize];
    assert_eq!(
        (
            cap.poll_micros,
            cap.publication_micros,
            cap.readiness_micros
        ),
        (45, 12, 8)
    );
    assert_eq!(
        trace.commands[RfpllCommand::Status as usize].poll_micros,
        30
    );
    assert!(!trace.invalid);
    let at = clock.now();
    clock
        .advance(Duration::from_micros(u64::from(u16::MAX) + 1))
        .expect("time fits");
    let mut timing = super::EngineTiming::default();
    timing.poll_finished(
        &clock,
        at,
        true,
        None,
        None,
        Some((
            &mut trace,
            RfpllCommandSample {
                command: RfpllCommand::Status,
                publication_micros: 0,
                readiness_micros: 0,
                invalid: false,
            },
        )),
    );
    assert!(timing.invalid);
}
