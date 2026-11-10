use super::*;

fn one_turn() -> Counts {
    Counts {
        turns: 1,
        collections: 2,
        walked: 9,
        told: 3,
        continued: 4,
        stopped_changed: 1,
        return_batches: 1,
        credits_restored: 2,
        no_credit: 1,
        no_credit_loans: 4,
        materialised: 5,
        stage_ticks: [10, 20, 30, 40, 50, 60, 0],
        ..Counts::default()
    }
}

#[test]
fn off_by_default_and_a_switch_takes_effect_at_the_writer_s_next_check() {
    let counter = DiagnosticsCounter::new();
    assert!(!counter.check(), "off by default");
    assert!(!counter.read().on);
    assert!(!counter.switch(true));
    assert!(counter.check());
    assert!(counter.switch(true), "already on: no switch");
    assert!(counter.switch(false));
    assert!(!counter.check());
    assert_eq!(counter.read().switches(), 2, "on, then off");
}

#[test]
fn counts_add_per_turn_and_a_reset_zeroes_them_at_the_next_check() {
    let counter = DiagnosticsCounter::new();
    counter.switch(true);
    assert!(counter.check());
    counter.add(&one_turn());
    counter.add(&one_turn());
    let read = counter.read();
    assert_eq!(read.counts.turns, 2);
    assert_eq!(read.counts.walked, 18);
    assert_eq!(read.counts.stage_ticks, [20, 40, 60, 80, 100, 120, 0]);
    counter.reset().unwrap();
    assert_eq!(
        counter.read().counts,
        read.counts,
        "applied by the writer only"
    );
    assert_eq!(counter.read().resets(), 1, "asked");
    assert!(!counter.read().settled, "not yet applied");
    assert!(counter.check(), "still on");
    let reset = counter.read();
    assert_eq!(reset.counts, Counts::default());
    assert_eq!(reset.resets(), 1);
    assert!(reset.settled);
    counter.add(&one_turn());
    assert_eq!(counter.read().counts, one_turn());
}

#[test]
fn a_reset_asked_while_off_is_applied_at_the_next_check_too() {
    let counter = DiagnosticsCounter::new();
    counter.switch(true);
    counter.check();
    counter.add(&one_turn());
    counter.switch(false);
    counter.reset().unwrap();
    assert!(!counter.check());
    assert_eq!(counter.read().counts, Counts::default());
}

#[test]
fn a_stage_sum_carries_into_its_high_half() {
    let counter = DiagnosticsCounter::new();
    let near = Counts {
        stage_ticks: [u64::from(u32::MAX), 0, 0, 0, 0, 0, 7],
        ..Counts::default()
    };
    counter.add(&near);
    counter.add(&near);
    assert_eq!(
        counter.read().counts.stage_ticks,
        [2 * u64::from(u32::MAX), 0, 0, 0, 0, 0, 14]
    );
}

#[test]
fn a_window_is_whole_only_when_on_throughout_with_no_switch_or_reset_between() {
    let counter = DiagnosticsCounter::new();
    counter.switch(true);
    counter.check();
    counter.add(&one_turn());
    let start = counter.read();
    counter.add(&one_turn());
    let end = counter.read();
    assert_eq!(end.window(&start).ok(), Some(one_turn()));
    // A reset inside the window: no window.
    counter.reset().unwrap();
    counter.check();
    assert_eq!(counter.read().window(&start), Err(NoWindow::Changed));
    // Switched off and on inside: no window.
    let start = counter.read();
    counter.switch(false);
    counter.switch(true);
    assert_eq!(counter.read().window(&start), Err(NoWindow::Changed));
    // Off at either end: no window.
    let start = counter.read();
    counter.switch(false);
    assert_eq!(counter.read().window(&start), Err(NoWindow::Changed));
    let off = DiagnosticsCounter::new();
    assert_eq!(off.read().window(&off.read()), Err(NoWindow::Changed));
}

#[test]
fn a_window_s_counts_wrap() {
    let start = Counts {
        turns: u32::MAX,
        stage_ticks: [u64::MAX, 0, 0, 0, 0, 0, 0],
        ..Counts::default()
    };
    let end = Counts {
        turns: 1,
        stage_ticks: [1, 0, 0, 0, 0, 0, 0],
        ..Counts::default()
    };
    let since = end.since(&start);
    assert_eq!(since.turns, 2);
    assert_eq!(since.stage_ticks[0], 2);
}

/// The owner's counterexample: on, off and on again between the window's
/// readings, the writer checking at each, so one turn went uncounted: the
/// window is not whole.
#[test]
fn on_off_on_within_a_window_leaves_no_whole_window() {
    let counter = DiagnosticsCounter::new();
    counter.switch(true);
    assert!(counter.check());
    let start = counter.read();
    assert!(start.settled);
    counter.add(&one_turn());
    counter.switch(false);
    assert!(!counter.check(), "this turn counts nothing");
    counter.switch(true);
    assert!(counter.check());
    counter.add(&one_turn());
    let end = counter.read();
    assert!(end.on && end.settled);
    assert_eq!(end.window(&start), Err(NoWindow::Changed));
}

/// A switch or a reset the writer has not applied yet leaves a reading
/// unsettled, and a window with such an end not whole.
#[test]
fn an_unfinished_change_at_either_end_leaves_no_whole_window() {
    let counter = DiagnosticsCounter::new();
    // Switched on, read before the writer's next turn.
    counter.switch(true);
    let unsettled = counter.read();
    assert!(unsettled.on && !unsettled.settled);
    assert!(counter.check());
    let settled = counter.read();
    assert_eq!(
        settled.window(&unsettled),
        Err(NoWindow::Changed),
        "the start was unfinished"
    );
    // A reset asked, read before the writer applied it.
    counter.reset().unwrap();
    let pending = counter.read();
    assert!(!pending.settled);
    assert_eq!(
        pending.window(&settled),
        Err(NoWindow::Changed),
        "the end was unfinished"
    );
    counter.check();
    // Switched off at the end, not yet applied.
    let start = counter.read();
    counter.switch(false);
    assert_eq!(counter.read().window(&start), Err(NoWindow::Changed));
    // Two settled readings of one word make a whole window.
    counter.switch(true);
    counter.check();
    let start = counter.read();
    counter.add(&one_turn());
    assert_eq!(counter.read().window(&start).ok(), Some(one_turn()));
}

#[test]
fn a_switch_that_changes_nothing_moves_no_marker() {
    let counter = DiagnosticsCounter::new();
    counter.switch(true);
    counter.check();
    let start = counter.read();
    assert!(counter.switch(true));
    assert_eq!(counter.read().marker, start.marker);
    assert_eq!(counter.read().window(&start).ok(), Some(Counts::default()));
}

/// The owner's first counterexample: once the switch count could come back
/// to the window's, a window with switched-off turns would be whole again.
/// The count stops at its end instead: the marker is spent, and no window
/// is whole, a window that starts after this too.
#[test]
fn a_spent_switch_count_leaves_every_window_unwhole() {
    let counter = DiagnosticsCounter::new();
    counter.switch(true);
    counter.check();
    let start = counter.read();
    for _ in 0..=MARKER_SWITCHES {
        counter.switch(false);
        assert!(!counter.check(), "a switched-off turn");
        counter.switch(true);
        counter.check();
    }
    let end = counter.read();
    assert!(end.exhausted());
    assert_eq!(end.switches(), MARKER_SWITCHES);
    assert_eq!(end.window(&start), Err(NoWindow::Exhausted));
    // Started after: still no window, and the state still switches.
    let late = counter.read();
    counter.add(&one_turn());
    assert_eq!(counter.read().window(&late), Err(NoWindow::Exhausted));
    assert!(counter.switch(false));
    assert!(!counter.check());
    assert!(!counter.switch(true));
    assert!(counter.check());
}

/// The owner's second counterexample: once the reset count could come back
/// to the window's, a window spanning zeroed counts gave a false delta. The
/// count stops at its end: later resets are refused, and no window is
/// whole.
#[test]
fn a_spent_reset_count_refuses_resets_and_leaves_no_false_delta() {
    let counter = DiagnosticsCounter::new();
    counter.switch(true);
    counter.check();
    counter.add(&one_turn());
    let start = counter.read();
    let mut refused = 0;
    for _ in 0..=MARKER_RESETS {
        if counter.reset().is_err() {
            refused += 1;
        }
        counter.check();
    }
    assert_eq!(refused, 1, "the one past the count's end");
    let end = counter.read();
    assert!(end.exhausted());
    assert_eq!(end.resets(), MARKER_RESETS);
    assert_eq!(end.window(&start), Err(NoWindow::Exhausted));
    assert_eq!(counter.reset(), Err(ResetRefused));
}

/// The owner's third counterexample: so many resets asked before the
/// writer's next check that the count came back to the applied one, and the
/// writer skipped the reset. Each is applied or refused, never skipped.
#[test]
fn resets_asked_before_one_check_are_applied_or_refused_never_skipped() {
    let counter = DiagnosticsCounter::new();
    counter.switch(true);
    counter.check();
    counter.add(&one_turn());
    let answers: Vec<_> = (0..=MARKER_RESETS).map(|_| counter.reset()).collect();
    let refused = answers.iter().filter(|answer| answer.is_err()).count();
    assert_eq!(refused, 1);
    assert_eq!(answers.last(), Some(&Err(ResetRefused)));
    counter.check();
    assert_eq!(
        counter.read().counts,
        Counts::default(),
        "the asked reset was applied"
    );
}
