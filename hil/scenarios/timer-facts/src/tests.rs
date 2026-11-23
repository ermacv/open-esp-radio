use super::*;

fn spread(samples: &[i32]) -> Spread {
    samples.iter().copied().fold(Spread::EMPTY, Spread::with)
}

fn tally(trials: u16, after_load: [u16; 3], before_load: u16, at: &[i32]) -> Tally {
    Tally {
        trials,
        lost: 0,
        after_load,
        before_load,
        at: spread(at),
        load: spread(&[40]),
    }
}

#[test]
fn two_tallies_add_their_counts_and_widen_their_spreads() {
    let merged = merged(
        tally(2, [0, 2, 0], 0, &[-3, 4]),
        tally(1, [1, 0, 0], 1, &[]),
    );
    assert_eq!(merged.trials, 3);
    assert_eq!(merged.after_load, [1, 2, 0]);
    assert_eq!(merged.before_load, 1);
    assert_eq!(
        (
            merged.at.count,
            merged.at.least,
            merged.at.most,
            merged.at.sum
        ),
        (2, -3, 4, 1)
    );
    assert_eq!(merged.load.count, 2);
}

#[test]
fn a_case_passes_only_with_one_alarm_a_trial_at_its_time() {
    assert!(exactly_one("x", tally(2, [0, 2, 0], 0, &[-3, 4]), -5, 5).is_ok());
    // An alarm before the load, a trial without its alarm, two alarms, an
    // alarm out of its tolerance: each fails.
    assert!(exactly_one("x", tally(2, [0, 2, 0], 1, &[-3, 4]), -5, 5).is_err());
    assert!(exactly_one("x", tally(2, [1, 1, 0], 0, &[4]), -5, 5).is_err());
    assert!(exactly_one("x", tally(2, [0, 1, 1], 0, &[-3, 4]), -5, 5).is_err());
    assert!(exactly_one("x", tally(2, [0, 2, 0], 0, &[-3, 6]), -5, 5).is_err());
    let mut lost = tally(2, [0, 2, 0], 0, &[-3, 4]);
    lost.lost = 1;
    assert!(exactly_one("x", lost, -5, 5).is_err());
}

#[test]
fn the_rate_converts_ticks_to_cycles_and_survives_no_ticks() {
    let rate = Rate {
        ticks: 160_000,
        cycles: 3_200_000,
    };
    assert_eq!(rate.cycles_of(16), 320);
    assert_eq!(
        Rate {
            ticks: 0,
            cycles: 1
        }
        .cycles_of(1),
        i64::MAX
    );
}
