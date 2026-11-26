//! The plane over the model port.

use super::*;
use crate::cost::{Response, transmission_micros};
use crate::model::{Installed, SINGLES, installed};
use core::num::{NonZeroU8, NonZeroU16};
use oer_portable_time::Duration;
use oer_protocols_ieee802_wire::{Eui48, IndividualAddress};
use oer_protocols_ieee80211_contracts_port::{
    AmpduRate, PortRate, Protection, RetryPlan, RetryStage, RxMeta,
};
use oer_protocols_ieee80211_core_upper_mac::ccmp::{
    CcmpPacketNumberStep, InstalledCcmpTxKey, SoftwareCcmpKey,
};
use oer_protocols_ieee80211_core_upper_mac::sequence::SequenceCounter;
use oer_protocols_ieee80211_testing_model::StationModel;
use oer_protocols_ieee80211_testing_model::{
    Air, MODEL_MAX_MPDU, ModelCaps, ModelLink, ModelMemory, ModelPort, Went,
};
use oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber;
use oer_protocols_ieee80211_wire::{
    address::{Bssid, OwnAddress},
    frame::{FrameControlFields, sequence_control},
    phy::{HtMcs, HtRate, LegacyRate, PpduBandwidth},
};
use std::vec::Vec;

const STATION: [u8; 6] = [2, 0, 0, 0, 0, 1];
const ACCESS_POINT: [u8; 6] = [2, 0, 0, 0, 0, 2];
const DESTINATION: [u8; 6] = [2, 0, 0, 0, 0, 0x10];
const PAYLOAD: usize = 100;
const BE: WmmUserPriority = WmmUserPriority::UP0;
const VIDEO: WmmUserPriority = WmmUserPriority::UP5;
const BEST_EFFORT: oer_protocols_ieee80211_wire::qos::WmmAccessCategory =
    oer_protocols_ieee80211_wire::qos::WmmAccessCategory::BestEffort;

/// A QoS data front without CCMP: the QoS header and LLC/SNAP.
const OPEN_FRONT: usize = IEEE80211_QOS_DATA_HEADER_LEN + LLC_SNAP_HEADER_LEN;

/// HT MCS 7 on 20 MHz.
fn ht() -> PhyRate {
    PhyRate::Ht(
        HtRate::new(
            HtMcs::new(7).unwrap(),
            PpduBandwidth::Mhz20,
            oer_protocols_ieee80211_wire::phy::HtGuardInterval::Normal,
        )
        .unwrap(),
    )
}

fn link(protection: LinkProtection) -> DataLink {
    DataLink {
        header: StaDataHeaderTemplate::new(
            OwnAddress::new(IndividualAddress::new(Eui48::new(STATION)).unwrap()),
            Bssid::new(IndividualAddress::new(Eui48::new(ACCESS_POINT)).unwrap()).unwrap(),
            PeerQos::Qos,
            protection,
        )
        .unwrap(),
        admission: AcAdmission::default(),
        retry: Some(AggregateRetry {
            lifetime: Duration::from_millis(100),
            attempts: 3,
        }),
        ht: Some(HtLimits {
            spacing: oer_protocols_ieee80211_wire::ht::MinimumMpduStartSpacing::NoRestriction,
            max_length: 65_535,
        }),
        windows: [None; 8],
        protection: Default::default(),
    }
}

/// Key material of a test key, which software alone owns.
struct TestKey;

impl SoftwareCcmpKey for TestKey {}

/// An MSDU named `tag`: its Ethernet header at the end of its memory's
/// headroom, its payload the body.
fn msdu(tag: u8, priority: WmmUserPriority) -> OutboundMsdu<ModelMemory> {
    let mut front = [0; HEADROOM];
    let ethernet = &mut front[crate::MSDU_ETHERNET_OFFSET..];
    ethernet[..6].copy_from_slice(&DESTINATION);
    ethernet[6..12].copy_from_slice(&STATION);
    ethernet[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    OutboundMsdu {
        memory: ModelMemory {
            id: u32::from(tag),
            len: PAYLOAD as u16,
            front,
        },
        length: ETHERNET_HEADER_LEN + PAYLOAD,
        priority,
    }
}

fn sequence(frame: &[u8]) -> u16 {
    SequenceNumber::from_sequence_control(sequence_control(frame).unwrap()).get()
}

fn retried(frame: &[u8]) -> bool {
    FrameControlFields::from_bytes(*frame.first_chunk::<2>().unwrap()).retry()
}

fn positions(positions: &[u8]) -> FrameSet {
    FrameSet::from_bits(positions.iter().fold(0, |bits, at| bits | 1 << at))
}

/// A single MPDU's plan of one transmission at `rate`.
fn once<R>(rate: R) -> RetryPlan<R> {
    RetryPlan::new(RetryStage {
        rate,
        transmissions: NonZeroU8::MIN,
        protection: Protection::None,
    })
}

/// The plane over a running model installation.
struct World<'id> {
    port: ModelPort<'id, StationModel<SINGLES>>,
    gate: GateOpen<'id>,
    plane: DataPlane<'id, ModelCaps>,
    single: PortRate<'id, ModelCaps>,
    aggregate: AmpduRate<'id, ModelCaps>,
    /// The link's QoS sequence spaces, which each intake draws from.
    sequences: [SequenceCounter; 8],
    /// The station's pairwise key and its packet numbers.
    key: Option<InstalledCcmpTxKey<TestKey>>,
    returned: Vec<ModelMemory>,
    /// The cost of each attempt that settled, in order.
    costs: Vec<TxEstimate>,
    micros: u64,
}

impl<'id> World<'id> {
    fn new(model: Installed<'id>, link: DataLink) -> Self {
        Self::at(model, link, ht())
    }

    /// The plane whose aggregates go at `rate`, its singles at 24 Mb/s.
    fn at(model: Installed<'id>, link: DataLink, rate: PhyRate) -> Self {
        let Installed {
            port, arenas, gate, ..
        } = model;
        let mut plane = DataPlane::new(arenas);
        plane.link(link);
        plane.aggregate_rate(Some(AggregateRate {
            rate,
            max_subframes: 8,
            max_length: 65_535,
        }));
        let single = port.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap();
        let aggregate = port.ampdu_rate(port.rate(rate).unwrap()).unwrap();
        Self {
            port,
            gate,
            plane,
            single,
            aggregate,
            sequences: [SequenceCounter::new(SequenceNumber::ZERO); 8],
            key: None,
            returned: Vec::new(),
            costs: Vec::new(),
            micros: 1,
        }
    }

    fn now(&self) -> Instant {
        Instant::from_micros(self.micros)
    }

    fn agree(&mut self, tid: WmmUserPriority, size: u16) {
        let head = tid.select(&self.sequences).peek();
        self.plane.agreement(tid, Some(TxWindow { head, size }));
    }

    fn take(&mut self, tag: u8, tid: WmmUserPriority) -> Intake<ModelMemory> {
        self.intake(msdu(tag, tid))
    }

    /// Offer `msdu` with the link's numbers.
    fn intake(&mut self, msdu: OutboundMsdu<ModelMemory>) -> Intake<ModelMemory> {
        let now = self.now();
        let mut numbers = TxNumbers::new(
            SequenceCounter::new(SequenceNumber::ZERO),
            Some(self.sequences),
        );
        let link = ModelLink {
            protected: self
                .plane
                .current()
                .is_some_and(|link| link.header.protection() == LinkProtection::Ccmp),
        };
        let intake = self.plane.intake(
            &mut self.port,
            &link,
            msdu,
            &mut numbers,
            self.key.as_mut(),
            now,
        );
        let (_, sequences) = numbers.into_parts();
        self.sequences = sequences.unwrap();
        intake
    }

    fn taken(&mut self, tag: u8, tid: WmmUserPriority) {
        assert_eq!(self.take(tag, tid), Intake::Taken(tid));
    }

    /// The PPDUs a plan goes as: a single's and an aggregate's (the plane
    /// takes the spacing from its plan).
    fn ppdus(
        single: PortRate<'id, ModelCaps>,
        aggregate: AmpduRate<'id, ModelCaps>,
    ) -> [Ppdu<'id, ModelCaps>; 2] {
        [
            Ppdu::Single(once(single)),
            Ppdu::Aggregate {
                rate: aggregate,
                spacing: oer_protocols_ieee80211_wire::ht::MinimumMpduStartSpacing::NoRestriction,
                protection: Protection::None,
                alone: once(single),
            },
        ]
    }

    /// The PPDU of `plan`, lent.
    fn how(
        ppdus: &[Ppdu<'id, ModelCaps>; 2],
    ) -> impl FnOnce(PpduPlan) -> Option<Ppdu<'id, ModelCaps>> {
        let [single, aggregate] = *ppdus;
        move |plan| {
            Some(match plan.kind {
                PpduKind::Single => single,
                PpduKind::Aggregate { .. } => aggregate,
            })
        }
    }

    /// Publish the next arena; its plan and the fronts the model sent.
    fn publish(&mut self) -> (PpduPlan, Vec<Vec<u8>>) {
        let now = self.now();
        let ppdus = Self::ppdus(self.single, self.aggregate);
        let how = Self::how(&ppdus);
        let plan = self
            .plane
            .publication(&mut self.port, &self.gate, now, how)
            .unwrap();
        let frames = self.port.published().last().unwrap().frames.clone();
        (plan, frames)
    }

    /// The next aggregate's BlockAck acknowledges the frames of `acked`.
    fn block_ack(&mut self, acked: FrameSet) {
        self.port.set_air(BEST_EFFORT, Air::Ack);
        self.port.acknowledge(acked);
    }

    fn answer(&mut self, air: Air) {
        self.port.set_air(BEST_EFFORT, air);
    }

    fn settle_with(&mut self, events: &mut impl EventSink<DataEvent>) -> Settled<(), (), ()> {
        let now = self.now();
        let ppdus = Self::ppdus(self.single, self.aggregate);
        let how = Self::how(&ppdus);
        let returned = &mut self.returned;
        let settled = self
            .plane
            .completed(
                &mut self.port,
                TxFacts::DUE,
                Some(&self.gate),
                now,
                how,
                |memory| returned.push(memory),
                events,
            )
            .unwrap();
        self.costs.push(settled.cost);
        // Each test answers its next publication anew.
        self.port.set_air(BEST_EFFORT, Air::Ack);
        settled
    }

    fn settle(&mut self) -> Settled<(), (), ()> {
        self.settle_with(&mut ())
    }

    /// The holes of the aggregate that ended, as its settlement reports them.
    fn settled_holes(&mut self) -> Option<u64> {
        let mut holes = Holes(Some(None));
        self.settle_with(&mut holes);
        holes.0.flatten()
    }

    fn returned_sequences(&mut self) -> Vec<u16> {
        let port = &self.port;
        self.returned
            .drain(..)
            .map(|memory| sequence(port.front(memory.id).unwrap()))
            .collect()
    }
}

/// The holes of the settlement it saw.
struct Holes(Option<Option<u64>>);

impl EventSink<DataEvent> for Holes {
    const ENABLED: bool = true;
    fn event(&mut self, event: DataEvent) {
        if let DataEvent::AggregateSettled(settled) = event {
            self.0 = Some(settled.holes);
        }
    }
}

macro_rules! cases {
    ($($name:ident => $body:ident),* $(,)?) => {$(
        #[test]
        fn $name() {
            installed(|model| $body(model));
        }
    )*};
}

cases! {
    a_full_block_ack_releases_every_frame_in_order_and_moves_the_window => full_block_ack,
    a_partial_block_ack_retries_the_missing_frames_in_the_same_arena => partial_block_ack,
    only_an_answered_first_transmission_reports_its_holes => holes_of_first_transmissions,
    frames_past_the_attempt_limit_are_dropped_and_owe_a_block_ack_request => attempt_limit,
    aged_frames_are_dropped_and_owe_a_block_ack_request => lifetime,
    a_sequence_number_past_the_window_waits => past_window,
    another_tids_standby_makes_the_msdu_wait => other_tid,
    a_protected_link_numbers_each_frame_and_refuses_without_key_or_link => key_and_link,
    handover_and_stop_keep_the_sequence_numbers => round_trip,
    the_stations_own_frames_share_the_sequence_spaces => shared_spaces,
    numbers_of_the_other_qos_kind_are_refused => numbering,
    a_key_whose_numbers_are_spent_refuses_the_msdu_and_writes_nothing => spent_key,
    withdrawing_unpublished_frames_keeps_the_link_and_the_arena_on_the_air => withdraw_unpublished,
    a_tid_without_agreement_goes_one_mpdu_at_a_time => single,
    a_lone_frame_under_an_agreement_goes_alone_and_owes_a_block_ack_request_when_dropped => lone_frame,
    an_ended_agreement_drains_its_arenas_as_single_mpdus_without_drops => agreement_end,
    the_standby_room_counts_down_to_the_tids_bound_while_an_arena_is_on_the_air => standby_room,
    a_whole_block_ack_puts_the_standby_on_the_air_before_the_settlement => ahead_whole,
    a_partial_block_ack_leaves_the_order_to_the_settlement => ahead_partial,
    nothing_goes_ahead_of_a_settlement_without_frames_in_the_other_arena => ahead_empty,
    each_attempt_is_charged_once_whole_for_its_tid => cost_split,
    a_single_mpdu_s_attempt_is_charged_and_a_withdrawn_frame_charges_nothing => cost_single,
    an_attempt_whose_end_is_unknown_is_charged_unknown => cost_unknown,
    a_prefix_the_rate_admits_goes_and_the_rest_stays_with_no_attempt_spent => prefix,
    an_aggregate_that_reached_no_receiver_spends_no_attempt_of_its_frames => unreached,
}

/// The PSDU of an open QoS data MPDU of [`PAYLOAD`] octets: its front, its
/// payload and its FCS.
const PSDU: u32 = (OPEN_FRONT + PAYLOAD + FCS_LEN) as u32;

/// What `frames` such MPDUs take in an aggregate.
const fn aggregate_octets(frames: u32) -> u32 {
    frames * subframe(PSDU as u16)
}

fn cost_split(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..3 {
        world.taken(tag, BE);
    }
    world.publish();
    // Frames 0 and 2 are acknowledged; 1 goes again alone in the arena.
    world.block_ack(positions(&[0, 2]));
    let settled = world.settle();
    assert_eq!(world.returned_sequences(), [0, 2]);
    // The whole attempt, once, for its TID: its PPDU and BlockAck.
    let whole = transmission_micros(ht(), aggregate_octets(3), Response::BlockAck).unwrap();
    assert_eq!(
        (settled.tid, settled.cost),
        (
            BE,
            TxEstimate::Estimated(oer_portable_time::Duration::from_micros(u64::from(whole)))
        )
    );
    world.taken(3, BE);
    let (retry, _) = world.publish();
    assert_eq!(retry.frames, 1);
    let settled = world.settle();
    assert_eq!(world.returned_sequences(), [1]);
    let alone = transmission_micros(world.single.get(), PSDU, Response::Ack).unwrap();
    assert_eq!(
        settled.cost,
        TxEstimate::Estimated(oer_portable_time::Duration::from_micros(u64::from(alone))),
        "the retry's own attempt"
    );
    assert_eq!(world.costs.len(), 2);
}

fn cost_single(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.taken(0, BE);
    world.publish();
    world.settle();
    assert_eq!(world.returned_sequences(), [0]);
    let once = transmission_micros(world.single.get(), PSDU, Response::Ack).unwrap();
    assert_eq!(
        world.costs,
        [TxEstimate::Estimated(
            oer_portable_time::Duration::from_micros(u64::from(once))
        )]
    );
    world.costs.clear();
    // Withdrawn frames that never went leave with no attempt to charge.
    world.taken(1, BE);
    let returned = &mut world.returned;
    world
        .plane
        .withdraw_unpublished(&mut world.port, |memory| returned.push(memory), &mut ());
    assert_eq!(world.returned_sequences(), [1]);
    assert!(world.costs.is_empty());
}

fn cost_unknown(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..2 {
        world.taken(tag, BE);
    }
    world.publish();
    world.answer(Air::Timeout);
    world.settle();
    // An attempt whose end is not known is charged unknown, never nothing;
    // its frames end with it.
    assert_eq!(world.costs, [TxEstimate::Unknown]);
    assert_eq!(world.returned_sequences(), [0, 1]);
    for tag in 2..4 {
        world.taken(tag, BE);
    }
    world.publish();
    world.settle();
    assert_eq!(world.returned_sequences(), [2, 3]);
    let whole = transmission_micros(ht(), aggregate_octets(2), Response::BlockAck).unwrap();
    assert_eq!(
        world.costs,
        [
            TxEstimate::Unknown,
            TxEstimate::Estimated(oer_portable_time::Duration::from_micros(u64::from(whole)))
        ]
    );
}

fn standby_room(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    let capacity = 32;
    // Nothing on the air: the next frame goes out at once.
    assert_eq!(world.plane.standby_room(capacity), None);
    for tag in 0..2 {
        world.taken(tag, BE);
    }
    assert_eq!(world.plane.standby_room(capacity), None);
    world.publish();
    // On the air with an empty standby: the whole arena.
    assert_eq!(world.plane.standby_room(capacity), Some(capacity));
    // The standby holds a BE frame: BE's bound of 8 subframes counts down.
    for tag in 2..5 {
        world.taken(tag, BE);
    }
    assert_eq!(world.plane.standby_room(capacity), Some(5));
    for tag in 5..10 {
        world.taken(tag, BE);
    }
    assert_eq!(world.plane.standby_room(capacity), Some(0));
    assert!(matches!(world.take(10, BE), Intake::Wait(_, _)));
    // The arena's own capacity bounds a wider TID bound.
    assert_eq!(world.plane.standby_room(4), Some(0));
    // Their whole BlockAck puts the standby on the air ahead of the
    // settlement: the emptied arena is the standby now, and empty.
    world.block_ack(FrameSet::prefix(2));
    assert!(world.settle().ahead.is_some());
    assert_eq!(world.plane.standby_room(capacity), Some(capacity));
}

fn full_block_ack(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..4 {
        world.taken(tag, BE);
    }
    let (plan, frames) = world.publish();
    assert_eq!(plan.frames, 4);
    assert!(matches!(plan.kind, PpduKind::Aggregate { .. }));
    assert_eq!(
        frames.iter().map(|f| sequence(f)).collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    // The front is the QoS header and LLC/SNAP, flush against the body.
    assert_eq!(frames[2].len(), OPEN_FRONT);
    assert_eq!(frames[2][OPEN_FRONT - 2..], 0x0800_u16.to_be_bytes());
    world.block_ack(FrameSet::prefix(4));
    let settled = world.settle();
    assert_eq!(
        (settled.delivered, settled.dropped, settled.retried),
        (4, 0, 0)
    );
    assert_eq!(settled.bar, None);
    let Some(RateLesson::Aggregate { tally, outcome }) = settled.lesson else {
        panic!("an A-MPDU teaches its tally");
    };
    assert_eq!(
        tally,
        BlockAckTally::new(NonZeroU16::new(4).unwrap(), 4).unwrap()
    );
    assert!(matches!(
        outcome,
        TxOutcome::Ended(AggregateEnd::Delivered { .. })
    ));
    assert_eq!(world.returned_sequences(), [0, 1, 2, 3]);
    assert_eq!(world.plane.window(BE).unwrap().head.get(), 4);
    assert_eq!(world.plane.publishable(), None);
}

fn ahead_whole(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..2 {
        world.taken(tag, BE);
    }
    let (first, _) = world.publish();
    for tag in 2..4 {
        world.taken(tag, BE);
    }
    world.block_ack(FrameSet::prefix(2));
    let settled = world.settle();
    let ahead = settled.ahead.expect("the standby goes first");
    assert_eq!(ahead.arena, other(first.arena));
    assert_eq!(ahead.frames, 2);
    assert!(world.plane.on_air());
    assert_eq!(world.returned_sequences(), [0, 1]);
    assert_eq!(world.plane.window(BE).unwrap().head.get(), 2);
    // The settled arena takes the next frame while the standby is on the air,
    // and goes ahead of the standby's settlement in its turn.
    world.taken(4, BE);
    world.block_ack(FrameSet::prefix(2));
    let settled = world.settle();
    assert_eq!(settled.delivered, 2);
    assert_eq!(world.returned_sequences(), [2, 3]);
    assert_eq!(settled.ahead.map(|plan| plan.arena), Some(first.arena));
}

fn ahead_partial(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..2 {
        world.taken(tag, BE);
    }
    let (first, _) = world.publish();
    world.taken(2, BE);
    world.taken(3, BE);
    world.block_ack(positions(&[0]));
    let settled = world.settle();
    assert!(settled.ahead.is_none());
    // The missing frame goes first, as the settlement decided.
    assert_eq!(world.plane.publishable(), Some(first.arena));
}

fn ahead_empty(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..2 {
        world.taken(tag, BE);
    }
    world.publish();
    world.block_ack(FrameSet::prefix(2));
    let settled = world.settle();
    assert!(settled.ahead.is_none());
    assert_eq!(world.returned_sequences(), [0, 1]);
}

fn partial_block_ack(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..4 {
        world.taken(tag, BE);
    }
    let (first, _) = world.publish();
    world.block_ack(positions(&[0, 2]));
    let settled = world.settle();
    assert_eq!(
        (settled.delivered, settled.dropped, settled.retried),
        (2, 0, 2)
    );
    assert_eq!(settled.bar, None);
    assert_eq!(world.returned_sequences(), [0, 2]);
    assert_eq!(world.plane.window(BE).unwrap().head.get(), 1);
    // The retained arena takes no frame: a new one goes to the other.
    world.taken(4, BE);
    assert_eq!(world.plane.publishable(), Some(first.arena));
    let (retry, frames) = world.publish();
    assert_eq!(retry.arena, first.arena);
    assert_eq!(retry.frames, 2);
    assert!(matches!(retry.kind, PpduKind::Aggregate { .. }));
    assert_eq!(
        frames.iter().map(|f| sequence(f)).collect::<Vec<_>>(),
        [1, 3]
    );
    assert!(frames.iter().all(|frame| retried(frame)));
    world.block_ack(FrameSet::prefix(2));
    let settled = world.settle();
    assert_eq!(
        (settled.delivered, settled.dropped, settled.retried),
        (2, 0, 0)
    );
    assert_eq!(world.returned_sequences(), [1, 3]);
    // The standby's frame is the oldest the window has not settled; the
    // whole BlockAck put it on the air ahead of the settlement.
    assert_eq!(world.plane.window(BE).unwrap().head.get(), 4);
    let lone = settled.ahead.expect("the standby went ahead");
    assert_eq!(
        (lone.arena, lone.kind),
        (other(first.arena), PpduKind::Single)
    );
    let frames = &world.port.published().last().unwrap().frames;
    assert_eq!(sequence(&frames[0]), 4);
}

fn holes_of_first_transmissions(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..4 {
        world.taken(tag, BE);
    }
    world.publish();
    world.block_ack(positions(&[0, 2]));
    assert_eq!(world.settled_holes(), Some(0b1010));
    // The retransmission's own hole is no position of the first aggregate.
    world.publish();
    world.block_ack(positions(&[0]));
    assert_eq!(world.settled_holes(), None);
}

fn attempt_limit(model: Installed<'_>) {
    let mut link = link(LinkProtection::Open);
    link.retry = link.retry.map(|retry| AggregateRetry {
        attempts: 2,
        ..retry
    });
    let mut world = World::new(model, link);
    world.agree(BE, 64);
    for tag in 0..3 {
        world.taken(tag, BE);
    }
    world.publish();
    world.answer(Air::NoAck);
    let settled = world.settle();
    assert_eq!((settled.dropped, settled.retried), (0, 3));
    let Some(RateLesson::Aggregate { outcome, .. }) = settled.lesson else {
        panic!("an unanswered A-MPDU teaches");
    };
    assert!(matches!(
        outcome,
        TxOutcome::Ended(AggregateEnd::AckTimeout(_))
    ));
    world.publish();
    world.answer(Air::NoAck);
    let settled = world.settle();
    assert_eq!((settled.dropped, settled.retried), (3, 0));
    assert_eq!(
        settled.bar.map(|(tid, head)| (tid, head.get())),
        Some((BE, 3))
    );
    assert_eq!(world.returned_sequences(), [0, 1, 2]);
    assert_eq!(world.plane.window(BE).unwrap().head.get(), 3);
}

fn lifetime(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    world.taken(0, BE);
    world.micros += 50_000;
    world.taken(1, BE);
    world.taken(2, BE);
    world.publish();
    world.block_ack(positions(&[1]));
    // The first MSDU is 100 ms old, the others 50 ms.
    world.micros += 50_000;
    let settled = world.settle();
    assert_eq!(
        (settled.delivered, settled.dropped, settled.retried),
        (1, 1, 1)
    );
    // Frame 2 is still missing: the window stops at it.
    assert_eq!(
        settled.bar.map(|(tid, head)| (tid, head.get())),
        Some((BE, 2))
    );
    assert_eq!(world.returned_sequences(), [0, 1]);
}

fn past_window(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 2);
    world.taken(0, BE);
    world.taken(1, BE);
    world.publish();
    let Intake::Wait(waiting, _) = world.take(2, BE) else {
        panic!("sequence number 2 lies past the window [0, 2)");
    };
    assert_eq!(waiting, msdu(2, BE));
    world.block_ack(FrameSet::prefix(2));
    world.settle();
    assert_eq!(world.plane.window(BE).unwrap().head.get(), 2);
    assert_eq!(world.intake(waiting), Intake::Taken(BE));
    let (_, frames) = world.publish();
    assert_eq!(sequence(&frames[0]), 2);
}

fn other_tid(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    world.agree(VIDEO, 64);
    world.taken(0, BE);
    let Intake::Wait(waiting, _) = world.take(1, VIDEO) else {
        panic!("the standby belongs to TID 0");
    };
    world.taken(2, BE);
    let (plan, _) = world.publish();
    assert_eq!((plan.tid, plan.frames), (BE, 2));
    assert_eq!(world.intake(waiting), Intake::Taken(VIDEO));
}

fn key_and_link(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Ccmp));
    let Intake::Refused { reason, .. } = world.take(0, BE) else {
        panic!("a protected link without its key refuses data");
    };
    assert_eq!(reason, IntakeRefusal::NoKey);
    world.key = Some(InstalledCcmpTxKey::install(
        TestKey,
        CcmpPacketNumberStep::ONE,
    ));
    world.taken(1, BE);
    let (plan, frames) = world.publish();
    assert_eq!(plan.kind, PpduKind::Single);
    let pn = CcmpHeader::new(CcmpPacketNumber::new(1).unwrap(), CcmpKeyId::PAIRWISE).encode();
    let at = IEEE80211_QOS_DATA_HEADER_LEN;
    assert_eq!(frames[0][at..at + CCMP_HEADER_LEN], pn);
    assert_eq!(frames[0].len(), StaDataHeaderTemplate::MAX_PREFIX_LEN);
    // A frame longer than the port sends takes neither number.
    let mut long = msdu(2, BE);
    long.length = MODEL_MAX_MPDU;
    let Intake::Refused { reason, .. } = world.intake(long) else {
        panic!("an MPDU longer than the port sends is refused");
    };
    assert_eq!(reason, IntakeRefusal::TooLong);
    world.taken(3, BE);
    let returned = &mut world.returned;
    world
        .plane
        .stop(&mut world.port, |memory| returned.push(memory), &mut ());
    // The numbers stay the station's, advanced past every frame taken.
    let key = world.key.as_ref().unwrap();
    assert_eq!(
        key.packet_numbers().last(),
        CcmpPacketNumber::new(2).unwrap()
    );
    assert_eq!(world.sequences[0].peek().get(), 2);
    let Intake::Refused { reason, .. } = world.take(4, BE) else {
        panic!("a stopped plane refuses data");
    };
    assert_eq!(reason, IntakeRefusal::NoLink);
}

fn round_trip(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..3 {
        world.taken(tag, BE);
    }
    let returned = &mut world.returned;
    let Some(&link) = world.plane.current() else {
        panic!("the plane holds its link");
    };
    world
        .plane
        .stop(&mut world.port, |memory| returned.push(memory), &mut ());
    assert_eq!(world.returned_sequences(), [0, 1, 2]);
    assert_eq!(world.sequences[0].peek().get(), 3);
    assert_eq!(world.sequences[5].peek().get(), 0);
    world.plane.link(link);
    world.taken(3, BE);
    let (plan, frames) = world.publish();
    assert_eq!(plan.kind, PpduKind::Single);
    assert_eq!(sequence(&frames[0]), 3);
}

fn single(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.taken(0, BE);
    assert!(matches!(world.take(1, BE), Intake::Wait(_, _)));
    let (plan, _) = world.publish();
    assert_eq!((plan.kind, plan.frames), (PpduKind::Single, 1));
    let settled = world.settle();
    assert_eq!(
        (settled.delivered, settled.dropped, settled.bar),
        (1, 0, None)
    );
    assert!(matches!(
        settled.lesson,
        Some(RateLesson::Single(TxOutcome::Ended(SingleEnd::Delivered {
            response: SingleResponse::Ack(_),
            ..
        })))
    ));
    assert_eq!(world.returned_sequences(), [0]);
    assert_eq!(world.plane.window(BE), None);
    // An unanswered single MPDU had the port's retry plan: it is dropped,
    // not published again.
    world.taken(1, BE);
    world.publish();
    world.answer(Air::NoAck);
    let settled = world.settle();
    assert_eq!(
        (settled.delivered, settled.dropped, settled.retried),
        (0, 1, 0)
    );
    assert!(matches!(
        settled.lesson,
        Some(RateLesson::Single(TxOutcome::Ended(SingleEnd::AckTimeout(
            _
        ))))
    ));
    assert_eq!(world.returned_sequences(), [1]);
    assert_eq!(world.plane.publishable(), None);
}

fn lone_frame(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    world.taken(0, BE);
    let (plan, _) = world.publish();
    assert_eq!(plan.kind, PpduKind::Single);
    world.answer(Air::NoAck);
    let settled = world.settle();
    assert_eq!((settled.dropped, settled.retried), (1, 0));
    assert_eq!(
        settled.bar.map(|(tid, head)| (tid, head.get())),
        Some((BE, 1))
    );
}

fn agreement_end(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..3 {
        world.taken(tag, BE);
    }
    let (aggregate, _) = world.publish();
    world.taken(3, BE);
    world.taken(4, BE);
    world.plane.agreement(BE, None);
    world.block_ack(positions(&[0]));
    let settled = world.settle();
    assert_eq!(
        (settled.delivered, settled.dropped, settled.retried),
        (1, 0, 2)
    );
    assert_eq!(settled.bar, None);
    assert_eq!(world.returned_sequences(), [0]);
    // The missing frames go one MPDU at a time from the same arena, with
    // the Retry bit of their A-MPDU.
    let (plan, frames) = world.publish();
    assert_eq!(
        (plan.arena, plan.kind, plan.frames),
        (aggregate.arena, PpduKind::Single, 2)
    );
    assert_eq!(frames.len(), 1);
    assert_eq!(sequence(&frames[0]), 1);
    assert!(retried(&frames[0]));
    let settled = world.settle();
    assert_eq!((settled.delivered, settled.retried), (1, 1));
    assert_eq!(world.returned_sequences(), [1]);
    let (_, frames) = world.publish();
    assert_eq!(sequence(&frames[0]), 2);
    world.answer(Air::NoAck);
    let settled = world.settle();
    assert_eq!(
        (settled.dropped, settled.retried, settled.bar),
        (1, 0, None)
    );
    assert_eq!(world.returned_sequences(), [2]);
    // The standby filled under the agreement drains the same way; a frame
    // that has not gone carries no Retry bit.
    for expected in [3, 4] {
        let (plan, frames) = world.publish();
        assert_eq!(
            (plan.arena, plan.kind),
            (other(aggregate.arena), PpduKind::Single)
        );
        assert_eq!(sequence(&frames[0]), expected);
        assert!(!retried(&frames[0]));
        assert_eq!(world.settle().delivered, 1);
        assert_eq!(world.returned_sequences(), [expected]);
    }
    assert_eq!(world.plane.publishable(), None);
}

fn shared_spaces(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    world.taken(0, BE);
    // The station sends a frame of its own in TID 0's space between two
    // MSDUs: the next MPDU follows it, and the standby keeps only
    // consecutive numbers.
    assert_eq!(world.sequences[0].take().get(), 1);
    assert!(matches!(world.take(1, BE), Intake::Wait(_, _)));
    let (plan, frames) = world.publish();
    assert_eq!(plan.frames, 1);
    assert_eq!(sequence(&frames[0]), 0);
    world.settle();
    world.taken(2, BE);
    let (_, frames) = world.publish();
    assert_eq!(sequence(&frames[0]), 2);
}

/// A key whose packet numbers are spent.
struct Spent;

impl TxKey for Spent {
    fn draw(&mut self) -> Result<CcmpPacketNumber, crate::numbers::PacketNumbersSpent> {
        Err(crate::numbers::PacketNumbersSpent)
    }
}

fn spent_key(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Ccmp));
    let now = world.now();
    let Intake::Refused { reason, memory } = world.plane.intake(
        &mut world.port,
        &ModelLink { protected: true },
        msdu(7, BE),
        &mut TxNumbers::new(
            SequenceCounter::new(SequenceNumber::ZERO),
            Some(world.sequences),
        ),
        Some(&mut Spent),
        now,
    ) else {
        panic!("a key whose numbers are spent refuses the MSDU");
    };
    assert_eq!(reason, IntakeRefusal::PacketNumberExhausted);
    assert_eq!(memory.id, 7, "its memory comes back");
    assert_eq!(world.port.front(7), None, "nothing was written");
}

fn numbering(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    let mut numbers = TxNumbers::new(SequenceCounter::new(SequenceNumber::ZERO), None);
    let now = world.now();
    let Intake::Refused { reason, .. } = world.plane.intake(
        &mut world.port,
        &ModelLink { protected: false },
        msdu(0, BE),
        &mut numbers,
        None::<&mut InstalledCcmpTxKey<TestKey>>,
        now,
    ) else {
        panic!("a QoS template refuses the station's non-QoS space");
    };
    assert_eq!(reason, IntakeRefusal::Numbering);
    assert_eq!(numbers.next(BE).get(), 0);
}

fn withdraw_unpublished(model: Installed<'_>) {
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    world.taken(0, BE);
    world.taken(1, BE);
    world.publish();
    world.taken(2, BE);
    assert!(world.plane.on_air() && world.plane.oldest_held(BE).is_some());
    let returned = &mut world.returned;
    world
        .plane
        .withdraw_unpublished(&mut world.port, |memory| returned.push(memory), &mut ());
    // The standby's frame goes back; the A-MPDU on the air settles later.
    assert_eq!(world.returned_sequences(), [2]);
    assert!(world.plane.is_connected() && world.plane.on_air());
    world.block_ack(FrameSet::prefix(2));
    assert_eq!(world.settle().delivered, 2);
    assert!(!world.plane.on_air() && world.plane.oldest_held(BE).is_none());
    world.taken(3, BE);
    let (_, frames) = world.publish();
    assert_eq!(sequence(&frames[0]), 3);
}

/// An aggregate of more frames than its rate admits sends its longest
/// prefix; the rest stay in their places, spent no attempt, and go next.
fn prefix(model: Installed<'_>) {
    // HT MCS 7 at 20 MHz: the model admits 3 000 octets of an aggregate,
    // two subframes of a 1 000-octet payload.
    let mut world = World::new(model, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..8 {
        let mut long = msdu(tag, BE);
        long.length = ETHERNET_HEADER_LEN + 1_000;
        long.memory.len = 1_000;
        assert_eq!(world.intake(long), Intake::Taken(BE));
    }
    let (plan, frames) = world.publish();
    assert_eq!(plan.frames, 8);
    let went = frames.len();
    assert!(went > 1 && went < 8, "a prefix of the eight: {went}");
    world.answer(Air::NoAck);
    let settled = world.settle();
    // The ones that went are kept for a retry, the others stay unsent.
    assert_eq!(settled.retried, 8);
    assert_eq!(settled.dropped, 0);
    let (retry, frames) = world.publish();
    assert_eq!(retry.frames, 8);
    // The frames that went once carry the Retry bit; the rest none.
    let retries = frames.iter().filter(|frame| retried(frame)).count();
    assert_eq!(retries, went.min(frames.len()));
}

/// An aggregate whose end says it reached no receiver (the air denied before
/// any transmission) spends no attempt of its frames: they outlive as many
/// such ends as the policy has attempts, then go on the air and are
/// delivered.
fn unreached(model: Installed<'_>) {
    let mut link = link(LinkProtection::Open);
    link.retry = link.retry.map(|retry| AggregateRetry {
        attempts: 2,
        ..retry
    });
    let mut world = World::new(model, link);
    world.agree(BE, 64);
    for tag in 0..3 {
        world.taken(tag, BE);
    }
    for _ in 0..2 {
        world.publish();
        world.answer(Air::Deny);
        let settled = world.settle();
        assert_eq!(
            (settled.delivered, settled.dropped, settled.retried),
            (0, 0, 3),
            "a denied air spends no attempt"
        );
        assert_eq!(settled.bar, None);
    }
    let (plan, frames) = world.publish();
    assert_eq!(plan.frames, 3);
    assert_eq!(frames.len(), 3);
    world.block_ack(FrameSet::prefix(3));
    let settled = world.settle();
    assert_eq!(
        (settled.delivered, settled.dropped, settled.retried),
        (3, 0, 0)
    );
    assert_eq!(world.returned_sequences(), [0, 1, 2]);
}

/// The single MPDUs' outcomes a selection was taught, by their ACK SNR.
#[derive(Default)]
struct Taught(Vec<Option<i8>>);

/// A backend's ACK evidence in the test: measured or not.
#[derive(Clone, Copy)]
enum Evidence {
    Measured(i8),
    Unmeasured,
}

impl RateSelection<RxMeta, i8, ()> for Taught {
    fn packet_number_step(&self) -> CcmpPacketNumberStep {
        CcmpPacketNumberStep::ONE
    }
    fn plan<R>(&self, _: FrameRates, _: impl Fn(PhyRate) -> Option<R>) -> Option<RetryPlan<R>> {
        None
    }
    fn associated(
        &mut self,
        _: &oer_protocols_ieee80211_core_upper_mac::rate_control::RatePeer,
        _: &RxMeta,
    ) {
    }
    fn disconnected(&mut self) {}
    fn completed<D>(&mut self, _: Instant, frame: FrameRates, outcome: &SingleOutcome<i8, D>) {
        assert_eq!(frame, FrameRates::Data);
        self.0.push(match outcome {
            TxOutcome::Ended(SingleEnd::Delivered {
                response: SingleResponse::Ack(snr),
                ..
            }) => Some(*snr),
            _ => None,
        });
    }
    fn completed_aggregate<D>(
        &mut self,
        _: Instant,
        _: BlockAckTally,
        _: &AggregateOutcome<(), D>,
    ) {
    }
}

#[test]
fn a_single_lesson_reaches_the_selection_through_its_ack_evidence() {
    let air = oer_protocols_ieee80211_contracts_port::Air {
        transmissions: NonZeroU16::MIN,
        rate: ht(),
    };
    let acked = |response| {
        RateLesson::<Evidence, (), ()>::Single(TxOutcome::Ended(SingleEnd::Delivered {
            air,
            response: SingleResponse::Ack(response),
        }))
    };
    let ack = |response| match response {
        Evidence::Measured(snr) => Some(snr),
        Evidence::Unmeasured => None,
    };
    let mut rates = Taught::default();
    let now = Instant::from_micros(1);
    acked(Evidence::Measured(20)).teach(now, &mut rates, ack);
    // Evidence the selection cannot read teaches nothing.
    acked(Evidence::Unmeasured).teach(now, &mut rates, ack);
    RateLesson::<Evidence, (), ()>::Single(TxOutcome::Ended(SingleEnd::AckTimeout(air)))
        .teach(now, &mut rates, ack);
    assert_eq!(rates.0, [Some(20), None]);
}

/// HE-MCS 7 on 20 MHz, one stream.
fn he() -> PhyRate {
    use oer_protocols_ieee80211_wire::phy::{FecCoding, HeGiLtf, HeMcs, HeRate, SpatialStreams};
    PhyRate::He(
        HeRate::new(
            HeMcs::new(7).unwrap(),
            SpatialStreams::ONE,
            PpduBandwidth::Mhz20,
            HeGiLtf::Ltf2xGi800Ns,
            FecCoding::Bcc,
            oer_protocols_ieee80211_wire::phy::Dcm::NotUsed,
        )
        .unwrap(),
    )
}

/// The link of an access point whose HE Operation names the HE TXOP
/// Duration RTS Threshold `threshold` (64 is 2 ms), or none.
fn he_link(threshold: Option<u16>) -> DataLink {
    use oer_protocols_ieee80211_core_upper_mac::protection::{BasicRates, BssProtection};
    use oer_protocols_ieee80211_wire::he::HeTxopDurationRtsThreshold;
    DataLink {
        protection: BssProtection {
            he_txop_rts_threshold: threshold.and_then(HeTxopDurationRtsThreshold::new),
            basic_rates: BasicRates::from_rate_elements(&[0x8c, 0x12, 0x98, 0x24], &[]),
            ..BssProtection::UNPROTECTED
        },
        ..link(LinkProtection::Open)
    }
}

/// The protection of the first publication of `frames` MSDUs of the link
/// `link`, its singles and aggregates at `rate`: an aggregate's, or each
/// stage's of a single's plan of two stages.
fn he_publication(link: DataLink, rate: PhyRate, frames: u8) -> Vec<Protection> {
    installed(|model| {
        let mut world = World::at(model, link, rate);
        if frames > 1 {
            world.agree(BE, 64);
        }
        for tag in 0..frames {
            world.taken(tag, BE);
        }
        let single = world.port.rate(rate).unwrap();
        let aggregate = world.aggregate;
        let now = world.now();
        let ppdus = [
            Ppdu::Single(
                once(single)
                    .then(RetryStage {
                        rate: single,
                        transmissions: NonZeroU8::MIN,
                        protection: Protection::None,
                    })
                    .ok()
                    .unwrap(),
            ),
            Ppdu::Aggregate {
                rate: aggregate,
                spacing: oer_protocols_ieee80211_wire::ht::MinimumMpduStartSpacing::NoRestriction,
                protection: Protection::None,
                alone: once(single),
            },
        ];
        world
            .plane
            .publication(&mut world.port, &world.gate, now, World::how(&ppdus))
            .unwrap();
        let publication = world.port.published().last().unwrap();
        assert_eq!(
            publication.single,
            if frames > 1 {
                Went::Aggregate
            } else {
                Went::Single
            }
        );
        publication.protection.clone()
    })
}

#[test]
fn a_finite_he_txop_threshold_protects_every_he_ppdu_and_its_retries() {
    let rts = |rate| {
        let mut policy = oer_protocols_ieee80211_core_upper_mac::ProtectionPolicy::new(
            oer_protocols_ieee80211_core_upper_mac::protection::RtsThreshold::Off,
        );
        policy.install_bss(he_link(Some(64)).protection);
        policy.every_he_txop_protection(rate)
    };
    assert!(matches!(rts(he()), Protection::RtsCts { .. }));
    // A single MPDU: each stage of its plan.
    assert_eq!(
        he_publication(he_link(Some(64)), he(), 1),
        [rts(he()), rts(he())]
    );
    // An A-MPDU.
    assert_eq!(he_publication(he_link(Some(64)), he(), 3), [rts(he())]);
}

#[test]
fn without_a_threshold_or_at_an_ht_rate_nothing_is_protected() {
    for (link, rate) in [(he_link(None), he()), (he_link(Some(64)), ht())] {
        for frames in [1, 3] {
            let protection = he_publication(link, rate, frames);
            assert!(protection.iter().all(|stage| *stage == Protection::None));
        }
    }
}

/// The front the plane writes is the MAC header, the CCMP header of a
/// protected link and the LLC/SNAP header (IEEE Std 802.11-2020, 9.3.2.1:
/// 24 octets, 26 with QoS Control; 12.5.3.2: 8 of CCMP; IETF RFC 1042: 8).
#[test]
fn a_front_is_its_mac_header_its_ccmp_header_and_its_llc_snap_header() {
    let front = |qos, protection| {
        let header = StaDataHeaderTemplate::new(
            OwnAddress::new(IndividualAddress::new(Eui48::new(STATION)).unwrap()),
            Bssid::new(IndividualAddress::new(Eui48::new(ACCESS_POINT)).unwrap()).unwrap(),
            qos,
            protection,
        )
        .unwrap();
        front_of(&header).get()
    };
    assert_eq!(front(PeerQos::NonQos, LinkProtection::Open), 32);
    assert_eq!(front(PeerQos::Qos, LinkProtection::Open), 34);
    assert_eq!(front(PeerQos::NonQos, LinkProtection::Ccmp), 40);
    assert_eq!(front(PeerQos::Qos, LinkProtection::Ccmp), 42);
}

/// An A-MPDU subframe is its delimiter and its MPDU padded to four octets
/// (IEEE Std 802.11-2020, 9.7.1).
#[test]
fn an_a_mpdu_subframe_is_its_mpdu_padded_to_four_and_its_delimiter() {
    assert_eq!(subframe(100), 104);
    assert_eq!(subframe(101), 108);
    assert_eq!(subframe(103), 108);
    assert_eq!(subframe(104), 108);
}

/// The originator sends within its window's head and size, and a number a
/// half space or more ahead lies behind the head (IEEE Std 802.11-2020,
/// 10.25.6, 10.24.7).
#[test]
fn a_window_admits_its_size_and_reads_a_half_space_ahead_as_behind() {
    let number = |value| SequenceNumber::new(value).unwrap();
    let window = TxWindow {
        head: number(4090),
        size: 8,
    };
    assert!(!window.ahead(number(4090)));
    assert!(!window.ahead(number(1)), "the window wraps: 4090 + 7");
    assert!(window.ahead(number(2)), "the first beyond the window");
    let horizon = 4090_u16.wrapping_add(SequenceNumber::HALF_SPACE) % 4096;
    assert!(window.ahead(number(horizon - 1)));
    assert!(
        !window.ahead(number(horizon)),
        "a half space ahead is behind"
    );
}

/// A TID's counter goes back only over a run of never-transmitted numbers
/// that ends at its last draw.
#[test]
fn withdrawn_numbers_rewind_the_counter_only_when_they_are_its_last_draws() {
    let number = |value| SequenceNumber::new(value).unwrap();
    let run = Withdrawn::widen(Some(Withdrawn::widen(None, number(3))), number(4));
    let withdrawn = Withdrawn {
        unsent: Some(run),
        transmitted: false,
    };
    assert_eq!(withdrawn.rewind_to(number(5)), Some(number(3)));
    assert_eq!(withdrawn.rewind_to(number(6)), None, "5 was drawn after");
    let holed = Withdrawn {
        unsent: Some(Withdrawn::widen(
            Some(Withdrawn::widen(None, number(3))),
            number(5),
        )),
        transmitted: false,
    };
    assert_eq!(holed.rewind_to(number(6)), None, "4 was transmitted");
    let none = Withdrawn {
        unsent: None,
        transmitted: true,
    };
    assert_eq!(none.rewind_to(number(6)), None);
}
