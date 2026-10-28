#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]

use super::*;
use core::{
    future::{Future, poll_fn},
    num::NonZeroU64,
    ops::ControlFlow,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_bluetooth_le_model::{Backend, Environment, ModelCaps, Resources};
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
    use oer_bluetooth_le_radio::{LeBand, LeTxCeiling};
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
    MissingConfig,
    WrongConfig,
    MissingLease,
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
        resources: Result<Resources<'p, Id>, oer_bluetooth_le_model::Fault>,
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
        } = resources.expect("prepared initial model resources");
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
            assert_eq!(service.complete(&mut *phase).await, Err(Fault::NoCommand));
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
                    let Ok(Accepted::Reset(reset)) =
                        service.accept(HciCommandPacket::new(RESET, &[]), &mut random)
                    else {
                        panic!("Reset must leave the old body")
                    };
                    let Some(Lease::Idle(ref session)) = service.lease else {
                        panic!("idle lease")
                    };
                    assert!(
                        reset.after_install(session).is_err(),
                        "old lease cannot complete Reset"
                    );
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
                    if matches!(
                        scenario,
                        Scenario::MissingConfig | Scenario::WrongConfig | Scenario::MissingLease
                    ) {
                        let pending = service.pending.as_mut().unwrap();
                        let original_config = pending.config.take();
                        pending.config = if matches!(scenario, Scenario::WrongConfig) {
                            Some(Config::End)
                        } else if matches!(scenario, Scenario::MissingLease) {
                            original_config
                        } else {
                            None
                        };
                        let retained_lease = if matches!(scenario, Scenario::MissingLease) {
                            service.lease.take()
                        } else {
                            None
                        };
                        let Err(SubmitError::InvalidState {
                            command,
                            configuration,
                            session,
                        }) = service.submit(&*phase).await
                        else {
                            panic!("inconsistent private state must be retained as a typed error")
                        };
                        assert_eq!(command.opcode(), opcode);
                        assert_eq!(
                            configuration,
                            match scenario {
                                Scenario::WrongConfig => Some(SubmissionConfiguration::End),
                                Scenario::MissingLease => Some(SubmissionConfiguration::Receive),
                                _ => None,
                            }
                        );
                        assert_eq!(
                            session,
                            if matches!(scenario, Scenario::MissingLease) {
                                SubmissionSession::Missing
                            } else {
                                SubmissionSession::Idle
                            }
                        );
                        assert!(service.pending.is_some());
                        assert_eq!(service.lease.is_some(), retained_lease.is_none());
                        assert!(service.response().is_none());
                        assert!(matches!(
                            service.submit(&*phase).await,
                            Err(SubmitError::Poisoned)
                        ));
                        assert_eq!(service.complete(&mut *phase).await, Err(Fault::Poisoned));
                        // Both capacity permits are available: refusal returned its permit.
                        let a = phase.take(&mut first).await;
                        let b = phase.take(&mut second).await;
                        phase.release(a);
                        phase.release(b);
                        break 'operation;
                    }
                    if let Some((a, b)) = held {
                        {
                            let mut wait = pin!(service.submit(&*phase));
                            poll_fn(|cx| {
                                assert!(wait.as_mut().poll(cx).is_pending());
                                Poll::Ready(())
                            })
                            .await;
                        }
                        assert!(service.pending.as_ref().unwrap().admitted.is_none());
                        assert!(matches!(service.lease, Some(Lease::Idle(_))));
                        phase.release(a);
                        phase.release(b);
                    }
                    if matches!(scenario, Scenario::Refused) {
                        assert_eq!(
                            service
                                .submit(&Alter {
                                    inner: &mut *phase,
                                    scenario,
                                })
                                .await
                                .expect("whole request refused"),
                            Submitted::Response
                        );
                        take(&mut service, opcode, 0x0c);
                        assert!(matches!(service.lease, Some(Lease::Idle(_))));
                        assert!(matches!(
                            service.accept(HciCommandPacket::new(opcode, parameters), &mut random),
                            Ok(Accepted::Port)
                        ));
                    }
                    if matches!(scenario, Scenario::WrongReceipt) {
                        assert!(matches!(
                            service
                                .submit(&Alter {
                                    inner: &mut *phase,
                                    scenario
                                })
                                .await,
                            Err(SubmitError::Receipt)
                        ));
                        assert!(service.pending.is_some());
                        assert!(service.response().is_none());
                        assert_eq!(service.complete(&mut *phase).await, Err(Fault::Poisoned));
                        assert!(matches!(
                            service.submit(&*phase).await,
                            Err(SubmitError::Poisoned)
                        ));
                        break 'operation;
                    }
                    assert_eq!(
                        service.submit(&*phase).await.expect("model admission"),
                        Submitted::AwaitTerminal
                    );
                    assert!(
                        service.response().is_none(),
                        "start completion waits for P2"
                    );
                    if matches!(scenario, Scenario::CancelWait) {
                        let mut wait = pin!(service.complete(&mut *phase));
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
                        let error = service
                            .complete(&mut TerminalAlter {
                                inner: &mut *phase,
                                scenario,
                            })
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
                            service.retained.is_some(),
                            "retain the actual returned owner"
                        );
                        assert!(service.response().is_none());
                        assert_eq!(service.complete(&mut *phase).await, Err(Fault::Poisoned));
                        break 'operation;
                    }
                    service.complete(&mut *phase).await.expect("model terminal");
                    if matches!(scenario, Scenario::Indeterminate) {
                        take(&mut service, opcode, 3);
                        assert!(service.lease.is_none());
                        break 'operation;
                    }
                    take(&mut service, opcode, 0);
                    assert!(matches!(service.lease, Some(Lease::Running(_))));
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
                        service.submit(&*phase).await.expect("End admission"),
                        Submitted::AwaitTerminal
                    );
                    if matches!(
                        scenario,
                        Scenario::EndMissingCount
                            | Scenario::EndTransmitCount
                            | Scenario::EndTransmitSaturated
                    ) {
                        assert_eq!(
                            service
                                .complete(&mut TerminalAlter {
                                    inner: &mut *phase,
                                    scenario
                                })
                                .await,
                            Err(Fault::Completion)
                        );
                        assert!(service.retained.is_some());
                        assert!(service.pending.is_none());
                        assert!(service.lease.is_none());
                        assert!(service.response().is_none());
                        break 'operation;
                    }
                    if matches!(scenario, Scenario::EndReceiverCount) {
                        service
                            .complete(&mut TerminalAlter {
                                inner: &mut *phase,
                                scenario,
                            })
                            .await
                            .expect("counted End terminal");
                    } else {
                        service.complete(&mut *phase).await.expect("End terminal");
                    }
                    assert_eq!(
                        &take(&mut service, hci::LE_TEST_END_OPCODE, 0)[6..],
                        if matches!(scenario, Scenario::EndReceiverCount) {
                            &[0x34, 0x12]
                        } else {
                            &[0, 0]
                        }
                    );
                    assert!(matches!(service.lease, Some(Lease::Idle(_))));
                }
            }
        }
        if service.faulted {
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
            .scope(slot.register(backend).ok().unwrap(), async |front| {
                run_port(
                    front,
                    Body {
                        env: env.clone(),
                        scenario,
                    },
                )
                .await
            })
            .await;
        if matches!(
            scenario,
            Scenario::WrongReceipt
                | Scenario::WrongTerminal
                | Scenario::WrongCompletion
                | Scenario::MissingConfig
                | Scenario::WrongConfig
                | Scenario::MissingLease
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
        resources: Result<Resources<'p, Id>, oer_bluetooth_le_model::Fault>,
    ) -> Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'p,
        Id: 'p,
        'p: 'a,
    {
        let Resources {
            seats: [seat, ..],
            session,
        } = resources.expect("prepared initial model resources");
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
                        service.submit(&*phase).await.expect("RX admission"),
                        Submitted::AwaitTerminal
                    );
                    service.complete(&mut *phase).await.expect("RX terminal");
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
                slot.register(Backend::new(env.clone())).ok().unwrap(),
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
                slot.reinstall(joined, |vacant| Backend::resume(vacant).unwrap())
                    .ok()
                    .unwrap(),
                async |front| run_port(front, ResetFlow::Reinstalled(reset)).await,
            )
            .await;
        assert!(matches!(second, PortEnd::Uninstalled { output: None, .. }));
        slot.end_model_world();
    }));
}

#[test]
fn submission_retains_mismatched_configuration_and_session_and_returns_capacity() {
    for input in [
        Scenario::MissingConfig,
        Scenario::WrongConfig,
        Scenario::MissingLease,
    ] {
        scenario(input);
    }
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

#[test]
fn dtm_parameter_conversion_checks_the_complete_octet_domain() {
    for value in u8::MIN..=u8::MAX {
        assert_eq!(channel(value).is_ok(), value < 40);
        assert_eq!(payload(value).is_ok(), value < 8);
    }
    assert_eq!(channel(40), Err(HciError::INVALID_HCI_PARAMETERS));
    assert_eq!(payload(8), Err(HciError::INVALID_HCI_PARAMETERS));
}
