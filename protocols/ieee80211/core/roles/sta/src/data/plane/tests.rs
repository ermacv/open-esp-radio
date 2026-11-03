use super::*;
use core::{future::Future, marker::PhantomData, task::Poll};
use oer_portable_time::Duration;
use oer_protocols_ieee80211_contracts_port::{
    AmpduPhase, AmpduRate, Channel, ChannelWidth, LowerMacPhase, PhaseRate, RxMeta,
};
use oer_protocols_ieee80211_core_upper_mac::ccmp::{
    CcmpPacketNumberStep, InstalledCcmpTxKey, SoftwareCcmpKey,
};
use oer_protocols_ieee80211_testing_model::{
    MODEL_MAX_MPDU, ModelAirDenied, ModelArena, ModelArenaAnswer, ModelBackend,
    ModelBlockAckEvidence, ModelCaps, ModelNetworkMemory, ModelResources,
};
use oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber;
use oer_protocols_ieee80211_wire::{
    address::{Bssid, IndividualAddress, OwnAddress},
    data::IEEE80211_QOS_DATA_HEADER_LEN,
    frame::{FrameControlFields, sequence_control},
    phy::{HtMcs, HtRate, LegacyRate, PpduBandwidth},
};
use oer_protocols_radio_port::{
    Disabled, Ended, Owner, PortBody, SlotIdentity, run_port, with_local_slot,
};
use std::{boxed::Box, rc::Rc, vec::Vec};

const STATION: [u8; 6] = [2, 0, 0, 0, 0, 1];
const ACCESS_POINT: [u8; 6] = [2, 0, 0, 0, 0, 2];
const DESTINATION: [u8; 6] = [2, 0, 0, 0, 0, 0x10];
const PAYLOAD: usize = 100;
const BE: WmmUserPriority = WmmUserPriority::UP0;
const VIDEO: WmmUserPriority = WmmUserPriority::UP5;

fn ht() -> PhyRate {
    PhyRate::Ht(HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, false).unwrap())
}

/// The phase-checked rates a test publishes at.
#[derive(Clone, Copy)]
struct Rates<'p, 'id> {
    single: PhaseRate<'p, 'id, ModelCaps>,
    aggregate: AmpduRate<'p, 'id, ModelCaps>,
}

/// A test body over the model phase's rates.
trait Case {
    fn run<'p, 'id: 'p>(rates: Rates<'p, 'id>);
}

struct Body<C>(PhantomData<C>);

impl<Id: SlotIdentity, C: Case> PortBody<Id, ModelBackend<Id>> for Body<C> {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id>, Disabled>,
        _: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()>
    where
        ModelBackend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let Ok(mut enabled) = owner.enable().await else {
            panic!("the model enables")
        };
        {
            let mut phase = enabled.phase();
            let single = phase.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap();
            let ht = phase.rate(ht()).unwrap();
            let aggregate = phase.station().ampdu_rate(ht).ok().unwrap();
            C::run(Rates { single, aggregate });
        }
        let Ok(disabled) = enabled
            .disable(None, &mut |_| panic!("no receive buffer is held"))
            .await
        else {
            panic!("the model disables")
        };
        Ended::uninstall(disabled, (), ())
    }
}

fn run<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    for _ in 0..1_000 {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
    }
    panic!("the model installation did not complete");
}

fn with_rates<C: Case>() {
    run(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (backend, _) = ModelBackend::new(
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            [
                IndividualAddress::new(STATION).unwrap(),
                IndividualAddress::new(ACCESS_POINT).unwrap(),
            ],
            Rc::new(oer_portable_time_virtual::VirtualClock::<8>::new()),
        );
        let end = oer_protocols_radio_port::LocalExecution
            .scope(
                oer_protocols_radio_port::conformance::registration::guarded(
                    slot.register(backend).ok().unwrap(),
                ),
                async |frontend| run_port(frontend, Body::<C>(PhantomData)).await,
            )
            .await;
        assert!(matches!(
            end,
            oer_protocols_radio_port::PortEnd::Uninstalled { .. }
        ));
        drop(end);
        slot.end_model_world();
    }));
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

struct World<'p, 'id> {
    rates: Rates<'p, 'id>,
    /// The station's QoS sequence spaces, which each intake borrows.
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

    /// Offer `msdu` with the station's numbers.
    fn intake(&mut self, msdu: OutboundMsdu<ModelNetworkMemory>) -> Intake<ModelNetworkMemory> {
        let now = self.now();
        let numbers = LinkNumbers {
            sequences: LinkMode::Qos(&mut self.sequences),
            packet_numbers: self.key.as_ref().map(InstalledCcmpTxKey::packet_numbers),
        };
        self.plane.intake(&mut self.arena, msdu, numbers, now)
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
                    PpduKind::Single => ArenaFormat::Single(rates.single),
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

    fn returned_sequences(&mut self) -> Vec<u16> {
        self.returned
            .drain(..)
            .map(|memory| sequence(memory.as_slice()))
            .collect()
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
    let mut counter = SequenceCounter::new(SequenceNumber::from_low_bits(0));
    let numbers = LinkNumbers {
        sequences: LinkMode::NonQos(&mut counter),
        packet_numbers: None,
    };
    let now = world.now();
    let Intake::Refused { reason, .. } =
        world
            .plane
            .intake(&mut world.arena, msdu(0, BE), numbers, now)
    else {
        panic!("a QoS template refuses the station's non-QoS space");
    };
    assert_eq!(reason, IntakeRefusal::Numbering);
    assert_eq!(counter.peek().get(), 0);
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
