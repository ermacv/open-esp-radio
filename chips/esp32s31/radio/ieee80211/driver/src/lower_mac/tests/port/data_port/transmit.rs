//! The S31 port's transmit path: what a single MPDU's terminal returns, how
//! a retry plan runs out, the order of a published head, its queued tails
//! and the barrier behind them (gate, configuration, key removal, stop),
//! and the named faults that keep a publication's owners.
use super::*;
use oer_protocols_ieee80211_wire::{
    frame::{AddressFields, DecodedHeader},
    qos::WmmAccessCategory,
};

type Access<'id, Id> = TerminalAccess<'id, Tickets, radio::Child<radio::Root<Id>, 2>>;
type Closed<'id, Id> = GateClosed<'id, radio::Child<radio::Root<Id>, 1>>;
type Outcome = SingleOutcome<Ack, Infallible>;
type TestSent<'id, Id> = Sent<'id, Id, TestDevice, 512, Draws>;

/// The sequence control every MPDU of these tests carries.
const SEQUENCE: u16 = 0x4560;

/// Admit an attempt from a prepared source and submit it: the admitted
/// ticket's index.
macro_rules! submit {
    ($fixture:expr, $ticket:expr, $source:expr, $category:expr, $context:expr) => {{
        let attempt = $fixture.port.admit(
            Attempt {
                context: Tag($context, None),
                ticket: $ticket,
                source: $source,
                access_category: $category,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).expect("power"),
                coex_operation: TxOperation::Data,
            }
            .admit(),
        );
        let ticket = <Fixture<'_, _> as DataPortHarness<'_>>::identity(&attempt);
        let now = $fixture.now();
        $fixture
            .port
            .submit(attempt, now, &mut $fixture.outbox)
            .ok()
            .expect("submitted");
        ticket
    }};
}

/// One returned single MPDU, opened with the terminal access.
struct Returned<'id, Id: radio::SlotIdentity + 'id> {
    ticket: Ticket<'id, Id>,
    context: Tag,
    buffer: AnyBuffer<'id, Id, TestDevice, 512, Draws>,
    body: Option<Payload>,
    outcome: Outcome,
}

/// One terminal of the port, in the order the sink took it.
enum Event<'id, Id: radio::SlotIdentity + 'id> {
    Tx(Box<Returned<'id, Id>>),
    GateClosed(Closed<'id, Id>),
    Configured(Tag),
    KeyRemoved,
    Other,
}

/// What an event shows, for comparing orders.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    /// The ticket, whether its MPDU went on the air, and its outcome.
    Tx(u8, bool, Outcome),
    GateClosed,
    Configured,
    KeyRemoved,
    Other,
}

impl<Id: radio::SlotIdentity> Event<'_, Id> {
    fn shape(&self) -> Shape {
        match self {
            Self::Tx(returned) => Shape::Tx(
                returned.ticket.index(),
                matches!(returned.buffer, AnyBuffer::Sent(_)),
                returned.outcome,
            ),
            Self::GateClosed(_) => Shape::GateClosed,
            Self::Configured(_) => Shape::Configured,
            Self::KeyRemoved => Shape::KeyRemoved,
            Self::Other => Shape::Other,
        }
    }
}

fn shapes<Id: radio::SlotIdentity>(events: &[Event<'_, Id>]) -> Vec<Shape> {
    events.iter().map(Event::shape).collect()
}

/// An attempt's plan and owners beyond the fixture's defaults.
struct Spec<'k, 'id, Id: radio::SlotIdentity + 'id> {
    key: Option<&'k PairwiseKey<'id, Id, Station>>,
    transmissions: u8,
    body: Option<Payload>,
    category: WmmAccessCategory,
    context: usize,
}

impl<Id: radio::SlotIdentity> Spec<'_, '_, Id> {
    /// An open Best Effort MPDU sent once.
    fn open(context: usize) -> Self {
        Self {
            key: None,
            transmissions: 1,
            body: None,
            category: WmmAccessCategory::BestEffort,
            context,
        }
    }
}

/// Run `test` on a fresh fixture with the installation's terminal access,
/// which opens the port's terminals.
fn with_access<R>(
    test: impl for<'id> FnOnce(
        &mut Fixture<'id, radio::LocalId<'id>>,
        &mut Access<'id, radio::LocalId<'id>>,
    ) -> R,
) -> R {
    with_fixture(|fixture| {
        let mut access = fixture.access.take().expect("terminal access");
        test(fixture, &mut access)
    })
}

impl<'id, Id: radio::SlotIdentity + 'id> Fixture<'id, Id> {
    /// Submit an individual data MPDU to the BSS under `spec`: its ticket.
    fn submit_mpdu(&mut self, spec: Spec<'_, 'id, Id>) -> u8 {
        let buffer = self.buffers.pop().expect("buffer");
        let ticket = self.tickets.pop().expect("ticket");
        let rate = self
            .port
            .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
            .expect("rate");
        let header = HeaderSpec::new(
            FrameControl::new(oer_protocols_ieee80211_wire::frame::DATA_TO_DS).expect("control"),
            0,
            IndividualAddress::new(BSSID).expect("receiver"),
            STATION,
            BSSID,
            SEQUENCE,
            None,
            None,
            None,
        )
        .expect("header");
        let frame = ClassFrame::new(TxFrame::Individual {
            frame: SizedFrame::new(
                AckHeader::new(header).expect("ACK"),
                MpduLength::new(64).expect("length"),
            )
            .expect("frame"),
            plan: RetryPlan::new(RetryStage {
                rate,
                transmissions: core::num::NonZeroU8::new(spec.transmissions).expect("budget"),
                protection: Protection::None,
            }),
        })
        .ok()
        .expect("class");
        let source = TxSource::Fresh {
            buffer,
            frame,
            body: spec.body,
            protection: match spec.key {
                Some(key) => TxProtection::Ccmp(NumberedKey {
                    key,
                    packet_number: oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber::new(7)
                        .expect("PN"),
                }),
                None => TxProtection::Open,
            },
        }
        .check()
        .ok()
        .expect("layout");
        let source =
            <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::prepare::<Station, Caps>(source)
                .ok()
                .expect("prepared");
        submit!(self, ticket, source, spec.category, spec.context)
    }

    /// Submit a returned Sent MPDU again under its own plan of
    /// `transmissions`: its ticket.
    fn retransmit(&mut self, ticket: Ticket<'id, Id>, mpdu: TestSent<'id, Id>, transmissions: u8) {
        let rate = self
            .port
            .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
            .expect("rate");
        let source = TxSource::Retransmission {
            mpdu,
            plan: RetransmissionPlan::Individual(RetryPlan::new(RetryStage {
                rate,
                transmissions: core::num::NonZeroU8::new(transmissions).expect("budget"),
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
        submit!(self, ticket, source, WmmAccessCategory::BestEffort, 92);
    }

    /// Install the station's PTK for the BSS: the key its terminal returns.
    fn pairwise(
        &mut self,
        material: u8,
    ) -> (
        <PortKeys<Id, Station> as KeyTypes<'id>>::Association,
        PairwiseKey<'id, Id, Station>,
    ) {
        let (association, _, slot) = self.association();
        let (install, _) = self.commands.install_pairwise(
            &association,
            slot,
            IndividualAddress::new(BSSID).expect("peer"),
            group_key(material),
        );
        assert!(matches!(self.run(install), Ok(CommandProgress::Done)));
        match self.outbox.terminals.pop() {
            Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key))) => {
                (association, key)
            }
            _ => panic!("the installation terminal returns the key"),
        }
    }

    /// Serve `turns` turns.
    /// Submit a BlockAckReq of `transmissions` publications at most.
    fn submit_block_ack_request(&mut self, transmissions: u8) {
        let fixture = self;
        let buffer = fixture.buffers.pop().expect("buffer");
        let ticket = fixture.tickets.pop().expect("ticket");
        let Ok(rate) = fixture
            .port
            .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
            .expect("rate")
            .legacy()
        else {
            panic!("a legacy rate");
        };
        let frame = ClassFrame::new(TxFrame::BlockAckRequest {
            receiver: IndividualAddress::new(BSSID).expect("receiver"),
            transmitter: STATION,
            tid: Tid::new(0).expect("TID"),
            starting_sequence:
                oer_protocols_ieee80211_wire::sequence::SequenceNumber::from_low_bits(0x10),
            plan: RetryPlan::new(RetryStage {
                rate,
                transmissions: core::num::NonZeroU8::new(transmissions).expect("budget"),
                protection: Protection::None,
            }),
        })
        .ok()
        .expect("class");
        let source = TxSource::Fresh {
            buffer,
            frame,
            body: None,
            protection: TxProtection::Open,
        }
        .check()
        .ok()
        .expect("layout");
        let source =
            <Family<_> as TxBufferPool<'_, BufferClass<512>>>::prepare::<Station, Caps>(source)
                .ok()
                .expect("prepared");
        submit!(fixture, ticket, source, WmmAccessCategory::BestEffort, 72);
    }

    fn turns(&mut self, turns: usize) {
        for _ in 0..turns {
            if let Some(done) = self.turn() {
                self.settle_arena(done);
            }
        }
    }

    /// Open every terminal the sink holds, in order.
    fn events(&mut self, access: &mut Access<'id, Id>) -> Vec<Event<'id, Id>> {
        core::mem::take(&mut self.outbox.terminals)
            .into_iter()
            .map(|terminal| match terminal {
                LowerMacTerminal::TxCompleted(TxCompleted::Single(completed)) => {
                    let parts = completed.into_parts(access);
                    let (buffer, body) = parts.payload.into_parts();
                    Event::Tx(Box::new(Returned {
                        ticket: parts.ticket,
                        context: parts.context,
                        buffer,
                        body,
                        outcome: parts.outcome,
                    }))
                }
                LowerMacTerminal::GateClosed(gate) => Event::GateClosed(gate),
                LowerMacTerminal::Configured(Configured::Station(permission)) => {
                    Event::Configured(permission.into_parts().1)
                }
                LowerMacTerminal::KeyRemoved(_) => Event::KeyRemoved,
                _ => Event::Other,
            })
            .collect()
    }

    /// Serve turns until the sink holds `count` terminals: them, opened.
    fn until(&mut self, access: &mut Access<'id, Id>, count: usize) -> Vec<Event<'id, Id>> {
        for _ in 0..16 {
            if self.outbox.terminals.len() >= count {
                break;
            }
            self.turns(1);
        }
        self.events(access)
    }

    fn publications(&self) -> usize {
        self.observed.borrow().publications
    }
}

/// The opened single terminal of `events`, the only event there.
fn single<'id, Id: radio::SlotIdentity>(events: Vec<Event<'id, Id>>) -> Returned<'id, Id> {
    let [Event::Tx(returned)] = <[_; 1]>::try_from(events).ok().expect("one terminal") else {
        panic!("a transmit terminal");
    };
    *returned
}

#[test]
fn a_reopened_gate_publishes_and_returns_the_attempts_own_owners() {
    with_access(|fixture, access| {
        fixture.start();
        fixture.configure_station();
        let gate = fixture.gate.take().expect("gate");
        assert!(matches!(
            fixture.run(PortCommand::CloseGate(gate)),
            Ok(CommandProgress::Done)
        ));
        let [Event::GateClosed(closed)] = <[_; 1]>::try_from(fixture.events(access))
            .ok()
            .expect("one terminal")
        else {
            panic!("the gate's terminal");
        };
        fixture.gate = Some(fixture.port.open_gate(closed));
        let ticket = fixture.submit_mpdu(Spec::open(71));
        let returned = single(fixture.until(access, 1));
        assert_eq!(returned.context, 71);
        assert_eq!(returned.ticket.index(), ticket);
        assert!(returned.body.is_none());
        assert!(returned.outcome.is_delivered());
        let AnyBuffer::Sent(mut sent) = returned.buffer else {
            panic!("a published MPDU returns Sent");
        };
        assert_eq!(
            DecodedHeader::decode(sent.published())
                .expect("the published header")
                .sequence_control(),
            SEQUENCE
        );
        assert_eq!(fixture.publications(), 1);
        fixture.stop();
        assert_eq!(fixture.observed.borrow().starts, 1);
        assert_eq!(fixture.observed.borrow().stops, 1);
    });
}

#[test]
fn an_unanswered_mpdu_runs_its_plan_out_in_one_terminal_with_doubling_backoff() {
    with_access(|fixture, access| {
        fixture.observed.borrow_mut().statuses = std::vec![(5, 0); 3];
        fixture.start();
        let ticket = fixture.submit_mpdu(Spec {
            transmissions: 3,
            ..Spec::open(91)
        });
        let returned = single(fixture.until(access, 1));
        assert_eq!(returned.ticket.index(), ticket);
        assert!(
            matches!(returned.outcome, TxOutcome::Ended(SingleEnd::AckTimeout(air)) if air.transmissions.get() == 3),
            "{:?}",
            returned.outcome
        );
        assert_eq!(fixture.publications(), 3);
        // Standard BE CWmin=15 doubles after each unanswered publication;
        // the fixed entropy selects 8, 24 and 56 slots from those windows.
        assert_eq!(fixture.observed.borrow().backoffs, [8, 24, 56]);
    });
}

#[test]
fn an_unanswered_block_ack_request_is_sent_again_without_a_retry_bit() {
    // Neither publication draws a BlockAck: the ordinary transmitter ends
    // each as unanswered, and the plan sends the request again. A control
    // frame has no Retry bit to set (IEEE Std 802.11-2020, 9.2.4.1.5).
    with_access(|fixture, access| {
        fixture.observed.borrow_mut().block_ack = Some((0x10, 0, false));
        fixture.start();
        fixture.configure_station();
        fixture.submit_block_ack_request(2);
        let returned = single(fixture.until(access, 1));
        assert!(
            matches!(returned.outcome, TxOutcome::Ended(SingleEnd::AckTimeout(air)) if air.transmissions.get() == 2),
            "the plan ran out without an answer: {:?}",
            returned.outcome
        );
        assert_eq!(fixture.publications(), 2, "the plan resent it");
        let AnyBuffer::Sent(mut sent) = returned.buffer else {
            panic!("a published request returns Sent");
        };
        let fields = AddressFields::parse(sent.published()).expect("the published request");
        assert!(
            !fields.control().retry(),
            "the resent request carries no Retry bit"
        );
    });
}

#[test]
fn a_block_ack_beside_a_failed_status_delivers_its_request_unmeasured() {
    // The hardware reports the BlockAck's result beside the completion's
    // status, and only a zero status carries an ACK sample: the answer is
    // the access point's, so the request is delivered without a measurement.
    for status in [2, 5] {
        with_access(|fixture, access| {
            {
                let mut observed = fixture.observed.borrow_mut();
                observed.block_ack = Some((0x10, 0b1, true));
                observed.statuses = std::vec![(status, 0)];
            }
            fixture.start();
            fixture.configure_station();
            fixture.submit_block_ack_request(1);
            let returned = single(fixture.until(access, 1));
            let TxOutcome::Ended(SingleEnd::Delivered {
                response: SingleResponse::BlockAck { ack, .. },
                ..
            }) = returned.outcome
            else {
                panic!(
                    "the answered request ends delivered: {:?}",
                    returned.outcome
                );
            };
            assert_eq!(ack.snr_sample(), SnrSample::Unmeasured);
        });
    }
}

#[test]
fn a_key_the_hardware_refuses_ends_its_attempt_without_a_retry() {
    for (status, detail) in [(1, 0xc0), (4, 0xc0)] {
        with_access(|fixture, access| {
            fixture.observed.borrow_mut().statuses = std::vec![(status, detail)];
            fixture.start();
            fixture.submit_mpdu(Spec::open(1));
            let returned = single(fixture.until(access, 1));
            assert_eq!(
                returned.outcome,
                TxOutcome::Indeterminate(IndeterminateCause::KeyRefused)
            );
            assert_eq!(fixture.publications(), 1, "the refused attempt ends");
        });
    }
}

#[test]
fn settings_during_a_configuration_wait_for_its_end() {
    with_access(|fixture, access| {
        fixture.start();
        fixture.hold_air(true);
        fixture.submit_mpdu(Spec::open(1));
        fixture.turns(1);
        let now = fixture.now();
        let command = PortCommand::Configure {
            permission: fixture.configure.take().expect("permission"),
            receive: StationReceive::Off,
        };
        assert!(matches!(
            fixture.port.command(command, now, &mut fixture.outbox),
            Ok(CommandProgress::Done)
        ));
        let edca = SupportedEdca::new(EdcaParameterSet::STANDARD_DEFAULT).expect("EDCA");
        assert!(matches!(
            fixture
                .port
                .command(PortCommand::SetEdca(edca), now, &mut fixture.outbox),
            Ok(CommandProgress::Done)
        ));
        fixture.turns(2);
        assert_eq!(fixture.port.fault(), None);
        fixture.hold_air(false);
        fixture.until(access, 2);
        assert_eq!(fixture.port.fault(), None);
    });
}

#[test]
fn caller_pn_and_external_body_survive_tx_and_a_removed_key_refuses_retransmission() {
    with_access(|fixture, access| {
        fixture.start();
        fixture.configure_station();
        let (association, key) = fixture.pairwise(9);
        let dropped = Rc::new(Cell::new(0));
        fixture.submit_mpdu(Spec {
            key: Some(&key),
            body: Some(Payload(dropped.clone())),
            ..Spec::open(91)
        });
        let returned = single(fixture.until(access, 1));
        assert!(returned.outcome.is_delivered());
        assert!(returned.body.is_none(), "a Sent MPDU keeps its body");
        assert_eq!(dropped.get(), 0);
        let AnyBuffer::Sent(mpdu) = returned.buffer else {
            panic!("Sent");
        };
        assert_eq!(mpdu.packet_number().expect("caller PN").value(), 7);
        assert_eq!(mpdu.body().expect("retained body").bytes(), b"payload");
        assert!(matches!(
            fixture.run(PortCommand::RemovePairwise { association, key }),
            Ok(CommandProgress::Done)
        ));
        assert!(matches!(
            fixture.events(access).as_slice(),
            [Event::KeyRemoved]
        ));
        let rate = fixture
            .port
            .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
            .expect("rate");
        let source = TxSource::Retransmission {
            mpdu,
            plan: RetransmissionPlan::Individual(RetryPlan::new(RetryStage {
                rate,
                transmissions: core::num::NonZeroU8::MIN,
                protection: Protection::None,
            })),
        }
        .check()
        .ok()
        .expect("layout");
        let refused =
            <Family<_> as TxBufferPool<'_, BufferClass<512>>>::prepare::<Station, Caps>(source)
                .err()
                .expect("retired key refused before admission");
        assert_eq!(refused.reason, PreparationError::RetransmissionKey);
        drop(refused);
        assert_eq!(dropped.get(), 1, "the refusal returned the body");
        assert_eq!(fixture.publications(), 1);
    });
}

/// A barrier that queued tails meet behind a published head.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Barrier {
    CloseGate,
    Configure,
}

/// Publish a held head, queue two tails behind it on its AC, request
/// `barrier`, then let the head's air answer: the terminals in order.
fn barrier_order(barrier: Barrier) {
    with_access(|fixture, access| {
        fixture.start();
        fixture.hold_air(true);
        let head = fixture.submit_mpdu(Spec::open(1));
        assert_eq!(fixture.publications(), 0, "the device reports at its poll");
        fixture.turns(1);
        assert_eq!(fixture.publications(), 1);
        let tail = fixture.submit_mpdu(Spec::open(2));
        let last = fixture.submit_mpdu(Spec::open(3));
        let command = match barrier {
            Barrier::CloseGate => PortCommand::CloseGate(fixture.gate.take().expect("gate")),
            Barrier::Configure => PortCommand::Configure {
                permission: fixture.configure.take().expect("permission"),
                receive: StationReceive::Off,
            },
        };
        let now = fixture.now();
        assert!(matches!(
            fixture.port.command(command, now, &mut fixture.outbox),
            Ok(CommandProgress::Done)
        ));
        fixture.turns(4);
        assert!(
            fixture.outbox.terminals.is_empty(),
            "{barrier:?}: neither a queued tail nor its barrier overtakes the published head"
        );
        fixture.hold_air(false);
        let events = fixture.until(access, 4);
        let cause = match barrier {
            Barrier::CloseGate => WithdrawCause::GateClosed,
            Barrier::Configure => WithdrawCause::Disabled,
        };
        let confirmation = match barrier {
            Barrier::CloseGate => Shape::GateClosed,
            Barrier::Configure => Shape::Configured,
        };
        let shapes = shapes(&events);
        let [Shape::Tx(first, true, delivered), rest @ ..] = shapes.as_slice() else {
            panic!("{barrier:?}: the published head ends first");
        };
        assert_eq!(*first, head);
        assert!(delivered.is_delivered());
        assert_eq!(
            rest,
            [
                Shape::Tx(tail, false, TxOutcome::Withdrawn(cause)),
                Shape::Tx(last, false, TxOutcome::Withdrawn(cause)),
                confirmation,
            ],
            "{barrier:?}: tails in their order, then the confirmation"
        );
        if let Some(Event::Configured(context)) = events.last() {
            assert_eq!(*context, 7, "the caller's own permission returns");
        }
        assert_eq!(fixture.publications(), 1, "no tail was published");
    });
}

#[test]
fn a_closed_gate_returns_the_published_head_then_its_queued_tails_then_confirms() {
    barrier_order(Barrier::CloseGate);
}

#[test]
fn a_configuration_returns_the_published_head_then_its_queued_tails_then_confirms() {
    barrier_order(Barrier::Configure);
}

/// A key removal or a stop meeting a published head and its tails.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Withdrawal {
    /// The head and its tails are under the removed key.
    RemoveWithHead,
    /// The open head is unrelated; the tails are under the removed key.
    RemoveBehindOtherHead,
    /// As above, the unrelated head answered only by its second publication.
    RemoveBehindRetryingHead,
    /// The port stops behind the published head.
    Stop,
}

fn withdrawal_order(kind: Withdrawal) {
    with_access(|fixture, access| {
        if kind == Withdrawal::RemoveBehindRetryingHead {
            fixture.observed.borrow_mut().statuses = std::vec![(5, 0), (0, 0)];
        }
        fixture.start();
        let (association, key) = fixture.pairwise(7);
        fixture.hold_air(true);
        let head = fixture.submit_mpdu(Spec {
            key: (kind == Withdrawal::RemoveWithHead).then_some(&key),
            transmissions: if kind == Withdrawal::RemoveBehindRetryingHead {
                2
            } else {
                1
            },
            ..Spec::open(1)
        });
        fixture.turns(1);
        assert_eq!(fixture.publications(), 1);
        let keyed = (kind != Withdrawal::Stop).then_some(&key);
        let tail = fixture.submit_mpdu(Spec {
            key: keyed,
            ..Spec::open(2)
        });
        let last = fixture.submit_mpdu(Spec {
            key: keyed,
            ..Spec::open(3)
        });
        let cause = if kind == Withdrawal::Stop {
            let waker = fixture.waker();
            let now = fixture.now();
            assert!(
                fixture
                    .port
                    .poll_stop(&mut Context::from_waker(&waker), now)
                    .is_pending()
            );
            fixture.turns(4);
            assert!(
                fixture
                    .port
                    .poll_stop(&mut Context::from_waker(&waker), now)
                    .is_pending(),
                "the stop waits for the published head"
            );
            assert!(
                fixture.outbox.terminals.is_empty(),
                "the stop keeps the tails behind the published head"
            );
            WithdrawCause::Disabled
        } else {
            let removal = PortCommand::RemovePairwise { association, key };
            let now = fixture.now();
            let removal = match fixture.port.command(removal, now, &mut fixture.outbox) {
                Ok(CommandProgress::Held(removal)) => removal,
                _ => panic!("{kind:?}: the removal waits behind the published head"),
            };
            fixture.turns(4);
            assert!(
                fixture.outbox.terminals.is_empty(),
                "{kind:?}: a tail cannot overtake a published head on its AC"
            );
            fixture.hold_air(false);
            assert!(matches!(fixture.run(removal), Ok(CommandProgress::Done)));
            WithdrawCause::KeyRemoved
        };
        if kind == Withdrawal::Stop {
            fixture.hold_air(false);
            fixture.stop();
        }
        let events = fixture.until(access, 3);
        let shapes = shapes(&events);
        let [Shape::Tx(first, true, delivered), rest @ ..] = shapes.as_slice() else {
            panic!("{kind:?}: the published head ends first");
        };
        assert_eq!(*first, head);
        assert!(delivered.is_delivered(), "{kind:?}: {delivered:?}");
        let tails = [
            Shape::Tx(tail, false, TxOutcome::Withdrawn(cause)),
            Shape::Tx(last, false, TxOutcome::Withdrawn(cause)),
        ];
        assert_eq!(
            rest.get(..2),
            Some(&tails[..]),
            "{kind:?}: original AC order"
        );
        if kind != Withdrawal::Stop {
            assert_eq!(rest.get(2..), Some(&[Shape::KeyRemoved][..]));
        }
        assert_eq!(
            fixture.publications(),
            if kind == Withdrawal::RemoveBehindRetryingHead {
                2
            } else {
                1
            }
        );
    });
}

#[test]
fn a_pairwise_removal_returns_its_published_head_then_withdraws_its_tails() {
    withdrawal_order(Withdrawal::RemoveWithHead);
}

#[test]
fn a_pairwise_removal_keeps_its_tails_behind_an_unrelated_published_head() {
    withdrawal_order(Withdrawal::RemoveBehindOtherHead);
}

#[test]
fn a_pairwise_removal_keeps_its_tails_behind_an_unrelated_head_still_retrying() {
    withdrawal_order(Withdrawal::RemoveBehindRetryingHead);
}

#[test]
fn a_stop_withdraws_queued_tails_only_after_the_published_head_ends() {
    withdrawal_order(Withdrawal::Stop);
}

#[test]
fn a_sent_mpdu_retransmitted_after_reopening_runs_its_own_plan() {
    // The receiver stays absent: the keyed head's first publication goes
    // unanswered while the gate closes, so it returns Sent; sent again
    // after the gate reopens, it runs its own plan of two to the timeout.
    with_access(|fixture, access| {
        fixture.observed.borrow_mut().statuses = std::vec![(5, 0); 3];
        fixture.start();
        let (association, key) = fixture.pairwise(7);
        fixture.hold_air(true);
        let head = fixture.submit_mpdu(Spec {
            key: Some(&key),
            transmissions: 2,
            ..Spec::open(1)
        });
        let tail = fixture.submit_mpdu(Spec {
            key: Some(&key),
            transmissions: 2,
            ..Spec::open(2)
        });
        fixture.turns(2);
        let gate = fixture.gate.take().expect("gate");
        let now = fixture.now();
        assert!(matches!(
            fixture
                .port
                .command(PortCommand::CloseGate(gate), now, &mut fixture.outbox),
            Ok(CommandProgress::Done)
        ));
        fixture.hold_air(false);
        let mut events = fixture.until(access, 3);
        assert_eq!(
            shapes(&events),
            [
                Shape::Tx(head, true, TxOutcome::Withdrawn(WithdrawCause::GateClosed)),
                Shape::Tx(tail, false, TxOutcome::Withdrawn(WithdrawCause::GateClosed)),
                Shape::GateClosed,
            ]
        );
        let Some(Event::GateClosed(closed)) = events.pop() else {
            panic!("the gate's terminal");
        };
        fixture.gate = Some(fixture.port.open_gate(closed));
        let Some(Event::Tx(returned)) = events.into_iter().next() else {
            panic!("the head's terminal");
        };
        let returned = *returned;
        let AnyBuffer::Sent(mpdu) = returned.buffer else {
            panic!("the Sent head");
        };
        fixture.retransmit(returned.ticket, mpdu, 2);
        let retried = single(fixture.until(access, 1));
        assert_eq!(retried.ticket.index(), head);
        assert!(
            matches!(retried.outcome, TxOutcome::Ended(SingleEnd::AckTimeout(air)) if air.transmissions.get() == 2),
            "{:?}",
            retried.outcome
        );
        let AnyBuffer::Sent(mut sent) = retried.buffer else {
            panic!("a retransmission returns Sent");
        };
        let header = DecodedHeader::decode(sent.published()).expect("the published header");
        assert!(
            header.control().retry(),
            "a data MPDU sent again is a retry"
        );
        assert_eq!(fixture.publications(), 3);
        assert!(matches!(
            fixture.run(PortCommand::RemovePairwise { association, key }),
            Ok(CommandProgress::Done)
        ));
    });
}

/// What ends a queue held behind its published head.
#[derive(Clone, Copy, Debug, PartialEq)]
enum QueueEnd {
    CloseGate,
    Stop,
}

#[test]
fn a_held_best_effort_head_keeps_its_queue_while_voice_and_video_complete() {
    for end in [QueueEnd::CloseGate, QueueEnd::Stop] {
        let stop = end == QueueEnd::Stop;
        with_access(|fixture, access| {
            fixture.start();
            let best_effort = crate::lower_mac::tests::queue_of(WmmAccessCategory::BestEffort);
            fixture.observed.borrow_mut().held_queues[best_effort] = true;
            let blocker = fixture.submit_mpdu(Spec::open(4));
            let head = fixture.submit_mpdu(Spec::open(0));
            let tail = fixture.submit_mpdu(Spec::open(1));
            let voice = fixture.submit_mpdu(Spec {
                category: WmmAccessCategory::Voice,
                ..Spec::open(2)
            });
            let video = fixture.submit_mpdu(Spec {
                category: WmmAccessCategory::Video,
                ..Spec::open(3)
            });
            let early = fixture.until(access, 2);
            let mut early = shapes(&early);
            early.sort_by_key(|shape| match shape {
                Shape::Tx(ticket, ..) => *ticket,
                _ => u8::MAX,
            });
            let mut expected = [voice, video];
            expected.sort_unstable();
            assert!(
                matches!(early.as_slice(), [Shape::Tx(a, true, x), Shape::Tx(b, true, y)]
                    if [*a, *b] == expected && x.is_delivered() && y.is_delivered()),
                "{end:?}: Voice and Video end past the held Best Effort head: {early:?}"
            );
            assert_eq!(fixture.publications(), 3, "the pair stays queued");
            if stop {
                let waker = fixture.waker();
                let now = fixture.now();
                assert!(
                    fixture
                        .port
                        .poll_stop(&mut Context::from_waker(&waker), now)
                        .is_pending()
                );
            } else {
                let gate = fixture.gate.take().expect("gate");
                let now = fixture.now();
                assert!(matches!(
                    fixture
                        .port
                        .command(PortCommand::CloseGate(gate), now, &mut fixture.outbox),
                    Ok(CommandProgress::Done)
                ));
            }
            fixture.turns(4);
            assert!(fixture.outbox.terminals.is_empty(), "{end:?}");
            fixture.observed.borrow_mut().held_queues[best_effort] = false;
            if stop {
                fixture.stop();
            }
            let events = fixture.until(access, if stop { 3 } else { 4 });
            let cause = if stop {
                WithdrawCause::Disabled
            } else {
                WithdrawCause::GateClosed
            };
            let shapes = shapes(&events);
            let [Shape::Tx(first, true, ended), rest @ ..] = shapes.as_slice() else {
                panic!("{end:?}: the blocker ends first");
            };
            assert_eq!(*first, blocker);
            assert!(ended.is_delivered());
            assert_eq!(
                rest.get(..2),
                Some(
                    &[
                        Shape::Tx(head, false, TxOutcome::Withdrawn(cause)),
                        Shape::Tx(tail, false, TxOutcome::Withdrawn(cause)),
                    ][..]
                ),
                "{end:?}: the pair returns withdrawn in FIFO order"
            );
            if !stop {
                assert_eq!(rest.get(2..), Some(&[Shape::GateClosed][..]));
            }
            assert_eq!(fixture.publications(), 3, "the pair was never published");
            assert_eq!(
                contexts(&events),
                [4, 0, 1],
                "{end:?}: each owner its context"
            );
        });
    }
}

/// The contexts of the transmit terminals in `events`, in order.
fn contexts<Id: radio::SlotIdentity>(events: &[Event<'_, Id>]) -> Vec<usize> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Tx(returned) => Some(returned.context.0),
            _ => None,
        })
        .collect()
}

#[test]
fn a_publication_past_its_bound_without_a_timeout_edge_ends_and_returns_its_owners() {
    // The air held the queue past its own timeout: the bound cancels the
    // frame, a handled end, and the port goes on.
    with_access(|fixture, access| {
        fixture.start();
        fixture.hold_air(true);
        fixture.submit_mpdu(Spec::open(1));
        fixture.turns(1);
        assert_eq!(fixture.publications(), 1);
        // The bound, then the abort's settle.
        for _ in 0..2 {
            let at = fixture.port.deadline().expect("the frame's deadline");
            fixture.advance(at.saturating_duration_since(fixture.now()));
            fixture.turns(1);
        }
        assert_eq!(fixture.port.fault(), None);
        assert_eq!(fixture.events(access).len(), 1, "the owners come back");
    });
}

#[test]
fn a_publication_the_cancel_finds_on_the_air_faults_past_its_last_bound_and_keeps_its_owners() {
    // No completion after the abort found the PPDU on the air: the port
    // cannot prove its DMA stopped, so it keeps the operation's owners in
    // its quarantine and answers every later call with a named fault.
    with_access(|fixture, access| {
        fixture.start();
        fixture.hold_air(true);
        fixture.observed.borrow_mut().taken_by_air = true;
        fixture.submit_mpdu(Spec::open(1));
        fixture.turns(1);
        assert_eq!(fixture.publications(), 1);
        let mut fault = None;
        for _ in 0..8 {
            let Some(at) = fixture.port.deadline() else {
                break;
            };
            fixture.advance(at.saturating_duration_since(fixture.now()));
            let waker = fixture.waker();
            let now = fixture.now();
            let facts = fixture.port.poll_facts(&mut Context::from_waker(&waker));
            if let Err(found) = fixture.port.service_tx(
                facts.tx.unwrap_or(TxFacts::DUE),
                facts.tbtt,
                now,
                &mut fixture.outbox,
            ) {
                fault = Some(found);
                break;
            }
        }
        let fault = fault.expect("the publication's last bound ends in a named fault");
        assert_eq!(
            fault,
            Fault::Core(crate::lower_mac::LowerMacFault::Tx(
                crate::ordinary_tx::OrdinaryTxError::RadioResetRequired(
                    crate::ordinary_tx::TxResetReason::ExecutorDeadline
                )
            ))
        );
        assert_eq!(fixture.port.fault(), Some(fault));
        assert!(
            fixture.events(access).is_empty(),
            "no terminal returns the owners"
        );
        let now = fixture.now();
        assert!(matches!(fixture.port.receive(4, now), Err(found) if found == fault));
        let waker = fixture.waker();
        assert!(matches!(
            fixture
                .port
                .poll_stop(&mut Context::from_waker(&waker), now),
            Poll::Ready(Err(StopUnconfirmed {
                cause: StopCause::Device,
                ..
            }))
        ));
        assert_eq!(fixture.publications(), 1);
    });
}

#[test]
fn an_arena_publication_without_the_registers_is_a_fault_that_returns_it() {
    with_access(|fixture, _| {
        fixture.start();
        fixture.observed.borrow_mut().hardware_unavailable = true;
        let now = fixture.now();
        let mut arena = fixture.port.arena();
        let [first, _] = arena.arenas().expect("the arenas");
        let memory = crate::mac::tx::test_support::model_backing().expect("model memory");
        assert!(arena.intake(&first, memory, write_frame).is_ok());
        let rate = arena
            .rate(PhyRate::Legacy(LegacyRate::Ofdm24M))
            .expect("the rate");
        let id = first.id();
        let published = arena.publish(
            ArenaPublication {
                arena: first,
                format: ArenaFormat::Single(RetryPlan::new(RetryStage {
                    rate,
                    transmissions: core::num::NonZeroU8::MIN,
                    protection: Protection::None,
                })),
                access_category: WmmAccessCategory::BestEffort,
                protection: Protection::None,
                encryption: MpduProtection::Open,
                power: SupportedPower::new(TxPower::Calibrated).expect("power"),
            },
            now,
        );
        let Err(PublishError::FaultBeforeTaking { publication, fault }) = published else {
            panic!("a fault that returns the publication");
        };
        assert_eq!(fault, Fault::HardwareUnavailable);
        assert_eq!(publication.arena.id(), id, "the arena's right came back");
    });
}

#[test]
fn a_port_without_its_registers_faults_without_publishing() {
    // The registers are held by an actor transaction: the port's first
    // hardware access is the typed HardwareUnavailable fault, not a panic.
    with_access(|fixture, access| {
        fixture.start();
        fixture.observed.borrow_mut().hardware_unavailable = true;
        fixture.submit_mpdu(Spec::open(1));
        assert_eq!(fixture.port.fault(), Some(Fault::HardwareUnavailable));
        fixture.observed.borrow_mut().hardware_unavailable = false;
        let now = fixture.now();
        assert!(matches!(
            fixture
                .port
                .service_tx(TxFacts::DUE, false, now, &mut fixture.outbox),
            Err(Fault::HardwareUnavailable)
        ));
        assert!(
            fixture.events(access).is_empty(),
            "the quarantine keeps the attempt"
        );
        assert_eq!(fixture.publications(), 0);
    });
}
