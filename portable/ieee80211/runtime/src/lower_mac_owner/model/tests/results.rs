use super::*;
use oer_ieee80211_lower_mac::{AggregateAttempt, MpduStartSpacing};

#[derive(Clone, Copy, Debug)]
enum Form {
    Single,
    Aggregate,
}

/// A returned outcome, of the operation's form.
#[derive(Debug, Eq, PartialEq)]
enum Returned {
    Single(ModelSingleOutcome),
    Aggregate(ModelAggregateOutcome),
}

struct Results {
    form: Form,
    result: AirAnswer,
}

impl Work for Results {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
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
        for (form, result) in [(self.form, self.result), (Form::Single, AirAnswer::Ack)] {
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
                        oer_ieee80211_mac::phy::HtRate::new(
                            HtMcs::new(7).unwrap(),
                            oer_ieee80211_mac::phy::PpduBandwidth::Mhz20,
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
                    let aggregate = push_subframe(aggregate, buffer, subframe_frame(0), None);
                    station
                        .submit_ampdu(
                            (AggregateAttempt {
                                context: (),
                                ticket,
                                aggregate,
                                protection: Protection::None,
                                power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                                coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
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
                    let oer_ieee80211_lower_mac::TerminalParts {
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
                    let oer_ieee80211_lower_mac::TerminalParts {
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
            ticket = returned_ticket;
            let ModelAnyBuffer::SmallSent(sent) = returned_buffer else {
                panic!("publication must return Sent")
            };
            let (fresh, body) = <ModelTypes as TxBufferPool<ModelSmall>>::renew(sent);
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
            let _port_end = block_on(run_port(backend, Body(Results { form, result })));
        }
    }
}

#[test]
fn aggregate_honors_timeout_injection() {
    let (backend, _) = backend();
    let _port_end = block_on(run_port(
        backend,
        Body(Results {
            form: Form::Aggregate,
            result: AirAnswer::HardwareTimeout,
        }),
    ));
}

#[test]
fn an_empty_block_ack_is_consumed_before_the_following_single() {
    let (backend, _) = backend();
    let _port_end = block_on(run_port(
        backend,
        Body(Results {
            form: Form::Aggregate,
            result: AirAnswer::BlockAck(0),
        }),
    ));
}

#[test]
fn an_aggregate_s_unmeasured_block_ack_still_delivers_it() {
    let (backend, _) = backend();
    let _port_end = block_on(run_port(
        backend,
        Body(Results {
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
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
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
    let _port_end = block_on(run_port(backend, Body(WrongAnswer)));
}
