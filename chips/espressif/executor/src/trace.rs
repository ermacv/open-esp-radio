//! Public Embassy trace ABI; only enqueue/dequeue change ready accounting,
//! and the begin and end of a task's poll name the executor's current task.
#![allow(
    unsafe_code,
    reason = "Embassy requires unique global trace ABI symbols"
)]

#[unsafe(export_name = "_embassy_trace_task_ready_begin")]
#[inline(never)]
fn ready_begin(executor: u32, _task: u32) {
    #[cfg(feature = "chip")]
    crate::executor::task_ready_begin(executor);
    #[cfg(all(test, not(feature = "chip")))]
    crate::ready::tests::with_state(executor, crate::ready::ReadyState::ready_begin);
}

#[unsafe(export_name = "_embassy_trace_task_exec_begin")]
#[inline(never)]
fn exec_begin(executor: u32, task: u32) {
    #[cfg(feature = "chip")]
    crate::executor::task_exec_begin(executor, task);
    #[cfg(all(test, not(feature = "chip")))]
    {
        crate::ready::tests::with_exec_state(executor, crate::ready::ReadyState::exec_begin);
        crate::task_handle::tests::with_current(executor, |current| current.begin(task));
    }
}

#[unsafe(export_name = "_embassy_trace_poll_start")]
fn poll_start(_executor: u32) {}
#[unsafe(export_name = "_embassy_trace_task_new")]
fn task_new(_executor: u32, _task: u32) {}
#[unsafe(export_name = "_embassy_trace_task_end")]
fn task_end(_executor: u32, _task: u32) {}
#[unsafe(export_name = "_embassy_trace_task_exec_end")]
#[inline(never)]
fn exec_end(executor: u32, _task: u32) {
    #[cfg(feature = "chip")]
    crate::executor::task_exec_end(executor);
    #[cfg(all(test, not(feature = "chip")))]
    crate::task_handle::tests::with_current(executor, crate::task_handle::CurrentTask::end);
}
#[unsafe(export_name = "_embassy_trace_executor_idle")]
fn idle(_executor: u32) {}
