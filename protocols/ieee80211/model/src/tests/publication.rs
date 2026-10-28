use super::*;
use oer_ieee80211_port::conformance::{
    self, BarrierCase, BarrierEvent, BarrierTrace, HeldSnapshot, MpduState,
};

#[derive(Clone, Copy)]
enum Barrier {
    GateAck,
    GateRetry,
    GateTimeout,
    RemoveKey,
}
type ModelBarrierTrace = BarrierTrace<
    Vec<BarrierEvent<ModelAck, ModelAirDenied>>,
    Vec<u8>,
    Vec<Vec<u8>>,
    BarrierEvent<ModelAck, ModelAirDenied>,
>;
type RecordedBarrier = Rc<RefCell<Option<ModelBarrierTrace>>>;
struct DuringPublication(
    Barrier,
    Option<RecordedBarrier>,
    Rc<oer_time_virtual::VirtualClock>,
);

fn held_snapshot<Id: SlotIdentity>(
    phase: &ModelPhase<'_, '_, ModelCaps, ModelBackend<Id>>,
) -> HeldSnapshot {
    HeldSnapshot {
        publications: phase.held.air.sent().len(),
        events: phase.held.events.items.inspect().len(),
    }
}

impl Work for DuringPublication {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let station_association = ModelKeys::<ModelBackend<Id>, Station>::begin_association(
            resources.station_association,
            oer_ieee80211_wire::security::Spp::NotNegotiated,
        );

        let mut terminal_access = resources.terminal_access;
        let ModelResources {
            mut tickets,
            mut small,
            station_pairwise: [slot, _],
            gate,
            ..
        } = resources;
        let air = phase.held.air.clone();
        let mut key = Some(
            phase
                .station()
                .install_pairwise_key(
                    &station_association,
                    slot,
                    IndividualAddress::new(PEER).unwrap(),
                    Ccmp128Key::new(&[7; 16]),
                )
                .into_core_key(),
        );
        air.hold_completion(WmmAccessCategory::BestEffort, true);
        air.answer(if matches!(self.0, Barrier::GateAck) {
            AirAnswer::Ack
        } else {
            AirAnswer::NoResponse
        });
        let head_ticket = tickets[0].index();
        let tail_ticket = tickets[1].index();
        submit(phase, tickets.remove(0), small.remove(0), key.as_ref());
        submit(phase, tickets.remove(0), small.remove(0), key.as_ref());
        for _ in 0..6 {
            yield_now().await;
        }
        assert_eq!(
            air.sent().len(),
            1,
            "a queued successor cannot pass a published head"
        );
        assert!(
            phase.held.events.is_empty(),
            "publication is not completion"
        );
        let before_request = held_snapshot(phase);
        let first = air.sent().remove(0);
        if matches!(self.0, Barrier::RemoveKey) {
            phase
                .station()
                .remove_pairwise_key(station_association, key.take().unwrap().into_backend());
        } else {
            phase.close_gate(gate);
        }
        for _ in 0..6 {
            yield_now().await;
        }
        assert_eq!(air.sent().len(), 1);
        assert!(
            phase.held.events.is_empty(),
            "no terminal or barrier before the current result"
        );
        let before_result = held_snapshot(phase);
        let mut just_before_timeout = None;
        if matches!(self.0, Barrier::GateTimeout) {
            self.2
                .advance(oer_time::Duration::from_micros(
                    MODEL_PUBLICATION_BOUND.as_micros() - 1,
                ))
                .unwrap();
            air.clock_advanced();
            for _ in 0..3 {
                yield_now().await;
            }
            assert!(phase.held.events.is_empty());
            just_before_timeout = Some(held_snapshot(phase));
            self.2.advance(oer_time::Duration::from_micros(1)).unwrap();
            air.clock_advanced();
        } else {
            air.hold_completion(WmmAccessCategory::BestEffort, false);
        }
        let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(
            terminal,
        ))))) = phase.next_event().await
        else {
            panic!("published head returns first, as Sent")
        };
        let oer_ieee80211_port::TerminalParts {
            ticket,
            outcome: completion,
            payload,
            ..
        } = terminal.into_parts(&mut terminal_access);
        let (ModelAnyBuffer::SmallSent(mpdu), body) = payload.into_parts() else {
            panic!("terminal returned an unexpected MPDU state")
        };

        assert_eq!(ticket.index(), head_ticket);
        assert!(body.is_none());
        assert_eq!(mpdu.packet_number(), first.packet_number);
        match self.0 {
            Barrier::GateAck => assert!(matches!(
                completion,
                TxOutcome::Ended(SingleEnd::Delivered {
                    air: Air {
                        transmissions: core::num::NonZeroU16::MIN,
                        ..
                    },
                    ..
                })
            )),
            Barrier::GateRetry => {
                assert_eq!(completion, TxOutcome::Withdrawn(WithdrawCause::GateClosed))
            }
            Barrier::RemoveKey => {
                assert_eq!(completion, TxOutcome::Withdrawn(WithdrawCause::KeyRemoved))
            }
            Barrier::GateTimeout => assert_eq!(
                completion,
                TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
            ),
        }
        let mut events = alloc::vec![BarrierEvent::Tx {
            ticket: ticket.index(),
            state: MpduState::Sent,
            outcome: completion,
        }];
        let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(
            terminal,
        ))))) = phase.next_event().await
        else {
            panic!("unpublished tail returns second, as Fresh")
        };
        let oer_ieee80211_port::TerminalParts {
            ticket: tail,
            outcome: completion,
            payload,
            ..
        } = terminal.into_parts(&mut terminal_access);
        let (ModelAnyBuffer::Small(_), _) = payload.into_parts() else {
            panic!("terminal returned an unexpected MPDU state")
        };

        assert_eq!(tail.index(), tail_ticket);
        let cause = if matches!(self.0, Barrier::RemoveKey) {
            WithdrawCause::KeyRemoved
        } else {
            WithdrawCause::GateClosed
        };
        assert_eq!(completion, TxOutcome::Withdrawn(cause));
        events.push(BarrierEvent::Tx {
            ticket: tail.index(),
            state: MpduState::Fresh,
            outcome: completion,
        });
        assert_eq!(
            air.sent().len(),
            1,
            "the barrier waits one publication, not the two-entry plan"
        );
        let publications_at_barrier = air.sent().len();
        let mut retried_frames = Vec::new();
        let mut retry_terminal = None;
        if matches!(self.0, Barrier::RemoveKey) {
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(_))))
            ));
            events.push(BarrierEvent::KeyRemoved);
        } else {
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(closed)))) =
                phase.next_event().await
            else {
                panic!("barrier follows both terminals")
            };
            events.push(BarrierEvent::GateClosed);
            let _gate = phase.open_gate(closed);
            air.hold_completion(WmmAccessCategory::BestEffort, false);
            if matches!(self.0, Barrier::GateRetry) {
                let TxFrame::Individual { plan, .. } = data_frame(phase, true) else {
                    unreachable!()
                };
                submit_frame::<_, ModelSmall>(
                    phase,
                    ticket,
                    TxSource::Retransmission {
                        mpdu,
                        plan: RetransmissionPlan::Individual(plan),
                    },
                );
                let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                    TxCompleted::Single(completed),
                )))) = phase.next_event().await
                else {
                    panic!("Sent retry terminal");
                };
                let parts = completed.into_parts(&mut terminal_access);
                let (returned, _) = parts.payload.into_parts();
                retry_terminal = Some(BarrierEvent::Tx {
                    ticket: parts.ticket.index(),
                    state: if matches!(
                        returned,
                        ModelAnyBuffer::SmallSent(_) | ModelAnyBuffer::LargeSent(_)
                    ) {
                        MpduState::Sent
                    } else {
                        MpduState::Fresh
                    },
                    outcome: parts.outcome,
                });
                assert!(matches!(returned, ModelAnyBuffer::SmallSent(_)));
                let retried = air.sent().pop().unwrap();
                assert_eq!(retried.packet_number, first.packet_number);
                assert_eq!(&retried.plaintext[22..24], &first.plaintext[22..24]);
                assert_eq!(
                    &retried.plaintext[24..],
                    &first.plaintext[24..],
                    "CCMP and payload unchanged"
                );
                assert_eq!(first.plaintext[1] & 0x08, 0);
                assert_ne!(retried.plaintext[1] & 0x08, 0);
                retried_frames = air
                    .sent()
                    .into_iter()
                    .skip(1)
                    .map(|frame| frame.plaintext)
                    .collect();
            }
        }
        if let Some(recorded) = self.1 {
            *recorded.borrow_mut() = Some(BarrierTrace {
                tickets: [head_ticket, tail_ticket],
                before_request,
                before_result,
                just_before_timeout,
                publications_at_barrier,
                events,
                first_frame: first.plaintext,
                retried_frames,
                retry_terminal,
            });
        }
    }
}
#[test]
fn gate_waits_one_publication_and_returns_sent_for_retry_with_original_numbers() {
    let clock = Rc::new(oer_time_virtual::VirtualClock::<8>::new());
    let (backend, _) = backend_on(clock.clone());
    let _port_end = block_on(run_model(
        backend,
        Body(DuringPublication(Barrier::GateRetry, None, clock.clone())),
    ));
}
#[test]
fn ack_wins_while_gate_closes_and_barrier_follows_the_result() {
    let clock = Rc::new(oer_time_virtual::VirtualClock::<8>::new());
    let (backend, _) = backend_on(clock.clone());
    let _port_end = block_on(run_model(
        backend,
        Body(DuringPublication(Barrier::GateAck, None, clock.clone())),
    ));
}
#[test]
fn missing_result_ends_at_publication_bound_without_finishing_the_plan() {
    let clock = Rc::new(oer_time_virtual::VirtualClock::<8>::new());
    let (backend, _) = backend_on(clock.clone());
    let _port_end = block_on(run_model(
        backend,
        Body(DuringPublication(Barrier::GateTimeout, None, clock.clone())),
    ));
}
#[test]
fn key_removal_waits_for_the_publication_then_returns_sent_without_retry() {
    let clock = Rc::new(oer_time_virtual::VirtualClock::<8>::new());
    let (backend, _) = backend_on(clock.clone());
    let _port_end = block_on(run_model(
        backend,
        Body(DuringPublication(Barrier::RemoveKey, None, clock.clone())),
    ));
}

struct ModelBarrierHarness;
impl conformance::BarrierHarness for ModelBarrierHarness {
    type Ack = ModelAck;
    type AirDenied = ModelAirDenied;
    type Events = Vec<BarrierEvent<ModelAck, ModelAirDenied>>;
    type Frame = Vec<u8>;
    type Retries = Vec<Vec<u8>>;
    async fn exercise_barrier(&mut self, case: BarrierCase) -> ModelBarrierTrace {
        let barrier = match case {
            BarrierCase::GateAck => Barrier::GateAck,
            BarrierCase::GateRetry => Barrier::GateRetry,
            BarrierCase::GateTimeout => Barrier::GateTimeout,
            BarrierCase::RemoveKey => Barrier::RemoveKey,
        };
        let clock = Rc::new(oer_time_virtual::VirtualClock::<8>::new());
        let (backend, _) = backend_on(clock.clone());
        let recorded = Rc::new(RefCell::new(None));
        let _port_end = run_model(
            backend,
            Body(DuringPublication(
                barrier,
                Some(recorded.clone()),
                clock.clone(),
            )),
        )
        .await;
        let trace = recorded.borrow_mut().take();
        trace.expect("the actual port execution records its publication trace")
    }
}

#[test]
fn common_backend_publication_barrier_and_sent_retry_suite() {
    block_on(async {
        conformance::check_barriers(&mut ModelBarrierHarness, conformance::Environment::Model)
            .await
            .unwrap();
    });
}
