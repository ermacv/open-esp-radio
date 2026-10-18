use super::*;

#[test]
fn accepts_the_nominal_probe() {
    assert!(
        validate(
            TimebaseProbeEvidence {
                intervals: 20,
                period_micros: 100_000,
                elapsed_micros: 2_000_000,
                minimum_interval_micros: 100_000,
                maximum_interval_micros: 100_100,
                early_intervals: 0,
            },
            2_020_000,
            2_000_000,
        )
        .is_ok()
    );
}

#[test]
fn rejects_the_previous_two_x_alarm_rate() {
    assert!(
        validate(
            TimebaseProbeEvidence {
                intervals: 20,
                period_micros: 100_000,
                elapsed_micros: 4_000_000,
                minimum_interval_micros: 200_000,
                maximum_interval_micros: 200_100,
                early_intervals: 0,
            },
            4_020_000,
            2_000_000,
        )
        .is_err()
    );
}
