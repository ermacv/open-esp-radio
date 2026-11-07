use super::{NO_EXECUTOR, Running};

/// Levels of the image's contexts on one core, as the interrupt controller
/// runs them: thread mode, the radio executor (P2), the radio interrupts (P3).
const THREAD: u8 = 0;
const RADIO_EXECUTOR: u8 = 2;
const RADIO_INTERRUPT: u8 = 3;

#[test]
fn no_context_runs_before_an_executor_enters() {
    let running = Running::new();
    assert_eq!(running.at(THREAD), NO_EXECUTOR);
    assert_eq!(running.at(RADIO_INTERRUPT), NO_EXECUTOR);
}

#[test]
fn a_hardware_interrupt_that_preempts_the_thread_executor_is_not_its_context() {
    let running = Running::new();
    running.enter(0, THREAD, || {
        assert_eq!(running.at(THREAD), 0);
        // A hardware interrupt preempts the run loop: the executor stays
        // entered beneath it, but the interrupt is not its context.
        assert_eq!(running.at(RADIO_INTERRUPT), NO_EXECUTOR);
        assert_eq!(running.at(THREAD), 0, "the run loop resumes as itself");
    });
    assert_eq!(running.at(THREAD), NO_EXECUTOR);
}

#[test]
fn a_hardware_interrupt_that_preempts_the_interrupt_executor_is_not_its_context() {
    let running = Running::new();
    running.enter(0, THREAD, || {
        // The radio executor preempts the application's run loop.
        running.enter(2, RADIO_EXECUTOR, || {
            assert_eq!(running.at(RADIO_EXECUTOR), 2);
            // A radio interrupt preempts the radio executor's poll.
            assert_eq!(running.at(RADIO_INTERRUPT), NO_EXECUTOR);
            // Nor does the preempted run loop's level name the radio
            // executor's context, nor its own while it is preempted.
            assert_eq!(running.at(THREAD), NO_EXECUTOR);
        });
        assert_eq!(running.at(THREAD), 0, "the run loop is restored");
        assert_eq!(running.at(RADIO_EXECUTOR), NO_EXECUTOR);
    });
}
