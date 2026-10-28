#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]

use super::*;
use oer_bluetooth_hci::{
    self as hci, HciCommandPacket, HciControllerResponse,
    bt_hci::cmd::{Cmd, Opcode, controller_baseband::Reset as HciReset, le::LeRand},
};
use oer_bluetooth_le_controller::dtm::Submission;
use oer_bluetooth_le_controller::dtm::*;
use oer_bluetooth_le_port::{self as port, *};
const RESET: Opcode = HciReset::OPCODE;
const RAND: Opcode = LeRand::OPCODE;
use core::{
    future::{Future, poll_fn},
    num::NonZeroU64,
    ops::ControlFlow,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_bluetooth_le_model::{Backend, Environment, ModelCaps, Resources};
use oer_radio_port::conformance::registration::guarded;
use oer_radio_port::*;
use oer_time::Duration;
use std::{rc::Rc, vec::Vec};

struct Random(Result<[u8; 8], EntropyUnavailable>);
impl Entropy for Random {
    fn random(&mut self) -> Result<[u8; 8], EntropyUnavailable> {
        self.0
    }
}
fn power() -> TxPower {
    use oer_bluetooth_le_port::{LeBand, LeTxCeiling};
    use oer_radio_regulatory::*;
    let band = TEST_DOMAIN.permit(LeBand).expect("test band");
    let ceiling = band.tx_ceiling(PowerLimits {
        antenna_gain_mbm: 0,
        chip_maximum_mbm: i32::MAX,
        owner_cap_mbm: None,
    });
    TxPower::within(LeTxCeiling::new(band, ceiling), 0).expect("test power")
}
fn run<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    for _ in 0..10_000 {
        if let Poll::Ready(output) = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            return output;
        }
    }
    panic!("model did not finish within its finite poll budget")
}
fn take<P: PoolPath, C: LeCapabilities>(
    service: &mut Service<'_, P, C>,
    opcode: Opcode,
    status: u8,
) -> Vec<u8> {
    let response = service.take_response().expect("HCI response");
    let bytes = response.as_bytes();
    assert_eq!(bytes.get(2), Some(&1), "one command credit");
    assert_eq!(
        bytes.get(3..5),
        Some(opcode.to_raw().to_le_bytes().as_slice())
    );
    assert_eq!(bytes.get(5), Some(&status));
    bytes.to_vec()
}
#[derive(Clone, Copy)]
enum Scenario {
    Immediate,
    CancelWait,
    CancelCapacity,
    Receiver,
    Transmitter,
    Refused,
    WrongReceipt,
    WrongTerminal,
    WrongCompletion,

    StartCount,
    EndMissingCount,
    EndTransmitCount,
    EndTransmitSaturated,
    EndReceiverCount,
    Indeterminate,
}

struct Alter<'a, F> {
    inner: &'a mut F,
    scenario: Scenario,
}
impl<F: PortPhase> PortPhase for Alter<'_, F> {
    type Event = F::Event;
    type Fault = F::Fault;
    async fn next_event(
        &mut self,
    ) -> Result<Result<Self::Event, EventsLost>, EventEnd<Self::Fault>> {
        self.inner.next_event().await
    }
}
// Corruption of terminal owners is deliberately applied only in a host fixture.
struct TerminalAlter<'a, F> {
    inner: &'a mut F,
    scenario: Scenario,
}
trait AlterTerminal {
    fn alter(&mut self, scenario: Scenario);
}
impl<P: PoolPath> AlterTerminal for Event<'_, P, ()> {
    fn alter(&mut self, scenario: Scenario) {
        let Event::Terminal(event) = self;
        match scenario {
            Scenario::WrongTerminal => event.admitted.id.0 ^= 1,
            Scenario::WrongCompletion => event.result = EventResult::NotExecuted,
            Scenario::StartCount | Scenario::EndTransmitCount | Scenario::EndReceiverCount => {
                let Completion::DirectTest { count, .. } = &mut event.completion;
                *count = Some(port::PacketCount {
                    packets: 0x1234,
                    saturated: false,
                });
            }
            Scenario::EndMissingCount => {
                let Completion::DirectTest { count, .. } = &mut event.completion;
                *count = None;
            }
            Scenario::EndTransmitSaturated => {
                let Completion::DirectTest { count, .. } = &mut event.completion;
                *count = Some(port::PacketCount {
                    packets: 0,
                    saturated: true,
                });
            }
            _ => {}
        }
    }
}
impl<F: PortPhase> PortPhase for TerminalAlter<'_, F>
where
    F::Event: AlterTerminal,
{
    type Event = F::Event;
    type Fault = F::Fault;
    async fn next_event(
        &mut self,
    ) -> Result<Result<Self::Event, EventsLost>, EventEnd<Self::Fault>> {
        let mut result = self.inner.next_event().await;
        if let Ok(Ok(event)) = &mut result {
            event.alter(self.scenario);
        }
        result
    }
}
macro_rules! phase {
    ($wrapper:ident) => {
        impl<'p, P: PoolPath, F: LePhase<'p, P, ()>> LePhase<'p, P, ()> for $wrapper<'_, F> {
            type Caps = F::Caps;
            async fn take<'a>(
                &'a self,
                seat: &'a mut Seat<'p, port::CommandCapacity, Child<P, 0>>,
            ) -> Permit<'p, port::CommandCapacity, Child<P, 0>>
            where
                'p: 'a,
            {
                self.inner.take(seat).await
            }
            fn release(&self, capacity: Permit<'p, port::CommandCapacity, Child<P, 0>>) {
                self.inner.release(capacity);
            }
            fn submit(
                &self,
                request: Request<'p, P, (), F::Caps>,
            ) -> port::Submission<'p, P, (), F::Caps, F::Fault> {
                if matches!(self.scenario, Scenario::Refused) {
                    return Ok(Err(port::Refused {
                        reason: port::Refusal::Unavailable,
                        request,
                    }));
                }
                let mut result = self.inner.submit(request);
                if matches!(self.scenario, Scenario::WrongReceipt)
                    && let Ok(Ok(receipt)) = &mut result
                {
                    receipt.id.0 ^= 1;
                }
                result
            }
        }
    };
}
phase!(Alter);
phase!(TerminalAlter);
struct Body {
    env: Rc<Environment>,
    scenario: Scenario,
}
impl<Id: SlotIdentity> PortBody<Id, Backend<Id>> for Body {
    type Output = ();
    async fn run<'a, 'p>(
        self,
        owner: Owner<'a, 'p, Id, Backend<Id>, Disabled>,
        resources: Resources<'p, Id>,
    ) -> Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'p,
        Id: 'p,
        'p: 'a,
    {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failure) => return Ended::fail_stop(failure.fail_stop(), ()),
        };
        let Resources {
            seats: [seat, mut first, mut second],
            session,
        } = resources;
        let mut service = Service::<_, ModelCaps>::new(
            session,
            seat,
            if matches!(
                self.scenario,
                Scenario::Transmitter | Scenario::EndTransmitCount | Scenario::EndTransmitSaturated
            ) {
                Some(power())
            } else {
                None
            },
        );
        {
            let mut phase = owner.phase();
            assert_eq!(
                complete(&mut service, &mut *phase).await,
                Err(Fault::NoCommand)
            );
            match self.scenario {
                Scenario::Immediate => {
                    let mut random = Random(Ok([0x92; 8]));
                    assert!(matches!(
                        service.accept(HciCommandPacket::new(RAND, &[]), &mut random),
                        Ok(Accepted::Response)
                    ));
                    assert!(
                        service
                            .accept(HciCommandPacket::new(RESET, &[]), &mut random)
                            .is_err()
                    );
                    let first = *service.response().unwrap();
                    assert_eq!(service.response(), Some(&first));
                    assert_eq!(&take(&mut service, RAND, 0)[6..], &[0x92; 8]);
                    random.0 = Err(EntropyUnavailable);
                    let _ = service
                        .accept(HciCommandPacket::new(RAND, &[]), &mut random)
                        .ok()
                        .unwrap();
                    assert_eq!(&take(&mut service, RAND, 3)[6..], &[0; 8]);
                    for (opcode, parameters, expected) in [
                        (RESET, &[1][..], 0x12),
                        (RAND, &[1][..], 0x12),
                        (hci::LE_RECEIVER_TEST_V1_OPCODE, &[40][..], 0x12),
                        (hci::LE_RECEIVER_TEST_V2_OPCODE, &[19, 2, 0][..], 0x11),
                        (hci::LE_TRANSMITTER_TEST_V1_OPCODE, &[19, 38, 0][..], 0x12),
                        (
                            hci::LE_TRANSMITTER_TEST_V2_OPCODE,
                            &[19, 37, 0, 1][..],
                            0x0c,
                        ),
                    ] {
                        let _ = service
                            .accept(HciCommandPacket::new(opcode, parameters), &mut random)
                            .ok()
                            .unwrap();
                        take(&mut service, opcode, expected);
                    }
                    let _ = service
                        .accept(
                            HciCommandPacket::new(hci::LE_TEST_END_OPCODE, &[]),
                            &mut random,
                        )
                        .ok()
                        .unwrap();
                    assert_eq!(
                        &take(&mut service, hci::LE_TEST_END_OPCODE, 0)[6..],
                        &[0, 0]
                    );
                    let Ok(Accepted::Reset(_reset)) =
                        service.accept(HciCommandPacket::new(RESET, &[]), &mut random)
                    else {
                        panic!("Reset must leave the old body")
                    };
                    assert!(service.response().is_none());
                }
                scenario => 'operation: {
                    let mut random = Random(Ok([0; 8]));
                    let transmit = matches!(
                        scenario,
                        Scenario::Transmitter
                            | Scenario::EndTransmitCount
                            | Scenario::EndTransmitSaturated
                    );
                    let (opcode, parameters) = if transmit {
                        (hci::LE_TRANSMITTER_TEST_V2_OPCODE, &[19, 37, 0, 1][..])
                    } else {
                        (hci::LE_RECEIVER_TEST_V1_OPCODE, &[19][..])
                    };
                    let held = if matches!(scenario, Scenario::CancelCapacity) {
                        Some((phase.take(&mut first).await, phase.take(&mut second).await))
                    } else {
                        None
                    };
                    assert!(matches!(
                        service.accept(HciCommandPacket::new(opcode, parameters), &mut random),
                        Ok(Accepted::Port)
                    ));
                    if let Some((a, b)) = held {
                        {
                            let mut wait = pin!(submit(&mut service, &*phase));
                            poll_fn(|cx| {
                                assert!(wait.as_mut().poll(cx).is_pending());
                                Poll::Ready(())
                            })
                            .await;
                        }
                        assert!(matches!(
                            service.submission::<()>(),
                            Ok(Submission::Capacity(_))
                        ));

                        phase.release(a);
                        phase.release(b);
                    }
                    if matches!(scenario, Scenario::Refused) {
                        assert_eq!(
                            submit(
                                &mut service,
                                &Alter {
                                    inner: &mut *phase,
                                    scenario,
                                }
                            )
                            .await
                            .expect("whole request refused"),
                            Submitted::Response
                        );
                        take(&mut service, opcode, 0x0c);

                        assert!(matches!(
                            service.accept(HciCommandPacket::new(opcode, parameters), &mut random),
                            Ok(Accepted::Port)
                        ));
                    }
                    if matches!(scenario, Scenario::WrongReceipt) {
                        assert!(matches!(
                            submit(
                                &mut service,
                                &Alter {
                                    inner: &mut *phase,
                                    scenario
                                }
                            )
                            .await,
                            Err(SubmitError::Receipt)
                        ));
                        assert_eq!(service.completion(), Err(Fault::Poisoned));
                        assert!(service.response().is_none());
                        assert_eq!(
                            complete(&mut service, &mut *phase).await,
                            Err(Fault::Poisoned)
                        );
                        assert!(matches!(
                            submit(&mut service, &*phase).await,
                            Err(SubmitError::Poisoned)
                        ));
                        break 'operation;
                    }
                    assert_eq!(
                        submit(&mut service, &*phase)
                            .await
                            .expect("model admission"),
                        Submitted::AwaitTerminal
                    );
                    assert_eq!(
                        submit(&mut service, &*phase)
                            .await
                            .expect("resume admission"),
                        Submitted::AwaitTerminal
                    );
                    assert!(
                        service
                            .accept(HciCommandPacket::new(RAND, &[]), &mut random)
                            .is_err(),
                        "admission retains credit"
                    );
                    assert!(
                        service.response().is_none(),
                        "start completion waits for P2"
                    );
                    if matches!(scenario, Scenario::CancelWait) {
                        let mut wait = pin!(complete(&mut service, &mut *phase));
                        poll_fn(|cx| {
                            assert!(wait.as_mut().poll(cx).is_pending());
                            Poll::Ready(())
                        })
                        .await;
                    }
                    if matches!(
                        scenario,
                        Scenario::WrongTerminal | Scenario::WrongCompletion | Scenario::StartCount
                    ) {
                        let error = complete(
                            &mut service,
                            &mut TerminalAlter {
                                inner: &mut *phase,
                                scenario,
                            },
                        )
                        .await;
                        assert_eq!(
                            error,
                            Err(if matches!(scenario, Scenario::WrongTerminal) {
                                Fault::Correlation
                            } else {
                                Fault::Completion
                            })
                        );
                        assert!(
                            service.retained_terminal().is_some(),
                            "retain the actual returned owner"
                        );
                        assert!(service.response().is_none());
                        assert_eq!(
                            complete(&mut service, &mut *phase).await,
                            Err(Fault::Poisoned)
                        );
                        break 'operation;
                    }
                    complete(&mut service, &mut *phase)
                        .await
                        .expect("model terminal");
                    if matches!(scenario, Scenario::Indeterminate) {
                        take(&mut service, opcode, 3);

                        break 'operation;
                    }
                    take(&mut service, opcode, 0);

                    let _ = service
                        .accept(HciCommandPacket::new(opcode, parameters), &mut random)
                        .ok()
                        .unwrap();
                    take(&mut service, opcode, 0x3a);
                    self.env
                        .advance(Duration::from_micros(10_000))
                        .expect("virtual time");
                    let _ = service
                        .accept(
                            HciCommandPacket::new(hci::LE_TEST_END_OPCODE, &[]),
                            &mut random,
                        )
                        .ok()
                        .unwrap();
                    assert_eq!(
                        submit(&mut service, &*phase).await.expect("End admission"),
                        Submitted::AwaitTerminal
                    );
                    if matches!(
                        scenario,
                        Scenario::EndMissingCount
                            | Scenario::EndTransmitCount
                            | Scenario::EndTransmitSaturated
                    ) {
                        assert_eq!(
                            complete(
                                &mut service,
                                &mut TerminalAlter {
                                    inner: &mut *phase,
                                    scenario
                                }
                            )
                            .await,
                            Err(Fault::Completion)
                        );
                        assert!(service.retained_terminal().is_some());

                        assert!(service.response().is_none());
                        break 'operation;
                    }
                    if matches!(scenario, Scenario::EndReceiverCount) {
                        complete(
                            &mut service,
                            &mut TerminalAlter {
                                inner: &mut *phase,
                                scenario,
                            },
                        )
                        .await
                        .expect("counted End terminal");
                    } else {
                        complete(&mut service, &mut *phase)
                            .await
                            .expect("End terminal");
                    }
                    assert_eq!(
                        &take(&mut service, hci::LE_TEST_END_OPCODE, 0)[6..],
                        if matches!(scenario, Scenario::EndReceiverCount) {
                            &[0x34, 0x12]
                        } else {
                            &[0, 0]
                        }
                    );
                }
            }
        }
        if service.completion() == Err(Fault::Poisoned) {
            return Ended::fail_stop(owner.fail_stop(), ());
        }
        match owner.disable(&mut |_| ControlFlow::Continue(())).await {
            Ok(owner) => Ended::uninstall(owner, (), ()),
            Err(owner) => Ended::fail_stop(owner.fail_stop(), ()),
        }
    }
}
fn scenario(scenario: Scenario) {
    run(with_local_slot(async |claim| {
        let env = Rc::new(Environment::new(NonZeroU64::new(1_000).unwrap()));
        let mut slot = claim.place();
        let backend = Backend::new(env.clone());
        if matches!(scenario, Scenario::Indeterminate) {
            backend.inject(oer_bluetooth_le_model::Stimulus::Indeterminate);
        }
        let end = LocalExecution
            .scope(
                guarded(slot.register(backend).ok().unwrap()),
                async |front| {
                    run_port(
                        front,
                        Body {
                            env: env.clone(),
                            scenario,
                        },
                    )
                    .await
                },
            )
            .await;
        if matches!(
            scenario,
            Scenario::WrongReceipt
                | Scenario::WrongTerminal
                | Scenario::WrongCompletion
                | Scenario::StartCount
                | Scenario::EndMissingCount
                | Scenario::EndTransmitCount
                | Scenario::EndTransmitSaturated
                | Scenario::Indeterminate
        ) {
            assert!(matches!(end, PortEnd::FailStopped { .. }));
        } else {
            assert!(matches!(end, PortEnd::Uninstalled { .. }));
        }
        assert_eq!(env.active_radios(), 0);
        slot.end_model_world();
    }));
}
#[test]
fn reset_rand_malformed_and_unsupported_commands() {
    scenario(Scenario::Immediate);
}
#[test]
fn receiver_start_end_and_busy_use_actual_port_terminals() {
    scenario(Scenario::Receiver);
}
#[test]
fn transmitter_end_returns_zero() {
    scenario(Scenario::Transmitter);
}
#[test]
fn cancelled_event_wait_retains_the_hci_command() {
    scenario(Scenario::CancelWait);
}
#[test]
fn cancelled_capacity_wait_retains_idle_session_and_command() {
    scenario(Scenario::CancelCapacity);
}
#[test]
fn refused_submission_returns_capacity_and_idle_session() {
    scenario(Scenario::Refused);
}
#[test]
fn foreign_receipt_faults_without_acknowledging_the_host() {
    scenario(Scenario::WrongReceipt);
}
#[test]
fn foreign_terminal_retains_its_actual_owner() {
    scenario(Scenario::WrongTerminal);
}
#[test]
fn inconsistent_completion_retains_its_actual_owner() {
    scenario(Scenario::WrongCompletion);
}
#[test]
fn indeterminate_completion_returns_no_reusable_lease() {
    scenario(Scenario::Indeterminate);
}

enum ResetFlow<Id: SlotIdentity> {
    First,
    Reinstalled(Reset<Root<Id>>),
}
impl<Id: SlotIdentity> PortBody<Id, Backend<Id>> for ResetFlow<Id> {
    type Output = Option<Reset<Root<Id>>>;
    async fn run<'a, 'p>(
        self,
        owner: Owner<'a, 'p, Id, Backend<Id>, Disabled>,
        resources: Resources<'p, Id>,
    ) -> Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'p,
        Id: 'p,
        'p: 'a,
    {
        let Resources {
            seats: [seat, ..],
            session,
        } = resources;
        match self {
            Self::Reinstalled(reset) => {
                let response = reset
                    .after_install(&session)
                    .ok()
                    .expect("fresh joined lease completes Reset");
                assert_eq!(response.as_bytes(), &[0x0e, 4, 1, 3, 12, 0]);
                Ended::uninstall(owner, (), None)
            }
            Self::First => {
                let mut owner = owner.enable().await.ok().expect("model enable");
                let mut service = Service::<_, ModelCaps>::new(session, seat, None);
                let reset = {
                    let mut phase = owner.phase();
                    let mut entropy = Random(Ok([0; 8]));
                    assert!(matches!(
                        service.accept(
                            HciCommandPacket::new(hci::LE_RECEIVER_TEST_V2_OPCODE, &[19, 1, 0]),
                            &mut entropy
                        ),
                        Ok(Accepted::Port)
                    ));
                    assert_eq!(
                        submit(&mut service, &*phase).await.expect("RX admission"),
                        Submitted::AwaitTerminal
                    );
                    complete(&mut service, &mut *phase)
                        .await
                        .expect("RX terminal");
                    take(&mut service, hci::LE_RECEIVER_TEST_V2_OPCODE, 0);
                    let Ok(Accepted::Reset(reset)) =
                        service.accept(HciCommandPacket::new(RESET, &[]), &mut entropy)
                    else {
                        panic!("running Reset must request uninstall")
                    };
                    assert!(service.response().is_none(), "no success before reinstall");
                    reset
                };
                let owner = owner
                    .disable(&mut |_| ControlFlow::Continue(()))
                    .await
                    .ok()
                    .expect("model stop");
                Ended::uninstall(owner, (), Some(reset))
            }
        }
    }
}

#[test]
fn active_reset_is_acknowledged_only_after_joined_reinstallation() {
    run(with_local_slot(async |claim| {
        let env = Rc::new(Environment::new(NonZeroU64::new(1_000).unwrap()));
        let mut slot = claim.place();
        let first = LocalExecution
            .scope(
                guarded(slot.register(Backend::new(env.clone())).ok().unwrap()),
                async |front| run_port(front, ResetFlow::First).await,
            )
            .await;
        let PortEnd::Uninstalled {
            joined,
            output: Some(reset),
        } = first
        else {
            panic!("first reset joins its stopped owner")
        };
        assert_eq!(env.active_radios(), 0);
        let second = LocalExecution
            .scope(
                guarded(
                    slot.reinstall(joined, |vacant| Backend::resume(vacant).unwrap())
                        .ok()
                        .unwrap(),
                ),
                async |front| run_port(front, ResetFlow::Reinstalled(reset)).await,
            )
            .await;
        assert!(matches!(second, PortEnd::Uninstalled { output: None, .. }));
        slot.end_model_world();
    }));
}

#[test]
fn terminal_count_must_match_the_operation_and_direction() {
    for input in [
        Scenario::StartCount,
        Scenario::EndMissingCount,
        Scenario::EndTransmitCount,
        Scenario::EndTransmitSaturated,
        Scenario::EndReceiverCount,
    ] {
        scenario(input);
    }
}
