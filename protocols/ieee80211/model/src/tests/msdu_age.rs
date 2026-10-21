use super::*;
use oer_ieee80211_port::conformance::{
    self,
    msdu_age::{self as contract, Case, End, Trace},
};
use oer_ieee80211_port::{AggregateAttempt, MpduStartSpacing};
use oer_time_virtual::VirtualClock;

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
fn index(buffer: &ModelAnyBuffer<'_>) -> u8 {
    match buffer {
        ModelAnyBuffer::Small(b) => b.slot.index(),
        ModelAnyBuffer::SmallSent(b) => b.buffer.slot.index(),
        _ => panic!("the age fixture uses the small pool"),
    }
}
fn send_aggregate<'k, 'p, 'id: 'k + 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    ticket: Slot<'id, ModelTickets, ModelPool<'id, 1>>,
    aggregate: ModelAggregate<'k, 'p, 'id, Station, ModelCaps, (), NonEmptyAggregate>,
) {
    phase
        .station()
        .submit_ampdu(
            (AggregateAttempt {
                context: (),
                ticket,
                aggregate,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                min_mpdu_start_spacing: MpduStartSpacing::new(0).unwrap(),
            })
            .admit(),
        )
        .unwrap();
}
async fn returned_aggregate<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    access: &mut oer_ieee80211_port::TerminalAccess<'id, ModelTickets, ModelPool<'id, 1>>,
) -> (
    Slot<'id, ModelTickets, ModelPool<'id, 1>>,
    ModelSubframes<'id>,
    End,
    Option<oer_ieee80211_port::BlockAckReport>,
) {
    let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Aggregate(
        done,
    ))))) = phase.next_event().await
    else {
        panic!("aggregate terminal returns its owners");
    };
    let oer_ieee80211_port::TerminalParts {
        ticket,
        payload,
        outcome,
        ..
    } = done.into_parts(access);
    let delivered = matches!(
        outcome,
        TxOutcome::Ended(oer_ieee80211_port::AggregateEnd::Delivered { .. })
    );
    let report = match outcome {
        TxOutcome::Ended(oer_ieee80211_port::AggregateEnd::Delivered { report, .. }) => {
            Some(report)
        }
        _ => None,
    };
    (ticket, payload, classify(outcome, delivered), report)
}
fn active_originator(
    start: SequenceNumber,
    now: Instant,
) -> oer_ieee80211_upper_mac::block_ack::TxBlockAckOriginator<1> {
    use oer_ieee80211_upper_mac::block_ack::{
        TxBlockAckOriginator, TxBlockAckOriginatorConfig, TxBlockAckOriginatorPolicy,
        next_nonzero_dialog_token,
    };
    let mut core = TxBlockAckOriginator::new(
        TxBlockAckOriginatorPolicy {
            tids: &[0],
            first_dialog_token: 1,
            next_dialog_token: next_nonzero_dialog_token,
        },
        TxBlockAckOriginatorConfig {
            window: 32,
            negotiation_timeout: Duration::from_micros(1000),
            amsdu_tids: 0,
        },
    )
    .unwrap();
    let request = core.begin(0, start, now).unwrap();
    core.on_response_action(
        oer_ieee80211_wire::block_ack::BlockAckAction::AddbaResponse {
            dialog_token: request.dialog_token,
            status: 0,
            tid: 0,
            immediate: true,
            amsdu: false,
            window: 32,
            timeout_tu: 0,
        },
    )
    .unwrap();
    core
}
impl Work for Exercise {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
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
        let mut owners = [ticket.index(), buffer.slot.index(), buffer.slot.index()];
        let returned_owners;
        let mut admitted = [first; 2];
        let returned;
        let mut ends = Vec::new();
        let mut core_bar = None;
        macro_rules! empty {
            () => {{
                let rate = phase
                    .rate(PhyRate::Ht(
                        oer_ieee80211_wire::phy::HtRate::new(
                            HtMcs::new(0).unwrap(),
                            oer_ieee80211_wire::phy::PpduBandwidth::Mhz20,
                            false,
                        )
                        .unwrap(),
                    ))
                    .unwrap();
                let mut station = phase.station();
                let rate = station.ampdu_rate(rate).ok().unwrap();
                station.aggregate(
                    IndividualAddress::new(PEER).unwrap(),
                    Tid::new(0).unwrap(),
                    rate,
                    TxProtection::Open,
                )
            }};
        }
        if self.case == Case::DeliveryBeforeExpiry {
            let mut originator =
                active_originator(SequenceNumber::new(6).unwrap(), self.clock.now());
            let missing = small.remove(0);
            owners[2] = missing.slot.index();
            let aggregate = push_subframe(
                empty!(),
                oer_ieee80211_port::TxNumbering::Open,
                buffer,
                subframe_frame(6),
                None,
            );
            let aggregate = push_subframe(
                aggregate,
                oer_ieee80211_port::TxNumbering::Open,
                missing,
                subframe_frame(7),
                None,
            );
            air.answer(AirAnswer::BlockAck(1));
            send_aggregate(phase, ticket, aggregate);
            let (ticket, subframes, end, report) =
                returned_aggregate(phase, &mut terminal_access).await;
            ends.push(end);
            let mut subframes = subframes.into_iter();
            let delivered = subframes.next().unwrap();
            let missing = subframes.next().unwrap();
            let report = report.expect("the actual BlockAck reports the preceding delivery");
            assert!(report.acknowledges(delivered.buffer.sequence()));
            assert!(!report.acknowledges(missing.buffer.sequence()));
            assert_eq!(
                originator.on_delivered(0, report.start_sequence, report.bitmap, PEER, STATION),
                None
            );
            let head = missing.buffer.sequence();
            assert_eq!(originator.operational(0).unwrap().starting_sequence, head);
            let delivered_at = delivered.admitted_at().unwrap();
            let delivered_index = index(&delivered.buffer);
            let (ModelAnyBuffer::SmallSent(missing), _) = missing.into_parts() else {
                panic!("the missing member was published");
            };
            let aggregate = push_retry(empty!(), missing);
            air.hold(WmmAccessCategory::BestEffort, true);
            send_aggregate(phase, ticket, aggregate);
            for _ in 0..4 {
                yield_now().await;
            }
            self.clock.advance(Case::LIFETIME).unwrap();
            air.clock_advanced();
            air.hold(WmmAccessCategory::BestEffort, false);
            let (ticket, subframes, end, _) = returned_aggregate(phase, &mut terminal_access).await;
            ends.push(end);
            let expired = subframes.into_iter().next().unwrap();
            let End::Expired(msdus) = end else {
                panic!("the undelivered head expired");
            };
            core_bar = Some(contract::CoreBar {
                head,
                tid: 0,
                addresses: [PEER, STATION],
                frame: originator
                    .on_expired(0, expired.buffer.sequence(), msdus, PEER, STATION)
                    .expect("a head reached by delivery still needs a BAR on expiry"),
            });
            returned = [delivered_at, expired.admitted_at().unwrap()];
            returned_owners = [ticket.index(), delivered_index, index(&expired.buffer)];
        } else if self.case == Case::Reformation {
            let mut originator =
                active_originator(SequenceNumber::new(7).unwrap(), self.clock.now());
            let aggregate = push_subframe(
                empty!(),
                oer_ieee80211_port::TxNumbering::Open,
                buffer,
                subframe_frame(7),
                None,
            );
            air.answer(AirAnswer::BlockAck(0));
            send_aggregate(phase, ticket, aggregate);
            let (ticket, subframes, end, _) = returned_aggregate(phase, &mut terminal_access).await;
            ends.push(end);
            let old = subframes.into_iter().next().unwrap();
            assert_eq!(old.admitted_at(), Some(first));
            let (ModelAnyBuffer::SmallSent(old), _) = old.into_parts() else {
                panic!("the first subframe was published");
            };
            self.clock.advance(Duration::from_micros(50)).unwrap();
            air.clock_advanced();
            let second = air.radio_now().unwrap();
            admitted[1] = second;
            let new = small.remove(0);
            owners[2] = new.slot.index();
            let aggregate = <ModelTypes as TxBufferPool<ModelSmall>>::push_aggregate(
                CheckedAggregate::new(empty!(), SubframeSource::Sent(old))
                    .ok()
                    .unwrap(),
            )
            .ok()
            .unwrap();
            let aggregate = push_subframe(
                aggregate,
                oer_ieee80211_port::TxNumbering::Open,
                new,
                subframe_frame(8),
                Some(ModelBody(alloc::vec![5])),
            );
            air.hold(WmmAccessCategory::BestEffort, true);
            send_aggregate(phase, ticket, aggregate);
            for _ in 0..4 {
                yield_now().await;
            }
            self.clock.advance(Duration::from_micros(51)).unwrap();
            air.clock_advanced();
            air.hold(WmmAccessCategory::BestEffort, false);
            let (ticket, subframes, end, _) = returned_aggregate(phase, &mut terminal_access).await;
            ends.push(end);
            let mut subframes = subframes.into_iter();
            let old = subframes.next().unwrap();
            let live = subframes.next().unwrap();
            // Compose the port report with the real upper-MAC core. The
            // backend never owns the agreement and never generates a BAR.
            let head = old.buffer.sequence();
            assert_eq!(end, End::Expired(1));
            core_bar = Some(contract::CoreBar {
                head,
                tid: 0,
                addresses: [PEER, STATION],
                frame: originator
                    .on_expired(0, head, 1, PEER, STATION)
                    .expect("the expired active head needs a BAR"),
            });
            returned = [old.admitted_at().unwrap(), live.admitted_at().unwrap()];
            let (old_buffer, _) = old.into_parts();
            let (ModelAnyBuffer::Small(live), body) = live.into_parts() else {
                panic!("a live unpublished subframe stays fresh");
            };
            returned_owners = [ticket.index(), index(&old_buffer), live.slot.index()];
            drop(old_buffer);
            // An unpublished live member also retains its age on re-formation.
            let aggregate = push_subframe(
                empty!(),
                oer_ieee80211_port::TxNumbering::Open,
                live,
                subframe_frame(8),
                body,
            );
            air.hold(WmmAccessCategory::BestEffort, true);
            send_aggregate(phase, ticket, aggregate);
            for _ in 0..4 {
                yield_now().await;
            }
            self.clock.advance(Duration::from_micros(49)).unwrap();
            air.clock_advanced();
            air.hold(WmmAccessCategory::BestEffort, false);
            let (_, subframes, end, _) = returned_aggregate(phase, &mut terminal_access).await;
            ends.push(end);
            assert_eq!(
                subframes.into_iter().next().unwrap().admitted_at(),
                Some(second)
            );
        } else {
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
            let oer_ieee80211_port::TerminalParts {
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
            returned = [payload.admitted_at().unwrap(); 2];
            let (buffer, _) = payload.into_parts();
            returned_owners = [ticket.index(), index(&buffer), index(&buffer)];
        }
        self.trace.replace(Some(Trace {
            ends,
            admitted,
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
            core_bar,
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
        let _end = run_port(
            backend,
            Body(Exercise {
                case,
                clock,
                trace: trace.clone(),
            }),
            oer_radio_port::LocalExecution,
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
        oer_radio_port::RelationRate::Affine { drift_ppm: 0 }
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
    impl PortBody<ModelBackend> for Probe {
        type Output = ();
        async fn run<'id>(
            self,
            owner: Owner<'_, 'id, ModelBackend, Disabled>,
            mut resources: ModelResources<'id>,
        ) -> Ended<'id, ModelBackend, ()> {
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
            buffer.admit(initial);
            self.0.advance(Duration::from_micros(3)).unwrap();
            buffer.admit(self.1.radio_now().unwrap());
            assert_eq!(buffer.admission(), Some(initial));
            let mut buffer = oer_ieee80211_port::ReturnedMpdu::<_, ()>::Fresh(buffer)
                .into_fresh(|()| panic!("fresh renewal consumes no Sent owner"));
            assert_eq!(buffer.admission(), None);
            let next = owner.now().await.unwrap().unwrap();
            buffer.admit(next);
            assert_eq!(buffer.admission(), Some(next));
            Ended::fail_stopped(owner.fail_stop(), ())
        }
    }
    let clock = Rc::new(VirtualClock::<8>::new());
    let (backend, air) = backend_on(clock.clone());
    let _end = block_on(run_port(
        backend,
        Probe(clock, air),
        oer_radio_port::LocalExecution,
    ));
}
