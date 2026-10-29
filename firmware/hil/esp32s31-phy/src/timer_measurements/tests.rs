use super::*;

#[test]
fn completed_alarm_separates_programming_irq_and_queue_dispatch() {
    let mut recorder = Recorder::new();
    assert!(recorder.begin(100));
    recorder.arm(110, 101, 106);
    recorder.interrupt(114, 114 + 1);
    recorder.dispatch(120, 123);
    let report = recorder.finish(130);
    assert!(!report.invalid);
    assert_eq!(report.elapsed_micros, 30);
    assert_eq!(report.programming.total_micros, 5);
    assert_eq!(report.alarm_to_irq.total_micros, 8);
    assert_eq!(report.deadline_lateness.total_micros, 4);
    assert_eq!(report.irq_to_dispatch.total_micros, 6);
    assert_eq!(report.dispatch.total_micros, 3);
    assert_eq!(report.interrupts, 1);
    assert_eq!(report.irq_ack.total_micros, 1);
    assert!(!report.armed_at_end && !report.irq_pending_at_end);
}

#[test]
fn reprogramming_partial_windows_and_coalescing_are_explicit() {
    let mut recorder = Recorder::new();
    recorder.begin(0);
    recorder.interrupt(1, 1 + 1); // Alarm predates observation window.
    recorder.arm(100, 2, 3);
    recorder.arm(50, 4, 5);
    recorder.interrupt(51, 51 + 1);
    recorder.dispatch(52, 54);
    recorder.arm(70, 55, 56);
    recorder.stop();
    recorder.arm(1000, 60, 61);
    let report = recorder.finish(62);
    assert!(!report.invalid);
    assert_eq!(report.programming.count, 4);
    assert_eq!(report.replaced, 1);
    assert_eq!(report.stopped, 1);
    assert_eq!(report.unmatched_interrupts, 1);
    assert_eq!(report.coalesced_interrupts, 1);
    assert_eq!(report.irq_to_dispatch.total_micros, 51);
    assert!(report.armed_at_end);
}

#[test]
fn exclusive_window_ignores_outside_work_and_does_not_reset_on_reentry() {
    let mut recorder = Recorder::new();
    recorder.arm(3, 1, 2);
    assert!(recorder.begin(10));
    recorder.arm(30, 11, 12);
    assert!(!recorder.begin(15));
    let report = recorder.finish(20);
    assert_eq!(report.programming.count, 1);
    assert_eq!(report.elapsed_micros, 10);
    recorder.interrupt(31, 31 + 1);
    assert!(recorder.finish(32).invalid);
    assert!(recorder.begin(40));
    assert_eq!(recorder.finish(41).programming.count, 0);
}

#[test]
fn early_irq_is_reported_without_inventing_negative_lateness() {
    let mut recorder = Recorder::new();
    recorder.begin(0);
    recorder.arm(100, 1, 2);
    recorder.interrupt(50, 50 + 1);
    let report = recorder.finish(60);
    assert_eq!(report.early_interrupts, 1);
    assert_eq!(report.deadline_lateness.total_micros, 0);
    assert!(report.irq_pending_at_end);
}

#[test]
fn backwards_clock_or_counter_overflow_invalidates_evidence() {
    let mut recorder = Recorder::new();
    recorder.begin(10);
    recorder.arm(30, 21, 20);
    assert!(recorder.finish(40).invalid);
    let mut recorder = Recorder::new();
    recorder.begin(0);
    recorder.report.interrupts = u32::MAX;
    recorder.interrupt(10, 10 + 1);
    assert!(recorder.finish(11).invalid);
    let mut recorder = Recorder::new();
    recorder.begin(10);
    assert!(recorder.finish(9).invalid);
}

#[test]
fn due_deadlines_distinguish_registration_from_programming_latency() {
    let mut recorder = Recorder::new();
    recorder.registration(0, 1); // Outside the window.
    recorder.begin(10);
    recorder.registration(10, 10); // Due exactly at registration.
    recorder.arm(10, 11, 12);
    recorder.registration(20, 15); // Still future at registration and program start.
    recorder.arm(20, 19, 20); // Becomes due inside schedule().
    recorder.registration(30, 21);
    recorder.arm(30, 22, 23); // Still future when programming returns.
    recorder.registration(40, 24); // Queue may retain the earlier deadline.
    let report = recorder.finish(25);
    assert!(!report.invalid);
    assert_eq!(report.registrations, 4);
    assert_eq!(report.due_at_registration, 1);
    assert_eq!(report.programming.count, 3);
    assert_eq!(report.due_at_program_start, 1);
    assert_eq!(report.due_at_program_return, 2);
    assert_eq!(report.replaced, 2);
}

#[test]
fn reversed_ack_timestamp_invalidates_the_window() {
    let mut recorder = Recorder::new();
    recorder.begin(0);
    recorder.interrupt(5, 4);
    assert!(recorder.finish(10).invalid);
}

#[test]
fn cross_core_reprogram_between_irq_entry_and_ack_is_unattributable() {
    let mut recorder = Recorder::new();
    recorder.begin(0);
    recorder.arm(10, 1, 2);
    // IRQ enters at 11, before locking. Another core replaces the alarm.
    recorder.arm(30, 12, 14);
    recorder.interrupt(11, 15);
    recorder.dispatch(16, 17);
    let report = recorder.finish(20);
    assert!(!report.invalid);
    assert_eq!(report.overlapping_interrupts, 1);
    assert_eq!(report.replaced, 1);
    assert_eq!(report.alarm_to_irq, Timing::default());
    assert_eq!(report.deadline_lateness, Timing::default());
    assert_eq!(report.early_interrupts, 0);
    assert_eq!(report.irq_ack.total_micros, 4);
    assert_eq!(report.irq_to_dispatch.total_micros, 5);
    assert!(!report.armed_at_end && !report.irq_pending_at_end);
}

#[test]
fn alarm_program_after_ack_is_still_invalid() {
    let mut recorder = Recorder::new();
    recorder.begin(0);
    recorder.arm(30, 12, 14);
    recorder.interrupt(10, 11);
    let report = recorder.finish(20);
    assert!(report.invalid);
    assert_eq!(report.overlapping_interrupts, 0);
}

#[test]
fn observer_events_drive_the_image_report_across_trace_clock_wrap() {
    use oer_espressif_executor::observation::timer::{Observation, Phase};
    let clock = oer_time_virtual::VirtualClock::<1>::new();
    use oer_time::Clock;
    clock
        .advance(oer_time::Duration::from_micros(u64::from(u32::MAX) - 9))
        .unwrap();
    let origin = clock.now().as_micros();
    let mut collector = Collector::new();
    let irq = Irq::new();
    assert!(collector.begin(origin));
    let mut send = |offset, phase, value, irq: Option<(u64, u64)>| {
        let now = clock.now().as_micros();
        clock
            .advance(oer_time::Duration::from_micros(origin + offset - now))
            .unwrap();
        collector.observe(
            Observation {
                executor: 2,
                phase,
                at_micros: (origin + offset) as u32,
                value,
            },
            clock.now().as_micros(),
            irq,
        );
    };
    send(1, Phase::Registration, (origin + 10) as u32, None);
    send(2, Phase::Arm, (origin + 10) as u32, None);
    send(5, Phase::Armed, 0, None);
    irq.publish((origin + 11) as u32, (origin + 12) as u32);
    send(14, Phase::Dispatch, 1, irq.read(origin + 14));
    send(16, Phase::Dispatched, 0, None);
    let report = collector.recorder.finish(origin + 20);
    assert!(!report.invalid);
    assert_eq!(report.registrations, 1);
    assert_eq!(report.programming.total_micros, 3);
    assert_eq!(report.irq_ack.total_micros, 1);
    assert_eq!(report.alarm_to_irq.total_micros, 6);
    assert_eq!(report.deadline_lateness.total_micros, 1);
    assert_eq!(report.irq_to_dispatch.total_micros, 3);
    assert_eq!(report.dispatch.total_micros, 2);
}

#[test]
fn incomplete_timer_edges_and_concurrent_irq_are_invalid_not_zero() {
    use core::sync::atomic::Ordering;
    use oer_espressif_executor::observation::timer::{Observation, Phase};
    let irq = Irq::new();
    assert_eq!(irq.read(10), None);
    irq.publish(1, 2);
    assert_eq!(irq.read(10), Some((1, 2)));
    irq.sequence.store(3, Ordering::Relaxed);
    assert_eq!(irq.read(10), None);
    let mut collector = Collector::new();
    assert!(collector.begin(0));
    collector.observe(
        Observation {
            executor: 2,
            phase: Phase::Armed,
            at_micros: 1,
            value: 0,
        },
        1,
        None,
    );
    assert!(collector.recorder.finish(2).invalid);
    assert_eq!(timestamp(0, 1, false), None);
    assert_eq!(timestamp(0, 1 << 31, true), None);
}
