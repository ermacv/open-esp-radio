//! A standalone port consumer: no runtime, chip, allocator or executor API is
//! needed to store observations and invoke the shared case runners.
#![cfg(feature = "test-support")]

use core::{
    future::Future,
    num::NonZeroU16,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_ieee80211_port::{
    AggregateEnd, Air, IndeterminateCause, PhyRate, SingleEnd, SingleResponse, TxOutcome,
    conformance::{
        self, Environment, MpduState,
        outcomes::{self, Case, Form, Identity, Outcome, Trace},
    },
};
use oer_ieee80211_wire::{phy::LegacyRate, sequence::SequenceNumber};

#[test]
fn age_verification_rejects_reset_age_lost_owners_and_backend_bars() {
    use conformance::msdu_age::{self, Case, End, Trace, Violation};
    let good = || Trace {
        ends: &[End::Expired(1)][..],
        publications: 0,
        expired_msdus: 1,
        admitted: [oer_ieee80211_port::Ieee80211Instant::from_micros(99); 2],
        returned: [oer_ieee80211_port::Ieee80211Instant::from_micros(99); 2],
        owners: [1, 2, 2],
        returned_owners: [1, 2, 2],
        backend_bars: 0,
        core_bar: None,
    };
    assert_eq!(msdu_age::verify(Case::BeforePublication, good()), Ok(()));
    let mut trace = good();
    trace.returned[0] = oer_ieee80211_port::Ieee80211Instant::from_micros(100);
    assert_eq!(
        msdu_age::verify(Case::BeforePublication, trace),
        Err(Violation::Age)
    );
    let mut trace = good();
    trace.returned_owners[1] = 3;
    assert_eq!(
        msdu_age::verify(Case::BeforePublication, trace),
        Err(Violation::Owners)
    );
    let mut trace = good();
    trace.backend_bars = 1;
    assert_eq!(
        msdu_age::verify(Case::BeforePublication, trace),
        Err(Violation::Publication)
    );
    let trace = Trace {
        ends: &[End::Delivered, End::Expired(1), End::Expired(1)][..],
        publications: 1,
        expired_msdus: 2,
        ..good()
    };
    assert_eq!(
        msdu_age::verify(Case::Reformation, trace),
        Err(Violation::Bar)
    );
}

#[test]
fn delivery_before_expiry_requires_the_bar_to_follow_the_new_head() {
    use conformance::msdu_age::{self, Case, CoreBar, End, Trace, Violation};
    let trace = |bar_start: u16| Trace {
        ends: [End::Delivered, End::Expired(1)],
        publications: 1,
        expired_msdus: 1,
        admitted: [oer_ieee80211_port::Ieee80211Instant::from_micros(99); 2],
        returned: [oer_ieee80211_port::Ieee80211Instant::from_micros(99); 2],
        owners: [1, 2, 3],
        returned_owners: [1, 2, 3],
        backend_bars: 0,
        core_bar: Some(CoreBar {
            head: SequenceNumber::new(7).unwrap(),
            tid: 0,
            addresses: [[2, 0, 0, 0, 0, 2], [2, 0, 0, 0, 0, 1]],
            frame: oer_ieee80211_wire::block_ack::encode_block_ack_request(
                [2, 0, 0, 0, 0, 2],
                [2, 0, 0, 0, 0, 1],
                0,
                SequenceNumber::new(bar_start).unwrap(),
            ),
        }),
    };
    assert_eq!(
        msdu_age::verify(Case::DeliveryBeforeExpiry, trace(8)),
        Ok(())
    );
    assert_eq!(
        msdu_age::verify(Case::DeliveryBeforeExpiry, trace(7)),
        Err(Violation::Bar)
    );
    let mut missing = trace(8);
    missing.core_bar = None;
    assert_eq!(
        msdu_age::verify(Case::DeliveryBeforeExpiry, missing),
        Err(Violation::Bar)
    );
}

fn ready<T>(future: impl Future<Output = T>) -> T {
    let mut future = pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the array fixture completes without I/O"),
    }
}
fn trace(case: Case) -> Trace<(), (), ()> {
    let air = Air {
        rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
        transmissions: NonZeroU16::MIN,
    };
    let outcome = match case {
        Case::Ack => Outcome::Single(TxOutcome::Ended(SingleEnd::Delivered {
            air,
            response: SingleResponse::Ack(()),
        })),
        Case::BlockAck => Outcome::Aggregate(TxOutcome::Ended(AggregateEnd::Delivered {
            air,
            report: oer_ieee80211_port::BlockAckReport::new(SequenceNumber::ZERO, 1),
            ack: (),
        })),
        Case::NoResponse(Form::Single) => {
            Outcome::Single(TxOutcome::Ended(SingleEnd::AckTimeout(air)))
        }
        Case::NoResponse(Form::Aggregate) => {
            Outcome::Aggregate(TxOutcome::Ended(AggregateEnd::AckTimeout(air)))
        }
        Case::Collision(Form::Single) => {
            Outcome::Single(TxOutcome::Ended(SingleEnd::ContentionLimit(None)))
        }
        Case::Collision(Form::Aggregate) => {
            Outcome::Aggregate(TxOutcome::Ended(AggregateEnd::ContentionLimit(None)))
        }
        Case::AirDenied(Form::Single) => Outcome::Single(TxOutcome::Ended(SingleEnd::AirDenied {
            air: None,
            denial: (),
        })),
        Case::AirDenied(Form::Aggregate) => {
            Outcome::Aggregate(TxOutcome::Ended(AggregateEnd::AirDenied {
                air: None,
                denial: (),
            }))
        }
        Case::HardwareTimeout(Form::Single) => Outcome::Single(TxOutcome::Indeterminate(
            IndeterminateCause::HardwareTimeout,
        )),
        Case::HardwareTimeout(Form::Aggregate) => Outcome::Aggregate(TxOutcome::Indeterminate(
            IndeterminateCause::HardwareTimeout,
        )),
    };
    Trace {
        admitted: Identity {
            ticket: 7,
            buffer: 3,
        },
        returned: Identity {
            ticket: 7,
            buffer: 3,
        },
        state: MpduState::Sent,
        rate: air.rate,
        sequence: SequenceNumber::ZERO,
        publications: usize::from(!matches!(case, Case::AirDenied(_))),
        outcome,
    }
}
#[derive(Default)]
struct ArrayFixture {
    calls: heapless::Vec<Case, 10>,
}
impl outcomes::Harness for ArrayFixture {
    type Ack = ();
    type BlockAck = ();
    type AirDenied = ();
    async fn exercise(&mut self, case: Case) -> Trace<(), (), ()> {
        self.calls
            .push(case)
            .expect("each declared case is exercised once");
        trace(case)
    }
}
#[test]
fn hardware_selection_reports_and_excludes_unproducible_outcomes() {
    let mut fixture = ArrayFixture::default();
    let report = ready(outcomes::check(&mut fixture, Environment::Hardware))
        .expect("the selected hardware observations satisfy the shared verifier");
    assert_eq!(
        fixture.calls.as_slice(),
        [
            Case::Ack,
            Case::BlockAck,
            Case::NoResponse(Form::Single),
            Case::NoResponse(Form::Aggregate)
        ]
    );
    assert_eq!(usize::from(report.passed), fixture.calls.len());
    assert_eq!(
        usize::from(report.model_only),
        Case::ALL.len() - fixture.calls.len()
    );
}
#[test]
fn model_selection_exercises_injected_faults_instead_of_silently_excluding_them() {
    let mut fixture = ArrayFixture::default();
    let report = ready(outcomes::check(&mut fixture, Environment::Model))
        .expect("the selected model observations satisfy the shared verifier");
    for form in [Form::Single, Form::Aggregate] {
        assert!(fixture.calls.contains(&Case::Collision(form)));
        assert!(fixture.calls.contains(&Case::AirDenied(form)));
        assert!(fixture.calls.contains(&Case::HardwareTimeout(form)));
    }
    assert_eq!(usize::from(report.passed), fixture.calls.len());
    assert_eq!(report.model_only, 0);
}
#[test]
fn outcome_verification_rejects_wrong_owners_state_form_and_extra_publications() {
    let mut wrong = trace(Case::Ack);
    wrong.returned.buffer = 4;
    assert_eq!(
        outcomes::verify(Case::Ack, wrong),
        Err(outcomes::Violation::Owners)
    );
    let mut wrong = trace(Case::Ack);
    wrong.state = MpduState::Fresh;
    assert_eq!(
        outcomes::verify(Case::Ack, wrong),
        Err(outcomes::Violation::State)
    );
    assert_eq!(
        outcomes::verify(Case::BlockAck, trace(Case::Ack)),
        Err(outcomes::Violation::Outcome)
    );
    let mut wrong = trace(Case::Ack);
    wrong.publications = 2;
    assert_eq!(
        outcomes::verify(Case::Ack, wrong),
        Err(outcomes::Violation::Publication)
    );
}
#[test]
fn retained_mpdu_verification_accepts_array_and_bounded_observation_storage() {
    use conformance::{BarrierCase, BarrierEvent, BarrierTrace, HeldSnapshot};
    let mut first = [0x5a; 40];
    first[0] = 0x08;
    first[1] = 0x41;
    let mut retry = first;
    assert!(oer_ieee80211_wire::frame::set_retry_bit(&mut retry));
    let mut retried_frames: heapless::Vec<[u8; 40], 1> = heapless::Vec::new();
    retried_frames
        .push(retry)
        .expect("one retry fits the fixture");
    let snapshot = || HeldSnapshot {
        publications: 1,
        events: 0,
    };
    let trace = BarrierTrace {
        tickets: [1, 2],
        before_request: snapshot(),
        before_result: snapshot(),
        just_before_timeout: None,
        publications_at_barrier: 1,
        events: [
            BarrierEvent::<(), ()>::Tx {
                ticket: 1,
                state: MpduState::Sent,
                outcome: TxOutcome::Withdrawn(oer_ieee80211_port::WithdrawCause::GateClosed),
            },
            BarrierEvent::Tx {
                ticket: 2,
                state: MpduState::Fresh,
                outcome: TxOutcome::Withdrawn(oer_ieee80211_port::WithdrawCause::GateClosed),
            },
            BarrierEvent::GateClosed,
        ],
        first_frame: first,
        retried_frames,
        retry_terminal: None,
    };
    assert_eq!(
        conformance::verify_barrier(BarrierCase::GateRetry, trace),
        Ok(())
    );
}

#[test]
fn hardware_group_runner_does_not_require_injected_ingress_or_stalled_commands() {
    use conformance::group_keys::{self, Completion, Owners, ProgressCase, ProgressTrace, Timing};
    use oer_time::{Clock, Duration};

    struct GroupFixture<'a> {
        clock: &'a oer_time_virtual::VirtualClock,
        calls: heapless::Vec<ProgressCase, 13>,
    }
    impl group_keys::Harness for GroupFixture<'_> {
        type Events = [group_keys::Event; 0];
        type Completions = heapless::Vec<Completion, 2>;
        type Retained = heapless::Vec<Owners, 1>;
        const GROUP_COMMAND_BOUND: Duration = Duration::from_millis(1);
        const SCENARIO_BOUND: Duration = Duration::from_millis(10);

        async fn exercise(&mut self, _: group_keys::Case) -> group_keys::Trace<Self::Events> {
            panic!("hardware execution must exclude independently injected ingress");
        }
        async fn exercise_progress(
            &mut self,
            case: ProgressCase,
        ) -> ProgressTrace<Self::Completions, Self::Retained> {
            self.calls.push(case).expect("each progress case fits");
            let admitted = Owners {
                slot: 2,
                permission: 0,
            };
            let mut events = heapless::Vec::new();
            if case == ProgressCase::ReplacementRollback {
                let mut retained = heapless::Vec::new();
                retained.push(admitted).expect("retained owner fits");
                return ProgressTrace {
                    rollback: Some(group_keys::Rollback {
                        previous_key_id: 1,
                        attempted_key_id: 2,
                        restored: true,
                    }),
                    admitted,
                    events,
                    retained,
                    poisoned: true,
                    timing: Timing {
                        admitted_at: self.clock.now(),
                        completed_at: self.clock.now(),
                    },
                    deadline_woke: false,
                    published: None,
                    blocked_while_published: false,
                };
            }
            let (mutation, published) = match case {
                ProgressCase::Disable { mutation, .. } => (mutation, None),
                ProgressCase::PublishedRemoval => {
                    let owners = group_keys::TxOwners {
                        ticket: 1,
                        buffer: 3,
                    };
                    events.push(Completion::Transmit(owners)).expect("TX fits");
                    (group_keys::Mutation::Remove, Some(owners))
                }
                ProgressCase::Expire(_) => {
                    panic!("hardware execution must exclude stalled commands");
                }
                ProgressCase::ReplacementRollback => unreachable!("handled above"),
            };
            events
                .push(Completion::Group {
                    mutation,
                    owners: admitted,
                })
                .expect("the group terminal fits after its published use");
            ProgressTrace {
                rollback: None,
                admitted,
                timing: Timing {
                    admitted_at: self.clock.now(),
                    completed_at: self.clock.now(),
                },
                events,
                retained: heapless::Vec::new(),
                poisoned: false,
                deadline_woke: false,
                published,
                blocked_while_published: published.is_some(),
            }
        }
    }

    let clock: oer_time_virtual::VirtualClock = oer_time_virtual::VirtualClock::new();
    let mut fixture = GroupFixture {
        clock: &clock,
        calls: heapless::Vec::new(),
    };
    let report = ready(group_keys::check(
        &mut fixture,
        &clock,
        Environment::Hardware,
    ))
    .expect("hardware-selected progress traces satisfy the shared verifier");
    assert!(fixture.calls.contains(&ProgressCase::PublishedRemoval));
    assert!(
        fixture
            .calls
            .iter()
            .any(|case| matches!(case, ProgressCase::Disable { applied: false, .. }))
    );
    assert!(
        fixture
            .calls
            .iter()
            .any(|case| matches!(case, ProgressCase::Disable { applied: true, .. }))
    );
    assert_eq!(usize::from(report.passed), fixture.calls.len());
    assert_eq!(
        usize::from(report.model_only),
        group_keys::Case::ALL.len() + group_keys::Mutation::ALL.len()
    );
}
