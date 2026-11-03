use super::*;

use oer_protocols_ieee80211_testing_conformance::{self as conformance, outcomes};
type OutcomeTrace = outcomes::Trace<ModelAck, ModelAirDenied>;

struct Results {
    result: AirAnswer,
    trace: Option<Rc<RefCell<Option<OutcomeTrace>>>>,
}

impl Work for Results {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let mut terminal_access = resources.terminal_access;
        let ModelResources {
            mut tickets,
            mut small,
            ..
        } = resources;
        let mut ticket = tickets.remove(0);
        let mut buffer = small.remove(0);
        let original_ticket = ticket.index();
        let original_buffer = buffer.slot.index();
        let air = phase.held.air.clone();
        air.answer(self.result);
        // The second operation has no injection and must get the default ACK.
        for (round, result) in [self.result, AirAnswer::Ack].into_iter().enumerate() {
            let rate = {
                {
                    let TxFrame::Individual { frame, plan } = data_frame(phase, false) else {
                        panic!()
                    };
                    let rate = plan.first().rate;
                    let frame = ClassFrame::new(TxFrame::Individual {
                        frame,
                        plan: RetryPlan::new(RetryStage {
                            rate,
                            transmissions: NonZeroU8::new(1).unwrap(),
                            protection: Protection::None,
                        }),
                    })
                    .ok()
                    .unwrap();
                    submit_frame(
                        phase,
                        ticket,
                        TxSource::Fresh {
                            buffer,
                            frame,
                            body: None,
                            protection: TxProtection::Open,
                        },
                    );
                    PhyRate::Legacy(LegacyRate::Ofdm6M)
                }
            };
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(completed)))) =
                phase.next_event().await
            else {
                panic!()
            };
            let (returned_ticket, returned_buffer, outcome) = match completed {
                TxCompleted::Single(terminal) => {
                    let oer_protocols_ieee80211_contracts_port::TerminalParts {
                        ticket,
                        outcome: completion,
                        payload,
                        ..
                    } = terminal.into_parts(&mut terminal_access);
                    let (buffer, body) = payload.into_parts();

                    assert!(body.is_none());
                    (ticket, buffer, completion)
                }
                _ => panic!("terminal changed operation kind"),
            };
            let air = Air {
                transmissions: NonZeroU16::MIN,
                rate,
            };
            let expected = match result {
                AirAnswer::HardwareTimeout => {
                    TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
                }
                AirAnswer::Ack => TxOutcome::Ended(SingleEnd::Delivered {
                    air,
                    response: SingleResponse::Ack(ModelAck {
                        snr_db: MODEL_ACK_SNR_DB,
                    }),
                }),
                AirAnswer::NoResponse => TxOutcome::Ended(SingleEnd::AckTimeout(air)),
                AirAnswer::Collision => TxOutcome::Ended(SingleEnd::ContentionLimit(None)),
                AirAnswer::AirDenied => TxOutcome::Ended(SingleEnd::AirDenied {
                    denial: ModelAirDenied,
                    air: None,
                }),
                AirAnswer::BlockAck(_) | AirAnswer::BlockAckUnmeasured(_) => {
                    panic!("not an answer a single MPDU takes")
                }
            };
            assert_eq!(outcome, expected);
            if round == 0 {
                if let Some(trace) = &self.trace {
                    let (actual_buffer, state) = match &returned_buffer {
                        ModelAnyBuffer::SmallSent(mpdu) => {
                            (mpdu.buffer.slot.index(), conformance::MpduState::Sent)
                        }
                        ModelAnyBuffer::Small(buffer) => {
                            (buffer.slot.index(), conformance::MpduState::Fresh)
                        }
                        _ => panic!("the fixture submitted a small MPDU"),
                    };
                    trace.replace(Some(outcomes::Trace {
                        admitted: outcomes::Identity {
                            ticket: original_ticket,
                            buffer: original_buffer,
                        },
                        returned: outcomes::Identity {
                            ticket: returned_ticket.index(),
                            buffer: actual_buffer,
                        },
                        state,
                        rate,
                        publications: phase.held.air.sent().len(),
                        outcome,
                    }));
                }
            }

            ticket = returned_ticket;
            let ModelAnyBuffer::SmallSent(sent) = returned_buffer else {
                panic!("publication must return Sent")
            };
            let (fresh, body) =
                <ModelTypes<ModelBackend<Id>> as TxBufferPool<ModelSmall>>::renew(sent);
            assert!(body.is_none());
            buffer = fresh;
            assert_eq!(ticket.index(), original_ticket);
            assert_eq!(buffer.slot.index(), original_buffer);
        }
        // One transmission per attempt that reached the air: a denied one
        // did not.
        let on_air = if matches!(self.result, AirAnswer::AirDenied) {
            1
        } else {
            2
        };
        assert_eq!(
            air.sent().len(),
            on_air,
            "one transmission per attempt on the air"
        );
    }
}

struct ModelOutcomeHarness;
impl outcomes::Harness for ModelOutcomeHarness {
    type Ack = ModelAck;
    type AirDenied = ModelAirDenied;
    async fn exercise(&mut self, case: outcomes::Case) -> OutcomeTrace {
        let result = match case {
            outcomes::Case::Ack => AirAnswer::Ack,
            outcomes::Case::NoResponse => AirAnswer::NoResponse,
            outcomes::Case::Collision => AirAnswer::Collision,
            outcomes::Case::AirDenied => AirAnswer::AirDenied,
            outcomes::Case::HardwareTimeout => AirAnswer::HardwareTimeout,
        };
        let (backend, _) = backend();
        let trace = Rc::new(RefCell::new(None));
        let _port_end = run_model(
            backend,
            Body(Results {
                result,
                trace: Some(trace.clone()),
            }),
        )
        .await;
        let captured = trace.borrow_mut().take();
        captured.expect("the result fixture captures actual terminals and owners")
    }
}
#[test]
fn shared_outcome_suite_observes_each_actual_single_terminal() {
    block_on(outcomes::check(
        &mut ModelOutcomeHarness,
        conformance::Environment::Model,
    ))
    .expect("the model satisfies every shared outcome case");
}

#[test]
fn every_result_is_consumed_by_exactly_one_attempt() {
    for result in [
        AirAnswer::Ack,
        AirAnswer::NoResponse,
        AirAnswer::Collision,
        AirAnswer::AirDenied,
        AirAnswer::HardwareTimeout,
    ] {
        let (backend, _) = backend();
        let _port_end = block_on(run_model(
            backend,
            Body(Results {
                result,
                trace: None,
            }),
        ));
    }
}

#[test]
fn an_answer_incompatible_with_the_checked_response_policy_is_refused_and_consumed() {
    let (_, air) = backend();
    for (form, wrong) in [
        (ResponsePolicy::Ack, AirAnswer::BlockAck(1)),
        (ResponsePolicy::Ack, AirAnswer::BlockAckUnmeasured(1)),
        (
            ResponsePolicy::BlockAck {
                start_sequence: SequenceNumber::ZERO,
            },
            AirAnswer::Ack,
        ),
        // A BlockAckReq's BlockAck is a single MPDU's, always measured.
        (
            ResponsePolicy::BlockAck {
                start_sequence: SequenceNumber::ZERO,
            },
            AirAnswer::BlockAckUnmeasured(1),
        ),
        (ResponsePolicy::NoResponseExpected, AirAnswer::Ack),
        (ResponsePolicy::NoResponseExpected, AirAnswer::BlockAck(1)),
    ] {
        air.answer(wrong);
        assert!(air.single_result(form).is_err());
        assert!(air.answers.inspect().is_empty());
    }
}

#[test]
fn failed_responses_keep_each_withdrawal_reason() {
    let rate = PhyRate::Legacy(LegacyRate::Ofdm6M);
    for cause in [
        WithdrawCause::Disabled,
        WithdrawCause::KeyRemoved,
        WithdrawCause::GateClosed,
    ] {
        for answer in [
            SingleAnswer::Unanswered,
            SingleAnswer::Collision,
            SingleAnswer::AirDenied,
        ] {
            let (outcome, _) =
                single_outcome(answer, Some((NonZeroU16::MIN, rate)), rate, Some(cause));
            assert_eq!(outcome, TxOutcome::Withdrawn(cause));
        }
    }
}

#[test]
fn lost_air_gives_no_rate_sample_and_counts_no_transmission() {
    let rate = PhyRate::Legacy(LegacyRate::Ofdm6M);
    let last = Some((NonZeroU16::MIN, rate));
    let (outcome, after) = single_outcome(SingleAnswer::AirDenied, last, rate, None);
    assert_eq!(after, last);
    assert!(outcome.rate_sample().is_none());
    assert_eq!(
        outcome,
        TxOutcome::Ended(SingleEnd::AirDenied {
            denial: ModelAirDenied,
            air: Some(Air {
                transmissions: NonZeroU16::MIN,
                rate,
            }),
        })
    );
}

struct WrongAnswer;
impl Work for WrongAnswer {
    async fn work<'p, 'id: 'p, Id: SlotIdentity + 'id>(
        self,
        phase: &mut ModelPhase<'p, 'id, ModelCaps, ModelBackend<Id>>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) {
        let ModelResources {
            mut tickets,
            mut small,
            ..
        } = resources;
        let ticket = tickets.remove(0);
        let index = ticket.index();
        phase.held.air.answer(AirAnswer::BlockAck(1));
        submit(phase, ticket, small.remove(0), None);
        assert!(phase.next_event().await.is_err());
        assert!(phase.held.air.sent().is_empty());
        let held = phase.held.quarantined.items.inspect();
        let Some(operation) = held.front() else {
            panic!("the rejected operation stays owned")
        };
        assert_eq!(operation.operation.ticket().index(), index);
    }
}

#[test]
fn wrong_answer_shape_retains_the_operation_before_publication() {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(backend, Body(WrongAnswer)));
}
