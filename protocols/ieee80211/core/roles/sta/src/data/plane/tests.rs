use super::*;
use core::marker::PhantomData;
use oer_portable_time::Duration;
use oer_protocols_ieee80211_contracts_port::{AmpduRate, PortArena, PortRate, RxMeta};
use oer_protocols_ieee80211_core_upper_mac::ccmp::{
    CcmpPacketNumberStep, InstalledCcmpTxKey, SoftwareCcmpKey,
};
use oer_protocols_ieee80211_testing_model::{
    MODEL_MAX_MPDU, ModelAirDenied, ModelArena, ModelArenaAnswer, ModelBlockAckEvidence, ModelCaps,
    ModelNetworkMemory,
};
use oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber;
use oer_protocols_ieee80211_wire::{
    address::{Bssid, IndividualAddress, OwnAddress},
    data::IEEE80211_QOS_DATA_HEADER_LEN,
    frame::{FrameControlFields, sequence_control},
    phy::{HtMcs, HtRate, LegacyRate, PpduBandwidth},
};
use std::{boxed::Box, vec::Vec};

const STATION: [u8; 6] = [2, 0, 0, 0, 0, 1];
const ACCESS_POINT: [u8; 6] = [2, 0, 0, 0, 0, 2];
const DESTINATION: [u8; 6] = [2, 0, 0, 0, 0, 0x10];
const PAYLOAD: usize = 100;
const BE: WmmUserPriority = WmmUserPriority::UP0;
const VIDEO: WmmUserPriority = WmmUserPriority::UP5;

fn ht() -> PhyRate {
    PhyRate::Ht(HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, false).unwrap())
}

/// The port-checked rates a test publishes at.
#[derive(Clone, Copy)]
struct Rates<'p, 'id> {
    single: PortRate<'p, ModelCaps>,
    aggregate: AmpduRate<'p, ModelCaps>,
    install: PhantomData<fn(&'id ()) -> &'id ()>,
}

/// A test body over an arena's checked rates.
trait Case {
    fn run<'p, 'id: 'p>(rates: Rates<'p, 'id>);
}

/// Run `C` with the rates a standalone model arena checks on its channel.
fn with_rates<C: Case>() {
    let arena = ModelArena::<ModelCaps>::new();
    let single = PortArena::rate(&arena, PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap();
    let ht = PortArena::rate(&arena, ht()).unwrap();
    let aggregate = PortArena::ampdu_rate(&arena, ht).ok().unwrap();
    C::run(Rates {
        single,
        aggregate,
        install: PhantomData,
    });
}

macro_rules! cases {
    ($($name:ident => $body:ident),* $(,)?) => {$(
        #[test]
        fn $name() {
            struct This;
            impl Case for This {
                fn run<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
                    $body(rates)
                }
            }
            with_rates::<This>();
        }
    )*};
}

fn link(protection: LinkProtection) -> DataLink {
    DataLink {
        header: StaDataHeaderTemplate::new(
            OwnAddress::new(IndividualAddress::new(STATION).unwrap()),
            Bssid::new(IndividualAddress::new(ACCESS_POINT).unwrap()).unwrap(),
            true,
            protection,
        )
        .unwrap(),
        admission: AcAdmission::default(),
        retry: Some(AggregateRetry {
            lifetime: Duration::from_millis(100),
            attempts: 3,
        }),
        ht: Some(HtLimits {
            spacing: MpduStartSpacing::new(0).unwrap(),
            max_length: 65_535,
        }),
        windows: [None; 8],
    }
}

/// Key material of a test key, which software alone owns.
struct TestKey;

impl SoftwareCcmpKey for TestKey {}

/// An MSDU whose payload octets are all `tag`.
fn msdu(tag: u8, priority: WmmUserPriority) -> OutboundMsdu<ModelNetworkMemory> {
    let mut memory: ModelNetworkMemory = Box::new([0; MODEL_MAX_MPDU]);
    let frame = &mut memory[MSDU_ETHERNET_OFFSET..];
    frame[..6].copy_from_slice(&DESTINATION);
    frame[6..12].copy_from_slice(&STATION);
    frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    frame[14..14 + PAYLOAD].fill(tag);
    OutboundMsdu {
        memory,
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

/// An arena's end the plane has not settled.
type Done<'p, 'id> = CompletedOf<'p, 'id, ModelArena<ModelCaps>>;

struct World<'p, 'id> {
    rates: Rates<'p, 'id>,
    /// The link's QoS sequence spaces, which each intake draws from.
    sequences: [SequenceCounter; 8],
    /// The station's pairwise key and its packet numbers.
    key: Option<InstalledCcmpTxKey<TestKey>>,
    arena: ModelArena<ModelCaps>,
    plane: DataPlane,
    returned: Vec<ModelNetworkMemory>,
    micros: u64,
}

impl<'p, 'id: 'p> World<'p, 'id> {
    fn new(rates: Rates<'p, 'id>, link: DataLink) -> Self {
        let mut arena = ModelArena::new();
        let mut plane = DataPlane::new(TxArena::arenas(&mut arena).unwrap())
            .ok()
            .unwrap();
        plane.handover(link).ok().unwrap();
        plane.aggregate_rate(Some(AggregateRate {
            rate: ht(),
            max_subframes: 8,
            max_length: 65_535,
        }));
        Self {
            rates,
            sequences: [SequenceCounter::new(SequenceNumber::from_low_bits(0)); 8],
            key: None,
            arena,
            plane,
            returned: Vec::new(),
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

    fn take(&mut self, tag: u8, tid: WmmUserPriority) -> Intake<ModelNetworkMemory> {
        self.intake(msdu(tag, tid))
    }

    /// Offer `msdu` with the link's numbers.
    fn intake(&mut self, msdu: OutboundMsdu<ModelNetworkMemory>) -> Intake<ModelNetworkMemory> {
        let now = self.now();
        let mut numbers = TxNumbers::new(
            SequenceCounter::new(SequenceNumber::ZERO),
            Some(self.sequences),
            self.key.take(),
        );
        let intake = self.plane.intake(&mut self.arena, msdu, &mut numbers, now);
        let (_, sequences, key) = numbers.into_parts();
        self.sequences = sequences.unwrap();
        self.key = key;
        intake
    }

    fn taken(&mut self, tag: u8, tid: WmmUserPriority) {
        assert_eq!(self.take(tag, tid), Intake::Taken(tid));
    }

    /// Publish the next arena; its plan and the frames the model read.
    fn publish(&mut self) -> (PpduPlan, Vec<Vec<u8>>) {
        let rates = self.rates;
        let mut planned = None;
        let publication = self
            .plane
            .publication(|plan| {
                planned = Some(plan);
                Some(match plan.kind {
                    PpduKind::Single => ArenaFormat::Single(once(rates.single)),
                    PpduKind::Aggregate { spacing } => ArenaFormat::Aggregate {
                        rate: rates.aggregate,
                        spacing,
                    },
                })
            })
            .unwrap();
        let now = self.now();
        assert!(self.arena.publish(publication, now).is_ok());
        let frames = self.arena.published().last().unwrap().frames.clone();
        (planned.unwrap(), frames)
    }

    /// Publish the arena the plane puts ahead of settling `done`: its plan.
    fn publish_ahead(&mut self, done: &Done<'p, 'id>) -> Option<PpduPlan> {
        let rates = self.rates;
        let mut planned = None;
        let publication = self.plane.publication_ahead(done, |plan| {
            planned = Some(plan);
            Some(match plan.kind {
                PpduKind::Single => ArenaFormat::Single(once(rates.single)),
                PpduKind::Aggregate { spacing } => ArenaFormat::Aggregate {
                    rate: rates.aggregate,
                    spacing,
                },
            })
        })?;
        let now = self.now();
        assert!(self.arena.publish(publication, now).is_ok());
        planned
    }

    /// The arena on the air's end, which the plane has not settled.
    fn ended(&mut self) -> Done<'p, 'id> {
        let now = self.now();
        self.arena.service((), now).unwrap().unwrap()
    }

    fn settle_ended(&mut self, done: Done<'p, 'id>) {
        let now = self.now();
        let returned = &mut self.returned;
        self.plane
            .completed(
                &mut self.arena,
                done,
                now,
                |memory| returned.push(memory),
                &mut (),
            )
            .ok()
            .unwrap();
    }

    fn block_ack(&mut self, acked: FrameMask) {
        let on_air = self.arena.on_air().unwrap();
        let report = self.arena.block_ack(on_air, acked).unwrap();
        self.arena.answer(ModelArenaAnswer::BlockAck(report));
    }

    fn settle(&mut self) -> Settled<ModelBlockAckEvidence, ModelAirDenied> {
        let now = self.now();
        let done = self.arena.service((), now).unwrap().unwrap();
        let returned = &mut self.returned;
        self.plane
            .completed(
                &mut self.arena,
                done,
                now,
                |memory| returned.push(memory),
                &mut (),
            )
            .ok()
            .unwrap()
    }

    /// The holes of the aggregate that ended, as its settlement reports them.
    fn settled_holes(&mut self) -> u32 {
        let now = self.now();
        let done = self.arena.service((), now).unwrap().unwrap();
        let returned = &mut self.returned;
        let mut holes = Holes(None);
        self.plane
            .completed(
                &mut self.arena,
                done,
                now,
                |memory| returned.push(memory),
                &mut holes,
            )
            .ok()
            .unwrap();
        holes.0.unwrap()
    }

    fn returned_sequences(&mut self) -> Vec<u16> {
        self.returned
            .drain(..)
            .map(|memory| sequence(memory.as_slice()))
            .collect()
    }
}

/// The holes of the settlement it saw.
struct Holes(Option<u32>);

impl EventSink<StaEvent> for Holes {
    const ENABLED: bool = true;
    fn event(&mut self, event: StaEvent) {
        if let StaEvent::AggregateSettled(settled) = event {
            self.0 = Some(settled.holes);
        }
    }
}

fn all(count: u8) -> FrameMask {
    FrameMask::first(count).unwrap()
}

fn mask(positions: &[u8]) -> FrameMask {
    FrameMask::from_bits(positions.iter().fold(0, |bits, at| bits | 1 << at)).unwrap()
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
    withdrawing_unpublished_frames_keeps_the_link_and_the_arena_on_the_air => withdraw_unpublished,
    a_tid_without_agreement_goes_one_mpdu_at_a_time => single,
    a_lone_frame_under_an_agreement_goes_alone_and_owes_a_block_ack_request_when_dropped => lone_frame,
    an_ended_agreement_drains_its_arenas_as_single_mpdus_without_drops => agreement_end,
    the_standby_room_counts_down_to_the_tids_bound_while_an_arena_is_on_the_air => standby_room,
    a_whole_block_ack_puts_the_standby_on_the_air_before_the_settlement => ahead_whole,
    a_partial_block_ack_leaves_the_order_to_the_settlement => ahead_partial,
    nothing_goes_ahead_of_a_settlement_without_frames_in_the_other_arena => ahead_empty,
}

fn standby_room<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    let capacity = TxArena::capacity(&world.arena);
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
    assert!(matches!(world.take(10, BE), Intake::Wait(_)));
    // The arena's own capacity bounds a wider TID bound.
    assert_eq!(world.plane.standby_room(4), Some(0));
    world.block_ack(all(2));
    world.settle();
    assert_eq!(world.plane.standby_room(capacity), None);
}

fn full_block_ack<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
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
    // The payload follows the QoS header and LLC/SNAP, moved down to it.
    let prefix = IEEE80211_QOS_DATA_HEADER_LEN + LLC_SNAP_HEADER_LEN;
    assert_eq!(frames[2].len(), prefix + PAYLOAD);
    assert!(frames[2][prefix..].iter().all(|octet| *octet == 2));
    world.block_ack(all(4));
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
        BlockAckTally::new(core::num::NonZeroU16::new(4).unwrap(), 4).unwrap()
    );
    assert!(matches!(
        outcome,
        TxOutcome::Ended(AggregateEnd::Delivered { .. })
    ));
    assert_eq!(world.returned_sequences(), [0, 1, 2, 3]);
    assert_eq!(world.plane.window(BE).unwrap().head.get(), 4);
    assert_eq!(world.plane.publishable(), None);
}

fn ahead_whole<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..2 {
        world.taken(tag, BE);
    }
    let (first, _) = world.publish();
    for tag in 2..4 {
        world.taken(tag, BE);
    }
    world.block_ack(all(2));
    let done = world.ended();
    let ahead = world.publish_ahead(&done).expect("the standby goes first");
    assert_eq!(ahead.arena, first.arena.other());
    assert_eq!(ahead.frames, 2);
    assert_eq!(world.arena.on_air(), Some(ahead.arena));
    assert!(world.returned_sequences().is_empty());
    world.settle_ended(done);
    assert_eq!(world.returned_sequences(), [0, 1]);
    assert_eq!(world.plane.window(BE).unwrap().head.get(), 2);
    // The settled arena takes the next frame while the standby is on the air.
    world.taken(4, BE);
    world.block_ack(all(2));
    let settled = world.settle();
    assert_eq!(settled.delivered, 2);
    assert_eq!(world.returned_sequences(), [2, 3]);
    assert_eq!(world.plane.publishable(), Some(first.arena));
}

fn ahead_partial<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..2 {
        world.taken(tag, BE);
    }
    let (first, _) = world.publish();
    world.taken(2, BE);
    world.taken(3, BE);
    world.block_ack(mask(&[0]));
    let done = world.ended();
    assert!(world.publish_ahead(&done).is_none());
    world.settle_ended(done);
    // The missing frame goes first, as the settlement decided.
    assert_eq!(world.plane.publishable(), Some(first.arena));
}

fn ahead_empty<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..2 {
        world.taken(tag, BE);
    }
    world.publish();
    world.block_ack(all(2));
    let done = world.ended();
    assert!(world.publish_ahead(&done).is_none());
    world.settle_ended(done);
    assert_eq!(world.returned_sequences(), [0, 1]);
}

fn partial_block_ack<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..4 {
        world.taken(tag, BE);
    }
    let (first, _) = world.publish();
    world.block_ack(mask(&[0, 2]));
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
    world.block_ack(all(2));
    let settled = world.settle();
    assert_eq!(
        (settled.delivered, settled.dropped, settled.retried),
        (2, 0, 0)
    );
    assert_eq!(world.returned_sequences(), [1, 3]);
    // The standby's frame is the oldest the window has not settled.
    assert_eq!(world.plane.window(BE).unwrap().head.get(), 4);
    let (lone, frames) = world.publish();
    assert_eq!(
        (lone.arena, lone.kind),
        (first.arena.other(), PpduKind::Single)
    );
    assert_eq!(sequence(&frames[0]), 4);
}

fn holes_of_first_transmissions<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..4 {
        world.taken(tag, BE);
    }
    world.publish();
    world.block_ack(mask(&[0, 2]));
    assert_eq!(world.settled_holes(), 0b1010);
    // The retransmission's own hole is no position of the first aggregate.
    world.publish();
    world.block_ack(mask(&[0]));
    assert_eq!(world.settled_holes(), 0);
}

fn attempt_limit<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut link = link(LinkProtection::Open);
    link.retry = link.retry.map(|retry| AggregateRetry {
        attempts: 2,
        ..retry
    });
    let mut world = World::new(rates, link);
    world.agree(BE, 64);
    for tag in 0..3 {
        world.taken(tag, BE);
    }
    world.publish();
    world.arena.answer(ModelArenaAnswer::NoResponse);
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
    world.arena.answer(ModelArenaAnswer::NoResponse);
    let settled = world.settle();
    assert_eq!((settled.dropped, settled.retried), (3, 0));
    assert_eq!(
        settled.bar.map(|(tid, head)| (tid, head.get())),
        Some((BE, 3))
    );
    assert_eq!(world.returned_sequences(), [0, 1, 2]);
    assert_eq!(world.plane.window(BE).unwrap().head.get(), 3);
}

fn lifetime<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    world.taken(0, BE);
    world.micros += 50_000;
    world.taken(1, BE);
    world.taken(2, BE);
    world.publish();
    world.block_ack(mask(&[1]));
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

fn past_window<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 2);
    world.taken(0, BE);
    world.taken(1, BE);
    world.publish();
    let Intake::Wait(waiting) = world.take(2, BE) else {
        panic!("sequence number 2 lies past the window [0, 2)");
    };
    assert_eq!(waiting, msdu(2, BE));
    world.block_ack(all(2));
    world.settle();
    assert_eq!(world.plane.window(BE).unwrap().head.get(), 2);
    assert_eq!(world.intake(waiting), Intake::Taken(BE));
    let (_, frames) = world.publish();
    assert_eq!(sequence(&frames[0]), 2);
}

fn other_tid<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    world.agree(VIDEO, 64);
    world.taken(0, BE);
    let Intake::Wait(waiting) = world.take(1, VIDEO) else {
        panic!("the standby belongs to TID 0");
    };
    world.taken(2, BE);
    let (plan, _) = world.publish();
    assert_eq!((plan.tid, plan.frames), (BE, 2));
    assert_eq!(world.intake(waiting), Intake::Taken(VIDEO));
}

fn key_and_link<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Ccmp));
    let Intake::Refused { reason, .. } = world.take(0, BE) else {
        panic!("a protected link without its key refuses data");
    };
    assert_eq!(reason, IntakeRefusal::NoKey);
    world.key = Some(InstalledCcmpTxKey::install(
        TestKey,
        CcmpPacketNumberStep::ONE,
    ));
    world.arena.set_key(true);
    world.taken(1, BE);
    let (plan, frames) = world.publish();
    assert_eq!(plan.kind, PpduKind::Single);
    assert_eq!(world.arena.published()[0].encryption, MpduProtection::Ccmp);
    let pn = CcmpHeader::new(CcmpPacketNumber::new(1).unwrap(), CcmpKeyId::PAIRWISE).encode();
    let at = IEEE80211_QOS_DATA_HEADER_LEN;
    assert_eq!(frames[0][at..at + CCMP_HEADER_LEN], pn);
    assert_eq!(frames[0].len(), MSDU_PAYLOAD_OFFSET + PAYLOAD);
    // A frame too long for its memory takes neither number.
    let mut long = msdu(2, BE);
    long.length = MODEL_MAX_MPDU;
    let Intake::Refused { reason, .. } = world.intake(long) else {
        panic!("an MPDU longer than its memory is refused");
    };
    assert_eq!(reason, IntakeRefusal::TooLong);
    world.taken(3, BE);
    let returned = &mut world.returned;
    assert!(
        world
            .plane
            .stop(&mut world.arena, |memory| returned.push(memory), &mut ())
            .unwrap()
            .is_some()
    );
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

fn round_trip<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..3 {
        world.taken(tag, BE);
    }
    let returned = &mut world.returned;
    let link = world
        .plane
        .stop(&mut world.arena, |memory| returned.push(memory), &mut ())
        .unwrap()
        .unwrap();
    assert_eq!(world.returned_sequences(), [0, 1, 2]);
    assert_eq!(world.sequences[0].peek().get(), 3);
    assert_eq!(world.sequences[5].peek().get(), 0);
    assert!(world.plane.handover(link).is_ok());
    assert!(matches!(
        world.plane.handover(link),
        Err((_, HandoverRefused::Connected))
    ));
    world.taken(3, BE);
    let (plan, frames) = world.publish();
    assert_eq!(plan.kind, PpduKind::Single);
    assert_eq!(sequence(&frames[0]), 3);
}

fn single<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.taken(0, BE);
    assert!(matches!(world.take(1, BE), Intake::Wait(_)));
    let (plan, _) = world.publish();
    assert_eq!((plan.kind, plan.frames), (PpduKind::Single, 1));
    world.arena.answer(ModelArenaAnswer::Ack);
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
    world.arena.answer(ModelArenaAnswer::NoResponse);
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

fn lone_frame<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    world.taken(0, BE);
    let (plan, _) = world.publish();
    assert_eq!(plan.kind, PpduKind::Single);
    world.arena.answer(ModelArenaAnswer::NoResponse);
    let settled = world.settle();
    assert_eq!((settled.dropped, settled.retried), (1, 0));
    assert_eq!(
        settled.bar.map(|(tid, head)| (tid, head.get())),
        Some((BE, 1))
    );
}

fn agreement_end<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    for tag in 0..3 {
        world.taken(tag, BE);
    }
    let (aggregate, _) = world.publish();
    world.taken(3, BE);
    world.taken(4, BE);
    world.plane.agreement(BE, None);
    world.block_ack(mask(&[0]));
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
    world.arena.answer(ModelArenaAnswer::Ack);
    let settled = world.settle();
    assert_eq!((settled.delivered, settled.retried), (1, 1));
    assert_eq!(world.returned_sequences(), [1]);
    let (_, frames) = world.publish();
    assert_eq!(sequence(&frames[0]), 2);
    world.arena.answer(ModelArenaAnswer::NoResponse);
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
            (aggregate.arena.other(), PpduKind::Single)
        );
        assert_eq!(sequence(&frames[0]), expected);
        assert!(!retried(&frames[0]));
        world.arena.answer(ModelArenaAnswer::Ack);
        assert_eq!(world.settle().delivered, 1);
        assert_eq!(world.returned_sequences(), [expected]);
    }
    assert_eq!(world.plane.publishable(), None);
}

fn shared_spaces<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    world.taken(0, BE);
    // The station sends a frame of its own in TID 0's space between two
    // MSDUs: the next MPDU follows it, and the standby keeps only
    // consecutive numbers.
    assert_eq!(world.sequences[0].take().get(), 1);
    assert!(matches!(world.take(1, BE), Intake::Wait(_)));
    let (plan, frames) = world.publish();
    assert_eq!(plan.frames, 1);
    assert_eq!(sequence(&frames[0]), 0);
    world.arena.answer(ModelArenaAnswer::Ack);
    world.settle();
    world.taken(2, BE);
    let (_, frames) = world.publish();
    assert_eq!(sequence(&frames[0]), 2);
}

fn numbering<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    let mut numbers = TxNumbers::<InstalledCcmpTxKey<TestKey>>::new(
        SequenceCounter::new(SequenceNumber::from_low_bits(0)),
        None,
        None,
    );
    let now = world.now();
    let Intake::Refused { reason, .. } =
        world
            .plane
            .intake(&mut world.arena, msdu(0, BE), &mut numbers, now)
    else {
        panic!("a QoS template refuses the station's non-QoS space");
    };
    assert_eq!(reason, IntakeRefusal::Numbering);
    assert_eq!(numbers.next(BE).get(), 0);
}

fn withdraw_unpublished<'p, 'id: 'p>(rates: Rates<'p, 'id>) {
    let mut world = World::new(rates, link(LinkProtection::Open));
    world.agree(BE, 64);
    world.taken(0, BE);
    world.taken(1, BE);
    world.publish();
    world.taken(2, BE);
    assert!(world.plane.on_air() && world.plane.holds_frames());
    let returned = &mut world.returned;
    world
        .plane
        .withdraw_unpublished(&mut world.arena, |memory| returned.push(memory), &mut ())
        .unwrap();
    // The standby's frame goes back; the A-MPDU on the air settles later.
    assert_eq!(world.returned_sequences(), [2]);
    assert!(world.plane.is_connected() && world.plane.on_air());
    world.block_ack(all(2));
    assert_eq!(world.settle().delivered, 2);
    assert!(!world.plane.on_air() && !world.plane.holds_frames());
    world.taken(3, BE);
    let (_, frames) = world.publish();
    assert_eq!(sequence(&frames[0]), 3);
}

/// The single MPDUs' outcomes a selection was taught, by their ACK SNR.
#[derive(Default)]
struct Taught(Vec<Option<i8>>);

impl RateSelection<RxMeta, i8, ModelBlockAckEvidence> for Taught {
    fn packet_number_step(&self) -> CcmpPacketNumberStep {
        CcmpPacketNumberStep::ONE
    }
    fn plan<R>(
        &self,
        _: FrameRates,
        _: impl Fn(PhyRate) -> Option<R>,
    ) -> Option<oer_protocols_ieee80211_contracts_port::RetryPlan<R>> {
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
        _: &AggregateOutcome<ModelBlockAckEvidence, D>,
    ) {
    }
}

#[test]
fn a_single_lesson_reaches_the_selection_through_its_ack_evidence() {
    let air = oer_protocols_ieee80211_contracts_port::Air {
        transmissions: core::num::NonZeroU16::MIN,
        rate: ht(),
    };
    let acked = |response| {
        RateLesson::<ModelBlockAckEvidence, ModelAirDenied>::Single(TxOutcome::Ended(
            SingleEnd::Delivered {
                air,
                response: SingleResponse::Ack(response),
            },
        ))
    };
    let ack = |response| match response {
        ModelBlockAckEvidence::Measured(snr) => Some(snr),
        ModelBlockAckEvidence::Unmeasured => None,
    };
    let mut rates = Taught::default();
    let now = Instant::from_micros(1);
    acked(ModelBlockAckEvidence::Measured(20)).teach(now, &mut rates, ack);
    // Evidence the selection cannot read teaches nothing.
    acked(ModelBlockAckEvidence::Unmeasured).teach(now, &mut rates, ack);
    RateLesson::<ModelBlockAckEvidence, ModelAirDenied>::Single(TxOutcome::Ended(
        SingleEnd::AckTimeout(air),
    ))
    .teach(now, &mut rates, ack);
    assert_eq!(rates.0, [Some(20), None]);
}

/// A single MPDU's plan of one transmission at `rate`.
fn once<R>(rate: R) -> oer_protocols_ieee80211_contracts_port::RetryPlan<R> {
    oer_protocols_ieee80211_contracts_port::RetryPlan::new(
        oer_protocols_ieee80211_contracts_port::RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::MIN,
            protection: oer_protocols_ieee80211_contracts_port::Protection::None,
        },
    )
}
