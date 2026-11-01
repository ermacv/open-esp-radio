//! Aggregates through the S31 port: built from pool buffers, published
//! through a descriptor chain, ended by their one terminal with every
//! subframe back in order, as the reference model ends them.
use super::*;
use oer_ieee80211_wire::phy::{HtMcs, HtRate, PpduBandwidth};

/// A QoS data subframe of TID 0 to the BSS: `length` octets at `sequence`.
pub(super) fn subframe(sequence: u16, length: usize) -> ClassSubframe<BufferClass<512>, Caps> {
    let header = HeaderSpec::new(
        FrameControl::new(oer_ieee80211_wire::frame::QOS_DATA_TO_DS).expect("control"),
        0,
        IndividualAddress::new(BSSID).expect("receiver"),
        STATION,
        BSSID,
        sequence << 4,
        None,
        Some(QosControl {
            tid: Tid::new(0).expect("TID"),
            end_of_service_period: false,
            amsdu_present: false,
            high: 0,
        }),
        None,
    )
    .expect("header");
    ClassSubframe::new(
        SizedFrame::new(
            AckHeader::new(header).expect("ACK"),
            MpduLength::new(length).expect("length"),
        )
        .expect("frame"),
    )
    .ok()
    .expect("a QoS data subframe")
}

/// How the scenario's aggregates go.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    /// The recipient acknowledges the first two of three subframes.
    Delivered,
    /// No BlockAck arrives.
    NoResponse,
    /// The gate closes before the driver publishes it.
    Withdrawn,
    /// Every MSDU ages before the publication.
    Expired,
    /// The unacknowledged subframes go again in a second aggregate.
    Retransmitted,
}

/// What a scenario observed of its last aggregate's terminal.
#[derive(Debug, Default)]
struct Seen {
    ends: Vec<&'static str>,
    report: Option<BlockAckReport>,
    expired: u64,
    /// Each returned subframe: its buffer place and whether it is Sent.
    returned: Vec<(u8, bool)>,
    /// The buffer places admitted, in order.
    admitted: Vec<u8>,
    /// Whether every retransmitted MPDU kept its sequence number and
    /// carries the Retry bit.
    retry_marked: bool,
    ticket: Option<(u8, u8)>,
}

struct AggregateBody {
    scenario: Scenario,
    timer: TestTimer,
    observed: Rc<RefCell<Observed>>,
    seen: Rc<RefCell<Seen>>,
}

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for AggregateBody {
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
            let permission = resources
                .station_commands
                .configure
                .with_context(Tag(42, None));
            phase.station().configure(
                permission,
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_)))) =
                phase.next_event().await
            else {
                panic!("configuration terminal")
            };
            if self.scenario == Scenario::Expired {
                let table = EdcaParameterSet::new(
                    EdcaParameterSet::STANDARD_DEFAULT.wmm(),
                    [oer_time::Duration::from_micros(100); 4],
                );
                let _applied = phase
                    .station()
                    .set_edca(SupportedEdca::new(table).expect("a sub-TU lifetime"));
            }
            let mut buffers = resources.buffers.into_iter();
            let mut tickets = resources.tickets.into_iter();
            let rate = PhyRate::Ht(
                HtRate::new(HtMcs::new(7).expect("MCS"), PpduBandwidth::Mhz20, false)
                    .expect("HT rate"),
            );
            let rate = phase.rate(rate).expect("a supported rate");
            let ticket = tickets.next().expect("a ticket");
            let ticket_index = ticket.index();
            let mut station = phase.station();
            let rate = station.ampdu_rate(rate).ok().expect("an aggregate rate");
            let mut builder = Some(station.aggregate(
                IndividualAddress::new(BSSID).expect("receiver"),
                Tid::new(0).expect("TID"),
                rate,
                TxProtection::Open,
            ));
            let mut admitted = Vec::new();
            let mut nonempty = None;
            for sequence in 100..103 {
                let buffer = buffers.next().expect("a pool buffer");
                admitted.push(buffer.index());
                let source = SubframeSource::Fresh {
                    numbering: TxNumbering::Open,
                    buffer,
                    frame: subframe(sequence, 64),
                    body: None,
                };
                nonempty = Some(match (builder.take(), nonempty.take()) {
                    (Some(empty), None) => push(empty, source),
                    (None, Some(nonempty)) => push(nonempty, source),
                    _ => unreachable!("one builder"),
                });
            }
            let aggregate = nonempty.expect("three subframes");
            assert_eq!(aggregate.subframes(), 3);
            self.seen.borrow_mut().admitted = admitted.clone();
            submit(&mut station, aggregate, ticket);
            drop(station);
            match self.scenario {
                Scenario::Withdrawn => phase.close_gate(resources.gate),
                Scenario::Expired => {
                    // Before P2 runs: every queued MSDU ages.
                    self.timer
                        .0
                        .advance(oer_time::Duration::from_micros(100))
                        .expect("virtual time");
                }
                _ => {}
            }
            let returned = terminal(&mut phase, &mut resources.terminal_access, &self.seen).await;
            if self.scenario == Scenario::Retransmitted {
                // The planner resends what the report missed: every Sent
                // subframe, unchanged, in a new aggregate.
                self.observed.borrow_mut().block_ack = Some((100, 0b111, true));
                let ticket = tickets.next().expect("a second ticket");
                let mut station = phase.station();
                let mut builder = Some(station.aggregate(
                    IndividualAddress::new(BSSID).expect("receiver"),
                    Tid::new(0).expect("TID"),
                    rate,
                    TxProtection::Open,
                ));
                let mut nonempty = None;
                for buffer in returned {
                    let AnyBuffer::Sent(sent) = buffer else {
                        panic!("a published subframe returns Sent")
                    };
                    let source = SubframeSource::Sent(sent);
                    nonempty = Some(match (builder.take(), nonempty.take()) {
                        (Some(empty), None) => push(empty, source),
                        (None, Some(nonempty)) => push(nonempty, source),
                        _ => unreachable!("one builder"),
                    });
                }
                submit(&mut station, nonempty.expect("the Sent subframes"), ticket);
                drop(station);
                let returned =
                    terminal(&mut phase, &mut resources.terminal_access, &self.seen).await;
                let mut marked = true;
                for (sequence, buffer) in (100..).zip(returned) {
                    let AnyBuffer::Sent(mut sent) = buffer else {
                        panic!("a published subframe returns Sent")
                    };
                    let bytes = sent.published();
                    let control = FrameControl::new(u16::from_le_bytes([bytes[0], bytes[1]]))
                        .expect("control");
                    marked &= control.retry()
                        && u16::from_le_bytes([bytes[22], bytes[23]]) == sequence << 4;
                }
                self.seen.borrow_mut().retry_marked = marked;
            }
            self.seen.borrow_mut().ticket = Some((ticket_index, ticket_index));
        }
        let Ok(owner) = owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
        else {
            panic!("disable")
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

pub(super) type Builder<'k, 'p, 'id, Id, S> =
    crate::port::Aggregate<'k, 'p, 'id, Id, TestDevice, 512, Draws, Station, Caps, S>;

/// Add `source` to `aggregate`; the scenario's subframes are all accepted.
pub(super) fn push<'k, 'p, 'id: 'k + 'p, Id: radio::SlotIdentity + 'id, S: AggregateState>(
    aggregate: Builder<'k, 'p, 'id, Id, S>,
    source: PoolSubframe<'id, Family<Id>, BufferClass<512>, Caps>,
) -> Builder<'k, 'p, 'id, Id, NonEmptyAggregate> {
    let checked = CheckedAggregate::new(aggregate, source)
        .ok()
        .expect("a subframe layout");
    <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::push_aggregate(checked)
        .ok()
        .expect("an accepted subframe")
}

/// Submit `aggregate` at the recipient's widest spacing.
pub(super) fn submit<'k, 'p, 'v, 'id: 'k + 'p, Id: radio::SlotIdentity + 'id>(
    station: &mut Esp32s31Interface<'p, 'v, 'id, Id, TestDevice, 512, Draws, Station>,
    aggregate: Builder<'k, 'p, 'id, Id, NonEmptyAggregate>,
    ticket: radio::Slot<'id, Tickets, radio::Child<radio::Root<Id>, 2>>,
) {
    station
        .submit_ampdu(
            AggregateAttempt {
                context: Tag(7, None),
                ticket,
                aggregate,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).expect("power"),
                min_mpdu_start_spacing: MpduStartSpacing::new(7).expect("spacing"),
            }
            .admit(),
        )
        .expect("submit");
}

/// The aggregate's terminal: its end and every subframe's MPDU, in order.
async fn terminal<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &mut Esp32s31Phase<'p, 'id, Id, TestDevice, 512, Draws>,
    access: &mut TerminalAccess<'id, Tickets, radio::Child<radio::Root<Id>, 2>>,
    seen: &Rc<RefCell<Seen>>,
) -> Vec<AnyBuffer<'id, Id, TestDevice, 512, Draws>> {
    loop {
        match phase.next_event().await {
            Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                TxCompleted::Aggregate(completed),
            )))) => {
                let returned = completed.into_parts(access);
                let mut seen = seen.borrow_mut();
                match returned.outcome {
                    TxOutcome::Ended(AggregateEnd::Delivered { air, report, .. }) => {
                        assert_eq!(air.transmissions, core::num::NonZeroU16::MIN);
                        seen.report = Some(report);
                        seen.ends.push("delivered");
                    }
                    TxOutcome::Ended(AggregateEnd::AckTimeout(_)) => seen.ends.push("ack-timeout"),
                    TxOutcome::Withdrawn(WithdrawCause::GateClosed) => seen.ends.push("withdrawn"),
                    TxOutcome::Expired { msdus } => {
                        seen.expired = msdus;
                        seen.ends.push("expired");
                    }
                    _ => seen.ends.push("other"),
                }
                let buffers: Vec<_> = returned
                    .payload
                    .into_iter()
                    .map(|mpdu| mpdu.into_parts().0)
                    .collect();
                seen.returned = buffers
                    .iter()
                    .map(|buffer| match buffer {
                        AnyBuffer::Fresh(fresh) => (fresh.index(), false),
                        AnyBuffer::Sent(sent) => (sent.index(), true),
                    })
                    .collect();
                return buffers;
            }
            Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(_)))) => {
                seen.borrow_mut().ends.push("gate-closed");
            }
            Ok(Ok(_)) => {}
            _ => panic!("the aggregate's terminal"),
        }
    }
}

fn exercise(scenario: Scenario) -> (Seen, Rc<RefCell<Observed>>) {
    let statuses = match scenario {
        Scenario::NoResponse | Scenario::Retransmitted => std::vec![(5, 0), (0, 0)],
        _ => std::vec![(0, 0)],
    };
    let (parts, observed, timer) = installed(statuses);
    observed.borrow_mut().block_ack = Some(match scenario {
        Scenario::NoResponse | Scenario::Retransmitted => (0, 0, false),
        _ => (100, 0b011, true),
    });
    let seen = Rc::new(RefCell::new(Seen::default()));
    let body = AggregateBody {
        scenario,
        timer,
        observed: observed.clone(),
        seen: seen.clone(),
    };
    let Report::Uninstalled { .. } = block_on(run_body(parts, body)) else {
        panic!("clean uninstall")
    };
    let seen = seen.take();
    (seen, observed)
}

#[test]
fn a_block_ack_ends_the_aggregate_and_returns_every_subframe_sent_in_order() {
    let (seen, observed) = exercise(Scenario::Delivered);
    assert_eq!(seen.ends, ["delivered"]);
    assert_eq!(
        seen.report,
        Some(BlockAckReport::new(
            oer_ieee80211_wire::sequence::SequenceNumber::from_low_bits(100),
            0b011
        ))
    );
    let places: Vec<_> = seen.returned.iter().map(|(place, _)| *place).collect();
    assert_eq!(places, seen.admitted);
    assert!(seen.returned.iter().all(|(_, sent)| *sent));
    assert_eq!(observed.borrow().publications, 1, "one PPDU, one attempt");
}

#[test]
fn an_unanswered_aggregate_ends_without_a_retry_of_its_own() {
    let (seen, observed) = exercise(Scenario::NoResponse);
    assert_eq!(seen.ends, ["ack-timeout"]);
    assert!(seen.returned.iter().all(|(_, sent)| *sent));
    assert_eq!(observed.borrow().publications, 1);
}

#[test]
fn a_closed_gate_withdraws_an_unpublished_aggregate_with_its_fresh_subframes() {
    let (seen, observed) = exercise(Scenario::Withdrawn);
    assert_eq!(seen.ends, ["withdrawn"]);
    let places: Vec<_> = seen.returned.iter().map(|(place, _)| *place).collect();
    assert_eq!(places, seen.admitted);
    assert!(seen.returned.iter().all(|(_, sent)| !*sent));
    assert_eq!(observed.borrow().publications, 0);
}

#[test]
fn aged_subframes_expire_by_position_before_publication() {
    let (seen, observed) = exercise(Scenario::Expired);
    assert_eq!(seen.ends, ["expired"]);
    assert_eq!(seen.expired, 0b111);
    assert!(seen.returned.iter().all(|(_, sent)| !*sent));
    assert_eq!(observed.borrow().publications, 0);
}

#[test]
fn a_sent_subframe_goes_again_with_its_sequence_number_and_the_retry_bit() {
    let (seen, observed) = exercise(Scenario::Retransmitted);
    assert_eq!(seen.ends, ["ack-timeout", "delivered"]);
    assert!(seen.retry_marked);
    assert_eq!(observed.borrow().publications, 2);
}

/// What a builder refused, by reason, with the refused buffer's place
/// coming back unchanged.
#[derive(Debug, Default)]
struct Refusals {
    reasons: Vec<(PreparationError, bool)>,
    /// Subframes the builder held after each refusal.
    held: Vec<usize>,
}

/// Builds aggregates and offers them subframes they must refuse.
struct RefusalBody {
    seen: Rc<RefCell<Refusals>>,
}

/// Offer `source` and expect a refusal: its reason, whether the subframe
/// came back at its place, and the unchanged builder.
fn refused<'k, 'p, 'id: 'k + 'p, Id: radio::SlotIdentity + 'id, S: AggregateState>(
    aggregate: Builder<'k, 'p, 'id, Id, S>,
    buffer: Buffer<'id, Id, TestDevice, 512, Draws>,
    frame: ClassSubframe<BufferClass<512>, Caps>,
    seen: &Rc<RefCell<Refusals>>,
) -> Builder<'k, 'p, 'id, Id, S> {
    let place = buffer.index();
    let source = SubframeSource::Fresh {
        numbering: TxNumbering::Open,
        buffer,
        frame,
        body: None,
    };
    let checked = CheckedAggregate::new(aggregate, source)
        .ok()
        .expect("a subframe layout");
    let Err(refusal) = <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::push_aggregate(checked)
    else {
        panic!("the builder refuses the subframe")
    };
    let returned = matches!(
        refusal.subframe,
        SubframeSource::Fresh { ref buffer, .. } if buffer.index() == place
    );
    let mut seen = seen.borrow_mut();
    seen.reasons.push((refusal.reason, returned));
    seen.held.push(refusal.aggregate.subframes());
    refusal.aggregate
}

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for RefusalBody {
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
        {
            let mut phase = owner.phase();
            let mut buffers = resources.buffers.into_iter();
            let rate = PhyRate::Ht(
                HtRate::new(HtMcs::new(7).expect("MCS"), PpduBandwidth::Mhz20, false)
                    .expect("HT rate"),
            );
            let rate = phase.rate(rate).expect("a supported rate");
            let mut station = phase.station();
            let rate = station.ampdu_rate(rate).ok().expect("an aggregate rate");
            let mut builder = || {
                station.aggregate(
                    IndividualAddress::new(BSSID).expect("receiver"),
                    Tid::new(0).expect("TID"),
                    rate,
                    TxProtection::Open,
                )
            };
            let fresh =
                |buffers: &mut dyn Iterator<Item = Buffer<'id, Id, TestDevice, 512, Draws>>,
                 sequence,
                 length| SubframeSource::Fresh {
                    numbering: TxNumbering::Open,
                    buffer: buffers.next().expect("a pool buffer"),
                    frame: subframe(sequence, length),
                    body: None,
                };

            // The BlockAck window: no repeated sequence number, none 64 or
            // more after the first.
            let aggregate = push(builder(), fresh(&mut buffers, 100, 64));
            let aggregate = refused(
                aggregate,
                buffers.next().expect("a pool buffer"),
                subframe(100, 64),
                &self.seen,
            );
            let aggregate = refused(
                aggregate,
                buffers.next().expect("a pool buffer"),
                subframe(164, 64),
                &self.seen,
            );
            let aggregate = push(aggregate, fresh(&mut buffers, 163, 64));
            drop(aggregate);

            // The subframe count: a place per BlockAck bit.
            let mut aggregate = push(builder(), fresh(&mut buffers, 0, 64));
            for sequence in 1..32 {
                aggregate = push(aggregate, fresh(&mut buffers, sequence, 64));
            }
            let aggregate = refused(
                aggregate,
                buffers.next().expect("a pool buffer"),
                subframe(32, 64),
                &self.seen,
            );
            drop(aggregate);

            // The length: at most the declared maximum, with every
            // subframe's FCS and delimiter at the widest spacing.
            let mut aggregate = push(builder(), fresh(&mut buffers, 0, 480));
            for sequence in 1..13 {
                aggregate = push(aggregate, fresh(&mut buffers, sequence, 480));
            }
            let aggregate = refused(
                aggregate,
                buffers.next().expect("a pool buffer"),
                subframe(13, 480),
                &self.seen,
            );
            drop(aggregate);
        }
        let Ok(owner) = owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
        else {
            panic!("disable")
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

#[test]
fn a_builder_refuses_what_its_window_count_or_length_cannot_hold_and_returns_it() {
    let (parts, observed, _) = installed(Vec::new());
    let seen = Rc::new(RefCell::new(Refusals::default()));
    let body = RefusalBody { seen: seen.clone() };
    let Report::Uninstalled { .. } = block_on(run_body(parts, body)) else {
        panic!("clean uninstall")
    };
    let seen = seen.take();
    assert_eq!(
        seen.reasons,
        [
            (PreparationError::BlockAckWindow, true),
            (PreparationError::BlockAckWindow, true),
            (PreparationError::AggregateFull, true),
            (PreparationError::AggregateFull, true),
        ]
    );
    assert_eq!(seen.held, [1, 1, 32, 13]);
    assert_eq!(observed.borrow().publications, 0);
}
