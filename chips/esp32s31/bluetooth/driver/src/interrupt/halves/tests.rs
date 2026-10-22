use super::*;
use core::{cell::Cell, task::Waker};
use oer_interrupt_table::signal::host;
use std::boxed::Box;

struct Bell(Cell<usize>);
impl RadioDoorbell for Bell {
    fn ring(&self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn only_reply_readiness_coalesces_and_each_exchange_rings_the_shared_line() {
    fn transferable<T: Send>() {}
    transferable::<ControllerInterruptHalf<host::Deferred>>();
    let signal = Box::leak(Box::new(IrqSignal::<host::Deferred>::new()));
    let bell = SchedulerDoorbell::new(Bell(Cell::new(0)), signal);
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(bell.poll_reply(&mut cx), Poll::Pending);
    bell.ring();
    bell.ring();
    assert_eq!(bell.doorbell.0.get(), 2);
    signal.raise(1);
    signal.raise(1);
    assert_eq!(bell.poll_reply(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(bell.poll_reply(&mut cx), Poll::Pending);
}

#[test]
fn a_foreign_executor_cannot_take_the_reply_notification() {
    let signal = Box::leak(Box::new(IrqSignal::<host::Foreign>::new()));
    let bell = SchedulerDoorbell::new(Bell(Cell::new(0)), signal);
    signal.raise(1);
    assert_eq!(
        bell.poll_reply(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(SchedulerInterruptError::ForeignContext))
    );
}
