#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]

use super::*;
use core::{
    future::Future,
    num::NonZeroU64,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_bluetooth_le_model::{Backend, Environment, ModelCaps, Resources};
use oer_bluetooth_le_port::LePhase;
use oer_radio_port::conformance::registration::guarded;
use oer_radio_port::*;
use std::rc::Rc;

struct Random(Result<[u8; 8], EntropyUnavailable>);
impl Entropy for Random {
    fn random(&mut self) -> Result<[u8; 8], EntropyUnavailable> {
        self.0
    }
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
#[derive(Clone, Copy)]
enum Scenario {
    MissingConfig,
    WrongConfig,
    MissingLease,
    OldReset,
}

struct Body {
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
        let mut owner = owner.enable().await.ok().expect("model enable");
        let Resources {
            seats: [seat, mut first, mut second],
            session,
        } = resources;
        let mut service = Service::<_, ModelCaps>::new(session, seat, None);
        let scenario = self.scenario;
        if matches!(self.scenario, Scenario::OldReset) {
            let Ok(Accepted::Reset(reset)) =
                service.accept(HciCommandPacket::new(RESET, &[]), &mut Random(Ok([0; 8])))
            else {
                panic!("Reset action")
            };
            let Some(Lease::Idle(session)) = &service.lease else {
                panic!("original idle lease")
            };
            assert!(
                reset.after_install(session).is_err(),
                "old epoch cannot complete Reset"
            );
            assert!(service.response().is_none());
            return Ended::fail_stop(owner.fail_stop(), ());
        }
        let opcode = hci::LE_RECEIVER_TEST_V1_OPCODE;
        assert!(matches!(
            service.accept(
                HciCommandPacket::new(opcode, &[19]),
                &mut Random(Ok([0; 8]))
            ),
            Ok(Accepted::Port)
        ));
        {
            let phase = owner.phase();
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
            let Submission::Capacity(seat) = service.submission::<()>().unwrap() else {
                panic!("unadmitted command retains seat")
            };
            let capacity = phase.take(seat).await;
            let Err(RequestRefusal {
                capacity,
                error:
                    SubmitError::InvalidState {
                        command,
                        configuration,
                        session,
                    },
            }) = service.request::<()>(capacity)
            else {
                panic!("inconsistent private state must be retained as a typed error")
            };
            phase.release(capacity);
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
                service.submission::<()>(),
                Err(SubmitError::Poisoned)
            ));
            assert_eq!(service.completion().map(|_| ()), Err(Fault::Poisoned));
            // Both capacity permits are available: refusal returned its permit.
            let a = phase.take(&mut first).await;
            let b = phase.take(&mut second).await;
            phase.release(a);
            phase.release(b);
        }
        Ended::fail_stop(owner.fail_stop(), ())
    }
}

#[test]
fn submission_retains_mismatched_configuration_and_session_and_returns_capacity() {
    for scenario in [
        Scenario::MissingConfig,
        Scenario::WrongConfig,
        Scenario::MissingLease,
    ] {
        scenario_test(scenario);
    }
}
#[test]
fn reset_rejects_the_original_installation_identity() {
    scenario_test(Scenario::OldReset);
}
fn scenario_test(scenario: Scenario) {
    run(with_local_slot(async |claim| {
        let env = Rc::new(Environment::new(NonZeroU64::new(1000).unwrap()));
        let mut slot = claim.place();
        let end = LocalExecution
            .scope(
                guarded(slot.register(Backend::new(env.clone())).ok().unwrap()),
                async |front| run_port(front, Body { scenario }).await,
            )
            .await;
        assert!(matches!(end, PortEnd::FailStopped { .. }));
        assert_eq!(env.active_radios(), 0);
        slot.end_model_world();
    }));
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
