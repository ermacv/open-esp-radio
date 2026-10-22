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
pub(crate) use raw::with_state;
