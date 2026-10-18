use core::cell::Cell;
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use super::*;

#[test]
fn values_below_sixteen_have_a_bucket_each() {
    for value in 0..16 {
        assert_eq!(bucket(value), value as usize);
        assert_eq!(bucket_bounds(value as usize), (value, value));
    }
}

#[test]
fn each_power_of_two_above_sixteen_splits_into_eight_buckets() {
    assert_eq!(bucket_bounds(bucket(16)), (16, 17));
    assert_eq!(bucket(17), bucket(16));
    assert_eq!(bucket(18), bucket(16) + 1);
    assert_eq!(bucket_bounds(bucket(31)), (30, 31));
    assert_eq!(bucket_bounds(bucket(32)), (32, 35));
    assert_eq!(bucket_bounds(bucket(1_000)), (960, 1_023));
    assert_eq!(bucket(u32::MAX), BUCKETS - 1);
    assert_eq!(bucket_bounds(BUCKETS - 1), (0xF000_0000, u32::MAX));
}

#[test]
fn the_buckets_tile_every_value_without_gaps() {
    let mut next = 0u32;
    for index in 0..BUCKETS {
        let (low, high) = bucket_bounds(index);
        assert_eq!(low, next, "bucket {index}");
        assert!(high >= low);
        assert_eq!(bucket(low), index);
        assert_eq!(bucket(high), index);
        // A bucket is at most an eighth of its lower bound wide.
        assert!(u64::from(high - low) * 8 <= u64::from(low.max(1)));
        next = high.wrapping_add(1);
    }
    assert_eq!(next, 0, "the last bucket ends at u32::MAX");
}

#[test]
fn an_empty_histogram_has_no_maximum_and_no_percentile() {
    static EMPTY: CycleHistogram = CycleHistogram::new();
    let snapshot = EMPTY.snapshot();
    assert_eq!(snapshot.count(), 0);
    assert_eq!(snapshot.max(), None);
    assert_eq!(snapshot.percentile(99), None);
    assert_eq!(snapshot.count_over(0).exact(), Some(0));
}

#[test]
fn the_ninety_ninth_percentile_is_the_bucket_of_its_rank() {
    let histogram = CycleHistogram::new();
    for _ in 0..990 {
        histogram.record(10);
    }
    for _ in 0..9 {
        histogram.record(1_000);
    }
    histogram.record(50_000);
    let snapshot = histogram.snapshot();
    assert_eq!(snapshot.count(), 1_000);
    assert_eq!(snapshot.max(), Some(50_000));
    // Rank 990 is the last sample of 10 cycles.
    assert_eq!(snapshot.percentile(99), Some(10));
    assert_eq!(snapshot.percentile(0), Some(10));
    // Rank 1 000 is the longest sample; its bucket ends above the maximum.
    assert_eq!(snapshot.percentile(100), Some(50_000));
    assert_eq!(snapshot.percentile(101), None);
}

#[test]
fn a_percentile_overestimates_by_less_than_an_eighth() {
    let histogram = CycleHistogram::new();
    for _ in 0..99 {
        histogram.record(100);
    }
    histogram.record(1_000_000);
    let p99 = histogram.snapshot().percentile(99).unwrap();
    assert_eq!(p99, 103);
    assert!(u64::from(p99 - 100) * 8 < 100);
}

#[test]
fn a_count_over_a_bucket_end_is_exact_and_inside_a_bucket_a_range() {
    let histogram = CycleHistogram::new();
    for cycles in [100, 950, 1_000, 1_010, 1_024, 5_000] {
        histogram.record(cycles);
    }
    let snapshot = histogram.snapshot();
    // 1 023 ends the bucket 960..=1 023.
    assert_eq!(snapshot.count_over(1_023).exact(), Some(2));
    // 950 shares the bucket 896..=959, which 959 ends.
    assert_eq!(snapshot.count_over(959).exact(), Some(4));
    // 1 000 is inside 960..=1 023, which it shares with 1 010.
    let over = snapshot.count_over(1_000);
    assert_eq!((over.at_least, over.at_most), (2, 4));
    assert_eq!(over.exact(), None);
    // Nothing exceeds the maximum.
    assert_eq!(snapshot.count_over(5_000).exact(), Some(0));
    assert_eq!(snapshot.count_over(u32::MAX).exact(), Some(0));
}

#[test]
fn a_sample_at_the_largest_value_counts() {
    let histogram = CycleHistogram::new();
    histogram.record(u32::MAX);
    let snapshot = histogram.snapshot();
    assert_eq!(snapshot.max(), Some(u32::MAX));
    assert_eq!(snapshot.percentile(50), Some(u32::MAX));
    assert_eq!(snapshot.count_over(u32::MAX - 1).at_most, 1);
}

/// A counter the test advances by hand.
struct FakeCycles(Cell<u32>);

impl CycleSource for FakeCycles {
    fn now(&self) -> u32 {
        self.0.get()
    }
}

/// Pending until its last poll; each poll spends the next of `costs`.
struct Spending<'a> {
    cycles: &'a FakeCycles,
    costs: &'a [u32],
    polls: usize,
}

impl Future for Spending<'_> {
    type Output = usize;

    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<usize> {
        let cost = self.costs[self.polls];
        self.cycles.0.set(self.cycles.0.get().wrapping_add(cost));
        self.polls += 1;
        if self.polls == self.costs.len() {
            Poll::Ready(self.polls)
        } else {
            Poll::Pending
        }
    }
}

#[test]
fn timed_records_each_poll_and_returns_the_output() {
    static POLLS: CycleHistogram = CycleHistogram::new();
    // The counter wraps during the second poll.
    let cycles = FakeCycles(Cell::new(u32::MAX - 100));
    let costs = [40, 2_000, 7];
    let mut timed = pin!(Timed::new(
        Spending {
            cycles: &cycles,
            costs: &costs,
            polls: 0,
        },
        &POLLS,
        &cycles,
    ));
    let mut context = Context::from_waker(Waker::noop());
    assert_eq!(timed.as_mut().poll(&mut context), Poll::Pending);
    assert_eq!(timed.as_mut().poll(&mut context), Poll::Pending);
    assert_eq!(timed.as_mut().poll(&mut context), Poll::Ready(3));

    let snapshot = POLLS.snapshot();
    assert_eq!(snapshot.count(), 3);
    assert_eq!(snapshot.max(), Some(2_000));
    assert_eq!(snapshot.percentile(34), Some(43));
    assert_eq!(snapshot.count_over(39).exact(), Some(2));
}
