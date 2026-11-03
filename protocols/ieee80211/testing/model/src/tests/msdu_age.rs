use super::*;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_testing_conformance::{
    self as conformance,
    msdu_age::{self as contract, Case, End, Trace},
};

type Recorded = Rc<RefCell<Option<Trace<Vec<End>>>>>;
struct Exercise {
    case: Case,
    clock: Rc<VirtualClock>,
    trace: Recorded,
}
fn classify<E>(outcome: TxOutcome<E>, delivered: bool) -> End {
    match outcome {
        TxOutcome::Expired { msdus } => End::Expired(msdus),
        TxOutcome::Ended(_) if delivered => End::Delivered,
        _ => panic!("the age fixture expects a known terminal"),
    }
}
fn index<Id: SlotIdentity>(buffer: &ModelAnyBuffer<'_, ModelBackend<Id>>) -> u8 {
    match buffer {
        ModelAnyBuffer::Small(b) => b.slot.index(),
        ModelAnyBuffer::SmallSent(b) => b.buffer.slot.index(),
        _ => panic!("the age fixture uses the small pool"),
    }
}
impl Work for Exercise {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let ModelResources {
            mut tickets,
            mut small,
            mut terminal_access,
            ..
        } = resources;
        let air = phase.held.air.clone();
        let table = EdcaParameterSet::new(
            EdcaParameterSet::STANDARD_DEFAULT.wmm(),
            [Case::LIFETIME; 4],
        );
        phase.station().set_edca(SupportedEdca::new(table).unwrap());
        let first = air.radio_now().unwrap();
        let ticket = tickets.remove(0);
        let buffer = small.remove(0);
        let owners = [ticket.index(), buffer.slot.index()];
        let returned_owners;
        let returned;
        let mut ends = Vec::new();
        {
            let before = self.case == Case::BeforePublication;
            air.hold(WmmAccessCategory::BestEffort, before);
            air.hold_completion(WmmAccessCategory::BestEffort, !before);
            air.answer(if self.case == Case::BetweenRetries {
                AirAnswer::NoResponse
            } else {
                AirAnswer::Ack
            });
            submit(phase, ticket, buffer, None);
            for _ in 0..6 {
                yield_now().await;
            }
            self.clock.advance(Case::LIFETIME).unwrap();
            air.clock_advanced();
            if before {
                air.hold(WmmAccessCategory::BestEffort, false);
            } else {
                air.hold_completion(WmmAccessCategory::BestEffort, false);
            }
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                TxCompleted::Single(done),
            )))) = phase.next_event().await
            else {
                panic!("a single terminal returns once");
            };
            let oer_protocols_ieee80211_contracts_port::TerminalParts {
                ticket,
                payload,
                outcome,
                ..
            } = done.into_parts(&mut terminal_access);
            if self.case == Case::HardwareOwned {
                assert!(outcome.is_delivered());
            }
            let delivered = outcome.is_delivered();
            ends.push(classify(outcome, delivered));
            returned = payload.admitted_at().unwrap();
            let (buffer, _) = payload.into_parts();
            returned_owners = [ticket.index(), index(&buffer)];
        }
        self.trace.replace(Some(Trace {
            ends,
            admitted: first,
            returned,
            owners,
            returned_owners,
            publications: air.contentions().len(),
            expired_msdus: air.expired_msdus(),
            backend_bars: air
                .sent()
                .iter()
                .filter(|f| f.frame.first() == Some(&0x84))
                .count(),
        }));
    }
}
struct Harness;
impl contract::Harness for Harness {
    type Ends = Vec<End>;
    async fn exercise(&mut self, case: Case) -> Trace<Self::Ends> {
        let clock = Rc::new(VirtualClock::<8>::new());
        let (backend, _) = backend_on(clock.clone());
        let trace = Rc::new(RefCell::new(None));
        let _end = run_model(
            backend,
            Body(Exercise {
                case,
                clock,
                trace: trace.clone(),
            }),
        )
        .await;
        trace
            .take()
            .expect("the fixture captured actual age and owners")
    }
}
#[test]
fn msdu_age_passes_the_shared_port_conformance_cases() {
    let report = block_on(contract::check(
        &mut Harness,
        conformance::Environment::Model,
    ))
    .unwrap();
    assert_eq!(usize::from(report.passed), Case::ALL.len());
    let report = block_on(contract::check(
        &mut Harness,
        conformance::Environment::Hardware,
    ))
    .unwrap();
    assert_eq!(report.passed, 0);
    assert_eq!(usize::from(report.model_only), Case::ALL.len());
}
#[test]
fn model_time_uses_the_injected_clock_and_an_offset_affine_radio_relation() {
    let clock = Rc::new(VirtualClock::<8>::new());
    let (_, air) = backend_on(clock.clone());
    let (relation, sample) = air.clock_relation();
    assert_eq!(
        relation.rate(),
        oer_protocols_radio_port::RelationRate::Affine { drift_ppm: 0 }
    );
    let initial = air.radio_now().unwrap();
    assert_ne!(initial.as_micros(), clock.now().as_micros());
    for _ in 0..10 {
        assert_eq!(air.radio_now().unwrap(), initial);
    }
    clock.advance(Duration::from_micros(7)).unwrap();
    let projected = relation
        .project(clock.now(), sample.generation, &sample)
        .unwrap();
    assert_eq!(air.radio_now().unwrap(), projected.at);
    assert_eq!(
        projected
            .at
            .checked_duration_since(initial)
            .unwrap()
            .as_micros(),
        7
    );
}

#[test]
fn port_now_projects_the_supplied_clock_and_renewal_starts_a_new_msdu() {
    struct Probe(Rc<VirtualClock>, Rc<ModelAir>);
    impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id>> for Probe {
        type Output = ();
        async fn run<'a, 'id>(
            self,
            owner: Owner<'a, 'id, Id, ModelBackend<Id>, Disabled>,
            mut resources: ModelResources<'id, ModelBackend<Id>>,
        ) -> Ended<Id, ModelBackend<Id>, ()>
        where
            ModelBackend<Id>: 'id,
            Id: 'id,
            'id: 'a,
        {
            assert_eq!(
                owner.now().await.unwrap().unwrap(),
                self.1.radio_now().unwrap()
            );
            let initial = owner.now().await.unwrap().unwrap();
            self.0.advance(Duration::from_micros(9)).unwrap();
            assert_eq!(
                owner
                    .now()
                    .await
                    .unwrap()
                    .unwrap()
                    .checked_duration_since(initial)
                    .unwrap()
                    .as_micros(),
                9
            );
            let mut buffer = resources.small.remove(0);
            let first_admission = self.1.radio_now().unwrap();
            buffer.admit(self.0.now());
            buffer.project_admission(&self.1).unwrap();
            self.0.advance(Duration::from_micros(3)).unwrap();
            buffer.admit(self.0.now());
            assert_eq!(buffer.admission(), Some(first_admission));
            let (mut buffer, renewed) =
                oer_protocols_ieee80211_contracts_port::ReturnedMpdu::<_, ()>::Fresh(buffer)
                    .into_fresh(|()| panic!("fresh renewal consumes no Sent owner"));
            assert_eq!(renewed, Ok(()));
            assert_eq!(buffer.admission(), None);
            let next = owner.now().await.unwrap().unwrap();
            buffer.admit(self.0.now());
            buffer.project_admission(&self.1).unwrap();
            assert_eq!(buffer.admission(), Some(next));
            Ended::fail_stop(owner.fail_stop(), ())
        }
    }
    let clock = Rc::new(VirtualClock::<8>::new());
    let (backend, air) = backend_on(clock.clone());
    let _end = block_on(run_model(backend, Probe(clock, air)));
}

#[test]
fn queued_admission_projects_the_original_monotonic_instant_and_age_survives_offset_changes() {
    struct OffsetChange {
        clock: Rc<VirtualClock>,
        expire: bool,
    }
    impl Work for OffsetChange {
        async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
            self,
            phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
            mut resources: ModelResources<'id, ModelBackend<Id>>,
        ) {
            let air = phase.held.air.clone();
            let category = WmmAccessCategory::BestEffort;
            let lifetime = Duration::from_micros(100);
            let table =
                EdcaParameterSet::new(EdcaParameterSet::STANDARD_DEFAULT.wmm(), [lifetime; 4]);
            phase.station().set_edca(SupportedEdca::new(table).unwrap());
            air.hold(category, true);
            air.answer(AirAnswer::Ack);
            let admitted = self.clock.now();
            submit(
                phase,
                resources.tickets.remove(0),
                resources.small.remove(0),
                None,
            );
            // No await has given P2 the queued command yet. A calibration
            // update must project admission, rather than sampling dequeue.
            self.clock.advance(Duration::from_micros(7)).unwrap();
            let (relation, mut sample) = air.clock_relation();
            sample.source = self.clock.now();
            sample.target = RadioInstant::from_micros(100_000);
            air.time.sample.set(sample);
            let expected = relation
                .project(admitted, sample.generation, &sample)
                .unwrap()
                .at;
            air.clock_advanced();
            for _ in 0..4 {
                yield_now().await;
            }
            assert!(air.sent().is_empty());
            // After dequeue, move the radio relation backwards. Radio age
            // would become negative; monotonic age still expires at equality.
            self.clock
                .advance(Duration::from_micros(if self.expire { 93 } else { 92 }))
                .unwrap();
            sample.source = self.clock.now();
            sample.target = RadioInstant::from_micros(1_000);
            air.time.sample.set(sample);
            air.clock_advanced();
            air.hold(category, false);
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                TxCompleted::Single(done),
            )))) = phase.next_event().await
            else {
                panic!("the queued attempt returns its terminal");
            };
            let parts = done.into_parts(&mut resources.terminal_access);
            assert_eq!(parts.payload.admitted_at(), Some(expected));
            assert_ne!(Some(expected), air.radio_now().ok());
            assert_eq!(
                matches!(parts.outcome, TxOutcome::Expired { msdus: 1 }),
                self.expire
            );
            if !self.expire {
                assert!(parts.outcome.is_delivered());
            }
        }
    }
    for expire in [false, true] {
        let clock = Rc::new(VirtualClock::<8>::new());
        let (backend, _) = backend_on(clock.clone());
        let end = block_on(run_model(backend, Body(OffsetChange { clock, expire })));
        assert!(matches!(end, crate::fixture::Report::Joined(())));
    }
}
