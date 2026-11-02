extern crate std;
use super::*;

#[test]
fn poll_stamp_preserves_core_and_guard_survives_counter_wrap() {
    stamp_preserves_core_and_guard_survives_counter_wrap::<1>();
    stamp_preserves_core_and_guard_survives_counter_wrap::<2>();
    stamp_preserves_core_and_guard_survives_counter_wrap::<5>();
}

fn stamp_preserves_core_and_guard_survives_counter_wrap<const HARTS: usize>() {
    use oer_time::{Clock, Duration, Instant};
    let assigned_core = Stamp::<HARTS>::assigned_core;
    let poll_stamp = Stamp::<HARTS>::poll_stamp;
    let poll_elapsed = Stamp::<HARTS>::poll_elapsed;
    let stamp_mask = Stamp::<HARTS>::STAMP_MASK;
    let clock = oer_time_virtual::VirtualClock::<1>::starting_at(Instant::from_micros(
        u64::from(stamp_mask) - 100,
    ));
    for core in 0..HARTS {
        let state = poll_stamp(core, clock.now().as_micros());
        assert_eq!(assigned_core(state), Some(core));
        assert_eq!(poll_elapsed(state, clock.now().as_micros()), 0);
        clock
            .advance(Duration::from_micros(u64::from(POLL_ADMISSION_MICROS) - 1))
            .expect("virtual advance");
        assert_eq!(
            poll_elapsed(state, clock.now().as_micros()),
            POLL_ADMISSION_MICROS - 1
        );
        clock
            .advance(Duration::from_micros(1))
            .expect("virtual advance");
        assert_eq!(
            poll_elapsed(state, clock.now().as_micros()),
            POLL_ADMISSION_MICROS
        );
        assert_eq!(assigned_core(state), Some(core));
    }
    assert_eq!(assigned_core(usize::MAX), None);
    assert_ne!(poll_stamp(HARTS - 1, u64::from(stamp_mask)), usize::MAX);
}
