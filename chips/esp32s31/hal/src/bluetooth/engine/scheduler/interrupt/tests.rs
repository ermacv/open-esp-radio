use super::*;
use core::{cell::Cell, pin::pin, task::Waker};
use std::boxed::Box;

struct Bell(Cell<usize>);
impl SchedulerDoorbell for Bell {
    fn ring(&self) { self.0.set(self.0.get() + 1); }
    fn poll_reply(&self, _: &mut Context<'_>) -> Poll<Result<(), SchedulerInterruptError>> { Poll::Pending }
}
fn halves() -> (SchedulerTaskHalf, SchedulerInterruptHalf, Bell) {
    let channel = Box::leak(Box::new(SchedulerInterruptChannel::new()));
    let (task, irq) = channel.install(17).expect("fresh channel");
    (task, irq, Bell(Cell::new(0)))
}
fn poll<F: core::future::Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn reply_is_durable_and_the_command_runs_once() {
    let (mut task, mut irq, bell) = halves();
    let mut future = pin!(task.select_progress(&bell));
    assert!(poll(future.as_mut()).is_pending());
    assert_eq!(bell.0.get(), 1);
    assert!(irq.service_with(|command| { assert!(matches!(command, Command::SelectProgress)); Ok(Reply::Done) }));
    assert!(!irq.service_with(|_| panic!("duplicate doorbell must not repeat a command")));
    assert_eq!(poll(future.as_mut()), Poll::Ready(Ok(())));
}

#[test]
fn abandoned_request_keeps_its_seat_and_cannot_be_replaced() {
    let (mut task, mut irq, bell) = halves();
    {
        let mut future = pin!(task.select_progress(&bell));
        assert!(poll(future.as_mut()).is_pending());
    }
    assert!(!task.is_quiet());
    assert!(irq.service_with(|_| Ok(Reply::Done)));
    assert_eq!(poll(pin!(task.select_settle(&bell))), Poll::Ready(Err(SchedulerInterruptError::Abandoned)));
    assert_eq!(bell.0.get(), 1);
    assert!(!task.is_quiet());
}

#[test]
fn foreign_epoch_and_command_number_cannot_touch_hardware() {
    for epoch in [false, true] {
        let (mut task, mut irq, bell) = halves();
        if epoch { irq.epoch += 1; } else { irq.sequence += 1; }
        let mut future = pin!(task.select_progress(&bell));
        assert!(poll(future.as_mut()).is_pending());
        assert!(irq.service_with(|_| panic!("foreign request must not execute")));
        assert_eq!(poll(future.as_mut()), Poll::Ready(Err(SchedulerInterruptError::ForeignReply)));
    }
}

#[test]
fn command_numbers_never_wrap_and_installation_does_not_duplicate_endpoints() {
    let (mut task, _, bell) = halves();
    task.sequence = u32::MAX;
    assert_eq!(poll(pin!(task.select_progress(&bell))), Poll::Ready(Err(SchedulerInterruptError::SequenceExhausted)));
    assert_eq!(bell.0.get(), 0);
    assert!(matches!(task.channel.install(18), Err(SchedulerInterruptError::Occupied)));
    assert!(task.is_quiet());
}
