use super::Recorder;
use crate::{HOLD_INTERVALS, HoldOperation as Op};
use oer_time::{Clock, Duration};
use oer_time_virtual::VirtualClock;

#[test]
fn nested_and_repeated_brackets_retain_raw_timestamps() {
    let clock = VirtualClock::<1>::new();
    let mut r = Recorder::new(1);
    r.observe(Op::Tracking, true, false, clock.now().as_micros());
    for _ in 0..2 {
        r.observe(Op::WifiBasebandOff, true, false, clock.now().as_micros());
        clock.advance(Duration::from_micros(7)).unwrap();
        r.observe(Op::WifiBasebandOff, false, false, clock.now().as_micros());
    }
    r.observe(Op::Tracking, false, false, clock.now().as_micros());
    let sample = r.finish(7, false);
    assert!(!sample.invalid);
    assert_eq!(sample.intervals.len(), 3);
    assert_eq!(
        sample.intervals[2].finished_micros - sample.intervals[2].started_micros,
        14
    );
}
#[test]
fn missing_failed_reversed_duplicate_and_overflowed_edges_invalidate() {
    let mut r = Recorder::new(1);
    r.observe(Op::Tracking, true, false, 10);
    assert!(r.finish(0, false).invalid);
    let mut r = Recorder::new(1);
    r.observe(Op::Tracking, true, false, 10);
    r.observe(Op::Tracking, false, true, 11);
    assert!(r.finish(0, true).invalid);
    let mut r = Recorder::new(1);
    r.observe(Op::Tracking, true, false, 10);
    r.observe(Op::Tracking, false, false, 9);
    assert!(r.finish(7, false).invalid);
    let mut r = Recorder::new(1);
    r.observe(Op::Tracking, true, false, 10);
    r.observe(Op::Tracking, true, false, 11);
    r.observe(Op::Tracking, false, false, 12);
    assert!(r.finish(7, false).invalid);
    let mut r = Recorder::new(1);
    for n in 0..=HOLD_INTERVALS {
        r.observe(Op::Tracking, true, false, n as u64);
        r.observe(Op::Tracking, false, false, n as u64);
    }
    assert!(r.finish(7, false).invalid);
}
