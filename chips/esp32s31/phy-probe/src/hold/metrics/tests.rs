use super::*;
use oer_time::{Clock, Duration};
use oer_time_virtual::VirtualClock;

#[test]
fn root_suspension_is_attributed_only_to_intervals_active_across_pending() {
    let clock = VirtualClock::<1>::new();
    let mut m = Metrics::default();
    m.enter(clock.now().as_micros());
    m.edge(0, true);
    m.edge(2, true);
    clock
        .advance(Duration::from_micros(10))
        .expect("advance clock");
    m.exit(clock.now().as_micros(), true);
    clock
        .advance(Duration::from_micros(30))
        .expect("advance clock");
    m.enter(clock.now().as_micros());
    m.edge(2, false);
    m.edge(3, true);
    clock
        .advance(Duration::from_micros(10))
        .expect("advance clock");
    m.exit(clock.now().as_micros(), true);
    clock
        .advance(Duration::from_micros(40))
        .expect("advance clock");
    m.enter(clock.now().as_micros());
    m.edge(3, false);
    m.edge(0, false);
    clock
        .advance(Duration::from_micros(10))
        .expect("advance clock");
    m.exit(clock.now().as_micros(), false);
    assert!(!m.invalid);
    assert_eq!(
        (m.rows[0].polls, m.rows[0].pending, m.rows[0].resumes),
        (3, 2, 2)
    );
    assert_eq!(m.rows[0].suspended_us, 70);
    assert_eq!(m.rows[2].suspended_us, 30);
    assert_eq!(m.rows[3].suspended_us, 40);
    assert_eq!((m.poll_us, m.maximum_poll_us), (30, 10));
}
#[test]
fn repeated_baseband_windows_within_a_poll_count_as_one_poll() {
    let clock = VirtualClock::<1>::new();
    let mut m = Metrics::default();
    m.enter(clock.now().as_micros());
    m.edge(0, true);
    for _ in 0..2 {
        m.edge(4, true);
        m.edge(4, false);
    }
    m.edge(0, false);
    m.exit(clock.now().as_micros(), false);
    assert!(!m.invalid);
    assert_eq!((m.rows[4].polls, m.rows[4].pending), (1, 0));
    m.exit(clock.now().as_micros(), false);
    assert!(m.invalid);
}

#[test]
fn invalid_operations_and_counter_overflow_invalidate_without_wrapping() {
    let mut metrics = Metrics::default();
    metrics.edge(usize::MAX, true);
    assert!(metrics.invalid);
    assert_eq!(metrics.active, 0);

    let mut metrics = Metrics::default();
    metrics.rows[0].polls = u32::MAX;
    metrics.enter(0);
    metrics.edge(0, true);
    assert!(metrics.invalid);
    assert_eq!(metrics.rows[0].polls, u32::MAX);

    let mut metrics = Metrics::default();
    metrics.edge(0, true);
    metrics.rows[0].pending = u32::MAX;
    metrics.rows[0].resumes = u32::MAX;
    metrics.rows[0].suspended_us = u64::MAX;
    metrics.enter(0);
    metrics.exit(0, true);
    assert!(metrics.invalid);
    metrics.invalid = false;
    metrics.enter(1);
    assert!(metrics.invalid);
    assert_eq!(metrics.rows[0].pending, u32::MAX);
    assert_eq!(metrics.rows[0].resumes, u32::MAX);
    assert_eq!(metrics.rows[0].suspended_us, u64::MAX);

    let mut metrics = Metrics {
        poll_us: u64::MAX,
        ..Metrics::default()
    };
    metrics.enter(0);
    metrics.exit(1, false);
    assert!(metrics.invalid);
    assert_eq!(metrics.poll_us, u64::MAX);
}

#[test]
fn reversed_clock_and_unpaired_edges_invalidate_the_evidence() {
    let mut metrics = Metrics::default();
    metrics.edge(0, false);
    assert!(metrics.invalid);
    let mut metrics = Metrics::default();
    metrics.edge(0, true);
    metrics.enter(10);
    metrics.exit(11, true);
    metrics.enter(9);
    assert!(metrics.invalid);
    let mut metrics = Metrics::default();
    metrics.enter(10);
    metrics.exit(9, false);
    assert!(metrics.invalid);
}
