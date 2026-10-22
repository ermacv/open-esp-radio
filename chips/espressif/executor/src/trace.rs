//! Public Embassy trace ABI; only enqueue/dequeue change ready accounting.
#![allow(
    unsafe_code,
    reason = "Embassy requires unique global trace ABI symbols"
)]

#[cfg(feature = "chip")]
fn with_state(executor: u32, f: impl FnOnce(&crate::ready::ReadyState)) {
    f(crate::executor::ready_state(executor));
}

#[cfg(all(test, not(feature = "chip")))]
use crate::ready::tests::with_state;

#[unsafe(export_name = "_embassy_trace_task_ready_begin")]
fn ready_begin(executor: u32, _task: u32) {
    with_state(executor, crate::ready::ReadyState::ready_begin);
}

#[unsafe(export_name = "_embassy_trace_task_exec_begin")]
fn exec_begin(executor: u32, _task: u32) {
    with_state(executor, crate::ready::ReadyState::exec_begin);
}

#[unsafe(export_name = "_embassy_trace_poll_start")]
fn poll_start(_executor: u32) {}
#[unsafe(export_name = "_embassy_trace_task_new")]
fn task_new(_executor: u32, _task: u32) {}
#[unsafe(export_name = "_embassy_trace_task_end")]
fn task_end(_executor: u32, _task: u32) {}
#[unsafe(export_name = "_embassy_trace_task_exec_end")]
fn exec_end(_executor: u32, _task: u32) {}
#[unsafe(export_name = "_embassy_trace_executor_idle")]
fn idle(_executor: u32) {}
