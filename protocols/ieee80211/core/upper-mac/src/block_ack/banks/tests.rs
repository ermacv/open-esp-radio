//! The owner decides each agreement edge and hands it to its port; the port
//! here records what the integration would carry out.

extern crate std;

use super::*;
use oer_ieee80211_contracts_port::{
    BandSet, Channel, ChannelWidth, HardwareServices, LowerMacCapabilities, PhyFormatSet,
    RateSupport, ReceiveFilter, RxMeta, WidthSet,
};
use oer_ieee80211_wire::phy::{HeMcs, HtMcs};
use oer_ieee80211_wire::{
    address::IndividualAddress,
    block_ack::frame::{encode_addba_request, encode_block_ack_request, parse_block_ack_action},
    frame::{FrameControl, MacHeader},
    status::status_code,
};
use std::vec::Vec;

const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const AP: [u8; 6] = [2, 0, 0, 0, 0, 2];
const GAP: Duration = Duration::from_micros(10_000);

struct Caps;

impl Ieee80211Capabilities for Caps {
    type SppScope = oer_ieee80211_wire::security::spp::Association;
    const CAPS: LowerMacCapabilities = LowerMacCapabilities {
        bands: BandSet::GHZ2_4,
        widths: WidthSet::MHZ20,
        rates: RateSupport {
            dsss_cck: true,
            ofdm: true,
            ht_max_mcs: HtMcs::new(7),
            he_max_mcs: HeMcs::new(9),
            he_max_bandwidth_mhz: 20,
            he_dcm: false,
            he_ldpc: false,
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
        rx_block_ack_max_tid: 7,
        rx_block_ack_max_window: 64,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
        edca_max_txop_limit_units_32_us: u16::MAX,
    };
}

struct Frame(Vec<u8>);

impl RxBuffer for Frame {
    fn bytes(&self) -> &[u8] {
        &self.0
    }
}

struct Types;

impl RxBlockAckTypes for Types {
    type Slot = u8;
    type Running = (u8, u8);
    type Frame = Frame;
    type Meta = RxMeta;
}

#[derive(Debug, Eq, PartialEq)]
enum Call {
    Add { slot: u8, tid: u8, window: u16 },
    Remove(u8),
    Recycle(Vec<u8>),
}

#[derive(Default)]
struct Recording(Vec<Call>);

impl RxBlockAckPort<Types> for Recording {
    type Caps = Caps;
    fn add(&mut self, slot: u8, agreement: SupportedBlockAck<Caps>) -> (u8, u8) {
        self.0.push(Call::Add {
            slot,
            tid: agreement.get().1.get(),
            window: agreement.get().3,
        });
        (slot, agreement.get().1.get())
    }

    fn remove(&mut self, running: (u8, u8)) -> u8 {
        self.0.push(Call::Remove(running.0));
        running.0
    }

    fn recycle(&mut self, frame: Frame) {
        self.0.push(Call::Recycle(frame.0));
    }
}

fn bssid() -> Bssid {
    Bssid::new(IndividualAddress::new(AP).unwrap()).unwrap()
}

fn addba(tid: u8, timeout_tu: u16) -> BlockAckAction {
    let body = encode_addba_request(
        5,
        tid,
        8,
        timeout_tu,
        false,
        SequenceNumber::new(100).unwrap(),
    )
    .unwrap();
    parse_block_ack_action(&body).unwrap()
}

fn qos_data(tid: u8, sequence: u16) -> Frame {
    let control = FrameControl::new(0x0288).unwrap();
    let qos = QosControl::from_bytes([tid, 0]);
    let header = MacHeader::new(
        control,
        0,
        LOCAL,
        AP,
        AP,
        SequenceNumber::new(sequence).unwrap().sequence_control(),
        None,
        Some(qos),
        None,
    )
    .unwrap();
    let mut bytes = std::vec![0; 40];
    let length = header.encode(&mut bytes).unwrap();
    bytes.truncate(length);
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
        RxDisposition::Deliver((frame, _)) => Some(frame.0),
        _ => None,
    }
}

#[test]
fn an_admitted_request_installs_one_agreement_and_owes_its_success_response() {
    let mut storage = RxBlockAckStorage::<Types, u32, 2, 8>::new([10, 11], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offer(now(1), 7, bssid(), addba(3, 0), true, &mut port, &mut ())
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
        BlockAckAction::AddbaResponse { status, tid, .. } => {
            assert_eq!((status, tid), (status_code::SUCCESS, 3));
        }
        other => panic!("expected an ADDBA Response, got {other:?}"),
    }
    view.sent(reply.place);
    assert!(view.next_reply().is_none());
}

#[test]
fn a_station_without_agreements_declines_through_the_reply_queue() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offer(now(1), 7, bssid(), addba(3, 0), false, &mut port, &mut ())
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
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offer(now(1), 7, bssid(), addba(0, 0), true, &mut port, &mut ())
        .unwrap();
    assert!(matches!(
        view.receive(now(2), Some(LOCAL), qos_data(0, 101), meta(), &mut ()),
        RxDisposition::Retained
    ));
    assert!(view.pop_ready().is_none());
    assert!(view.deadline().is_some());
    assert!(matches!(
        view.receive(now(3), Some(LOCAL), qos_data(0, 100), meta(), &mut ()),
        RxDisposition::Retained
    ));
    let order: Vec<_> = core::iter::from_fn(|| view.pop_ready())
        .map(|(frame, _)| frame.0)
        .collect();
    assert_eq!(order, [qos_data(0, 100).0, qos_data(0, 101).0]);
    // Before a link exists nothing is reordered.
    assert_eq!(
        delivered(view.receive(now(4), None, qos_data(0, 103), meta(), &mut ())),
        Some(qos_data(0, 103).0)
    );
}

#[test]
fn a_forward_block_ack_request_releases_the_hole_and_is_recycled() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offer(now(1), 7, bssid(), addba(0, 0), true, &mut port, &mut ())
        .unwrap();
    assert!(matches!(
        view.receive(now(2), Some(LOCAL), qos_data(0, 101), meta(), &mut ()),
        RxDisposition::Retained
    ));
    let bar = encode_block_ack_request(LOCAL, AP, 0, SequenceNumber::new(102).unwrap());
    assert!(matches!(
        view.receive(now(3), Some(LOCAL), Frame(bar.to_vec()), meta(), &mut ()),
        RxDisposition::Recycle(_)
    ));
    assert_eq!(view.pop_ready().unwrap().0.0, qos_data(0, 101).0);
}

#[test]
fn inactivity_ends_the_agreement_with_a_delba_and_returns_its_slot() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offer(now(1), 7, bssid(), addba(2, 1), true, &mut port, &mut ())
        .unwrap();
    let response = view.next_reply().unwrap();
    view.sent(response.place);
    view.tick(now(1 + 1024), &mut port, &mut ());
    assert_eq!(port.0.last(), Some(&Call::Remove(10)));
    let reply = view.next_reply().unwrap();
    assert_eq!(reply.place, ReplyPlace::Queued);
    assert!(matches!(
        parse_block_ack_action(&reply.body),
        Some(BlockAckAction::Delba {
            tid: 2,
            initiator: false,
            ..
        })
    ));
}

#[test]
fn a_new_association_returns_every_bank_and_retained_frame_first() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offer(now(1), 7, bssid(), addba(0, 0), true, &mut port, &mut ())
        .unwrap();
    assert!(matches!(
        view.receive(now(2), Some(LOCAL), qos_data(0, 101), meta(), &mut ()),
        RxDisposition::Retained
    ));
    view.offer(now(3), 8, bssid(), addba(0, 0), true, &mut port, &mut ())
        .unwrap();
    assert_eq!(
        port.0,
        [
            Call::Add {
                slot: 10,
                tid: 0,
                window: 8
            },
            Call::Remove(10),
            Call::Recycle(qos_data(0, 101).0),
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
fn an_action_waits_retained_while_no_reply_place_is_free() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    for _ in 0..PENDING {
        view.offer(now(1), 7, bssid(), addba(3, 0), false, &mut port, &mut ())
            .unwrap();
    }
    assert!(!view.can_receive_management());
    let mut action = std::vec![0xd0, 0, 0, 0];
    action.extend_from_slice(&LOCAL);
    action.extend_from_slice(&AP);
    action.extend_from_slice(&AP);
    action.extend_from_slice(&[0, 0, 3, 0]);
    assert!(matches!(
        view.receive(now(2), Some(LOCAL), Frame(action.clone()), meta(), &mut ()),
        RxDisposition::Retained
    ));
    assert!(view.pop_deferred().is_none());
    let reply = view.next_reply().unwrap();
    view.sent(reply.place);
    assert_eq!(view.pop_deferred().unwrap().0.0, action);
}

#[test]
fn an_external_reply_place_never_panics_or_changes_another_bank() {
    let mut storage = RxBlockAckStorage::<Types, u32, 1, 8>::new([10], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offer(now(1), 7, bssid(), addba(0, 0), true, &mut port, &mut ())
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
    let mut storage = RxBlockAckStorage::<Types, u8, 1, 8>::new([0], GAP);
    let mut view = storage.view();
    let mut port = Recording::default();
    view.offer(
        now(1),
        7,
        bssid(),
        addba(0, 0),
        true,
        &mut port,
        &mut events,
    )
    .unwrap();
    assert!(matches!(
        view.receive(now(2), Some(LOCAL), qos_data(0, 101), meta(), &mut events),
        RxDisposition::Retained
    ));
    assert!(matches!(
        view.receive(now(3), Some(LOCAL), qos_data(0, 101), meta(), &mut events),
        RxDisposition::Recycle(_)
    ));
    view.tick(now(20_000), &mut port, &mut events);
    assert!(view.pop_ready().is_some());
    view.clear(&mut port, &mut events);
    let events = events.0;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RxBlockAckEvent::Started { tid: 0, window: 8 }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                RxBlockAckEvent::Stopped {
                    tid: 0,
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
        type Meta = Counted<'a>;
    }
    let reads = Cell::new(0);
    let mut common = meta();
    common.ampdu = RxEvidence::HardwareObserved(true);
    let mut storage = RxBlockAckStorage::<CountedTypes<'_>, u8, 1, 8>::new([0], GAP);
    let mut view = storage.view();
    let counted = Counted {
        meta: common,
        reads: &reads,
    };
    let _received = view.receive(now(1), None, qos_data(0, 100), counted, &mut ());
    assert_eq!(reads.get(), 0);
    let mut events = Kept::default();
    let _received = view.receive(now(2), None, qos_data(0, 100), counted, &mut events);
    assert_eq!(reads.get(), 1);
    assert!(matches!(
        events.0.as_slice(),
        [RxBlockAckEvent::Contained {
            ampdu: RxEvidence::HardwareObserved(true),
            bytes: Some(_),
            ..
        }]
    ));
    assert_eq!(
        core::mem::size_of::<RxBlockAckView<'_, Types, u8>>(),
        core::mem::size_of::<(
            &mut Header<u8>,
            &mut [Bank<Types>],
            &mut [Option<Retained<Types>>]
        )>()
    );
}
