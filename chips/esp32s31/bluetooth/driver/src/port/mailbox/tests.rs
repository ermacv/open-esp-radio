use super::*;
use core::{future::Future, pin::pin, task::Waker};
use oer_bluetooth_le_port::{Cause, Receive, TestChannel, TestPhy};
use oer_memory::Doorbell;
use oer_time::{Clock, Instant};
use oer_time_virtual::VirtualClock;
use std::{boxed::Box, rc::Rc};

#[derive(Clone)]
struct TestTimer(Rc<VirtualClock>);
impl Clock for TestTimer {
    fn now(&self) -> Instant {
        self.0.now()
    }
}
impl Timer for TestTimer {
    async fn wait_until(&self, deadline: Instant) {
        self.0.wait_until(deadline).await;
    }
}
struct Fixture {
    commands: &'static Commands,
    clocks: &'static ClockRequests,
    state: &'static ClockStorage,
    cancel: &'static AtomicBool,
}
impl Fixture {
    fn new() -> (Self, MailboxDevice<TestTimer>) {
        let bell = Box::leak(Box::new(Doorbell::new()));
        let commands = Box::leak(Box::new(Commands::new(bell, 1)));
        let clocks = Box::leak(Box::new(ClockRequests::new(bell, 2)));
        let cancel = Box::leak(Box::new(AtomicBool::new(false)));
        let state = Box::leak(Box::new(ClockStorage::new()));
        let value = Box::leak(Box::new(ClockState::new(
            clocks.claim().expect("one fixture clock poster"),
            cancel,
        )));
        assert!(state.install(value).is_ok());
        let device = MailboxDevice::new(
            TestTimer(Rc::new(VirtualClock::new())),
            commands.claim().expect("one fixture command poster"),
            state,
            cancel,
            Duration::from_millis(100),
        );
        (
            Self {
                commands,
                clocks,
                state,
                cancel,
            },
            device,
        )
    }
}
fn rx() -> Start {
    Start::Receive(Receive {
        channel: TestChannel::new(19).expect("fixture channel"),
        phy: TestPhy::Le1M,
    })
}

#[test]
fn cancelled_start_resumes_the_same_request_and_keeps_the_running_owner() {
    let (f, mut device) = Fixture::new();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut start = pin!(device.start(rx()));
        assert!(start.as_mut().poll(&mut cx).is_pending());
    }
    let (command, response) = f.commands.take_request().expect("retained start");
    assert_eq!(command.operation, Operation::Start(rx()));
    {
        let mut start = pin!(device.start(rx()));
        assert!(start.as_mut().poll(&mut cx).is_pending());
        assert!(f.commands.take_request().is_none());
        response.answer(Answer {
            epoch: command.epoch,
            result: Ok(Completion::Started(Started::Executed)),
        });
        assert_eq!(
            start.as_mut().poll(&mut cx),
            Poll::Ready(Ok(Started::Executed))
        );
    }
    assert!(device.is_running());
    assert!(!f.cancel.load(Ordering::Acquire));
}

#[test]
fn cancelled_operation_cannot_be_replaced_by_a_different_device_call() {
    let (f, mut device) = Fixture::new();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut start = pin!(device.start(rx()));
        assert!(start.as_mut().poll(&mut cx).is_pending());
    }
    let (command, response) = f.commands.take_request().expect("original start");
    {
        let mut end = pin!(device.end());
        assert_eq!(
            end.as_mut().poll(&mut cx),
            Poll::Ready(Err(Fault::Accounting))
        );
    }
    response.answer(Answer {
        epoch: command.epoch,
        result: Ok(Completion::Started(Started::Executed)),
    });
    assert!(f.cancel.load(Ordering::Acquire));
    assert!(f.commands.take_request().is_none());
    assert!(
        device.pending.is_some(),
        "quarantine retains the original transaction"
    );
}

#[test]
fn exact_clock_is_independent_of_an_outstanding_dtm_start() {
    let (f, mut device) = Fixture::new();
    let mut clock = device.clock();
    let mut cx = Context::from_waker(Waker::noop());
    let mut start = pin!(device.start(rx()));
    assert!(start.as_mut().poll(&mut cx).is_pending());
    let (command, start_response) = f.commands.take_request().expect("start command");
    let mut now = pin!(clock.now());
    assert!(now.as_mut().poll(&mut cx).is_pending());
    let (request, answer) = f.clocks.take_request().expect("independent clock request");
    let at = LeInstant::from_micros(1234);
    answer.answer(ClockAnswer {
        request,
        result: Ok(at),
    });
    assert_eq!(now.as_mut().poll(&mut cx), Poll::Ready(Ok(Ok(at))));
    assert!(start.as_mut().poll(&mut cx).is_pending());
    start_response.answer(Answer {
        epoch: command.epoch,
        result: Ok(Completion::Started(Started::Executed)),
    });
    assert_eq!(
        start.as_mut().poll(&mut cx),
        Poll::Ready(Ok(Started::Executed))
    );
}

#[test]
fn abandoned_clock_answer_never_satisfies_a_new_read() {
    let (f, mut device) = Fixture::new();
    let mut clock = device.clock();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut now = pin!(clock.now());
        assert!(now.as_mut().poll(&mut cx).is_pending());
    }
    let (old, response) = f.clocks.take_request().expect("old clock request");
    {
        let mut now = pin!(clock.now());
        assert_eq!(
            now.as_mut().poll(&mut cx),
            Poll::Ready(Ok(Err(ClockError::Unavailable)))
        );
    }
    response.answer(ClockAnswer {
        request: old,
        result: Ok(LeInstant::from_micros(111)),
    });
    let mut now = pin!(clock.now());
    assert!(now.as_mut().poll(&mut cx).is_pending());
    let (fresh, answer) = f.clocks.take_request().expect("new physical latch");
    assert_ne!(old.sequence, fresh.sequence);
    let at = LeInstant::from_micros(222);
    answer.answer(ClockAnswer {
        request: fresh,
        result: Ok(at),
    });
    assert_eq!(now.as_mut().poll(&mut cx), Poll::Ready(Ok(Ok(at))));
}

#[test]
fn fault_in_an_abandoned_clock_answer_closes_publication() {
    let (f, mut device) = Fixture::new();
    let mut clock = device.clock();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut now = pin!(clock.now());
        assert!(now.as_mut().poll(&mut cx).is_pending());
    }
    let (request, response) = f.clocks.take_request().expect("clock request");
    response.answer(ClockAnswer {
        request,
        result: Err(Fault::Hardware),
    });
    let mut now = pin!(clock.now());
    assert_eq!(
        now.as_mut().poll(&mut cx),
        Poll::Ready(Err(Poisoned {
            cause: Fault::Hardware
        }))
    );
    assert!(f.cancel.load(Ordering::Acquire));
    assert!(f.clocks.take_request().is_none());
}

#[test]
fn cancelled_shutdown_cannot_advance_epoch_until_the_graph_return_ack() {
    let (f, mut device) = Fixture::new();
    let _clock = device.clock();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut stop = pin!(device.shutdown());
        assert!(stop.as_mut().poll(&mut cx).is_pending());
    }
    let (command, response) = f.commands.take_request().expect("shutdown request");
    device = match device.reinitialize() {
        Err(retained) => retained,
        Ok(_) => panic!("unfinished shutdown cannot reinitialize"),
    };
    assert_eq!(device.epoch(), 1);
    response.answer(Answer {
        epoch: command.epoch,
        result: Ok(Completion::Done),
    });
    {
        let mut stop = pin!(device.shutdown());
        assert_eq!(stop.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    let mut device = match device.reinitialize() {
        Ok(device) => device,
        Err(_) => panic!("ordinary stopped installation can advance"),
    };
    assert_eq!(device.epoch(), 2);
    let _clock = device.clock();
    let mut enable = pin!(device.enable());
    assert!(enable.as_mut().poll(&mut cx).is_pending());
    let (new, response) = f.commands.take_request().expect("new enable");
    assert_eq!(new.epoch, 2);
    response.answer(Answer {
        epoch: 1,
        result: Ok(Completion::Done),
    });
    assert_eq!(
        enable.as_mut().poll(&mut cx),
        Poll::Ready(Err(Fault::Accounting))
    );
    assert!(f.cancel.load(Ordering::Acquire));
}

#[test]
fn refused_second_start_preserves_the_existing_running_session() {
    let (f, mut device) = Fixture::new();
    device.running = true;
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut start = pin!(device.start(rx()));
        assert!(start.as_mut().poll(&mut cx).is_pending());
        let (request, answer) = f.commands.take_request().expect("start request");
        answer.answer(Answer {
            epoch: request.epoch,
            result: Ok(Completion::Started(Started::NotExecuted(Cause::AirDenied))),
        });
        assert_eq!(
            start.as_mut().poll(&mut cx),
            Poll::Ready(Ok(Started::NotExecuted(Cause::AirDenied)))
        );
    }
    assert!(device.is_running());
}

#[test]
fn clock_from_a_retired_epoch_cannot_reach_the_new_installation() {
    let (f, mut device) = Fixture::new();
    let mut old = device.clock();
    assert_eq!(f.state.with(|s| s.retire(1)), Some(Ok(())));
    assert_eq!(f.state.with(|s| s.activate(2)), Some(Ok(())));
    let mut cx = Context::from_waker(Waker::noop());
    let mut now = pin!(old.now());
    assert_eq!(
        now.as_mut().poll(&mut cx),
        Poll::Ready(Ok(Err(ClockError::Unavailable)))
    );
    assert!(f.clocks.take_request().is_none());
    assert!(!f.cancel.load(Ordering::Acquire));
}
