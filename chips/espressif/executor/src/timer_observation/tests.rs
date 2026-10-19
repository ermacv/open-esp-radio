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
