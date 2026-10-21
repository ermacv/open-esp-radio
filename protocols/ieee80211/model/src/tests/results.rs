use super::*;
use oer_ieee80211_port::{AggregateAttempt, MpduStartSpacing};

use oer_ieee80211_port::conformance::{
    self,
    outcomes::{self, Form},
};
type OutcomeTrace = outcomes::Trace<ModelAck, ModelBlockAckEvidence, ModelAirDenied>;

/// A returned outcome, of the operation's form.
#[derive(Debug, Eq, PartialEq)]
enum Returned {
    Single(ModelSingleOutcome),
    Aggregate(ModelAggregateOutcome),
}

struct Results {
    form: Form,
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
        // The second operation has no injection and must get the default ACK,
        // including when the first operation was an aggregate.
        for (round, (form, result)) in [(self.form, self.result), (Form::Single, AirAnswer::Ack)]
            .into_iter()
            .enumerate()
        {
            let rate = match form {
                Form::Single => {
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
                Form::Aggregate => {
                    let rate = PhyRate::Ht(
                        oer_ieee80211_wire::phy::HtRate::new(
                            HtMcs::new(7).unwrap(),
                            oer_ieee80211_wire::phy::PpduBandwidth::Mhz20,
                            false,
                        )
                        .unwrap(),
                    );
                    let checked_rate = phase.rate(rate).unwrap();
                    let mut station = phase.station();
                    let checked_rate = station.ampdu_rate(checked_rate).ok().unwrap();
                    let aggregate = station.aggregate(
                        IndividualAddress::new(PEER).unwrap(),
                        Tid::new(0).unwrap(),
                        checked_rate,
                        TxProtection::Open,
                    );
                    let aggregate = push_subframe(
                        aggregate,
                        oer_ieee80211_port::TxNumbering::Open,
                        buffer,
                        subframe_frame(0),
                        None,
                    );
                    station
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
                    rate
                }
            };
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(completed)))) =
                phase.next_event().await
            else {
                panic!()
            };
            let (returned_ticket, returned_buffer, outcome) = match (form, completed) {
                (Form::Single, TxCompleted::Single(terminal)) => {
                    let oer_ieee80211_port::TerminalParts {
                        ticket,
                        outcome: completion,
                        payload,
                        ..
                    } = terminal.into_parts(&mut terminal_access);
                    let (buffer, body) = payload.into_parts();

                    assert!(body.is_none());
                    (ticket, buffer, Returned::Single(completion))
                }
                (Form::Aggregate, TxCompleted::Aggregate(terminal)) => {
                    let oer_ieee80211_port::TerminalParts {
                        ticket,
                        payload,
                        outcome,
                        ..
                    } = terminal.into_parts(&mut terminal_access);
                    assert_eq!(payload.len(), 1);
                    let (buffer, body) = payload.into_iter().next().unwrap().into_parts();
                    assert!(body.is_none());
                    (ticket, buffer, Returned::Aggregate(outcome))
                }
                _ => panic!("terminal changed operation kind"),
            };
            let air = Air {
                transmissions: NonZeroU16::MIN,
                rate,
            };
            let expected = match (form, result) {
                (Form::Single, AirAnswer::HardwareTimeout) => Returned::Single(
                    TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
                ),
                (Form::Aggregate, AirAnswer::HardwareTimeout) => Returned::Aggregate(
                    TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
                ),
                (Form::Single, AirAnswer::Ack) => {
                    Returned::Single(TxOutcome::Ended(SingleEnd::Delivered {
                        air,
                        response: SingleResponse::Ack(ModelAck {
                            snr_db: MODEL_ACK_SNR_DB,
                        }),
                    }))
                }
                (
                    Form::Aggregate,
                    AirAnswer::BlockAck(bits) | AirAnswer::BlockAckUnmeasured(bits),
                ) => Returned::Aggregate(TxOutcome::Ended(AggregateEnd::Delivered {
                    air,
                    report: BlockAckReport {
                        start_sequence: SequenceNumber::new(0).unwrap(),
                        bitmap: bits & 1,
                    },
                    ack: if matches!(result, AirAnswer::BlockAck(_)) {
                        ModelBlockAckEvidence::Measured(MODEL_ACK_SNR_DB)
                    } else {
                        ModelBlockAckEvidence::Unmeasured
                    },
                })),
                (Form::Single, AirAnswer::NoResponse) => {
                    Returned::Single(TxOutcome::Ended(SingleEnd::AckTimeout(air)))
                }
                (Form::Aggregate, AirAnswer::NoResponse) => {
                    Returned::Aggregate(TxOutcome::Ended(AggregateEnd::AckTimeout(air)))
                }
                (Form::Single, AirAnswer::Collision) => {
                    Returned::Single(TxOutcome::Ended(SingleEnd::ContentionLimit(None)))
                }
                (Form::Aggregate, AirAnswer::Collision) => {
                    Returned::Aggregate(TxOutcome::Ended(AggregateEnd::ContentionLimit(None)))
                }
                (Form::Single, AirAnswer::AirDenied) => {
                    Returned::Single(TxOutcome::Ended(SingleEnd::AirDenied {
                        denial: ModelAirDenied,
                        air: None,
                    }))
                }
                (Form::Aggregate, AirAnswer::AirDenied) => {
                    Returned::Aggregate(TxOutcome::Ended(AggregateEnd::AirDenied {
                        denial: ModelAirDenied,
                        air: None,
                    }))
                }
                (Form::Single, AirAnswer::BlockAck(_) | AirAnswer::BlockAckUnmeasured(_))
                | (Form::Aggregate, AirAnswer::Ack) => panic!("not an answer this form takes"),
            };
            assert_eq!(outcome, expected, "{form:?}");
            if round == 0 {
                if let Some(trace) = &self.trace {
                    let (actual_buffer, state, sequence) = match &returned_buffer {
                        ModelAnyBuffer::SmallSent(mpdu) => (
                            mpdu.buffer.slot.index(),
                            conformance::MpduState::Sent,
                            SequenceNumber::from_sequence_control(u16::from_le_bytes(
                                mpdu.buffer.memory[22..24]
                                    .try_into()
                                    .expect("the fixture submitted a complete sequence control"),
                            )),
                        ),
                        ModelAnyBuffer::Small(buffer) => (
                            buffer.slot.index(),
                            conformance::MpduState::Fresh,
                            SequenceNumber::ZERO,
                        ),
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
                        sequence,
                        publications: phase.held.air.sent().len(),
                        outcome: match outcome {
                            Returned::Single(outcome) => outcomes::Outcome::Single(outcome),
                            Returned::Aggregate(outcome) => outcomes::Outcome::Aggregate(outcome),
                        },
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
    type BlockAck = ModelBlockAckEvidence;
    type AirDenied = ModelAirDenied;
    async fn exercise(&mut self, case: outcomes::Case) -> OutcomeTrace {
        let result = match case {
            outcomes::Case::Ack => AirAnswer::Ack,
            outcomes::Case::BlockAck => AirAnswer::BlockAck(1),
            outcomes::Case::NoResponse(_) => AirAnswer::NoResponse,
            outcomes::Case::Collision(_) => AirAnswer::Collision,
            outcomes::Case::AirDenied(_) => AirAnswer::AirDenied,
            outcomes::Case::HardwareTimeout(_) => AirAnswer::HardwareTimeout,
        };
        let (backend, _) = backend();
        let trace = Rc::new(RefCell::new(None));
        let _port_end = run_model(
            backend,
            Body(Results {
                form: case.form(),
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
fn shared_outcome_suite_observes_each_actual_single_or_aggregate_terminal() {
    block_on(outcomes::check(
        &mut ModelOutcomeHarness,
        conformance::Environment::Model,
    ))
    .expect("the model satisfies every shared outcome case");
}

#[test]
fn every_result_is_consumed_by_exactly_one_single_or_aggregate_attempt() {
    for form in [Form::Single, Form::Aggregate] {
        for result in [
            match form {
                Form::Single => AirAnswer::Ack,
                Form::Aggregate => AirAnswer::BlockAck(1),
            },
            AirAnswer::NoResponse,
            AirAnswer::Collision,
            AirAnswer::AirDenied,
            AirAnswer::HardwareTimeout,
        ] {
            let (backend, _) = backend();
            let _port_end = block_on(run_model(
                backend,
                Body(Results {
                    form,
                    result,
                    trace: None,
                }),
            ));
        }
    }
}

#[test]
fn aggregate_honors_timeout_injection() {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(
        backend,
        Body(Results {
            trace: None,
            form: Form::Aggregate,
            result: AirAnswer::HardwareTimeout,
        }),
    ));
}

#[test]
fn an_empty_block_ack_is_consumed_before_the_following_single() {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(
        backend,
        Body(Results {
            trace: None,
            form: Form::Aggregate,
            result: AirAnswer::BlockAck(0),
        }),
    ));
}

#[test]
fn an_aggregate_s_unmeasured_block_ack_still_delivers_it() {
    let (backend, _) = backend();
    let _port_end = block_on(run_model(
        backend,
        Body(Results {
            trace: None,
            form: Form::Aggregate,
            result: AirAnswer::BlockAckUnmeasured(1),
        }),
    ));
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
        assert!(air.answers.borrow().is_empty());
    }
    air.answer(AirAnswer::Ack);
    assert!(air.aggregate_result().is_err());
    assert!(air.answers.borrow().is_empty());
}

#[test]
fn failed_responses_keep_each_withdrawal_reason_for_both_forms() {
    let rate = PhyRate::Legacy(LegacyRate::Ofdm6M);
    let report = BlockAckReport {
        start_sequence: SequenceNumber::ZERO,
        bitmap: 0,
    };
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
        for answer in [
            AggregateAnswer::Unanswered,
            AggregateAnswer::Collision,
            AggregateAnswer::AirDenied,
        ] {
            assert_eq!(
                aggregate_outcome(answer, rate, report, Some(cause)),
                TxOutcome::Withdrawn(cause)
            );
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
        let held = phase.held.quarantined.items.borrow();
        let Some(super::super::Work::Single(operation)) = held.front() else {
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
