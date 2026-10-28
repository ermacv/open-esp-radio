use super::*;
use core::pin::pin;
use oer_bluetooth_le_wire::TestChannel;
use oer_time_virtual::VirtualClock;
use std::{rc::Rc, task::Waker};

#[derive(Clone)]
struct TestTimer(Rc<VirtualClock>);
impl Clock for TestTimer {
    fn now(&self) -> Instant {
        self.0.now()
    }
}
impl Timer for TestTimer {
    async fn wait_until(&self, deadline: Instant) {
        self.0.wait_until(deadline).await
    }
}
struct Notification(atomic_waker::AtomicWaker);
impl Notify for Notification {
    const NEW: Self = Self(atomic_waker::AtomicWaker::new());
    fn notify(&self) {
        self.0.wake();
    }
    fn poll_wait(&self, cx: &mut Context<'_>) -> Poll<()> {
        self.0.register(cx.waker());
        Poll::Pending
    }
}
struct TestClock(TestTimer);
impl ClockClient for TestClock {
    fn activate(&mut self) -> Result<(), Fault> {
        Ok(())
    }
    async fn retire(&mut self) -> Result<(), Fault> {
        Ok(())
    }
    async fn now(&mut self) -> PortResult<oer_time::RadioInstant<LeRadio>, ClockError, Fault> {
        Ok(Ok(oer_time::RadioInstant::from_micros(
            self.0.now().as_micros(),
        )))
    }
}
#[derive(Default)]
struct Calls {
    started: Cell<u32>,
    ended: Cell<u32>,
    shutdown: Cell<u32>,
    quarantined: Cell<bool>,
    quarantine_calls: Cell<u32>,
    drops: Cell<u32>,
}
struct TestDevice {
    timer: TestTimer,
    calls: Rc<Calls>,
    mode: Mode,
    running: bool,
}
impl Device for TestDevice {
    type Timer = TestTimer;
    type Notify = Notification;
    type Clock = TestClock;
    fn epoch(&self) -> u64 {
        17
    }
    fn timer(&self) -> TestTimer {
        self.timer.clone()
    }
    fn clock(&mut self) -> TestClock {
        TestClock(self.timer.clone())
    }
    fn shutdown_bound(&self) -> Duration {
        Duration::from_millis(2)
    }
    fn is_running(&self) -> bool {
        self.running
    }
    async fn enable(&mut self) -> Result<(), Fault> {
        Ok(())
    }
    async fn start(&mut self, _: Start) -> Result<Started, Fault> {
        self.calls.started.set(self.calls.started.get() + 1);
        if matches!(self.mode, Mode::Indeterminate) {
            return Err(Fault::Hardware);
        }
        self.running = true;
        Ok(Started::Executed)
    }
    async fn end(&mut self) -> Result<PacketCount, Fault> {
        self.calls.ended.set(self.calls.ended.get() + 1);
        self.running = false;
        Ok(PacketCount {
            packets: 19,
            saturated: false,
        })
    }
    async fn progress(&mut self) -> Result<(), Fault> {
        let mut yielded = false;
        poll_fn(|cx| {
            if yielded {
                Poll::Ready(())
            } else {
                yielded = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await;
        Ok(())
    }
    async fn shutdown(&mut self) -> Result<(), Fault> {
        assert!(!self.running);
        self.calls.shutdown.set(self.calls.shutdown.get() + 1);
        Ok(())
    }
    fn quarantine(&mut self) {
        self.calls.quarantined.set(true);
        self.calls
            .quarantine_calls
            .set(self.calls.quarantine_calls.get() + 1);
    }
}
struct Tracked(Rc<Calls>);
impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.drops.set(self.0.drops.get() + 1);
    }
}
impl<Id: SlotIdentity> ModelSlotOwner<Id> for Backend<Id, TestDevice, Tracked> {
    fn end_world(&mut self, _: Option<&mut Self::Queues>) {
        if let Some(d) = self.device.as_mut() {
            d.running = false;
        }
    }
    fn end_vacant_world(d: &mut Self::Vacant) {
        d.device.running = false;
    }
}
#[derive(Clone, Copy)]
enum Mode {
    Ordinary,
    Indeterminate,
    Unread,
    SinkReject,
}
struct Body {
    calls: Rc<Calls>,
    mode: Mode,
}
impl<Id: SlotIdentity> PortBody<Id, Backend<Id, TestDevice, Tracked>> for Body {
    type Output = ();
    async fn run<'a, 'p>(
        self,
        owner: Owner<'a, 'p, Id, Backend<Id, TestDevice, Tracked>, Disabled>,
        resources: Resources<'p, Id>,
    ) -> Ended<Id, Backend<Id, TestDevice, Tracked>, ()>
    where
        Backend<Id, TestDevice, Tracked>: 'p,
        Id: 'p,
        'p: 'a,
    {
        let mut owner = match owner.enable().await {
            Ok(o) => o,
            Err(o) => return Ended::fail_stop(o.fail_stop(), ()),
        };
        let Resources {
            seats: [mut a, ..],
            session,
        } = resources;
        {
            let mut phase = owner.phase();
            let credit = phase.take(&mut a).await;
            let submitted = phase.submit(Request {
                id: LeOperationId(5),
                capacity: credit,
                command: Command::DirectTest(DtmCommand::Receive(
                    session,
                    Supported::new(Receive {
                        channel: TestChannel::new(19).expect("test channel"),
                        phy: TestPhy::Le1M,
                    })
                    .expect("supported 1M"),
                )),
                context: PortContext::new(Tracked(self.calls.clone())),
            });
            assert!(matches!(
                submitted,
                Ok(Ok(Admitted {
                    id: LeOperationId(5),
                    epoch: 17
                }))
            ));
            if !matches!(self.mode, Mode::Unread | Mode::SinkReject) {
                // Cancelling an event wait neither serves nor removes the command.
                {
                    let wait = phase.next_event();
                    let mut wait = pin!(wait);
                    assert!(
                        wait.as_mut()
                            .poll(&mut Context::from_waker(Waker::noop()))
                            .is_pending()
                    );
                }
                assert_eq!(self.calls.started.get(), 0);
                let Event::Terminal(t) = phase
                    .next_event()
                    .await
                    .expect("terminal before stream end")
                    .expect("no event loss");
                assert_eq!(t.admitted.id, LeOperationId(5));
                assert_eq!(t.admitted.epoch, 17);
                if matches!(self.mode, Mode::Indeterminate) {
                    assert_eq!(t.result, EventResult::Indeterminate);
                    assert!(matches!(
                        t.completion,
                        Completion::DirectTest {
                            session: ReturnedSession::Quarantined,
                            count: None
                        }
                    ));
                    drop(t.context);
                    assert!(matches!(
                        phase.next_event().await,
                        Err(EventEnd::Quarantined {
                            cause: Fault::Hardware
                        })
                    ));
                    drop(phase);
                    return Ended::fail_stop(owner.fail_stop(), ());
                }
                assert_eq!(t.result, EventResult::Executed { anchor: None });
                let Completion::DirectTest {
                    session: ReturnedSession::Running(session),
                    ..
                } = t.completion
                else {
                    panic!("executed start returns actual running session")
                };
                drop(t.context);
                let credit = phase.take(&mut a).await;
                assert!(matches!(
                    phase.submit(Request {
                        id: LeOperationId(6),
                        capacity: credit,
                        command: Command::DirectTest(DtmCommand::End(session)),
                        context: PortContext::new(Tracked(self.calls.clone()))
                    }),
                    Ok(Ok(_))
                ));
                let Event::Terminal(t) = phase
                    .next_event()
                    .await
                    .expect("End terminal")
                    .expect("no loss");
                assert!(matches!(
                    t.completion,
                    Completion::DirectTest {
                        session: ReturnedSession::Idle(_),
                        count: Some(PacketCount {
                            packets: 19,
                            saturated: false
                        })
                    }
                ));
                drop(t.context);
            }
        }
        if matches!(self.mode, Mode::SinkReject) {
            let result = owner.disable(&mut ControlFlow::Break).await;
            let Err(owner) = result else {
                panic!("rejecting terminal cannot disable normally")
            };
            Ended::fail_stop(owner.fail_stop(), ())
        } else {
            match owner.disable(&mut |_| ControlFlow::Continue(())).await {
                Ok(owner) => Ended::uninstall(owner, (), ()),
                Err(owner) => Ended::fail_stop(owner.fail_stop(), ()),
            }
        }
    }
}
fn run<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    for _ in 0..200 {
        if let Poll::Ready(x) = f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            return x;
        }
    }
    panic!("bounded host scenario did not finish")
}
fn scenario(mode: Mode) {
    run(with_local_slot(async |claim| {
        let calls = Rc::new(Calls::default());
        let mut slot = claim.place();
        let device = TestDevice {
            timer: TestTimer(Rc::new(VirtualClock::new())),
            calls: calls.clone(),
            mode,
            running: false,
        };
        let end = LocalExecution
            .scope(
                crate::port::Registration::guarded(
                    slot.register(Backend::new(device))
                        .ok()
                        .expect("fresh test slot"),
                ),
                async |front| {
                    run_port(
                        front,
                        Body {
                            calls: calls.clone(),
                            mode,
                        },
                    )
                    .await
                },
            )
            .await;
        if matches!(mode, Mode::Ordinary | Mode::Unread) {
            assert!(matches!(end, PortEnd::Uninstalled { .. }));
            assert_eq!(calls.shutdown.get(), 1);
            assert!(!calls.quarantined.get());
        } else {
            assert!(matches!(end, PortEnd::FailStopped { .. }));
            assert!(calls.quarantined.get());
            assert_eq!(calls.shutdown.get(), 0);
        }
        if matches!(mode, Mode::Ordinary) {
            assert_eq!(
                (calls.started.get(), calls.ended.get(), calls.drops.get()),
                (1, 1, 2)
            );
        }
        if matches!(mode, Mode::SinkReject) {
            assert_eq!(
                calls.drops.get(),
                0,
                "rejected terminal remains owned by slot"
            );
        }
        slot.end_model_world();
        if matches!(mode, Mode::SinkReject) {
            assert_eq!(calls.drops.get(), 1);
        }
    }));
}
#[test]
fn start_end_disable_and_uninstall_return_original_session() {
    scenario(Mode::Ordinary)
}
#[test]
fn indeterminate_retains_command_and_credit_and_returns_no_session() {
    scenario(Mode::Indeterminate)
}
#[test]
fn disable_settles_unread_admitted_work() {
    scenario(Mode::Unread)
}
#[test]
fn rejecting_sink_retains_original_terminal_in_permanent_storage() {
    scenario(Mode::SinkReject)
}

#[test]
fn cancelled_fifo_leader_does_not_block_the_next_original_seat() {
    run(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let calls = Rc::new(Calls::default());
        let (mut front, registration) = slot
            .register(Backend::<_, _, Tracked>::new(TestDevice {
                timer: TestTimer(Rc::new(VirtualClock::new())),
                calls,
                mode: Mode::Ordinary,
                running: false,
            }))
            .ok()
            .expect("fresh capacity fixture");
        let p = front.ports();
        let pool = &*p.credits;
        let first = pool.take(0).await;
        {
            let mut cancelled = std::boxed::Box::pin(pool.take(1));
            let mut next = std::boxed::Box::pin(pool.take(2));
            let mut cx = Context::from_waker(Waker::noop());
            assert!(cancelled.as_mut().poll(&mut cx).is_pending());
            assert!(next.as_mut().poll(&mut cx).is_pending());
            drop(cancelled);
            pool.release(first);
            let Poll::Ready(credit) = next.as_mut().poll(&mut cx) else {
                panic!("second original waiter is now FIFO leader")
            };
            pool.release(credit);
        }
        let returned = pool.take(1).await;
        pool.release(returned);
        drop(front);
        drop(registration);
        slot.end_model_world();
    }));
}

#[test]
fn cancelled_registration_is_cleaned_by_the_device_owner_guard_before_first_poll() {
    for construct_future in [false, true] {
        let calls = Rc::new(Calls::default());
        let device = TestDevice {
            timer: TestTimer(Rc::new(VirtualClock::new())),
            calls: calls.clone(),
            mode: Mode::Ordinary,
            running: false,
        };
        let mut scope = pin!(with_local_slot(async |claim| {
            let mut slot = claim.place();
            let (frontend, raw) = slot
                .register(Backend::<_, _, Tracked>::new(device))
                .unwrap();
            drop(frontend);
            assert_eq!(
                calls.quarantine_calls.get(),
                0,
                "the frontend only reports cancellation"
            );
            let registration = Registration::new(raw);
            assert_eq!(
                calls.quarantine_calls.get(),
                0,
                "the owner's guard is armed before polling"
            );
            if construct_future {
                drop(registration.run());
            } else {
                drop(registration);
            }
            assert_eq!(calls.quarantine_calls.get(), 1);
            assert!(calls.quarantined.get());
            assert_eq!(
                calls.shutdown.get(),
                0,
                "cancellation cannot claim ordinary shutdown"
            );
            slot.end_model_world();
            assert_eq!(calls.quarantine_calls.get(), 1);
        }));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(scope.as_mut().poll(&mut cx).is_ready());
    }
}

#[test]
fn an_ended_owner_driver_cannot_repeat_cleanup_when_polled_again() {
    let calls = Rc::new(Calls::default());
    let device = TestDevice {
        timer: TestTimer(Rc::new(VirtualClock::new())),
        calls: calls.clone(),
        mode: Mode::Ordinary,
        running: false,
    };
    let mut scope = pin!(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (frontend, raw) = slot
            .register(Backend::<_, _, Tracked>::new(device))
            .unwrap();
        drop(frontend);
        {
            let mut driver = pin!(Registration::new(raw).run());
            let mut cx = Context::from_waker(Waker::noop());
            assert!(matches!(
                driver.as_mut().poll(&mut cx),
                Poll::Ready(Err(LifecycleError::Cancelled))
            ));
            assert_eq!(calls.quarantine_calls.get(), 1);
            assert!(driver.as_mut().poll(&mut cx).is_pending());
            assert_eq!(calls.quarantine_calls.get(), 1);
        }
        slot.end_model_world();
    }));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(scope.as_mut().poll(&mut cx).is_ready());
}
