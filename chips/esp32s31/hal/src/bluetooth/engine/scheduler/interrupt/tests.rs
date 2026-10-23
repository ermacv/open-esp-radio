use super::*;
use core::{cell::Cell, pin::pin, task::Waker};
use std::boxed::Box;

struct Bell(Cell<usize>);
impl SchedulerDoorbell for Bell {
    fn ring(&self) {
        self.0.set(self.0.get() + 1);
    }
    fn poll_reply(&self, _: &mut Context<'_>) -> Poll<Result<(), SchedulerInterruptError>> {
        Poll::Pending
    }
}
fn halves() -> (SchedulerTaskHalf, SchedulerInterruptHalf, Bell) {
    fn transferable<T: Send>() {}
    transferable::<Command>();
    transferable::<Reply>();
    transferable::<SchedulerInterruptHalf>();
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
    assert!(irq.service_with(|command| {
        assert!(matches!(command, Command::SelectProgress));
        Ok(Reply::Done)
    }));
    assert!(!irq.service_with(|_| panic!("duplicate doorbell must not repeat a command")));
    assert_eq!(poll(future.as_mut()), Poll::Ready(Ok(())));
}

#[test]
fn each_in_place_reply_is_consumed_once_and_replaces_the_previous_value() {
    let (mut task, mut irq, bell) = halves();
    for value in [true, false] {
        {
            let mut future = pin!(task.progress(&bell));
            assert!(poll(future.as_mut()).is_pending());
            assert!(irq.service_with(|command| {
                assert!(matches!(command, Command::Progress));
                Ok(Reply::Flag(value))
            }));
            assert_eq!(poll(future.as_mut()), Poll::Ready(Ok(value)));
        }
        assert!(task.is_quiet());
        assert!(task.take_reply().is_none());
        assert!(!irq.service_with(|_| panic!("consumed request cannot execute again")));
    }
    assert_eq!(bell.0.get(), 2);
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
    assert_eq!(
        poll(pin!(task.select_settle(&bell))),
        Poll::Ready(Err(SchedulerInterruptError::Abandoned))
    );
    assert_eq!(bell.0.get(), 1);
    assert!(!task.is_quiet());
}

#[test]
fn foreign_epoch_and_command_number_cannot_touch_hardware() {
    for epoch in [false, true] {
        let (mut task, mut irq, bell) = halves();
        if epoch {
            irq.epoch += 1;
        } else {
            irq.sequence += 1;
        }
        let mut future = pin!(task.select_progress(&bell));
        assert!(poll(future.as_mut()).is_pending());
        assert!(irq.service_with(|_| panic!("foreign request must not execute")));
        assert_eq!(
            poll(future.as_mut()),
            Poll::Ready(Err(SchedulerInterruptError::ForeignReply))
        );
    }
}

#[test]
fn command_numbers_never_wrap_and_installation_does_not_duplicate_endpoints() {
    let (mut task, _, bell) = halves();
    task.sequence = u32::MAX;
    assert_eq!(
        poll(pin!(task.select_progress(&bell))),
        Poll::Ready(Err(SchedulerInterruptError::SequenceExhausted))
    );
    assert_eq!(bell.0.get(), 0);
    assert!(matches!(
        task.channel.install(18),
        Err(SchedulerInterruptError::Occupied)
    ));
    assert!(task.is_quiet());
}

#[test]
fn retirement_consumes_both_halves_and_reinstallation_never_reuses_a_number() {
    let (mut task, mut irq, bell) = halves();
    let channel = task.channel;
    {
        let mut future = pin!(task.select_progress(&bell));
        assert!(poll(future.as_mut()).is_pending());
        assert!(irq.service_with(|_| Ok(Reply::Done)));
        assert_eq!(poll(future.as_mut()), Poll::Ready(Ok(())));
    }
    assert_eq!(task.sequence, 1);
    assert!(task.retire(irq).is_ok());
    let (mut task, mut irq) = channel.install(18).expect("both previous halves retired");
    assert_eq!(task.sequence, 1);
    let mut future = pin!(task.select_settle(&bell));
    assert!(poll(future.as_mut()).is_pending());
    assert!(irq.service_with(|_| Ok(Reply::Done)));
    assert_eq!(poll(future.as_mut()), Poll::Ready(Ok(())));
    assert_eq!(irq.sequence, 2);
}

#[test]
fn stale_reply_cannot_free_the_request_seat() {
    let (mut task, mut irq, bell) = halves();
    {
        let mut future = pin!(task.select_progress(&bell));
        assert!(poll(future.as_mut()).is_pending());
    }
    assert!(irq.service_with(|_| Ok(Reply::Done)));
    task.sequence += 1;
    assert!(matches!(
        task.take_reply(),
        Some(Err(SchedulerInterruptError::ForeignReply))
    ));
    assert!(matches!(
        task.take_reply(),
        Some(Err(SchedulerInterruptError::ForeignReply))
    ));
    assert!(!irq.service_with(|_| panic!("stale response must retain the actual message")));
    assert!(!task.is_quiet());
    assert!(matches!(
        task.retire(irq),
        Err((SchedulerInterruptError::Abandoned, _, _))
    ));
}

#[test]
fn outlined_interrupt_clock_preserves_settle_results_and_deadline() {
    use oer_time::{Clock, Duration};
    use oer_time_virtual::VirtualClock;
    for ready_on in [None, Some(1), Some(2), Some(3)] {
        let clock: VirtualClock = VirtualClock::default();
        let outlined = SchedulerInterruptClock::new(&clock);
        assert_eq!(outlined.now(), clock.now());
        let mut attempts = 0;
        let result = crate::settle::settle(
            crate::settle::Settle::new(super::super::DIAGNOSTIC_SETTLE, &outlined),
            || {
                attempts += 1;
                clock
                    .advance(Duration::from_micros(10))
                    .expect("virtual time advances");
                (Some(attempts) == ready_on).then_some(attempts)
            },
        );
        let expected = match ready_on {
            Some(1) => (Ok(1), 1),
            Some(2) => (Ok(2), 2),
            _ => (Err(crate::settle::Unsettled), 2),
        };
        assert_eq!((result, attempts), expected);
        assert_eq!(outlined.now(), clock.now());
    }
}

#[test]
fn a_reply_during_wait_registration_is_observed_in_the_same_poll() {
    struct DuringRegistration(core::cell::RefCell<SchedulerInterruptHalf>);
    impl SchedulerDoorbell for DuringRegistration {
        fn ring(&self) {}
        fn poll_reply(&self, _: &mut Context<'_>) -> Poll<Result<(), SchedulerInterruptError>> {
            self.0.borrow_mut().service_with(|_| Ok(Reply::Done));
            Poll::Pending
        }
    }
    let (mut task, irq, _) = halves();
    let bell = DuringRegistration(core::cell::RefCell::new(irq));
    assert_eq!(poll(pin!(task.select_progress(&bell))), Poll::Ready(Ok(())));
}
