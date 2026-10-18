use super::*;

#[test]
fn closed_interval_excludes_polls_during_reporting_and_retains_them_for_the_next_interval() {
    let counters = TaskPollSet::new();
    let tasks = [
        counters.network(),
        counters.radio(),
        counters.udp_rx(),
        counters.udp_tx(),
        counters.tcp(),
    ];
    for task in tasks {
        task.record(7);
    }
    let start = counters.snapshot();
    for task in tasks {
        task.record(101);
    }
    let end = counters.snapshot();
    let interval = end.wrapping_delta_since(start);
    // Reporting can yield while waiting for USB space, letting all tasks run.
    for task in tasks {
        task.record(5_001);
    }
    let later = counters.snapshot().wrapping_delta_since(end);
    for (measured, reporting) in [
        (interval.network, later.network),
        (interval.radio, later.radio),
        (interval.udp_rx, later.udp_rx),
        (interval.udp_tx, later.udp_tx),
        (interval.tcp, later.tcp),
    ] {
        assert_eq!(measured.polls, 1);
        assert_eq!(measured.poll_micros, 101);
        assert_eq!(measured.lifetime_max_micros, 101);
        assert_eq!(measured.over_5_000_micros, 0);
        assert_eq!(reporting.polls, 1);
        assert_eq!(reporting.poll_micros, 5_001);
        assert_eq!(reporting.lifetime_max_micros, 5_001);
    }
}

#[test]
fn interval_delta_retains_lifetime_maximum() {
    let counters = TaskPollCounters::new();
    let before = counters.snapshot();
    counters.record(101);
    counters.record(5_001);
    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.polls, 2);
    assert_eq!(delta.poll_micros, 5_102);
    assert_eq!(delta.lifetime_max_micros, 5_001);
    assert_eq!(delta.over_100_micros, 2);
    assert_eq!(delta.over_5_000_micros, 1);
}

#[test]
fn preaggregated_batch_preserves_every_counter() {
    let counters = TaskPollCounters::new();
    counters.record_batch(TaskPollSnapshot {
        polls: 256,
        poll_micros: 8_192,
        lifetime_max_micros: 731,
        over_100_micros: 3,
        over_500_micros: 1,
        over_1_000_micros: 0,
        over_5_000_micros: 0,
    });
    counters.record_batch(TaskPollSnapshot {
        polls: 17,
        poll_micros: 411,
        lifetime_max_micros: 89,
        over_100_micros: 0,
        over_500_micros: 0,
        over_1_000_micros: 0,
        over_5_000_micros: 0,
    });

    assert_eq!(
        counters.snapshot(),
        TaskPollSnapshot {
            polls: 273,
            poll_micros: 8_603,
            lifetime_max_micros: 731,
            over_100_micros: 3,
            over_500_micros: 1,
            over_1_000_micros: 0,
            over_5_000_micros: 0,
        }
    );
}
