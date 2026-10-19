use oer_time::{Duration, Instant};

use super::next_tbtt;

/// The recovered vendor loop on the `u64` axis.
fn iterative(send_tick: u64, interval: u64, now: u64) -> u64 {
    let mut next = send_tick;
    loop {
        next += interval;
        if next >= now && next - now <= interval {
            return next;
        }
    }
}

fn tbtt(send_tick: u64, interval: u64, now: u64) -> Option<u64> {
    next_tbtt(
        Instant::from_micros(send_tick),
        Duration::from_micros(interval),
        Instant::from_micros(now),
    )
    .map(Instant::as_micros)
}

#[test]
fn matches_the_recovered_loop_for_bounded_catch_up_cases() {
    for interval in [1, 999, 1_000, 4_096, 102_400] {
        for elapsed in [0, 1, interval - 1, interval, interval + 1, interval * 7] {
            let send_tick = 0x1000_0000;
            let now = send_tick + elapsed;
            assert_eq!(
                tbtt(send_tick, interval, now),
                Some(iterative(send_tick, interval, now))
            );
        }
    }
}

#[test]
fn the_schedule_crosses_the_u32_microsecond_boundary_without_wrapping() {
    // 102.4 ms beacons around 2^32 µs (about 71.6 minutes after boot), where
    // the vendor's u32 tick wraps to zero.
    let interval = 102_400;
    let send_tick = (1 << 32) - 50_000;
    assert_eq!(
        tbtt(send_tick, interval, send_tick + 1),
        Some(send_tick + interval)
    );
    assert_eq!(
        tbtt(send_tick, interval, send_tick + 3 * interval - 1),
        Some(send_tick + 3 * interval)
    );
}

#[test]
fn a_cursor_ahead_of_now_advances_one_interval() {
    assert_eq!(tbtt(1_000, 100, 0), Some(1_100));
}

#[test]
fn a_huge_catch_up_is_constant_time() {
    assert_eq!(tbtt(0, 1, u64::MAX - 1), Some(u64::MAX - 1));
}

#[test]
fn rejects_zero_interval_and_the_end_of_time() {
    assert_eq!(tbtt(0, 0, 0), None);
    assert_eq!(tbtt(u64::MAX - 1, 10, u64::MAX), None);
}
