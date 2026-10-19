//! Public-port response policy, independent of the model's private classifiers.
use super::*;
use embassy_futures::yield_now;
use oer_ieee80211_lower_mac::PhaseTypes;
use oer_ieee80211_lower_mac::{
    AggregateAttempt, AggregateEnd, Air, AmpduPhase, BlockAckReport, CheckedAggregate,
    ClassSubframe, GroupAddress, IndeterminateCause, MpduStartSpacing, NoAckHeader, QosControl,
    SingleEnd, SingleResponse, SubframeSource, Tid, TxMpdu, TxOutcome, WithdrawCause,
};
use oer_ieee80211_mac::{
    phy::{HtMcs, HtRate, PpduBandwidth},
    sequence::SequenceNumber,
};
use oer_ieee80211_runtime::lower_mac_owner::model::{
    AirAnswer, ModelAggregateOutcome, ModelCaps, ModelPhase, ModelSingleOutcome,
};

/// What a terminal reported, for single MPDUs and aggregates alike.
#[derive(Debug, Eq, PartialEq)]
enum Seen {
    /// Delivered; `report` is the BlockAck's window, and `solicited` whether
    /// a response answered it.
    Delivered {
        air: Air,
        report: Option<BlockAckReport>,
        solicited: bool,
    },
    AckTimeout(Air),
    ContentionLimit(Option<Air>),
    Withdrawn(WithdrawCause),
    Indeterminate(IndeterminateCause),
    Other,
}

impl Seen {
    fn single(outcome: ModelSingleOutcome) -> Self {
        match outcome {
            TxOutcome::Ended(SingleEnd::Delivered { air, response }) => Self::Delivered {
                air,
                report: match response {
                    SingleResponse::BlockAck { report, .. } => Some(report),
                    SingleResponse::Unsolicited | SingleResponse::Ack(_) => None,
                },
                solicited: !matches!(response, SingleResponse::Unsolicited),
            },
            TxOutcome::Ended(SingleEnd::AckTimeout(air)) => Self::AckTimeout(air),
            TxOutcome::Ended(SingleEnd::ContentionLimit(air)) => Self::ContentionLimit(air),
            TxOutcome::Ended(SingleEnd::CtsTimeout(_) | SingleEnd::AirDenied { .. }) => Self::Other,
            TxOutcome::Withdrawn(cause) => Self::Withdrawn(cause),
            TxOutcome::Indeterminate(cause) => Self::Indeterminate(cause),
        }
    }

    fn aggregate(outcome: ModelAggregateOutcome) -> Self {
        match outcome {
            TxOutcome::Ended(AggregateEnd::Delivered { air, report, .. }) => Self::Delivered {
                air,
                report: Some(report),
                solicited: true,
            },
            TxOutcome::Ended(AggregateEnd::AckTimeout(air)) => Self::AckTimeout(air),
            TxOutcome::Ended(AggregateEnd::ContentionLimit(air)) => Self::ContentionLimit(air),
            TxOutcome::Ended(AggregateEnd::CtsTimeout(_) | AggregateEnd::AirDenied { .. }) => {
                Self::Other
            }
            TxOutcome::Withdrawn(cause) => Self::Withdrawn(cause),
            TxOutcome::Indeterminate(cause) => Self::Indeterminate(cause),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Frame {
    Individual,
    NoAck,
    Group,
    Bar,
    Aggregate,
}

#[derive(Clone, Copy, Debug)]
enum Barrier {
    None,
    Gate,
    Disable,
}

#[derive(Clone, Copy, Debug)]
enum Expected {
    Delivered(Option<BlockAckReport>),
    Unacknowledged,
    Missing,
    Collision,
    Timeout,
    Rejected,
}

struct Case {
    air: Rc<ModelAir>,
    frame: Frame,
    answer: Option<AirAnswer>,
    expected: Expected,
    barrier: Barrier,
}

fn header() -> HeaderSpec<IndividualAddress> {
    HeaderSpec::new(
        FrameControl::new(0x0188).unwrap(),
        0,
        IndividualAddress::new(PEER).unwrap(),
        STATION,
        PEER,
        7 << 4,
        None,
        Some(QosControl {
            tid: Tid::new(0).unwrap(),
            end_of_service_period: false,
            amsdu_present: false,
            high: 0,
        }),
        None,
    )
    .unwrap()
}

fn frame<'p, 'id: 'p>(phase: &ModelPhase<'p, 'id>, kind: Frame) -> TxFrame<'p, 'id, ModelCaps> {
    let rate = phase.rate(PhyRate::Legacy(LegacyRate::Ofdm6M)).unwrap();
    let plan = RetryPlan::new(RetryStage {
        rate,
        transmissions: NonZeroU8::MIN,
        protection: Protection::None,
    });
    match kind {
        Frame::Individual => TxFrame::Individual {
            frame: SizedFrame::new(
                AckHeader::new(header()).unwrap(),
                MpduLength::new(40).unwrap(),
            )
            .unwrap(),
            plan,
        },
        Frame::NoAck => TxFrame::IndividualNoAck {
            frame: SizedFrame::new(
                NoAckHeader::new(header()).unwrap(),
                MpduLength::new(40).unwrap(),
            )
            .unwrap(),
            rate: phase.no_ack(rate).ok().unwrap(),
        },
        Frame::Group => TxFrame::Group {
            frame: SizedFrame::new(
                HeaderSpec::new(
                    FrameControl::new(0x0040).unwrap(),
                    0,
                    GroupAddress::new([0xff; 6]).unwrap(),
                    STATION,
                    [0xff; 6],
                    0,
                    None,
                    None,
                    None,
                )
                .unwrap(),
                MpduLength::new(40).unwrap(),
            )
            .unwrap(),
            rate,
        },
        Frame::Bar => TxFrame::BlockAckRequest {
            receiver: IndividualAddress::new(PEER).unwrap(),
            transmitter: STATION,
            tid: Tid::new(0).unwrap(),
            starting_sequence: SequenceNumber::new(7).unwrap(),
            plan: RetryPlan::new(RetryStage {
                rate: rate.legacy().unwrap(),
                transmissions: NonZeroU8::MIN,
                protection: Protection::None,
            }),
        },
        Frame::Aggregate => unreachable!(),
    }
}

impl Case {
    fn check<'id>(
        &self,
        event: LowerMacEvent<'id, ModelTypes>,
        ticket_index: u8,
        terminal_access: &mut oer_ieee80211_lower_mac::TerminalAccess<
            'id,
            <ModelTypes as PhaseTypes<'id>>::TicketKind,
            <ModelTypes as PhaseTypes<'id>>::TicketPath,
        >,
    ) -> bool {
        let (ticket, payload, outcome) = match event {
            LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(
                terminal,
            ))) => {
                assert!(!matches!(self.frame, Frame::Aggregate));
                let terminal = terminal.into_parts(terminal_access);
                (
                    terminal.ticket,
                    terminal.payload,
                    Seen::single(terminal.outcome),
                )
            }
            LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Aggregate(
                terminal,
            ))) => {
                assert!(matches!(self.frame, Frame::Aggregate));
                let terminal = terminal.into_parts(terminal_access);
                assert_eq!(terminal.payload.len(), 1);
                (
                    terminal.ticket,
                    terminal.payload.into_iter().next().unwrap(),
                    Seen::aggregate(terminal.outcome),
                )
            }
            LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(_)) => return false,
            _ => panic!("unexpected event"),
        };
        assert_eq!(ticket.index(), ticket_index);
        let (buffer, body) = payload.into_parts();
        assert!(matches!(buffer, ModelAnyBuffer::SmallSent(_)));
        assert!(body.is_none());
        let rate = match self.frame {
            Frame::Aggregate => PhyRate::Ht(
                HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
            ),
            _ => PhyRate::Legacy(LegacyRate::Ofdm6M),
        };
        let air = Air {
            transmissions: core::num::NonZeroU16::MIN,
            rate,
        };
        let expected = match self.expected {
            Expected::Delivered(report) => Seen::Delivered {
                air,
                report,
                solicited: true,
            },
            Expected::Unacknowledged => Seen::Delivered {
                air,
                report: None,
                solicited: false,
            },
            Expected::Timeout => Seen::Indeterminate(IndeterminateCause::HardwareTimeout),
            Expected::Missing | Expected::Collision => match self.barrier {
                Barrier::Gate => Seen::Withdrawn(WithdrawCause::GateClosed),
                Barrier::Disable => Seen::Withdrawn(WithdrawCause::Disabled),
                Barrier::None if matches!(self.expected, Expected::Missing) => {
                    Seen::AckTimeout(air)
                }
                Barrier::None => Seen::ContentionLimit(None),
            },
            Expected::Rejected => panic!("incompatible answer fabricated success"),
        };
        assert_eq!(
            outcome, expected,
            "{:?} / {:?} / {:?}",
            self.frame, self.answer, self.barrier
        );
        true
    }
}

impl PortBody<ModelBackend> for Case {
    type Output = ();
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, ModelBackend, Disabled>,
        mut resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend, ()> {
        let mut owner = owner.enable().await.ok().unwrap();
        if let Some(answer) = self.answer {
            self.air.answer(answer);
        }
        self.air
            .hold_completion(WmmAccessCategory::BestEffort, true);
        let ticket = resources.tickets.pop().unwrap();
        let ticket_index = ticket.index();
        let buffer = resources.small.pop().unwrap();
        let mut seen = 0;
        {
            let mut phase = owner.phase();
            if matches!(self.frame, Frame::Aggregate) {
                let rate = phase
                    .rate(PhyRate::Ht(
                        HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
                    ))
                    .unwrap();
                let mut station = phase.station();
                let rate = station.ampdu_rate(rate).ok().unwrap();
                let aggregate = station.aggregate(
                    IndividualAddress::new(PEER).unwrap(),
                    Tid::new(0).unwrap(),
                    rate,
                    TxProtection::Open,
                );
                let source = SubframeSource::Fresh {
                    buffer,
                    frame: ClassSubframe::new(
                        SizedFrame::new(
                            AckHeader::new(header()).unwrap(),
                            MpduLength::new(40).unwrap(),
                        )
                        .unwrap(),
                    )
                    .ok()
                    .unwrap(),
                    body: None,
                };
                let checked = CheckedAggregate::new(aggregate, source).ok().unwrap();
                let aggregate = <ModelTypes as TxBufferPool<ModelSmall>>::push_aggregate(checked)
                    .ok()
                    .unwrap();
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
            } else {
                let source = TxSource::Fresh {
                    buffer,
                    frame: ClassFrame::new(frame(&phase, self.frame)).ok().unwrap(),
                    body: None,
                    protection: TxProtection::Open,
                };
                let source = <ModelTypes as TxBufferPool<ModelSmall>>::prepare::<Station, _>(
                    source.check().ok().unwrap(),
                )
                .ok()
                .unwrap();
                phase
                    .station()
                    .submit(
                        (Attempt {
                            context: (),
                            ticket,
                            source,
                            access_category: WmmAccessCategory::BestEffort,
                            protection: Protection::None,
                            power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                            coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
                        })
                        .admit(),
                    )
                    .unwrap();
            }
            if matches!(self.expected, Expected::Rejected) {
                assert!(phase.next_event().await.is_err());
                assert!(self.air.sent().is_empty());
            } else {
                while self.air.sent().is_empty() {
                    yield_now().await;
                }
                if matches!(self.barrier, Barrier::Gate) {
                    phase.close_gate(resources.gate);
                }
                self.air
                    .hold_completion(WmmAccessCategory::BestEffort, false);
                if !matches!(self.barrier, Barrier::Disable) {
                    while seen == 0 {
                        seen += usize::from(self.check(
                            phase.next_event().await.unwrap().unwrap(),
                            ticket_index,
                            &mut resources.terminal_access,
                        ));
                    }
                }
            }
        }
        if matches!(self.expected, Expected::Rejected) {
            return Ended::fail_stopped(owner.fail_stop(), ());
        }
        let mut sink = |event| {
            seen += usize::from(self.check(
                LowerMacEvent::Terminal(event),
                ticket_index,
                &mut resources.terminal_access,
            ));
            core::ops::ControlFlow::Continue(())
        };
        let owner = owner.disable(&mut sink).await.ok().unwrap();
        assert_eq!(seen, 1);
        assert_eq!(self.air.sent().len(), 1);
        Ended::uninstall(owner, (), ())
    }
}

fn exercise(frame: Frame, answer: Option<AirAnswer>, expected: Expected, barrier: Barrier) {
    let (backend, air) = backend();
    let result = run(run_port(
        backend,
        Case {
            air,
            frame,
            answer,
            expected,
            barrier,
        },
    ));
    assert_eq!(
        matches!(result, oer_radio_port::PortEnd::FailStopped { .. }),
        matches!(expected, Expected::Rejected)
    );
}

#[test]
fn checked_frames_determine_the_solicited_response_across_barriers() {
    for barrier in [Barrier::None, Barrier::Gate, Barrier::Disable] {
        for kind in [
            Frame::Individual,
            Frame::NoAck,
            Frame::Group,
            Frame::Bar,
            Frame::Aggregate,
        ] {
            let success = match kind {
                Frame::Individual => Expected::Delivered(None),
                Frame::NoAck | Frame::Group => Expected::Unacknowledged,
                Frame::Bar | Frame::Aggregate => Expected::Delivered(Some(BlockAckReport {
                    start_sequence: SequenceNumber::new(7).unwrap(),
                    bitmap: if matches!(kind, Frame::Bar) {
                        u64::MAX
                    } else {
                        1
                    },
                })),
            };
            exercise(kind, None, success, barrier);
            exercise(
                kind,
                Some(AirAnswer::NoResponse),
                if matches!(kind, Frame::NoAck | Frame::Group) {
                    Expected::Unacknowledged
                } else {
                    Expected::Missing
                },
                barrier,
            );
            exercise(
                kind,
                Some(AirAnswer::Collision),
                Expected::Collision,
                barrier,
            );
            exercise(
                kind,
                Some(AirAnswer::HardwareTimeout),
                Expected::Timeout,
                barrier,
            );
        }
        for kind in [Frame::Bar, Frame::Aggregate] {
            for bitmap in [0, 1, 1 << 63] {
                exercise(
                    kind,
                    Some(AirAnswer::BlockAck(bitmap)),
                    Expected::Delivered(Some(BlockAckReport {
                        start_sequence: SequenceNumber::new(7).unwrap(),
                        bitmap: if matches!(kind, Frame::Bar) {
                            bitmap
                        } else {
                            bitmap & 1
                        },
                    })),
                    barrier,
                );
            }
        }
    }
}

#[test]
fn unsolicited_responses_poison_before_publication() {
    for (kind, answer) in [
        (Frame::Individual, AirAnswer::BlockAck(1)),
        (Frame::NoAck, AirAnswer::Ack),
        (Frame::NoAck, AirAnswer::BlockAck(1)),
        (Frame::Group, AirAnswer::Ack),
        (Frame::Group, AirAnswer::BlockAck(1)),
        (Frame::Bar, AirAnswer::Ack),
        (Frame::Aggregate, AirAnswer::Ack),
    ] {
        exercise(kind, Some(answer), Expected::Rejected, Barrier::None);
    }
}
