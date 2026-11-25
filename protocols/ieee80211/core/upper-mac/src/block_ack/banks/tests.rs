//! The owner decides each agreement edge and hands it to its port; the port
//! here records what the integration would carry out.

extern crate std;

use super::*;
use oer_protocols_ieee802_wire::{Eui48, IndividualAddress};
use oer_protocols_ieee80211_contracts_port::{
    BandSet, Channel, ChannelWidth, HardwareServices, LowerMacCapabilities, PhyFormatSet,
    RateSupport, ReceiveFilter, RxMeta, WidthSet,
};
use oer_protocols_ieee80211_wire::management::DialogToken;
use oer_protocols_ieee80211_wire::phy::{HeMcs, HtMcs};
use oer_protocols_ieee80211_wire::{
    block_ack::frame::{encode_addba_request, encode_block_ack_request, parse_block_ack_action},
    frame::{MacHeader, QosControl},
    status::status_code,
};
use std::vec::Vec;

const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const AP: [u8; 6] = [2, 0, 0, 0, 0, 2];
const TID_0: Tid = Tid::constant::<0>();
const TID_2: Tid = Tid::constant::<2>();
const GAP: Duration = Duration::from_micros(10_000);

struct Caps;

impl Ieee80211Capabilities for Caps {
    type SppScope = oer_protocols_ieee80211_wire::security::spp::Association;
    const CAPS: LowerMacCapabilities = LowerMacCapabilities {
        bands: BandSet::GHZ2_4,
        formats: oer_protocols_ieee80211_contracts_port::FormatWidths {
            non_ht: WidthSet::MHZ20,
            ht: WidthSet::MHZ20,
            he: WidthSet::MHZ20,
        },
        rates: RateSupport {
            dsss_cck: oer_protocols_ieee80211_contracts_port::Support::Supported,
            ofdm: oer_protocols_ieee80211_contracts_port::Support::Supported,
            ht_max_mcs: HtMcs::new(7),
            he_max_mcs: HeMcs::new(9),

            he_dcm: oer_protocols_ieee80211_contracts_port::Support::Unsupported,
            he_ldpc: oer_protocols_ieee80211_contracts_port::Support::Unsupported,
            spatial_streams: 1,
        },
        services: HardwareServices::FCS,
        vifs: 1,
        tx_queues: 4,
        max_mpdu_length: 2304,
        tx_power_ceiling_min_dbm: None,
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: ReceiveFilter::BSS_MEMBER,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        rx_block_ack_max_tid: oer_protocols_ieee80211_wire::qos::Tid::constant::<7>(),
        rx_block_ack_max_window: 64,
        rx_amsdu_in_ampdu: true,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
        edca_max_txop_limit_units_32_us: u16::MAX,
    };
}

struct Frame(Vec<u8>);

/// The agreements' calls with a frame's view beside its owner, as a round
/// gives them: the harness reads the view from a copy of the owner's bytes,
/// since its owner here is the bytes themselves.
trait Given<'v, E> {
    fn given(
        &mut self,
        now: Instant,
        local: Option<[u8; 6]>,
        frame: Frame,
        meta: RxMeta,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) -> RxDisposition<Types>;
    /// An Action offered, each bank command it owes carried out at once by
    /// `port`, and released frames recycled through it.
    #[allow(clippy::too_many_arguments, reason = "the owner's offer with its port")]
    fn offered(
        &mut self,
        now: Instant,
        association: E,
        bssid: Bssid,
        action: BlockAckAction,
        agreements: RxAgreements,
        port: &mut Recording,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) -> Result<(), RxBlockAckRefused>;
    /// A tick, its bank commands carried out at once by `port`.
    fn ticked(
        &mut self,
        now: Instant,
        port: &mut Recording,
        events: &mut impl EventSink<RxBlockAckEvent>,
    );
    /// A clear, its frames recycled and its commands carried out by `port`.
    fn cleared(&mut self, port: &mut Recording, events: &mut impl EventSink<RxBlockAckEvent>);
}

fn header_of(bytes: &[u8]) -> RxHead {
    RxHead::of(bytes).unwrap()
}

impl<'v, E: Copy + Eq, const WINDOW: usize> Given<'v, E> for RxBlockAckView<'v, Types, E, WINDOW> {
    fn given(
        &mut self,
        now: Instant,
        local: Option<[u8; 6]>,
        frame: Frame,
        meta: RxMeta,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) -> RxDisposition<Types> {
        let bytes = frame.0.clone();
        let header = header_of(&bytes);
        let view = RxView {
            head: &header,
            bytes: &bytes,
        };
        self.receive(
            now,
            local.map(Eui48::new),
            frame,
            view,
            &meta,
            RetainRoom::Free,
            events,
        )
    }

    fn offered(
        &mut self,
        now: Instant,
        association: E,
        bssid: Bssid,
        action: BlockAckAction,
        agreements: RxAgreements,
        port: &mut Recording,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) -> Result<(), RxBlockAckRefused> {
        let answer = self.offer(
            now,
            association,
            bssid,
            action,
            agreements,
            events,
            |frame| {
                port.0.push(Call::Recycle(frame.0));
            },
        );
        drive(self, port, events);
        answer
    }

    fn ticked(
        &mut self,
        now: Instant,
        port: &mut Recording,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) {
        self.tick(now, events);
        drive(self, port, events);
    }

    fn cleared(&mut self, port: &mut Recording, events: &mut impl EventSink<RxBlockAckEvent>) {
        self.clear(events, |frame| port.0.push(Call::Recycle(frame.0)));
        drive(self, port, events);
    }
}

/// Carry out every command the banks owe, each ending at once: an
/// addition runs, a removal empties its slot.
fn drive<E: Copy + Eq, const WINDOW: usize>(
    view: &mut RxBlockAckView<'_, Types, E, WINDOW>,
    port: &mut Recording,
    events: &mut impl EventSink<RxBlockAckEvent>,
) {
    while let Some(command) = view.next_command() {
        match command {
            BankCommand::Add {
                ticket,
                slot,
                agreement,
            } => {
                port.0.push(Call::Add {
                    slot,
                    tid: agreement.get().1.get(),
                    window: agreement.get().3,
                });
                view.added(ticket, (slot, agreement.get().1.get()), events);
            }
            BankCommand::End { ticket, running } => {
                port.0.push(Call::Remove(running.0));
                view.emptied(ticket, running.0);
            }
        }
    }
}

struct Types;

impl RxBlockAckTypes for Types {
    type Slot = u8;
    type Running = (u8, u8);
    type Frame = Frame;
    type Caps = Caps;
}

#[derive(Debug, Eq, PartialEq)]
enum Call {
    Add { slot: u8, tid: u8, window: u16 },
    Remove(u8),
    Recycle(Vec<u8>),
}

/// The port's calls.
#[derive(Default)]
struct Recording(Vec<Call>);

fn bssid() -> Bssid {
    Bssid::new(IndividualAddress::new(Eui48::new(AP)).unwrap()).unwrap()
}

fn addba(tid: u8, timeout_tu: u16) -> BlockAckAction {
    let body = encode_addba_request(
        DialogToken::new(5),
        Tid::new(tid).unwrap(),
        8,
        timeout_tu,
        oer_protocols_ieee80211_wire::block_ack::frame::AmsduSupport::NotSupported,
        SequenceNumber::new(100).unwrap(),
    )
    .unwrap();
    parse_block_ack_action(&body).unwrap()
}

fn qos_data(tid: u8, sequence: u16) -> Frame {
    // QoS Data, From DS: Frame Control, Duration/ID, Addresses 1 to 3,
    // Sequence Control, QoS Control.
    let mut bytes = std::vec![0x88, 0x02, 0, 0];
    bytes.extend_from_slice(&LOCAL);
    bytes.extend_from_slice(&AP);
    bytes.extend_from_slice(&AP);
    bytes.extend_from_slice(
        &SequenceNumber::new(sequence)
            .unwrap()
            .sequence_control()
            .to_bytes(),
    );
    bytes.extend_from_slice(&QosControl::from_bytes([tid, 0]).to_bytes());
    assert!(MacHeader::decode(&bytes).is_some_and(|header| header.header_len() == bytes.len()));
    bytes.extend_from_slice(&sequence.to_le_bytes());
    Frame(bytes)
}

fn meta() -> RxMeta {
    RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap())
}

fn now(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

fn delivered(disposition: RxDisposition<Types>) -> Option<Vec<u8>> {
    match disposition {
        RxDisposition::Deliver(frame) => Some(frame.0),
        _ => None,
    }
}

#[test]
fn an_admitted_request_installs_one_agreement_and_owes_its_success_response() {
    let mut storage = RxBlockAckStorage::<Types, u32, 2, 8, 8>::new([10, 11], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        addba(3, 0),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    assert_eq!(
        port.0,
        [Call::Add {
            slot: 10,
            tid: 3,
            window: 8
        }]
    );
    let reply = view.next_reply().unwrap();
    assert_eq!((reply.place, reply.association), (ReplyPlace::Bank(0), 7));
    match parse_block_ack_action(&reply.body).unwrap() {
        BlockAckAction::AddbaResponse {
            status, parameters, ..
        } => {
            assert_eq!(
                (status, parameters.tid()),
                (status_code::SUCCESS, Tid::constant::<3>())
            );
        }
        other => panic!("expected an ADDBA Response, got {other:?}"),
    }
    view.sent(reply.place);
    assert!(view.next_reply().is_none());
}

#[test]
fn a_64_mpdu_request_gets_the_16_mpdu_window_its_frames_hold_and_reorders_across_it() {
    let mut storage = RxBlockAckStorage::<Types, u32, 2, 16, 16>::new([10, 11], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    let body = encode_addba_request(
        DialogToken::new(5),
        Tid::constant::<0>(),
        64,
        0,
        oer_protocols_ieee80211_wire::block_ack::frame::AmsduSupport::NotSupported,
        SequenceNumber::new(100).unwrap(),
    )
    .unwrap();
    let request = parse_block_ack_action(&body).unwrap();
    view.offered(
        now(1),
        7,
        bssid(),
        request,
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    assert_eq!(
        port.0,
        [Call::Add {
            slot: 10,
            tid: 0,
            window: 16
        }]
    );
    let reply = view.next_reply().unwrap();
    match parse_block_ack_action(&reply.body).unwrap() {
        BlockAckAction::AddbaResponse {
            status, parameters, ..
        } => {
            assert_eq!((status, parameters.window()), (status_code::SUCCESS, 16));
        }
        other => panic!("expected an ADDBA Response, got {other:?}"),
    }
    view.sent(reply.place);
    // The first MPDU of a 16-MPDU aggregate is missing: the other 15 wait,
    // then all 16 leave in sequence order when it arrives.
    for sequence in 101..116 {
        assert!(matches!(
            view.given(now(2), Some(LOCAL), qos_data(0, sequence), meta(), &mut ()),
            RxDisposition::Retained
        ));
    }
    assert!(view.pop_ready().is_none());
    assert!(matches!(
        view.given(now(3), Some(LOCAL), qos_data(0, 100), meta(), &mut ()),
        RxDisposition::Retained
    ));
    let order: Vec<_> = core::iter::from_fn(|| view.pop_ready())
        .map(|frame| frame.0)
        .collect();
    let expected: Vec<_> = (100..116).map(|sequence| qos_data(0, sequence).0).collect();
    assert_eq!(order, expected);
}

#[test]
fn a_station_without_agreements_declines_through_the_reply_queue() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        addba(3, 0),
        RxAgreements::Declined,
        &mut port,
        &mut (),
    )
    .unwrap();
    assert!(port.0.is_empty());
    let reply = view.next_reply().unwrap();
    assert_eq!(reply.place, ReplyPlace::Queued);
    match parse_block_ack_action(&reply.body).unwrap() {
        BlockAckAction::AddbaResponse { status, .. } => {
            assert_eq!(status, status_code::REQUEST_DECLINED);
        }
        other => panic!("expected an ADDBA Response, got {other:?}"),
    }
}

#[test]
fn reordered_data_is_retained_then_released_in_sequence_order() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        addba(0, 0),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    assert!(matches!(
        view.given(now(2), Some(LOCAL), qos_data(0, 101), meta(), &mut ()),
        RxDisposition::Retained
    ));
    assert!(view.pop_ready().is_none());
    let deadline = storage.deadline();
    assert!(deadline.is_some());
    let mut view = storage.view();
    assert!(matches!(
        view.given(now(3), Some(LOCAL), qos_data(0, 100), meta(), &mut ()),
        RxDisposition::Retained
    ));
    let order: Vec<_> = core::iter::from_fn(|| view.pop_ready())
        .map(|frame| frame.0)
        .collect();
    assert_eq!(order, [qos_data(0, 100).0, qos_data(0, 101).0]);
    // Before a link exists nothing is reordered.
    assert_eq!(
        delivered(view.given(now(4), None, qos_data(0, 103), meta(), &mut ())),
        Some(qos_data(0, 103).0)
    );
}

#[test]
fn a_frame_of_another_receiver_or_transmitter_moves_no_window() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        addba(0, 0),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    const OTHER: [u8; 6] = [2, 0, 0, 0, 0, 0x77];
    let bar = |receiver, transmitter| {
        encode_block_ack_request(
            Eui48::new(receiver),
            Eui48::new(transmitter),
            Tid::constant::<0>(),
            SequenceNumber::new(102).unwrap(),
        )
    };
    for at in [4, 10] {
        let mut frame = qos_data(0, 101);
        frame.0[at..at + 6].copy_from_slice(&OTHER);
        let bytes = frame.0.clone();
        assert_eq!(
            delivered(view.given(now(2), Some(LOCAL), frame, meta(), &mut ())),
            Some(bytes),
            "address at {at}"
        );
    }
    // A fragment, the last one, is not reordered either.
    let mut frame = qos_data(0, 101);
    frame.0[22] |= 1;
    let bytes = frame.0.clone();
    assert_eq!(
        delivered(view.given(now(2), Some(LOCAL), frame, meta(), &mut ())),
        Some(bytes)
    );
    for request in [bar(OTHER, AP), bar(LOCAL, OTHER)] {
        view.given(
            now(3),
            Some(LOCAL),
            Frame(request.to_vec()),
            meta(),
            &mut (),
        );
    }
    // The window is where it was: the sequence it starts at is in order.
    assert!(matches!(
        view.given(now(4), Some(LOCAL), qos_data(0, 101), meta(), &mut ()),
        RxDisposition::Retained
    ));
    assert!(view.pop_ready().is_none());
}

#[test]
fn a_forward_block_ack_request_releases_the_hole_and_is_recycled() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        addba(0, 0),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    assert!(matches!(
        view.given(now(2), Some(LOCAL), qos_data(0, 101), meta(), &mut ()),
        RxDisposition::Retained
    ));
    let bar = encode_block_ack_request(
        Eui48::new(LOCAL),
        Eui48::new(AP),
        Tid::constant::<0>(),
        SequenceNumber::new(102).unwrap(),
    );
    assert!(matches!(
        view.given(now(3), Some(LOCAL), Frame(bar.to_vec()), meta(), &mut ()),
        RxDisposition::Recycle(_)
    ));
    assert_eq!(view.pop_ready().unwrap().0, qos_data(0, 101).0);
}

#[test]
fn inactivity_ends_the_agreement_with_a_delba_and_returns_its_slot() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        addba(2, 1),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    let response = view.next_reply().unwrap();
    view.sent(response.place);
    view.ticked(now(1 + 1024), &mut port, &mut ());
    assert_eq!(port.0.last(), Some(&Call::Remove(10)));
    let reply = view.next_reply().unwrap();
    assert_eq!(reply.place, ReplyPlace::Queued);
    assert!(matches!(
        parse_block_ack_action(&reply.body),
        Some(BlockAckAction::Delba {
            tid: TID_2,
            initiator: oer_protocols_ieee80211_wire::block_ack::frame::DelbaInitiator::Recipient,
            ..
        })
    ));
}

#[test]
fn a_new_association_returns_every_bank_and_retained_frame_first() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        addba(0, 0),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    assert!(matches!(
        view.given(now(2), Some(LOCAL), qos_data(0, 101), meta(), &mut ()),
        RxDisposition::Retained
    ));
    // The old association's agreement is removed first: the request asked
    // meanwhile goes unanswered, and the peer's next one is added.
    assert_eq!(
        view.offered(
            now(3),
            8,
            bssid(),
            addba(0, 0),
            RxAgreements::Admitted,
            &mut port,
            &mut (),
        ),
        Err(RxBlockAckRefused::Capacity(RxBlockAckCapacity))
    );
    view.offered(
        now(4),
        8,
        bssid(),
        addba(0, 0),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    assert_eq!(
        port.0,
        [
            Call::Add {
                slot: 10,
                tid: 0,
                window: 8
            },
            Call::Recycle(qos_data(0, 101).0),
            Call::Remove(10),
            Call::Add {
                slot: 10,
                tid: 0,
                window: 8
            },
        ]
    );
    assert_eq!(view.association(), Some(8));
}

#[test]
fn an_external_reply_place_never_panics_or_changes_another_bank() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        addba(0, 0),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    view.sent(ReplyPlace::Bank(usize::MAX));
    assert_eq!(view.next_reply().unwrap().place, ReplyPlace::Bank(0));
}

#[derive(Default)]
struct Kept(Vec<RxBlockAckEvent>);

impl EventSink<RxBlockAckEvent> for Kept {
    fn event(&mut self, event: RxBlockAckEvent) {
        self.0.push(event);
    }
}

#[test]
fn events_follow_real_ba_reorder_timeout_duplicate_and_clear_decisions() {
    let mut events = Kept::default();
    let mut storage = RxBlockAckStorage::<Types, u8, 1, 8, 8>::new([0], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        addba(0, 0),
        RxAgreements::Admitted,
        &mut port,
        &mut events,
    )
    .unwrap();
    assert!(matches!(
        view.given(now(2), Some(LOCAL), qos_data(0, 101), meta(), &mut events),
        RxDisposition::Retained
    ));
    assert!(matches!(
        view.given(now(3), Some(LOCAL), qos_data(0, 101), meta(), &mut events),
        RxDisposition::Recycle(_)
    ));
    view.ticked(now(20_000), &mut port, &mut events);
    assert!(view.pop_ready().is_some());
    view.cleared(&mut port, &mut events);
    let events = events.0;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                RxBlockAckEvent::Started {
                    tid: TID_0,
                    window: 8
                }
            ))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                RxBlockAckEvent::Stopped {
                    tid: TID_0,
                    reason: AgreementStop::Clear
                }
            ))
            .count(),
        1
    );
    assert!(events.iter().any(|event| matches!(
        event,
        RxBlockAckEvent::Reordered {
            cause: ReorderCause::Receive,
            held: 1,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RxBlockAckEvent::Reordered {
            cause: ReorderCause::Duplicate,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RxBlockAckEvent::Reordered {
            cause: ReorderCause::Timeout,
            released: 1,
            missing: 1,
            ..
        }
    )));
}

#[test]
fn containment_is_built_only_for_an_enabled_sink_and_the_view_holds_no_sink() {
    use core::cell::Cell;
    #[derive(Clone, Copy)]
    struct Counted<'a> {
        meta: RxMeta,
        reads: &'a Cell<u32>,
    }
    impl RxMetadata for Counted<'_> {
        fn common(&self) -> &RxMeta {
            self.reads.set(self.reads.get().wrapping_add(1));
            &self.meta
        }
    }
    struct CountedTypes<'a> {
        _lifetime: core::marker::PhantomData<&'a ()>,
    }
    impl<'a> RxBlockAckTypes for CountedTypes<'a> {
        type Slot = u8;
        type Running = (u8, u8);
        type Frame = Frame;
        type Caps = Caps;
    }
    let reads = Cell::new(0);
    let mut common = meta();
    common.aggregation = RxEvidence::HardwareObserved(Aggregation::Ampdu);
    let mut storage = RxBlockAckStorage::<CountedTypes<'_>, u8, 1, 8, 8>::new([0], GAP);
    let mut view = storage.view();
    let counted = Counted {
        meta: common,
        reads: &reads,
    };
    let bytes = qos_data(0, 100).0;
    let header = header_of(&bytes);
    let seen = RxView {
        head: &header,
        bytes: &bytes,
    };
    let _received = view.receive(
        now(1),
        None,
        qos_data(0, 100),
        seen,
        &counted,
        RetainRoom::Free,
        &mut (),
    );
    assert_eq!(reads.get(), 0);
    let mut events = Kept::default();
    let _received = view.receive(
        now(2),
        None,
        qos_data(0, 100),
        seen,
        &counted,
        RetainRoom::Free,
        &mut events,
    );
    assert_eq!(reads.get(), 1);
    assert!(matches!(
        events.0.as_slice(),
        [RxBlockAckEvent::Contained {
            aggregation: RxEvidence::HardwareObserved(Aggregation::Ampdu),
            bytes: Some(_),
            ..
        }]
    ));
    assert_eq!(
        core::mem::size_of::<RxBlockAckView<'_, Types, u8, 8>>(),
        core::mem::size_of::<(
            &mut Header<u8, 8>,
            &mut [Bank<Types, 8>],
            &mut [Option<Frame>]
        )>()
    );
}

#[test]
fn the_agreements_hold_frames_from_a_retained_mpdu_until_its_release_is_taken() {
    let mut storage = RxBlockAckStorage::<Types, u32, 2, 4, 4>::new([10, 11], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    assert!(!view.retained() != 0);
    view.offered(
        now(1),
        7,
        bssid(),
        addba(0, 0),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    let reply = view.next_reply().unwrap();
    view.sent(reply.place);
    // 101 waits in the window for the hole at 100.
    assert!(matches!(
        view.given(now(2), Some(LOCAL), qos_data(0, 101), meta(), &mut ()),
        RxDisposition::Retained
    ));
    assert!(view.retained() != 0 && !view.has_ready());
    // 100 releases both; they are held until taken.
    assert!(matches!(
        view.given(now(2), Some(LOCAL), qos_data(0, 100), meta(), &mut ()),
        RxDisposition::Retained
    ));
    assert!(view.retained() != 0);
    while view.pop_ready().is_some() {}
    assert!(!view.retained() != 0);
}

#[test]
fn an_in_order_mpdu_goes_out_at_once_unless_released_frames_wait() {
    let mut storage = RxBlockAckStorage::<Types, u32, 2, 4, 4>::new([10, 11], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        addba(0, 0),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    let reply = view.next_reply().unwrap();
    view.sent(reply.place);
    // The window's start, with nothing buffered behind it, is delivered
    // without taking a slot or a place in delivery order.
    for sequence in 100..103 {
        assert!(matches!(
            view.given(now(2), Some(LOCAL), qos_data(0, sequence), meta(), &mut ()),
            RxDisposition::Deliver(frame) if frame.0 == qos_data(0, sequence).0
        ));
    }
    assert!(!view.has_ready());
    // 104 waits for 103; 103 then releases both in order.
    assert!(matches!(
        view.given(now(2), Some(LOCAL), qos_data(0, 104), meta(), &mut ()),
        RxDisposition::Retained
    ));
    assert!(matches!(
        view.given(now(2), Some(LOCAL), qos_data(0, 103), meta(), &mut ()),
        RxDisposition::Retained
    ));
    // The next in-order MPDU waits behind the released ones.
    assert!(matches!(
        view.given(now(2), Some(LOCAL), qos_data(0, 105), meta(), &mut ()),
        RxDisposition::Retained
    ));
    let order: Vec<_> = core::iter::from_fn(|| view.pop_ready())
        .map(|frame| frame.0)
        .collect();
    assert_eq!(
        order,
        [qos_data(0, 103).0, qos_data(0, 104).0, qos_data(0, 105).0]
    );
    // A duplicate of a delivered MPDU is behind the window and recycled.
    assert!(matches!(
        view.given(now(2), Some(LOCAL), qos_data(0, 101), meta(), &mut ()),
        RxDisposition::Recycle(_)
    ));
}

/// An ADDBA Request for `tid` asking for `window`, zero included, which the
/// encoder refuses to write.
fn request_for(dialog_token: u8, tid: u8, window: u16) -> BlockAckAction {
    BlockAckAction::AddbaRequest {
        dialog_token: DialogToken::new(dialog_token),
        timeout_tu: 0,
        starting_sequence: SequenceNumber::new(100).unwrap(),
        parameters: oer_protocols_ieee80211_wire::block_ack::frame::AddbaParameters::new(
            oer_protocols_ieee80211_wire::block_ack::frame::BlockAckPolicy::Immediate,
            oer_protocols_ieee80211_wire::block_ack::frame::AmsduSupport::NotSupported,
            Tid::new(tid).unwrap(),
            window,
        )
        .unwrap(),
    }
}

/// The ADDBA records of `events`, in order.
fn requested(events: &[RxBlockAckEvent]) -> Vec<(Tid, u16, AddbaOutcome)> {
    events
        .iter()
        .filter_map(|event| match *event {
            RxBlockAckEvent::Requested {
                tid,
                requested,
                outcome,
            } => Some((tid, requested, outcome)),
            _ => None,
        })
        .collect()
}

/// Under the receive window `WINDOW`, with as many frames: each request
/// records what it asked and gets at most the window.
fn requests_get_their_window<const WINDOW: usize>() {
    let whole = u16::try_from(WINDOW).unwrap();
    for (asked, window) in [(0, whole), (8, 8), (64, whole)] {
        let mut events = Kept::default();
        let mut storage = RxBlockAckStorage::<Types, u32, 2, WINDOW, WINDOW>::new([10, 11], GAP);
        let mut view = storage.view();
        let mut port = Recording::default();
        view.offered(
            now(1),
            7,
            bssid(),
            request_for(5, 0, asked),
            RxAgreements::Admitted,
            &mut port,
            &mut events,
        )
        .unwrap();
        assert_eq!(
            requested(&events.0),
            [(
                Tid::constant::<0>(),
                asked,
                AddbaOutcome::Accepted { window }
            )],
            "a request for {asked}"
        );
        assert!(
            matches!(
                events.0.as_slice(),
                [
                    RxBlockAckEvent::Requested { .. },
                    RxBlockAckEvent::Started { tid: TID_0, window: started },
                ] if *started == window
            ),
            "the agreement starts at its addition's end, after the request: {:?}",
            events.0
        );
    }
}

#[test]
fn each_request_records_the_asked_buffer_size_and_the_window_it_got() {
    requests_get_their_window::<16>();
    requests_get_their_window::<32>();
}

#[test]
fn the_window_binds_below_the_frames() {
    // Thirty-two frames under a window of 16: a request for 64 gets 16, a
    // second TID the frames the first leaves.
    let mut events = Kept::default();
    let mut storage = RxBlockAckStorage::<Types, u32, 2, 32, 16>::new([10, 11], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    for (dialog, tid) in [(5, 0), (6, 1)] {
        view.offered(
            now(1),
            7,
            bssid(),
            request_for(dialog, tid, 64),
            RxAgreements::Admitted,
            &mut port,
            &mut events,
        )
        .unwrap();
    }
    assert_eq!(
        requested(&events.0),
        [
            (
                Tid::constant::<0>(),
                64,
                AddbaOutcome::Accepted { window: 16 }
            ),
            (
                Tid::constant::<1>(),
                64,
                AddbaOutcome::Accepted { window: 16 }
            ),
        ]
    );
}

#[test]
fn a_second_concurrent_tid_records_the_one_frame_left_for_it() {
    let mut events = Kept::default();
    let mut storage = RxBlockAckStorage::<Types, u32, 2, 16, 16>::new([10, 11], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    for (dialog, tid) in [(5, 0), (6, 1)] {
        view.offered(
            now(1),
            7,
            bssid(),
            request_for(dialog, tid, 64),
            RxAgreements::Admitted,
            &mut port,
            &mut events,
        )
        .unwrap();
    }
    assert_eq!(
        requested(&events.0),
        [
            (
                Tid::constant::<0>(),
                64,
                AddbaOutcome::Accepted { window: 16 }
            ),
            (
                Tid::constant::<1>(),
                64,
                AddbaOutcome::Accepted { window: 1 }
            ),
        ]
    );
    // The first agreement keeps its window: nothing ends it.
    assert!(
        !events
            .0
            .iter()
            .any(|event| matches!(event, RxBlockAckEvent::Stopped { .. }))
    );
}

#[test]
fn a_repeat_a_refusal_and_a_request_without_a_reply_place_each_record_their_outcome() {
    let mut events = Kept::default();
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    let mut offer = |view: &mut RxBlockAckView<'_, Types, u32, 8>, action, supported| {
        view.offered(
            now(1),
            7,
            bssid(),
            action,
            supported,
            &mut port,
            &mut events,
        )
    };
    offer(&mut view, request_for(5, 0, 8), RxAgreements::Admitted).unwrap();
    offer(&mut view, request_for(5, 0, 8), RxAgreements::Admitted).unwrap();
    // The same dialog with changed parameters, a TID no bank serves.
    offer(&mut view, request_for(5, 0, 4), RxAgreements::Admitted).unwrap();
    offer(&mut view, request_for(9, 9, 8), RxAgreements::Admitted).unwrap();
    for dialog in 0..PENDING - 2 {
        offer(
            &mut view,
            request_for(u8::try_from(dialog).unwrap(), 3, 8),
            RxAgreements::Declined,
        )
        .unwrap();
    }
    assert!(!view.can_receive_management());
    assert_eq!(
        offer(&mut view, request_for(20, 3, 0), RxAgreements::Declined),
        Err(RxBlockAckRefused::Capacity(RxBlockAckCapacity))
    );
    let records = requested(&events.0);
    assert_eq!(
        records[..4],
        [
            (
                Tid::constant::<0>(),
                8,
                AddbaOutcome::Accepted { window: 8 }
            ),
            (
                Tid::constant::<0>(),
                8,
                AddbaOutcome::Repeated { window: 8 }
            ),
            (
                Tid::constant::<0>(),
                4,
                AddbaOutcome::Refused(AddbaStatus::RequestDeclined)
            ),
            (
                Tid::constant::<9>(),
                8,
                AddbaOutcome::Refused(AddbaStatus::RequestDeclined)
            ),
        ]
    );
    assert_eq!(
        records.last(),
        Some(&(Tid::constant::<3>(), 0, AddbaOutcome::Unanswered))
    );
    assert_eq!(records.len(), 4 + PENDING - 2 + 1);
}

/// With only the hole's place free, a frame that would be retained goes
/// back, and the frame that closes the hole is still taken.
#[test]
fn a_full_room_gives_back_what_would_be_retained_and_takes_the_holes_frame() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        addba(0, 0),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    let full = |view: &mut RxBlockAckView<'_, Types, u32, 8>, sequence: u16| {
        let frame = qos_data(0, sequence);
        let bytes = frame.0.clone();
        let header = header_of(&bytes);
        let seen = RxView {
            head: &header,
            bytes: &bytes,
        };
        view.receive(
            now(2),
            Some(Eui48::new(LOCAL)),
            frame,
            seen,
            &meta(),
            RetainRoom::Full,
            &mut (),
        )
    };
    assert!(matches!(full(&mut view, 101), RxDisposition::Recycle(_)));
    assert_eq!(view.retained(), 0);
    assert_eq!(delivered(full(&mut view, 100)), Some(qos_data(0, 100).0));
}

/// The slots come back only when every bank is free: a running agreement
/// keeps them, its removal frees them.
#[test]
fn the_slots_come_back_once_every_bank_is_free() {
    let mut storage = RxBlockAckStorage::<Types, u32, 2, 8, 8>::new([10, 11], GAP);
    let mut port = Recording::default();
    storage
        .view()
        .offered(
            now(1),
            7,
            bssid(),
            addba(0, 0),
            RxAgreements::Admitted,
            &mut port,
            &mut (),
        )
        .unwrap();
    assert_eq!(storage.slots_home(), Err(BanksBusy), "an agreement runs");
    storage.view().cleared(&mut port, &mut ());
    assert!(storage.is_free());
    assert_eq!(storage.slots_home(), Ok([10, 11]));
}

/// An addition that expired in the port frees its bank and starts nothing.
#[test]
fn an_expired_addition_frees_its_bank() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8, 8>::new([10], GAP);
    let mut view = storage.view();
    view.offer(
        now(1),
        7,
        bssid(),
        addba(0, 0),
        RxAgreements::Admitted,
        &mut (),
        |_| {},
    )
    .unwrap();
    let Some(BankCommand::Add { ticket, slot, .. }) = view.next_command() else {
        panic!("the addition is owed");
    };
    assert!(
        view.next_command().is_none(),
        "one command at a time a bank"
    );
    view.emptied(ticket, slot);
    assert!(view.next_reply().is_none(), "nothing started: no response");
    assert!(storage.is_free());
}

/// Offer `action` to `view`, admitted, without carrying out any command.
fn asked<const WINDOW: usize>(
    view: &mut RxBlockAckView<'_, Types, u32, WINDOW>,
    action: BlockAckAction,
) {
    tried(view, action).unwrap();
}

/// Offer `action` to `view`, admitted: a request left unanswered is the
/// refusal of its capacity.
fn tried<const WINDOW: usize>(
    view: &mut RxBlockAckView<'_, Types, u32, WINDOW>,
    action: BlockAckAction,
) -> Result<(), RxBlockAckRefused> {
    view.offer(
        now(1),
        7,
        bssid(),
        action,
        RxAgreements::Admitted,
        &mut (),
        |_| {},
    )
}

/// The responses `view` owes, each taken as sent.
fn replies<const WINDOW: usize>(view: &mut RxBlockAckView<'_, Types, u32, WINDOW>) -> usize {
    let mut count = 0;
    while let Some(reply) = view.next_reply() {
        view.sent(reply.place);
        count += 1;
    }
    count
}

/// A request repeated while its TID's addition waits or runs is that
/// agreement's own: one bank, one addition, one response after it.
#[test]
fn a_request_repeated_during_its_addition_takes_no_second_bank() {
    let mut storage = RxBlockAckStorage::<Types, u32, 2, 16, 16>::new([10, 11], GAP);
    let mut view = storage.view();
    asked(&mut view, request_for(5, 0, 8));
    // While the addition waits for its command.
    asked(&mut view, request_for(6, 0, 8));
    let Some(BankCommand::Add { ticket, slot, .. }) = view.next_command() else {
        panic!("the addition is owed");
    };
    // While the addition runs in the port.
    asked(&mut view, request_for(7, 0, 8));
    assert!(view.next_command().is_none(), "no second bank for TID 0");
    assert_eq!(replies(&mut view), 0, "the response follows the addition");
    view.added(ticket, (slot, 0), &mut ());
    assert_eq!(replies(&mut view), 1, "one response, after the addition");
    assert!(view.next_command().is_none());
}

/// A request for a TID whose agreement ends goes unanswered and takes no
/// bank: the peer asks again once the bank is free.
#[test]
fn a_request_during_its_tids_end_is_unanswered() {
    let mut storage = RxBlockAckStorage::<Types, u32, 2, 16, 16>::new([10, 11], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        request_for(5, 0, 8),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    assert_eq!(replies(&mut view), 1);
    // Asked anew with other parameters: the running agreement ends.
    assert_eq!(
        tried(&mut view, request_for(6, 0, 4)),
        Err(RxBlockAckRefused::Capacity(RxBlockAckCapacity))
    );
    // While its end waits for its command.
    assert_eq!(
        tried(&mut view, request_for(7, 0, 4)),
        Err(RxBlockAckRefused::Capacity(RxBlockAckCapacity))
    );
    let Some(BankCommand::End { ticket, running }) = view.next_command() else {
        panic!("the end is owed");
    };
    // While its end runs in the port.
    assert_eq!(
        tried(&mut view, request_for(8, 0, 4)),
        Err(RxBlockAckRefused::Capacity(RxBlockAckCapacity))
    );
    assert!(view.next_command().is_none(), "no bank for an ending TID");
    view.emptied(ticket, running.0);
    assert_eq!(replies(&mut view), 0, "unanswered while it ended");
    assert!(storage.is_free());
}

/// With every bank busy and one freeing, a request the station declines is
/// still answered: only an admitted one waits for the bank.
#[test]
fn a_declined_request_is_answered_while_a_bank_frees() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8, 8>::new([10], GAP);
    let mut view = storage.view();
    asked(&mut view, request_for(5, 0, 8));
    view.offer(
        now(1),
        7,
        bssid(),
        request_for(6, 1, 8),
        RxAgreements::Declined,
        &mut (),
        |_| {},
    )
    .unwrap();
    assert_eq!(replies(&mut view), 1, "the decline is queued");
}

/// An end that could not begin is owed again, to the same bank.
#[test]
fn an_end_owed_again_is_given_again() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offered(
        now(1),
        7,
        bssid(),
        request_for(5, 0, 8),
        RxAgreements::Admitted,
        &mut port,
        &mut (),
    )
    .unwrap();
    assert_eq!(
        tried(&mut view, request_for(6, 0, 4)),
        Err(RxBlockAckRefused::Capacity(RxBlockAckCapacity))
    );
    let Some(BankCommand::End { ticket, running }) = view.next_command() else {
        panic!("the end is owed");
    };
    view.owe_end(ticket, running);
    let Some(BankCommand::End { ticket, running }) = view.next_command() else {
        panic!("the end is owed again");
    };
    view.emptied(ticket, running.0);
    assert!(storage.is_free());
}

/// The slots that went home come back, and the banks serve again.
#[test]
fn the_slots_back_serve_again() {
    let mut storage = RxBlockAckStorage::<Types, u32, 2, 8, 8>::new([10, 11], GAP);
    let slots = storage.slots_home().unwrap();
    assert!(!storage.is_free(), "away, the banks serve nothing");
    storage.slots_back(slots);
    assert!(storage.is_free());
    let mut view = storage.view();
    asked(&mut view, request_for(5, 0, 8));
    assert!(matches!(
        view.next_command(),
        Some(BankCommand::Add { slot: 10, .. })
    ));
}

#[test]
fn a_window_is_refused_exactly_at_its_bounds() {
    let capability = "the receive window is 1 to the capabilities' rx_block_ack_max_window";
    let frames = "the retained frames hold a whole window: WINDOW <= FRAMES";
    assert_eq!(window_refusal(16, 16, 64), None);
    assert_eq!(window_refusal(64, 64, 64), None);
    assert_eq!(window_refusal(1, 1, 1), None);
    assert_eq!(window_refusal(16, 32, 64), None);
    assert_eq!(window_refusal(0, 16, 64), Some(capability));
    assert_eq!(window_refusal(65, 65, 64), Some(capability));
    assert_eq!(window_refusal(16, 15, 64), Some(frames));
    assert_eq!(
        window_refusal(256, 256, u16::MAX),
        Some("the in-order releases are counted in a byte")
    );
    assert_eq!(window_refusal(255, 255, u16::MAX), None);
}
