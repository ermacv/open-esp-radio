use super::*;
use core::{
    future::Future,
    num::{NonZeroU32, NonZeroU64},
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_bluetooth_le_port::*;
use oer_time::Duration;

pub(super) fn run<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    for _ in 0..10_000 {
        if let Poll::Ready(value) = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            return value;
        }
    }
    panic!("virtual fixture did not finish within its finite poll budget");
}
fn power() -> TxPower {
    use oer_bluetooth_le_radio::{LeBand, LeTxCeiling};
    use oer_radio_regulatory::*;
    let band = TEST_DOMAIN.permit(LeBand).expect("test LE band permitted");
    let ceiling = band.tx_ceiling(PowerLimits {
        antenna_gain_mbm: 0,
        chip_maximum_mbm: i32::MAX,
        owner_cap_mbm: None,
    });
    TxPower::within(LeTxCeiling::new(band, ceiling), 0).expect("test power within ceiling")
}
fn env() -> Environment {
    Environment::new(NonZeroU64::new(1_000).expect("nonzero test period"))
}
pub(super) fn rx(channel: u8) -> Receive {
    Receive {
        channel: TestChannel::new(channel).expect("test channel"),
        phy: TestPhy::Le1M,
    }
}
pub(super) fn tx(channel: u8) -> Transmit {
    Transmit {
        channel: TestChannel::new(channel).expect("test channel"),
        phy: TestPhy::Le1M,
        payload: TestPayloadType::new(0).expect("test payload"),
        length: 37,
        power: power(),
    }
}

#[test]
#[cfg(feature = "test-support")]
fn every_supported_shared_case_passes_on_real_model_queues() {
    use oer_bluetooth_le_port::conformance::{self, Case, Fixture, Target};
    let mut fixture = super::conformance::ModelFixture::new(power());
    for case in Case::ALL {
        if !case.supported(&ModelCaps::CAPS) {
            continue;
        }
        let trace = run(fixture.run(case)).expect("model scenario completes");
        assert_eq!(
            conformance::verify(case, &trace),
            Ok(()),
            "{case:?}: {trace:?}"
        );
    }
    let report = run(conformance::run(&mut fixture, Target::Model)).expect("model conformance");
    assert_eq!(report.passed, 14);
    assert_eq!(report.unsupported, 1);
    assert_eq!(report.model_only, 0);
}

#[test]
fn medium_is_independent_of_backend_progress_and_has_exact_loss_phase() {
    let air = env();
    let a = air.attach();
    let b = air.attach();
    let c = air.attach();
    air.receive(&b, rx(19));
    air.receive(&c, rx(20));
    air.transmit(&a, tx(19));
    air.set_loss(Loss::EveryNth(NonZeroU32::new(3).expect("nonzero loss")));
    air.advance(Duration::from_micros(2_500)).expect("advance");
    air.advance(Duration::from_micros(2_500)).expect("advance");
    assert_eq!(
        air.stop(&b),
        PacketCount {
            packets: 4,
            saturated: false
        }
    );
    assert_eq!(air.stop(&c), PacketCount::default());
    air.stop(&a);
    air.receive(&b, rx(19));
    air.advance(Duration::from_micros(10_000))
        .expect("quiet advance");
    assert_eq!(air.stop(&b), PacketCount::default());
}
#[test]
fn receiver_only_counts_packets_after_it_starts_and_loss_changes_are_inputs() {
    let air = env();
    let a = air.attach();
    let b = air.attach();
    air.transmit(&a, tx(19));
    air.advance(Duration::from_micros(9_500)).expect("advance");
    air.receive(&b, rx(19));
    air.advance(Duration::from_micros(500)).expect("advance");
    air.set_loss(Loss::All);
    air.advance(Duration::from_micros(10_000))
        .expect("loss advance");
    assert_eq!(air.stop(&b).packets, 1);
    air.detach(&a);
    assert_eq!(air.active_radios(), 0);
}
#[test]
fn count_saturation_and_time_overflow_are_explicit() {
    let air = env();
    let a = air.attach();
    let b = air.attach();
    air.transmit(&a, tx(19));
    air.receive(&b, rx(19));
    air.advance(Duration::from_micros(65_536_000))
        .expect("advance");
    assert_eq!(
        air.stop(&b),
        PacketCount {
            packets: u16::MAX,
            saturated: true
        }
    );
    let now = air.now();
    assert!(air.advance(Duration::from_micros(u64::MAX)).is_err());
    assert_eq!(air.now(), now);
}

struct Tracked(alloc::rc::Rc<core::cell::Cell<u32>>);
impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
struct AbandonBody {
    drops: alloc::rc::Rc<core::cell::Cell<u32>>,
    power: TxPower,
}
impl<Id: oer_radio_port::SlotIdentity> oer_radio_port::PortBody<Id, Backend<Id, Tracked>>
    for AbandonBody
{
    type Output = ();
    async fn run<'a, 'p>(
        self,
        owner: oer_radio_port::Owner<'a, 'p, Id, Backend<Id, Tracked>, oer_radio_port::Disabled>,
        resources: Resources<'p, Id>,
    ) -> oer_radio_port::Ended<Id, Backend<Id, Tracked>, ()>
    where
        Backend<Id, Tracked>: 'p,
        Id: 'p,
        'p: 'a,
    {
        let mut owner = match owner.enable().await {
            Ok(o) => o,
            Err(f) => return oer_radio_port::Ended::fail_stop(f.fail_stop(), ()),
        };
        let Resources {
            seats: [mut seat, ..],
            session,
        } = resources;
        {
            let phase = owner.phase();
            let capacity = phase.take(&mut seat).await;
            let config = Supported::new(Transmit {
                power: self.power,
                ..tx(19)
            })
            .expect("supported test transmitter");
            assert!(matches!(
                phase.submit(Request {
                    id: LeOperationId(9),
                    capacity,
                    command: Command::DirectTest(DtmCommand::Transmit(session, config)),
                    context: oer_radio_port::PortContext::new(Tracked(self.drops))
                }),
                Ok(Ok(_))
            ));
        }
        core::future::pending().await
    }
}
#[test]
fn p1_drop_never_stops_radio_and_slot_retains_actual_owners_until_world_end() {
    use oer_radio_port::*;
    run(with_local_slot(async |claim| {
        let air = alloc::rc::Rc::new(env());
        let drops = alloc::rc::Rc::new(core::cell::Cell::new(0));
        let mut slot = claim.place();
        let (front, registration) = slot
            .register(Backend::new(air.clone()))
            .ok()
            .expect("fresh test slot");
        let mut service = alloc::boxed::Box::pin(run_port(
            front,
            AbandonBody {
                drops: drops.clone(),
                power: power(),
            },
        ));
        let mut driver = alloc::boxed::Box::pin(
            oer_radio_port::conformance::registration::MockRegistration::new(registration).run(),
        );
        let mut cx = Context::from_waker(Waker::noop());
        assert!(service.as_mut().poll(&mut cx).is_pending());
        assert!(driver.as_mut().poll(&mut cx).is_pending());
        assert_eq!(air.active_radios(), 1);
        drop(service);
        assert_eq!(
            air.active_radios(),
            1,
            "P1 drop publishes cleanup, it cannot stop P2 hardware"
        );
        assert_eq!(drops.get(), 0);
        assert!(matches!(driver.as_mut().poll(&mut cx), Poll::Ready(Err(_))));
        drop(driver);
        assert_eq!(air.active_radios(), 0);
        assert_eq!(
            drops.get(),
            0,
            "cancelled terminal owners stay in the permanent slot"
        );
        slot.end_model_world();
        assert_eq!(drops.get(), 1);
    }));
}
struct EmptyBody;
impl<Id: oer_radio_port::SlotIdentity> oer_radio_port::PortBody<Id, Backend<Id>> for EmptyBody {
    type Output = ();
    async fn run<'a, 'p>(
        self,
        owner: oer_radio_port::Owner<'a, 'p, Id, Backend<Id>, oer_radio_port::Disabled>,
        _: Resources<'p, Id>,
    ) -> oer_radio_port::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'p,
        Id: 'p,
        'p: 'a,
    {
        let owner = match owner.enable().await {
            Ok(o) => o,
            Err(f) => return oer_radio_port::Ended::fail_stop(f.fail_stop(), ()),
        };
        match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(o) => oer_radio_port::Ended::uninstall(o, (), ()),
            Err(o) => oer_radio_port::Ended::fail_stop(o.fail_stop(), ()),
        }
    }
}
#[test]
fn a_late_p2_stop_cannot_reopen_the_expired_supervisor() {
    use oer_radio_port::*;
    run(with_local_slot(async |claim| {
        let air = alloc::rc::Rc::new(env());
        let mut slot = claim.place();
        let (front, registration) = slot
            .register(Backend::new(air.clone()))
            .ok()
            .expect("fresh test slot");
        let mut service = alloc::boxed::Box::pin(run_port(front, EmptyBody));
        let mut driver = alloc::boxed::Box::pin(
            oer_radio_port::conformance::registration::MockRegistration::new(registration).run(),
        );
        let mut cx = Context::from_waker(Waker::noop());
        assert!(service.as_mut().poll(&mut cx).is_pending());
        assert!(driver.as_mut().poll(&mut cx).is_pending());
        assert!(service.as_mut().poll(&mut cx).is_pending());
        air.advance(Duration::from_micros(1_001))
            .expect("advance model deadline");
        let outcome = match service.as_mut().poll(&mut cx) {
            Poll::Ready(PortEnd::FailStopped { outcome, .. }) => outcome,
            _ => panic!("late P2 cannot join"),
        };
        assert!(matches!(outcome.outcome(), Abandon::Unconfirmed(_)));
        drop(service);
        assert!(matches!(driver.as_mut().poll(&mut cx), Poll::Ready(Err(_))));
        drop(driver);
        slot.end_model_world();
    }));
}

struct PostFaultBody {
    drops: alloc::rc::Rc<core::cell::Cell<u32>>,
}
impl<Id: oer_radio_port::SlotIdentity> oer_radio_port::PortBody<Id, Backend<Id, Tracked>>
    for PostFaultBody
{
    type Output = ();
    async fn run<'a, 'p>(
        self,
        owner: oer_radio_port::Owner<'a, 'p, Id, Backend<Id, Tracked>, oer_radio_port::Disabled>,
        resources: Resources<'p, Id>,
    ) -> oer_radio_port::Ended<Id, Backend<Id, Tracked>, ()>
    where
        Backend<Id, Tracked>: 'p,
        Id: 'p,
        'p: 'a,
    {
        use oer_radio_port::{Ended, PortContext, PortPhase};
        let mut owner = match owner.enable().await {
            Ok(o) => o,
            Err(f) => return Ended::fail_stop(f.fail_stop(), ()),
        };
        {
            let mut phase = owner.phase();
            let Resources {
                seats: [mut seat, ..],
                session,
            } = resources;
            let capacity = phase.take(&mut seat).await;
            let config = Supported::new(rx(19)).expect("supported RX");
            assert!(matches!(
                phase.submit(Request {
                    id: LeOperationId(1),
                    capacity,
                    command: Command::DirectTest(DtmCommand::Receive(session, config)),
                    context: PortContext::new(Tracked(self.drops.clone()))
                }),
                Ok(Ok(_))
            ));
            let Event::Terminal(terminal) = phase
                .next_event()
                .await
                .expect("admitted terminal before stream end")
                .expect("no lossy DTM reports");
            assert_eq!(terminal.result, EventResult::NotExecuted);
            let Completion::DirectTest {
                session: ReturnedSession::Idle(session),
                ..
            } = terminal.completion
            else {
                panic!("fault before publication returns idle lease");
            };
            drop(terminal.context);
            let capacity = phase.take(&mut seat).await;
            let config = Supported::new(rx(19)).expect("supported RX");
            assert!(matches!(
                phase.submit(Request {
                    id: LeOperationId(2),
                    capacity,
                    command: Command::DirectTest(DtmCommand::Receive(session, config)),
                    context: PortContext::new(Tracked(self.drops))
                }),
                Err(oer_radio_port::Poisoned {
                    cause: Fault::Injected
                })
            ));
            assert!(phase.view.shared.pending.with(|pending| pending.is_empty()));
            assert_eq!(
                phase
                    .view
                    .shared
                    .poisoned_requests
                    .with(|requests| requests.len()),
                1
            );
            assert!(matches!(
                phase.next_event().await,
                Err(EventEnd::Drained {
                    cause: Fault::Injected
                })
            ));
        }
        Ended::fail_stop(owner.fail_stop(), ())
    }
}
#[test]
fn poison_before_admission_retains_the_whole_request_without_inventing_a_terminal() {
    use oer_radio_port::*;
    run(with_local_slot(async |claim| {
        let air = alloc::rc::Rc::new(env());
        let drops = alloc::rc::Rc::new(core::cell::Cell::new(0));
        let backend = Backend::new(air.clone());
        backend.inject(Stimulus::DrainedFault);
        let mut slot = claim.place();
        let registered = slot.register(backend).ok().expect("fresh test slot");
        let end = LocalExecution
            .scope(
                oer_radio_port::conformance::registration::guarded(registered),
                async |f| {
                    run_port(
                        f,
                        PostFaultBody {
                            drops: drops.clone(),
                        },
                    )
                    .await
                },
            )
            .await;
        assert!(matches!(end, PortEnd::FailStopped { .. }));
        assert_eq!(
            drops.get(),
            1,
            "unadmitted context remains in durable quarantine"
        );
        slot.end_model_world();
        assert_eq!(drops.get(), 2);
    }));
}
