use super::*;

fn spread_of(samples: &[i32]) -> Spread {
    samples.iter().copied().fold(Spread::EMPTY, Spread::with)
}

fn tally(trials: u16, after_strobe: [u16; 3], on_time: u16, at: &[i32]) -> Tally {
    Tally {
        trials,
        lost: 0,
        before_strobe: 0,
        after_strobe,
        early: 0,
        on_time,
        first: spread_of(at),
        at: spread_of(at),
    }
}

#[test]
fn two_tallies_add_their_counts_and_widen_their_spreads() {
    let mut second = tally(1, [1, 0, 0], 0, &[]);
    second.early = 1;
    let merged = merged(tally(2, [0, 2, 0], 2, &[-3, 4]), second);
    assert_eq!(merged.trials, 3);
    assert_eq!(merged.after_strobe, [1, 2, 0]);
    assert_eq!((merged.early, merged.on_time), (1, 2));
    assert_eq!(
        (
            merged.at.count,
            merged.at.least,
            merged.at.most,
            merged.at.sum
        ),
        (2, -3, 4, 1)
    );
}

#[test]
fn a_case_passes_only_with_one_alarm_a_trial_at_its_time() {
    assert!(exactly_one("x", tally(2, [0, 2, 0], 2, &[-3, 4])).is_ok());
    let mut before = tally(2, [0, 2, 0], 2, &[-3, 4]);
    before.before_strobe = 1;
    assert!(exactly_one("x", before).is_err());
    assert!(exactly_one("x", tally(2, [1, 1, 0], 1, &[4])).is_err());
    assert!(exactly_one("x", tally(2, [0, 1, 1], 2, &[-3, 4])).is_err());
    assert!(exactly_one("x", tally(2, [0, 2, 0], 1, &[-3])).is_err());
    let mut early = tally(2, [0, 2, 0], 2, &[-3, 4]);
    early.early = 1;
    assert!(exactly_one("x", early).is_err());
}

#[test]
fn a_trial_without_its_alarm_at_the_new_target_is_unanswered() {
    let mut cell = tally(10, [0, 6, 4], 9, &[]);
    cell.lost = 0;
    assert_eq!(unanswered(cell), 1);
    cell.lost = 1;
    assert_eq!(unanswered(cell), 0);
}

#[test]
fn a_future_target_raised_when_an_alarm_lies_at_its_time() {
    let at = |first, second| Attempt::Made {
        strobe: 0,
        alarms: 2,
        first,
        second,
    };
    assert!(future_raised(&at(Some(-5), Some(3_205)), 3_200, 320));
    assert!(!future_raised(&at(Some(-5), None), 3_200, 320));
    assert!(!future_raised(&at(Some(-5), Some(3_600)), 3_200, 320));
    assert!(future_raised(&Attempt::Lost, 3_200, 320));
}

#[test]
fn a_short_timer_refutes_only_without_an_alarm_and_with_the_target_ahead() {
    let shot = |read, alarms| Timed::Made {
        strobe: 0,
        read,
        alarms,
        first: None,
    };
    assert!(refutes(&shot(-1, 0)));
    assert!(!refutes(&shot(0, 0)));
    assert!(!refutes(&shot(-1, 1)));
    assert!(!refutes(&Timed::Lost));
}

#[test]
fn the_rate_converts_ticks_and_quarters_to_cycles() {
    let rate = Rate {
        ticks: 160_000,
        cycles: 3_200_000,
    };
    assert_eq!(rate.cycles_of(16), 320);
    assert_eq!(rate.cycles_of_quarters(-3), -15);
    assert_eq!(rate.cycles_of_quarters(40), 200);
    assert_eq!(
        Rate {
            ticks: 0,
            cycles: 1
        }
        .cycles_of(1),
        i64::MAX
    );
}
