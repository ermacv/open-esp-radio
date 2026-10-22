extern crate std;
use super::*;

#[test]
fn clearing_work_preserves_tokens_and_dequeue_preserves_work() {
    let state = ReadyState::new();
    state.ready_begin();
    state.mark_work();
    state.clear_work();
    assert!(state.ready());
    assert!(!state.work());
    state.mark_work();
    state.exec_begin();
    assert!(!state.ready());
    assert!(state.work());
}

#[test]
fn capacity_failure_cannot_wrap_to_no_ready() {
    let state = ReadyState::new();
    for _ in 0..127 {
        state.ready_begin();
    }
    assert!(std::panic::catch_unwind(|| state.ready_begin()).is_err());
    assert!(state.ready());
    for _ in 0..127 {
        state.exec_begin();
    }
    assert!(!state.ready());
    assert!(std::panic::catch_unwind(|| state.exec_begin()).is_err());
    assert!(!state.ready());
}

#[cfg(feature = "trace-tests")]
mod raw;
#[cfg(feature = "trace-tests")]
pub(crate) use raw::{with_exec_state, with_state};

#[test]
fn poll_stamp_preserves_core_and_guard_survives_counter_wrap() {
    use oer_time::{Clock, Duration, Instant};
    let clock = oer_time_virtual::VirtualClock::<1>::starting_at(Instant::from_micros(
        u64::from(STAMP_MASK) - 100,
    ));
    for core in [0, 1] {
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
    assert_ne!(poll_stamp(1, u64::from(STAMP_MASK)), usize::MAX);
}
