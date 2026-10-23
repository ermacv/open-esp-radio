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

#[test]
fn failed_ready_updates_preserve_a_pending_work_flag() {
    let state = ReadyState::new();
    state.mark_work();
    for _ in 0..127 {
        state.ready_begin();
    }
    assert!(std::panic::catch_unwind(|| state.ready_begin()).is_err());
    assert!(state.ready() && state.work());
    for _ in 0..127 {
        state.exec_begin();
    }
    assert!(!state.ready() && state.work());
    assert!(std::panic::catch_unwind(|| state.exec_begin()).is_err());
    assert!(!state.ready() && state.work());
}

#[test]
fn registry_lookup_rejects_zero_and_finds_only_the_registered_context() {
    let ids = [0, 101, 0, 202];
    let lookup = |id| find_executor(id, |slot| ids[slot.index()]);
    assert_eq!(lookup(0), None);
    assert_eq!(lookup(99), None);
    assert_eq!(lookup(101), Some(ExecutorIndex::One));
    assert_eq!(lookup(202), Some(ExecutorIndex::Three));
}
