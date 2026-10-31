//! The shared publication/barrier suite against the S31 backend. The device
//! model holds the head's completion while the barrier is requested, then
//! answers with a status or the hardware's TX-timeout edge. The model keeps
//! no copy of published DMA memory, so the encoded MPDU is read from the
//! returned Sent owner: memory the hardware no longer owns, holding the
//! bytes and Retry bit of its last publication.
use super::*;
use oer_ieee80211_testing_conformance::{
    BarrierCase, BarrierEvent, BarrierHarness, HeldSnapshot, check_barriers,
};

type Terminal = BarrierEvent<Ack, Infallible>;
type Trace =
    oer_ieee80211_testing_conformance::BarrierTrace<Vec<Terminal>, Vec<u8>, Vec<Vec<u8>>, Terminal>;

/// Completion statuses for the head and, after GateRetry, its Sent retry.
fn statuses(case: BarrierCase) -> Vec<(u8, u8)> {
    match case {
        BarrierCase::GateAck => std::vec![(0, 0)],
        // The receiver stays absent across the head and its retry plan.
        BarrierCase::GateRetry => std::vec![(5, 0); 3],
        BarrierCase::RemoveKey => std::vec![(5, 0)],
        BarrierCase::GateTimeout => Vec::new(),
    }
}

struct Barriers;

struct BarrierBody {
    case: BarrierCase,
    timer: TestTimer,
    observed: Rc<RefCell<Observed>>,
    /// The trace as observed: a fail-stop may cancel the body first.
    seen: Rc<RefCell<Option<Trace>>>,
}

/// The retry budget of the head and of its Sent retransmission.
const BUDGET: u8 = 2;

fn retransmit<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &mut Esp32s31Phase<'p, 'id, Id, TestDevice, 512>,
    mpdu: Sent<'id, Id, TestDevice, 512>,
    ticket: radio::Slot<'id, Tickets, radio::Child<radio::Root<Id>, 2>>,
) {
    let rate = phase
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .expect("rate");
    let source = TxSource::Retransmission {
        mpdu,
        plan: RetransmissionPlan::Individual(RetryPlan::new(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::new(BUDGET).expect("nonzero budget"),
            protection: Protection::None,
        })),
    }
    .check()
    .ok()
    .expect("layout");
    let source =
        <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::prepare::<Station, Caps>(source)
            .ok()
            .expect("the key is still installed");
    phase
        .station()
        .submit(
            Attempt {
                context: Tag(92, None),
                ticket,
                source,
                access_category: oer_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).expect("power"),
                coex_operation: TxOperation::Data,
            }
            .admit(),
        )
        .expect("submit");
}

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for BarrierBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let Ok(mut owner) = owner.enable().await else {
            panic!("enable")
        };
        let mut resources = resources;
        {
            let mut phase = owner.phase();
            let (start, bank) = resources.station_association.into_parts();
            let ([slot], _) = bank.into_parts();
            let association = start.begin(oer_ieee80211_wire::security::Spp::NotNegotiated);
            let key = phase.station().install_pairwise_key(
                &association,
                slot,
                IndividualAddress::new(BSSID).expect("peer"),
                oer_ieee80211_contracts_security::Ccmp128Key::new(&[7; 16]),
            );
            let [head, tail, ..] = resources.buffers;
            let [head_ticket, tail_ticket, ..] = resources.tickets;
            let tickets = [head_ticket.index(), tail_ticket.index()];
            self.observed.borrow_mut().hold_completion = true;
            submit_mpdu_budget(&mut phase, head, head_ticket, None, Some(&key), BUDGET);
            submit_mpdu_budget(&mut phase, tail, tail_ticket, None, Some(&key), BUDGET);
            for _ in 0..6 {
                barriers::yield_once().await;
            }
            // Events the backend delivered before the fixture asked: each
            // probe that finds one keeps it for the verifier.
            let mut early = Vec::new();
            let snapshot =
                async |early: &mut Vec<_>,
                       phase: &mut Esp32s31Phase<'_, 'id, Id, TestDevice, 512>| {
                    let mut event = pin!(phase.next_event());
                    if let Poll::Ready(event) =
                        core::future::poll_fn(|cx| Poll::Ready(event.as_mut().poll(cx))).await
                    {
                        early.push(event);
                    }
                    HeldSnapshot {
                        publications: self.observed.borrow().publications,
                        events: early.len(),
                    }
                };
            let before_request = snapshot(&mut early, &mut phase).await;
            let mut key = Some(key);
            let mut association = Some(association);
            if self.case == BarrierCase::RemoveKey {
                phase.station().remove_pairwise_key(
                    association.take().expect("association"),
                    key.take().expect("key"),
                );
            } else {
                phase.close_gate(resources.gate);
            }
            for _ in 0..6 {
                barriers::yield_once().await;
            }
            let before_result = snapshot(&mut early, &mut phase).await;
            let just_before_timeout = if self.case == BarrierCase::GateTimeout {
                let held = snapshot(&mut early, &mut phase).await;
                self.observed.borrow_mut().timeout_edge = true;
                // P2 takes the edge and arms its abort settle before time
                // moves; otherwise the publication deadline expires first.
                for _ in 0..6 {
                    barriers::yield_once().await;
                }
                Some(held)
            } else {
                self.observed.borrow_mut().hold_completion = false;
                None
            };
            let mut events = Vec::new();
            let mut head = None;
            let mut first_frame = Vec::new();
            let mut closed = None;
            let mut publications_at_barrier = 0;
            let mut early = early.into_iter();
            while events.len() < 3 {
                let event = match early.next() {
                    Some(event) => event,
                    None => {
                        let mut event = pin!(phase.next_event());
                        core::future::poll_fn(|cx| {
                            let ready = event.as_mut().poll(cx);
                            // A timed-out publication's abort settles in time.
                            if ready.is_pending() && self.case == BarrierCase::GateTimeout {
                                let _ = self.timer.0.advance_to_next_deadline();
                                cx.waker().wake_by_ref();
                            }
                            ready
                        })
                        .await
                    }
                };
                match event {
                    Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                        TxCompleted::Single(completed),
                    )))) => {
                        let returned = completed.into_parts(&mut resources.terminal_access);
                        let ticket = returned.ticket.index();
                        let (buffer, _) = returned.payload.into_parts();
                        let state = match buffer {
                            AnyBuffer::Sent(mut sent) => {
                                if ticket == tickets[0] {
                                    first_frame = sent.published().to_vec();
                                    head = Some((returned.ticket, sent));
                                }
                                MpduState::Sent
                            }
                            AnyBuffer::Fresh(_) => MpduState::Fresh,
                        };
                        events.push(BarrierEvent::Tx {
                            ticket,
                            state,
                            outcome: returned.outcome,
                        });
                    }
                    Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(gate)))) => {
                        publications_at_barrier = self.observed.borrow().publications;
                        closed = Some(gate);
                        events.push(BarrierEvent::GateClosed);
                    }
                    Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(_)))) => {
                        publications_at_barrier = self.observed.borrow().publications;
                        events.push(BarrierEvent::KeyRemoved);
                    }
                    _ => panic!("an unexpected barrier event"),
                }
            }
            let _open = closed.map(|gate| phase.open_gate(gate));
            let mut retried_frames = Vec::new();
            let mut retry_terminal = None;
            if self.case == BarrierCase::GateRetry {
                let (ticket, mpdu) = head.take().expect("the Sent head");
                retransmit(&mut phase, mpdu, ticket);
                let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                    TxCompleted::Single(completed),
                )))) = phase.next_event().await
                else {
                    panic!("the retry terminal")
                };
                let returned = completed.into_parts(&mut resources.terminal_access);
                let (buffer, _) = returned.payload.into_parts();
                let state = match buffer {
                    AnyBuffer::Sent(mut sent) => {
                        retried_frames.push(sent.published().to_vec());
                        MpduState::Sent
                    }
                    AnyBuffer::Fresh(_) => MpduState::Fresh,
                };
                retry_terminal = Some(BarrierEvent::Tx {
                    ticket: returned.ticket.index(),
                    state,
                    outcome: returned.outcome,
                });
            }
            if let (Some(association), Some(key)) = (association, key) {
                phase.station().remove_pairwise_key(association, key);
                let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(_)))) =
                    phase.next_event().await
                else {
                    panic!("the key removal terminal")
                };
            }
            *self.seen.borrow_mut() = Some(Trace {
                tickets,
                before_request,
                before_result,
                just_before_timeout,
                publications_at_barrier,
                events,
                first_frame,
                retried_frames,
                retry_terminal,
            });
        }
        let Ok(owner) = owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        else {
            panic!("disable")
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

impl BarrierHarness for Barriers {
    type Ack = Ack;
    type AirDenied = Infallible;
    type Events = Vec<Terminal>;
    type Frame = Vec<u8>;
    type Retries = Vec<Vec<u8>>;
    async fn exercise_barrier(&mut self, case: BarrierCase) -> Trace {
        let (parts, observed, timer) = installed(statuses(case));
        let seen = Rc::new(RefCell::new(None));
        let body = BarrierBody {
            case,
            timer,
            observed: observed.clone(),
            seen: seen.clone(),
        };
        let Report::Uninstalled { .. } = run_body(parts, body).await else {
            panic!("clean uninstall")
        };
        seen.borrow_mut().take().expect("the barrier was observed")
    }
}

#[test]
fn the_s31_backend_passes_the_shared_publication_barrier_suite() {
    // The host device model supplies the held completion and the timeout
    // edge, so every case runs, GateTimeout included.
    let report = block_on(check_barriers(&mut Barriers, Environment::Model));
    assert_eq!(
        report.map(|report| (report.passed, report.model_only)),
        Ok((4, 0))
    );
}

#[test]
fn a_sent_retry_after_reopening_runs_its_own_plan_to_the_absent_receiver() {
    let trace = block_on(Barriers.exercise_barrier(BarrierCase::GateRetry));
    let Some(BarrierEvent::Tx {
        ticket,
        state: MpduState::Sent,
        outcome: TxOutcome::Ended(SingleEnd::AckTimeout(air)),
    }) = &trace.retry_terminal
    else {
        panic!("the retry ends in the absent receiver's timeout")
    };
    assert_eq!(*ticket, trace.tickets[0]);
    assert_eq!(air.transmissions.get(), u16::from(BUDGET));
    assert_eq!(trace.retried_frames.len(), 1);
}
