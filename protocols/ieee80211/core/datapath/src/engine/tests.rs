//! The engine over the model port: its cases, its sizes and the search of
//! every order of its sections (`search`). The tests reach the engine
//! through its views, as a controller does.

use super::*;
use crate::AggregateRetry;
use crate::model::{Running, running};
use crate::plane::{AggregateRate, HtLimits, PpduKind};
use core::num::NonZeroU8;
use oer_portable_time::Duration;
use oer_protocols_ieee80211_contracts_port::DataPort;
use oer_protocols_ieee80211_contracts_port::{
    AmpduRate, FrameSet, HEADROOM, MpduStartSpacing, Ppdu, Protection, RetryStage,
};
use oer_protocols_ieee80211_core_upper_mac::ccmp::{
    CcmpPacketNumberStep, InstalledCcmpTxKey, SoftwareCcmpKey,
};
use oer_protocols_ieee80211_core_upper_mac::qos::AcAdmission;
use oer_protocols_ieee80211_core_upper_mac::sequence::SequenceCounter;
use oer_protocols_ieee80211_testing_model::{
    Air, MODEL_AGREEMENTS, MODEL_GROUPS, ModelCaps, ModelLent, ModelLink, ModelMemory, ModelPort,
};
use oer_protocols_ieee80211_wire::address::{Bssid, OwnAddress};
use oer_protocols_ieee80211_wire::phy::{
    HtGuardInterval, HtMcs, HtRate, LegacyRate, PhyRate, PpduBandwidth,
};
use oer_protocols_ieee80211_wire::security::LinkProtection;
use oer_protocols_ieee80211_wire::station::StaDataHeaderTemplate;
use std::collections::VecDeque;
use std::vec::Vec;

mod receive;
mod search;
mod search_rx;

/// The pressure episode's bound the tests' engines take: the deleted
/// actor's 10 ms.
pub(super) const LATE: Duration = Duration::from_millis(10);

/// The receive policy the tests' engines take: the episode's bound, and a
/// reorder gap of 100 ms.
pub(super) const POLICY: ReceivePolicy = ReceivePolicy {
    late: LATE,
    gap: Duration::from_millis(100),
};
pub(super) const STATION: [u8; 6] = [2, 0, 0, 0, 0, 1];
pub(super) const ACCESS_POINT_ADDRESS: [u8; 6] = [2, 0, 0, 0, 0, 2];
pub(super) const CLIENT_A: [u8; 6] = [2, 0, 0, 0, 0, 0x0a];
pub(super) const CLIENT_B: [u8; 6] = [2, 0, 0, 0, 0, 0x0b];
pub(super) const DESTINATION: [u8; 6] = [2, 0, 0, 0, 0, 0x10];
pub(super) const PAYLOAD: usize = 100;
pub(super) const ETHERNET: usize = 14;
pub(super) const BEST_EFFORT: WmmAccessCategory = WmmAccessCategory::BestEffort;

/// A station's single places in the tests.
pub(super) const STATION_SINGLES: usize = 8;
/// An access point's controller's.
pub(super) const AP_SINGLES: usize = 17;

/// Key material of a test key, which software alone owns.
pub(super) struct TestKey;

impl SoftwareCcmpKey for TestKey {}

pub(super) type Key = InstalledCcmpTxKey<TestKey>;

/// A station's engine over the model port, with a pool of `POOL`.
pub(super) type StationEngine<'id, const POOL: usize> = Engine<
    'id,
    ModelPort<'id, STATION_SINGLES, 0>,
    Arenas,
    Key,
    1,
    POOL,
    STATION_SINGLES,
    MODEL_GROUPS,
    MODEL_AGREEMENTS,
>;

/// An access point's engine of `PEERS` clients over the model port.
pub(super) type ApEngine<'id, const PEERS: usize, const POOL: usize> = Engine<
    'id,
    ModelPort<'id, AP_SINGLES, DATA_SINGLES>,
    Singles,
    Key,
    PEERS,
    POOL,
    AP_SINGLES,
    MODEL_GROUPS,
    MODEL_AGREEMENTS,
>;

/// The network's end: what it offers, each MSDU's Ethernet frame, and what
/// came home, in order; what it received, lent or copied, in order; its
/// lend limit and its room for copies.
pub(super) struct Net {
    pub offered: VecDeque<ModelMemory>,
    pub frames: Vec<(u32, WmmUserPriority, Vec<u8>)>,
    pub home: Vec<u32>,
    /// The units it holds, with the Ethernet frame of each.
    pub lent: Vec<(ModelLent, EthernetSpan)>,
    /// Every Ethernet frame it received, in order: a lent unit's place, or
    /// a copy's bytes.
    pub received: Vec<Got>,
    /// The units it may hold at once.
    pub lend_limit: u8,
    /// The frames its storage takes now: a copy takes one, and the test
    /// gives room back as the network would when it consumes a copy.
    pub copy_room: u8,
}

/// An Ethernet frame the network received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Got {
    /// A unit lent at its place.
    Lent(u8),
    /// A copy of these bytes.
    Copied(Vec<u8>),
}

impl Default for Net {
    fn default() -> Self {
        Self {
            offered: VecDeque::new(),
            frames: Vec::new(),
            home: Vec::new(),
            lent: Vec::new(),
            received: Vec::new(),
            lend_limit: 4,
            copy_room: 4,
        }
    }
}

impl Net {
    /// Offer MSDU `id` of `priority` to `destination`.
    pub fn offer(&mut self, id: u32, priority: WmmUserPriority, destination: [u8; 6]) {
        let mut ethernet = Vec::new();
        ethernet.extend_from_slice(&destination);
        ethernet.extend_from_slice(&STATION);
        ethernet.extend_from_slice(&0x0800_u16.to_be_bytes());
        let mut front = [0; HEADROOM];
        front[crate::MSDU_ETHERNET_OFFSET..].copy_from_slice(&ethernet);
        let mut frame = ethernet;
        frame.extend((0..PAYLOAD).map(|octet| octet as u8 ^ id as u8));
        self.frames.push((id, priority, frame));
        self.offered.push_back(ModelMemory {
            id,
            len: PAYLOAD as u16,
            front,
        });
    }

    fn of(&self, memory: &ModelMemory) -> Option<&(u32, WmmUserPriority, Vec<u8>)> {
        self.frames.iter().find(|(id, _, _)| *id == memory.id)
    }
}

impl Network<ModelMemory, ModelLent> for Net {
    fn take(&mut self) -> Option<ModelMemory> {
        self.offered.pop_front()
    }

    fn length(&self, memory: &ModelMemory) -> usize {
        self.of(memory).map_or(0, |(_, _, frame)| frame.len())
    }

    fn priority(&self, memory: &ModelMemory) -> WmmUserPriority {
        self.of(memory)
            .map_or(WmmUserPriority::UP0, |(_, priority, _)| *priority)
    }

    fn frame<'m>(&'m self, memory: &'m ModelMemory, length: usize) -> &'m [u8] {
        self.of(memory)
            .map_or(&[], |(_, _, frame)| &frame[..length.min(frame.len())])
    }

    fn home(&mut self, memory: ModelMemory) {
        self.home.push(memory.id);
    }

    fn room(&self) -> Room {
        let held = u8::try_from(self.lent.len()).unwrap_or(u8::MAX);
        Room::new(self.lend_limit.saturating_sub(held), self.copy_room)
    }

    fn lent(&mut self, _permit: LendPermit, unit: ModelLent, frame: EthernetSpan) {
        self.received.push(Got::Lent(unit.place()));
        self.lent.push((unit, frame));
    }

    fn copied(&mut self, _permit: CopyPermit, frame: &[u8]) {
        self.copy_room = self.copy_room.saturating_sub(1);
        self.received.push(Got::Copied(frame.to_vec()));
    }
}

/// One transmission at `rate`.
pub(super) fn once<R>(rate: R) -> RetryPlan<R> {
    RetryPlan::new(RetryStage {
        rate,
        transmissions: NonZeroU8::MIN,
        protection: Protection::None,
    })
}

/// The rate policy of the tests: one rate a kind; it counts what it
/// learned.
pub(super) struct Fixed<'id> {
    pub plan: RetryPlan<PortRate<'id, ModelCaps>>,
    pub single: Ppdu<'id, ModelCaps>,
    pub aggregate: Ppdu<'id, ModelCaps>,
    pub lessons: usize,
}

impl<'id> Rates<'id, ModelCaps> for Fixed<'id> {
    fn arena(&mut self, _peer: u8, plan: PpduPlan) -> Option<Ppdu<'id, ModelCaps>> {
        Some(match plan.kind {
            PpduKind::Single => self.single,
            PpduKind::Aggregate { .. } => self.aggregate,
        })
    }

    fn single(
        &mut self,
        _peer: u8,
        _tid: WmmUserPriority,
    ) -> Option<RetryPlan<PortRate<'id, ModelCaps>>> {
        Some(self.plan)
    }

    fn learn<A: Copy, B: Copy, D: Copy>(
        &mut self,
        _peer: u8,
        _lesson: RateLesson<A, B, D>,
        _now: Instant,
    ) {
        self.lessons += 1;
    }
}

/// What the engine reported, kept by value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Event {
    Lost(DataLoss),
    Settled(crate::events::AggregateSettled),
    Refused(IntakeRefusal),
}

/// The events the engine reported.
#[derive(Default)]
pub(super) struct Seen(pub Vec<Event>);

impl EventSink<DataEvent> for Seen {
    fn event(&mut self, event: DataEvent) {
        self.0.push(match event {
            DataEvent::DataLost(loss) => Event::Lost(loss),
            DataEvent::AggregateSettled(settled) => Event::Settled(settled),
            DataEvent::MsduRefused(reason) => Event::Refused(reason),
        });
    }
}

/// What the engine told the controller: TBTTs, the closed gate, the keys'
/// ends and the named TIDs' numbers.
pub(super) struct Controller<'id> {
    pub tbtts: Vec<TbttEvent>,
    pub gate: Option<GateClosed<'id>>,
    pub keys: Vec<KeyEnd>,
    pub named: Vec<(WmmUserPriority, SequenceNumber)>,
    /// The peer of each number told.
    pub named_peers: Vec<u8>,
    /// The frames read in place: the peer and the bytes.
    pub frames: Vec<(Option<u8>, Vec<u8>)>,
    /// The times a reply of the receive agreements was told owed.
    pub replies: u32,
    /// The receive agreements' commands told ended.
    pub agreements_ended: u32,
}

impl<'id> Told<'id> for Controller<'id> {
    fn tbtt(&mut self, event: TbttEvent) {
        self.tbtts.push(event);
    }

    fn gate_closed(&mut self, gate: GateClosed<'id>) {
        self.gate = Some(gate);
    }

    fn key_ended(&mut self, end: KeyEnd) {
        self.keys.push(end);
    }

    fn tid_named<const PEERS: usize>(
        &mut self,
        peer: Peer<PEERS>,
        tid: WmmUserPriority,
        number: SequenceNumber,
    ) {
        self.named_peers.push(peer.index());
        self.named.push((tid, number));
    }

    fn reply_owed(&mut self) {
        self.replies += 1;
    }

    fn agreement_ended(&mut self) {
        self.agreements_ended += 1;
    }

    fn frame<M: RxMetadata>(&mut self, peer: Option<u8>, frame: &Received<'_, M>) {
        self.frames.push((peer, frame.bytes.to_vec()));
    }
}

/// HT MCS 7 on 20 MHz.
pub(super) fn ht() -> PhyRate {
    PhyRate::Ht(
        HtRate::new(
            HtMcs::new(7).unwrap(),
            PpduBandwidth::Mhz20,
            HtGuardInterval::Normal,
        )
        .unwrap(),
    )
}

/// The rates the tests send at, from the port.
pub(super) fn fixed<'id, const S: usize, const D: usize>(
    port: &ModelPort<'id, S, D>,
) -> Fixed<'id> {
    let single = port.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap();
    let aggregate: AmpduRate<'id, ModelCaps> = port.ampdu_rate(port.rate(ht()).unwrap()).unwrap();
    Fixed {
        plan: once(single),
        single: Ppdu::Single(once(single)),
        aggregate: Ppdu::Aggregate {
            rate: aggregate,
            spacing: MpduStartSpacing::new(0).unwrap(),
            protection: Protection::None,
            alone: once(single),
        },
        lessons: 0,
    }
}

/// Write a station's open QoS link with `attempts` an aggregate member may
/// miss and a lifetime of `lifetime` into its places.
pub(super) fn station_link(attempts: u8, lifetime: Duration) -> StationLink<ModelLink> {
    StationLink {
        data: DataLink {
            header: StaDataHeaderTemplate::new(
                OwnAddress::new(IndividualAddress::new(STATION).unwrap()),
                Bssid::new(IndividualAddress::new(ACCESS_POINT_ADDRESS).unwrap()).unwrap(),
                PeerQos::Qos,
                LinkProtection::Open,
            )
            .unwrap(),
            admission: AcAdmission::default(),
            retry: Some(AggregateRetry { lifetime, attempts }),
            ht: Some(HtLimits {
                spacing: MpduStartSpacing::new(0).unwrap(),
                max_length: 65_535,
            }),
            windows: [None; 8],
            protection: Default::default(),
        },
        port: ModelLink { protected: false },
        numbers: fresh_numbers(),
    }
}

/// A link's numbers from zero, a space a TID.
fn fresh_numbers() -> TxNumbers {
    TxNumbers::new(
        SequenceCounter::new(SequenceNumber::ZERO),
        Some([SequenceCounter::new(SequenceNumber::ZERO); 8]),
    )
}

/// An access point's open link to `client`.
pub(super) fn client_link(client: [u8; 6]) -> ClientLink {
    ClientLink {
        client,
        access_point: ACCESS_POINT_ADDRESS,
        qos: PeerQos::Qos,
        numbers: fresh_numbers(),
    }
}

/// What a section is lent, held apart from the engine.
pub(super) struct Around<'id, const S: usize, const D: usize> {
    pub port: ModelPort<'id, S, D>,
    pub net: Net,
    pub rates: Fixed<'id>,
    pub seen: Seen,
    pub told: Controller<'id>,
    pub micros: u64,
}

impl<'id, const S: usize, const D: usize> Around<'id, S, D> {
    pub fn new(port: ModelPort<'id, S, D>) -> Self {
        let rates = fixed(&port);
        Self {
            port,
            net: Net::default(),
            rates,
            seen: Seen::default(),
            told: Controller {
                tbtts: Vec::new(),
                gate: None,
                keys: Vec::new(),
                named: Vec::new(),
                named_peers: Vec::new(),
                frames: Vec::new(),
                replies: 0,
                agreements_ended: 0,
            },
            micros: 1,
        }
    }

    /// Let `micros` pass.
    pub fn pass(&mut self, micros: u64) {
        self.micros += micros;
    }

    /// One section's turn, a microsecond after the last.
    pub fn turn(
        &mut self,
    ) -> Turn<'_, ModelPort<'id, S, D>, Net, Fixed<'id>, Seen, Controller<'id>> {
        self.micros += 1;
        Turn {
            port: &mut self.port,
            network: &mut self.net,
            rates: &mut self.rates,
            events: &mut self.seen,
            told: &mut self.told,
            now: Instant::from_micros(self.micros),
        }
    }
}

/// The station's engine's flowing data, which the test expects.
pub(super) fn station_flowing<'e, 'id, const POOL: usize>(
    engine: &'e mut StationEngine<'id, POOL>,
) -> FlowingData<'e, StationEngine<'id, POOL>> {
    match engine.data() {
        DataPlace::Flowing(flowing) => flowing,
        _ => panic!("the data flows"),
    }
}

/// An access point's engine's flowing data, which the test expects.
pub(super) fn ap_flowing<'e, 'id, const PEERS: usize, const POOL: usize>(
    engine: &'e mut ApEngine<'id, PEERS, POOL>,
) -> FlowingData<'e, ApEngine<'id, PEERS, POOL>> {
    match engine.data() {
        DataPlace::Flowing(flowing) => flowing,
        _ => panic!("the data flows"),
    }
}

/// Act on the station's linked access point.
pub(super) fn linked<'id, const POOL: usize>(
    engine: &mut StationEngine<'id, POOL>,
    act: impl FnOnce(&mut LinkedPeer<'_, StationEngine<'id, POOL>>),
) {
    let mut data = station_flowing(engine);
    let PeerPlace::Linked(mut linked) = data.peer(ACCESS_POINT) else {
        panic!("the peer is linked");
    };
    act(&mut linked);
}

/// Name `tid` of a link whose places hold nothing of it, and begin its
/// transmit agreement of `size` on the number told.
pub(super) fn agree<'id, const POOL: usize>(
    peer: &mut LinkedPeer<'_, StationEngine<'id, POOL>>,
    tid: WmmUserPriority,
    size: u16,
    around: &mut Around<'id, STATION_SINGLES, 0>,
) {
    let NamePlace::Unnamed(unnamed) = peer.naming() else {
        panic!("no TID is named");
    };
    let _ = unnamed.name(tid, &mut around.turn()).unwrap();
    let NamePlace::Named(named) = peer.naming() else {
        panic!("the TID's places hold nothing of it: its number is told at once");
    };
    let _ = named.tx_agreement(size, &mut around.turn());
}

/// A station's engine flowing, its gate open, linked, aggregating at HT
/// MCS 7.
fn station<'id, const POOL: usize>(
    run: Running<'id, STATION_SINGLES, 0>,
    engine: &mut StationEngine<'id, POOL>,
    attempts: u8,
    lifetime: Duration,
) -> Around<'id, STATION_SINGLES, 0> {
    let Running {
        port,
        data: rights,
        gate,
    } = run;
    let mut around = Around::new(port);
    let DataPlace::Stopped(data) = engine.data() else {
        panic!("a new engine's data is stopped");
    };
    let mut flowing = data.flows(rights);
    let GatePlace::Shut(shut) = flowing.gate() else {
        panic!("the gate is shut");
    };
    let _ = shut.open(gate, &mut around.turn());
    let PeerPlace::Free(free) = flowing.peer(ACCESS_POINT) else {
        panic!("the peer is free");
    };
    let _ = free.link(station_link(attempts, lifetime), &mut around.turn());
    engine.aggregate_rate(&AggregateRate {
        rate: ht(),
        max_subframes: 8,
        max_length: 65_535,
    });
    around
}

const HOUR: Duration = Duration::from_secs(3600);

/// The access point's pairwise key of material `byte`, with its transmit
/// key of packet numbers from one.
fn key_change(byte: u8) -> KeyChange<Key> {
    KeyChange {
        peer: IndividualAddress::new(ACCESS_POINT_ADDRESS).unwrap(),
        key: oer_protocols_ieee80211_contracts_security::Ccmp128Key::new(&[byte; 16]),
        spp: oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        transmit: InstalledCcmpTxKey::install(TestKey, CcmpPacketNumberStep::ONE),
    }
}

/// Ask the station's linked access point's key to change to `byte`'s.
fn change_key<'id, const POOL: usize>(
    engine: &mut StationEngine<'id, POOL>,
    around: &mut Around<'id, STATION_SINGLES, 0>,
    byte: u8,
) -> Rest {
    let mut data = station_flowing(engine);
    let PeerPlace::Linked(mut linked) = data.peer(ACCESS_POINT) else {
        panic!("the access point is linked");
    };
    let KeyPlace::Idle(idle) = linked.key() else {
        panic!("no command runs");
    };
    idle.change(key_change(byte), &mut around.turn())
}

/// Whether the station holds a transmit key: its port key is installed.
fn transmit_key<const POOL: usize>(engine: &StationEngine<'_, POOL>) -> bool {
    matches!(
        &engine.data,
        Data::Flowing(super::Running {
            interface: Interface::Idle {
                key: StationKey::Installed(..),
                ..
            },
            ..
        })
    )
}

/// Whether a key's command runs, as the key's view says.
fn key_changing<'id, const POOL: usize>(engine: &mut StationEngine<'id, POOL>) -> bool {
    let mut data = station_flowing(engine);
    let PeerPlace::Linked(mut linked) = data.peer(ACCESS_POINT) else {
        panic!("the access point is linked");
    };
    matches!(linked.key(), KeyPlace::Changing(_))
}

/// Mutant 5 of the model: a key change done runs the intake. What the
/// network offered while the key changed waits in its queue, and goes once
/// the installation ended.
#[test]
fn a_key_changes_behind_its_command_and_its_end_runs_the_intake() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        let _ = change_key(&mut engine, &mut around, 1);
        assert!(key_changing(&mut engine));
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        assert!(
            around.port.published().is_empty(),
            "nothing goes while the key changes"
        );
        assert_eq!(engine.queued(), 1);
        let _ = engine.round(&mut around.turn());
        assert_eq!(around.told.keys, [KeyEnd::Installed]);
        assert_eq!(around.port.published().len(), 1);
        assert_eq!(engine.queued(), 0);
        assert!(transmit_key(&engine), "the key is the link's");
    });
}

/// Mutants 6 and 19 of the model: no MSDU is taken under a key about to go,
/// and what the arenas hold unpublished when the key changes goes home. The
/// key's command waits for the arena on the air, and the arena settles at
/// its end.
#[test]
fn a_key_change_waits_for_the_air_and_sends_home_what_the_old_key_wrote() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        for id in 1..=2 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        // One arena on the air with 1, the standby written with 2.
        assert_eq!(around.port.published().len(), 1);
        let _ = change_key(&mut engine, &mut around, 1);
        around.net.offer(3, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        let _ = engine.round(&mut around.turn());
        // The key's command waits at the port's barrier; nothing is taken.
        assert!(key_changing(&mut engine));
        assert_eq!(engine.queued(), 1);
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        // 1 delivered, 2 written under the old key went home, 3 goes under
        // the new one.
        assert_eq!(around.told.keys, [KeyEnd::Installed]);
        assert_eq!(around.net.home, [1, 2]);
        assert_eq!(around.port.published().len(), 2);
        assert_eq!(engine.queued(), 0);
        // 2's number was never transmitted: 3 takes it.
        assert_eq!(first_sequence(&around, 1), 1);
    });
}

/// The sequence number of the first frame of the `index`th publication.
fn first_sequence<const S: usize, const D: usize>(around: &Around<'_, S, D>, index: usize) -> u16 {
    let front = &around.port.published()[index].frames[0];
    SequenceNumber::from_sequence_control(u16::from_le_bytes([front[22], front[23]])).get()
}

/// A frame that was transmitted and waited for its retry goes home at a
/// key's end: its number was seen, so a BlockAckReq to the window's new
/// head is owed.
#[test]
fn a_key_change_owes_a_block_ack_request_for_a_transmitted_frame_it_sent_home() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 2, &mut around);
        });
        for id in 1..=2 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        assert_eq!(around.port.published().len(), 1);
        let _ = change_key(&mut engine, &mut around, 1);
        around.port.set_air(BEST_EFFORT, Air::NoAck);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        // Both missed and were kept for a retry; the key's end sent them
        // home.
        assert_eq!(around.told.keys, [KeyEnd::Installed]);
        assert_eq!(around.net.home, [1, 2]);
        let mut owed = None;
        linked(&mut engine, |peer| {
            owed = peer.bar_owed(WmmUserPriority::UP0)
        });
        assert_eq!(owed.map(SequenceNumber::get), Some(2));
    });
}

/// A key change that sends home a transmitted frame and a never-transmitted
/// one sets the counter back over the latter and owes a BlockAckReq to it:
/// the peer's window moves past what went and waits at the next number
/// drawn. Once the BlockAckReq went, it is owed no more.
#[test]
fn a_key_change_owes_a_block_ack_request_to_the_number_it_set_back_and_it_ends_once_sent() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 4, &mut around);
        });
        for id in 1..=2 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        around.net.offer(3, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        // 1 and 2 (numbers 0 and 1) on the air, 3 (number 2) in the standby.
        assert_eq!(around.port.published().len(), 1);
        let _ = change_key(&mut engine, &mut around, 1);
        around.port.set_air(BEST_EFFORT, Air::NoAck);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.told.keys, [KeyEnd::Installed]);
        let mut home = around.net.home.clone();
        home.sort_unstable();
        assert_eq!(home, [1, 2, 3]);
        linked(&mut engine, |peer| {
            let owed = peer.bar_owed(WmmUserPriority::UP0);
            assert_eq!(owed.map(SequenceNumber::get), Some(2));
            peer.bar_sent(WmmUserPriority::UP0);
            assert_eq!(peer.bar_owed(WmmUserPriority::UP0), None);
        });
        // The next MSDU draws the number set back to.
        around.net.offer(4, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        let front = around.port.front(4).unwrap();
        let control = u16::from_le_bytes([front[22], front[23]]);
        assert_eq!(SequenceNumber::from_sequence_control(control).get(), 2);
    });
}

/// A section whose budget ran out owes no round for a named TID's queue:
/// nothing takes from it until its name ends.
#[test]
fn a_named_tids_queue_owes_no_round() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(1, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            let NamePlace::Unnamed(unnamed) = peer.naming() else {
                panic!("no TID is named");
            };
            let _ = unnamed.name(WmmUserPriority::UP0, &mut around.turn());
        });
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        around.net.offer(2, WmmUserPriority::UP3, DESTINATION);
        // 2 took the budget; 1 waits for the name's end.
        assert_eq!(engine.offered(&mut around.turn()), Rest::Wake(None));
        assert_eq!(engine.queued(), 1);
    });
}

/// A whole aggregate's end puts the standby on the air in its own turn, as
/// it was written; an MSDU its window held joins the next standby. While
/// the window holds the standby's TID, the intake ends at once and owes
/// nothing.
#[test]
fn a_whole_aggregates_end_puts_the_standby_on_the_air_as_it_was_written() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 3, &mut around);
        });
        for id in 1..=2 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        around.net.offer(3, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        // 1 and 2 (numbers 0 and 1) on the air, 3 (number 2) in the
        // standby; 4 (number 3) is beyond the window.
        // Voice waits for the standby's publication too: the intake ends
        // at once.
        around.net.offer(4, WmmUserPriority::UP0, DESTINATION);
        around.net.offer(5, WmmUserPriority::UP6, DESTINATION);
        assert_eq!(engine.offered(&mut around.turn()), Rest::Wake(None));
        assert_eq!(engine.queued(), 2);
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let published = around.port.published();
        assert_eq!(published.len(), 2);
        assert_eq!(published[0].frames.len(), 2);
        assert_eq!(published[1].frames.len(), 1, "3 went alone");
        assert_eq!(engine.queued(), 1, "4 took the next standby");
    });
}

/// A named TID's MSDU does not join the standby that holds the TID's
/// frames: it waits for the name's end.
#[test]
fn a_named_tids_msdu_does_not_join_its_standby() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 8, &mut around);
        });
        for id in 1..=2 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
            let _ = engine.offered(&mut around.turn());
        }
        // 1 on the air, 2 in the standby, which takes more of UP0; UP0 is
        // named again.
        linked(&mut engine, |peer| {
            let NamePlace::Unnamed(unnamed) = peer.naming() else {
                panic!("no TID is named");
            };
            let _ = unnamed.name(WmmUserPriority::UP0, &mut around.turn());
        });
        around.net.offer(3, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        assert_eq!(engine.queued(), 1);
        assert!(around.port.front(3).is_none(), "3 was not written");
    });
}

/// An arena kept for a retry goes on the air before the standby, and its
/// aggregate's settlement reports the rate it went at.
#[test]
fn an_arena_kept_for_a_retry_goes_before_the_standby() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 8, &mut around);
        });
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        for id in 2..=3 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        // 2 and 3 on the air in the second arena; 4 in the first, the
        // standby.
        around.net.offer(4, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        around.port.set_air(BEST_EFFORT, Air::Ack);
        around.port.acknowledge(FrameSet::prefix(1));
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let published = around.port.published();
        assert_eq!(published.len(), 3);
        assert_eq!(published[1].frames.len(), 2);
        // 3, missed, goes again before 4.
        let retry = &published[2].frames;
        assert_eq!(retry.len(), 1);
        assert_ne!(retry[0][1] & 0x08, 0, "a retry");
        assert!(around.seen.0.iter().any(|event| matches!(
            event,
            Event::Settled(settled) if settled.members == 2 && settled.rate == Some(ht())
        )));
        // The retry's answer is 3's: it goes home.
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert!(around.net.home.contains(&3));
    });
}

/// An aggregate of a link that ended keeps nothing for a retry: what it
/// missed goes home.
#[test]
fn an_aggregate_of_an_ended_link_sends_what_it_missed_home() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 8, &mut around);
        });
        for id in 1..=2 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        assert_eq!(around.port.published()[0].frames.len(), 2);
        {
            let mut data = station_flowing(&mut engine);
            let PeerPlace::Linked(peer) = data.peer(ACCESS_POINT) else {
                panic!("the access point is linked");
            };
            let _ = peer.unlink(&mut around.turn());
        }
        around.port.set_air(BEST_EFFORT, Air::NoAck);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let mut home = around.net.home.clone();
        home.sort_unstable();
        assert_eq!(home, [1, 2]);
        assert_eq!(around.port.published().len(), 1);
    });
}

/// A missed aggregate's retry holds only its missed frames, and its answer
/// settles them, not the frames acknowledged before.
#[test]
fn an_aggregates_retry_settles_its_missed_frames_alone() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 8, &mut around);
        });
        for id in 1..=3 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        assert_eq!(around.port.published()[0].frames.len(), 3);
        around.port.set_air(BEST_EFFORT, Air::Ack);
        around.port.acknowledge(FrameSet::prefix(1));
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.net.home, [1]);
        assert_eq!(around.port.published()[1].frames.len(), 2, "2 and 3 again");
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.net.home, [1, 2, 3]);
        assert!(around.seen.0.iter().any(|event| matches!(
            event,
            Event::Settled(settled) if settled.members == 2 && settled.acknowledged == 2
        )));
        assert_eq!(around.port.published().len(), 2);
    });
}

/// A missed aggregate's retry spends the attempts of its missed frames
/// alone: missed again, both have no attempt left and go home.
#[test]
fn an_aggregates_retry_spends_the_attempts_of_its_missed_frames() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 8, &mut around);
        });
        for id in 1..=3 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        around.port.set_air(BEST_EFFORT, Air::Ack);
        around.port.acknowledge(FrameSet::prefix(1));
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.port.published()[1].frames.len(), 2, "2 and 3 again");
        around.port.set_air(BEST_EFFORT, Air::Ack);
        around.port.acknowledge(FrameSet::from_bits(0));
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let mut home = around.net.home.clone();
        home.sort_unstable();
        assert_eq!(home, [1, 2, 3]);
        assert_eq!(around.port.published().len(), 2, "no third attempt");
    });
}

/// An aggregate of a link that ended keeps nothing for the next link: what
/// it missed goes home, and the new link sends none of it.
#[test]
fn an_aggregate_of_an_ended_link_keeps_nothing_for_the_next_link() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 8, &mut around);
        });
        for id in 1..=2 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        {
            let mut data = station_flowing(&mut engine);
            let PeerPlace::Linked(peer) = data.peer(ACCESS_POINT) else {
                panic!("the access point is linked");
            };
            let _ = peer.unlink(&mut around.turn());
        }
        {
            let mut data = station_flowing(&mut engine);
            let PeerPlace::Free(free) = data.peer(ACCESS_POINT) else {
                panic!("the access point is free");
            };
            let _ = free.link(station_link(2, HOUR), &mut around.turn());
        }
        around.port.set_air(BEST_EFFORT, Air::NoAck);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let mut home = around.net.home.clone();
        home.sort_unstable();
        assert_eq!(home, [1, 2]);
        assert_eq!(around.port.published().len(), 1);
    });
}

/// The standby's TID goes on first: an MSDU of another TID waits for the
/// standby's publication without taking the budget.
#[test]
fn the_standbys_tid_goes_on_and_another_waits_without_taking_the_budget() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(1, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 8, &mut around);
        });
        for id in 1..=2 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
            let _ = engine.offered(&mut around.turn());
        }
        // 1 on the air, 2 in the standby.
        around.net.offer(3, WmmUserPriority::UP6, DESTINATION);
        around.net.offer(4, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        assert!(around.port.front(4).is_some(), "4 joined the standby");
        assert!(around.port.front(3).is_none(), "voice waits");
    });
}

/// A link without QoS has no TID's space to name: the naming is refused and
/// nothing changes.
#[test]
fn a_link_without_qos_names_no_tid() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        {
            let mut data = station_flowing(&mut engine);
            let PeerPlace::Linked(peer) = data.peer(ACCESS_POINT) else {
                panic!("the access point is linked");
            };
            let _ = peer.unlink(&mut around.turn());
        }
        let mut link = station_link(2, HOUR);
        link.data.header = StaDataHeaderTemplate::new(
            OwnAddress::new(IndividualAddress::new(STATION).unwrap()),
            Bssid::new(IndividualAddress::new(ACCESS_POINT_ADDRESS).unwrap()).unwrap(),
            PeerQos::NonQos,
            LinkProtection::Open,
        )
        .unwrap();
        link.numbers = TxNumbers::new(SequenceCounter::new(SequenceNumber::ZERO), None);
        {
            let mut data = station_flowing(&mut engine);
            let PeerPlace::Free(free) = data.peer(ACCESS_POINT) else {
                panic!("the access point is free");
            };
            let _ = free.link(link, &mut around.turn());
        }
        linked(&mut engine, |peer| {
            let NamePlace::Unnamed(unnamed) = peer.naming() else {
                panic!("no TID is named");
            };
            assert!(matches!(
                unnamed.name(WmmUserPriority::UP0, &mut around.turn()),
                Err(NoSpace)
            ));
            assert!(matches!(peer.naming(), NamePlace::Unnamed(_)));
        });
    });
}

/// The station's own sequence space is taken one number at a time, apart
/// from its TIDs' spaces.
#[test]
fn the_stations_own_numbers_are_taken_one_by_one_apart_from_its_tids() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            assert_eq!(peer.own_number().map(SequenceNumber::get), Some(0));
            assert_eq!(peer.own_number().map(SequenceNumber::get), Some(1));
        });
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        let front = around.port.front(1).unwrap();
        let control = u16::from_le_bytes([front[22], front[23]]);
        assert_eq!(SequenceNumber::from_sequence_control(control).get(), 0);
    });
}

/// A section takes at most its budget of MSDUs from the queues, and owes a
/// round while more wait.
#[test]
fn a_section_takes_its_budget_and_owes_a_round_while_more_wait() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(2, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 8, &mut around);
        });
        for id in 1..=5 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        assert_eq!(engine.offered(&mut around.turn()), Rest::Round);
        assert_eq!(engine.queued(), 3);
        assert_eq!(engine.round(&mut around.turn()), Rest::Round);
        assert_eq!(engine.queued(), 1);
        assert_eq!(engine.round(&mut around.turn()), Rest::Wake(None));
        assert_eq!(engine.queued(), 0);
    });
}

/// A rekey is the installed key's removal and the new one's installation:
/// the controller is told the installation's end once.
#[test]
fn a_rekey_removes_the_installed_key_and_installs_the_new_one() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        let _ = change_key(&mut engine, &mut around, 1);
        let _ = engine.round(&mut around.turn());
        let _ = change_key(&mut engine, &mut around, 2);
        let _ = engine.round(&mut around.turn());
        // The removal ended; the installation runs.
        assert!(key_changing(&mut engine));
        let _ = engine.round(&mut around.turn());
        assert!(!key_changing(&mut engine));
        assert_eq!(around.told.keys, [KeyEnd::Installed, KeyEnd::Installed]);
    });
}

/// A stop during a rekey's removal ends the link the new key was for: the
/// new key is not installed, the removal is told, and the data stops.
#[test]
fn a_stop_during_a_rekey_installs_no_new_key() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        let _ = change_key(&mut engine, &mut around, 1);
        let _ = engine.round(&mut around.turn());
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        // The rekey's removal waits for the arena on the air.
        let _ = change_key(&mut engine, &mut around, 2);
        assert!(matches!(
            station_flowing(&mut engine).stop(&mut around.turn()),
            DataStop::Going
        ));
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let mut answer = DataStop::Going;
        for _ in 0..4 {
            let DataPlace::Stopping(stopping) = engine.data() else {
                break;
            };
            answer = stopping.poll(&mut around.turn());
        }
        assert!(matches!(answer, DataStop::Stopped(_)));
        assert_eq!(around.told.keys, [KeyEnd::Installed, KeyEnd::Removed]);
    });
}

/// An installation that expired leaves no key: nothing protected goes, and
/// the controller is told.
#[test]
fn an_expired_installation_leaves_the_link_without_a_key() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        around.port.expire_next();
        let _ = change_key(&mut engine, &mut around, 1);
        let _ = engine.round(&mut around.turn());
        assert_eq!(around.told.keys, [KeyEnd::Expired]);
        assert!(!transmit_key(&engine));
    });
}

/// Mutant 17 of the model: a stop does not leave a key's command waiting.
/// The stop waits for the key's end, removes the key, closes the gate, and
/// only then gives the rights back.
#[test]
fn a_stop_during_a_key_change_waits_for_it_removes_the_key_and_closes_the_gate() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        let _ = change_key(&mut engine, &mut around, 1);
        assert!(matches!(
            station_flowing(&mut engine).stop(&mut around.turn()),
            DataStop::Going
        ));
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.told.keys, [KeyEnd::Installed]);
        let mut answer = DataStop::Going;
        for _ in 0..4 {
            let DataPlace::Stopping(stopping) = engine.data() else {
                break;
            };
            answer = stopping.poll(&mut around.turn());
        }
        assert!(matches!(answer, DataStop::Stopped(_)));
        assert_eq!(around.told.keys, [KeyEnd::Installed, KeyEnd::Removed]);
        assert!(around.told.gate.is_some());
    });
}

#[test]
fn a_station_without_an_agreement_sends_one_mpdu_an_arena_and_delivers_in_order() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        for id in 1..=3 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        // One arena on the air, the other filled: one frame each.
        assert_eq!(around.port.published().len(), 1);
        assert_eq!(engine.queued(), 1);
        for _ in 0..3 {
            around.port.set_air(BEST_EFFORT, Air::Ack);
            let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        }
        assert_eq!(around.net.home, [1, 2, 3]);
        assert_eq!(around.rates.lessons, 3);
        assert!(
            around
                .port
                .published()
                .iter()
                .all(|went| went.frames.len() == 1)
        );
    });
}

#[test]
fn a_station_under_an_agreement_aggregates_within_its_window() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 2, &mut around);
        });
        for id in 1..=4 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        assert_eq!(around.port.published().len(), 1);
        for _ in 0..5 {
            around.port.set_air(BEST_EFFORT, Air::Ack);
            let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        }
        assert!(
            around
                .port
                .published()
                .iter()
                .all(|went| went.frames.len() <= 2)
        );
        assert_eq!(around.net.home, [1, 2, 3, 4]);
    });
}

#[test]
fn a_station_without_a_link_refuses_what_the_network_offers_at_once() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let Running {
            port, data: rights, ..
        } = run;
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = Around::new(port);
        let DataPlace::Stopped(data) = engine.data() else {
            panic!("stopped");
        };
        let _ = data.flows(rights);
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        assert_eq!(around.net.home, [1]);
        assert_eq!(around.seen.0, [Event::Refused(IntakeRefusal::NoLink)]);
    });
}

#[test]
fn a_held_link_keeps_its_data_until_released() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            let _ = peer.hold(&mut around.turn());
        });
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        assert!(around.port.published().is_empty());
        linked(&mut engine, |peer| {
            let _ = peer.release(&mut around.turn());
        });
        assert_eq!(around.port.published().len(), 1);
    });
}

/// The deleted actor's test of this name: a TID's named number (an ADDBA
/// Request's Starting Sequence Number) holds the TID's data until its
/// agreement begins, on that number, and the first data MPDU after carries
/// it.
#[test]
fn the_addba_starting_sequence_is_the_first_data_mpdu_numbered_after_it() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            let NamePlace::Unnamed(unnamed) = peer.naming() else {
                panic!("no TID is named");
            };
            let _ = unnamed.name(WmmUserPriority::UP0, &mut around.turn());
        });
        // Nothing of UP0 was in the arenas: its number is told at once.
        assert_eq!(
            around.told.named,
            [(WmmUserPriority::UP0, SequenceNumber::ZERO)]
        );
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        assert!(around.port.published().is_empty());
        linked(&mut engine, |peer| {
            let NamePlace::Named(named) = peer.naming() else {
                panic!("UP0's number was told");
            };
            assert_eq!(named.number(), SequenceNumber::ZERO);
            let _ = named.tx_agreement(2, &mut around.turn());
        });
        assert_eq!(around.port.published().len(), 1);
        // The first data MPDU after the name carries the named number.
        let front = around.port.front(1).unwrap();
        let control = u16::from_le_bytes([front[22], front[23]]);
        assert_eq!(SequenceNumber::from_sequence_control(control).get(), 0);
    });
}

/// A TID named while the arenas hold its frames is told its number only
/// once they have gone: nothing of it is below the number, and the first
/// data MPDU after carries it.
#[test]
fn a_named_tid_is_told_its_number_once_its_frames_in_the_arenas_have_gone() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        for id in 1..=2 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        // 1 (number 0) on the air, 2 (number 1) in the standby.
        linked(&mut engine, |peer| {
            let NamePlace::Unnamed(unnamed) = peer.naming() else {
                panic!("no TID is named");
            };
            let _ = unnamed.name(WmmUserPriority::UP0, &mut around.turn());
        });
        around.net.offer(3, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        assert!(
            around.told.named.is_empty(),
            "UP0's frames are in the arenas"
        );
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert!(around.told.named.is_empty(), "the standby holds UP0's 2");
        linked(&mut engine, |peer| {
            assert!(matches!(peer.naming(), NamePlace::Draining(_)));
        });
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.port.published().len(), 2, "3 waits in its queue");
        assert_eq!(
            around.told.named,
            [(WmmUserPriority::UP0, SequenceNumber::new(2).unwrap())]
        );
        linked(&mut engine, |peer| {
            let NamePlace::Named(named) = peer.naming() else {
                panic!("UP0's number was told");
            };
            let _ = named.tx_agreement(2, &mut around.turn());
        });
        let front = around.port.front(3).unwrap();
        let control = u16::from_le_bytes([front[22], front[23]]);
        assert_eq!(SequenceNumber::from_sequence_control(control).get(), 2);
    });
}

/// The deleted actor's `a_voice_frame_goes_ahead_of_a_best_effort_backlog`:
/// a voice MSDU behind a best-effort backlog goes in the next arena.
#[test]
fn a_voice_msdu_goes_in_the_next_arena_ahead_of_a_best_effort_backlog() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        for id in 1..=3 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        // 1 on the air, 2 in the standby, 3 queued behind them.
        around.net.offer(4, WmmUserPriority::UP6, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        // After the standby that was written, voice goes before the backlog.
        let third = &around.port.published()[2].frames[0];
        assert_eq!(third[24] & 0x07, 6, "the voice MSDU went third");
    });
}

/// The deleted actor's
/// `a_tid_held_by_its_window_does_not_hold_the_other_tid_of_its_category`:
/// a TID held by its window lets the other TID of its category take the
/// next arena, and goes once its window moves.
#[test]
fn a_tid_held_by_its_window_lets_the_other_tid_of_its_category_go() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 1, &mut around);
        });
        for id in 1..=2 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        around.net.offer(3, WmmUserPriority::UP3, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        // UP0's window of one holds 2 behind 1; UP3, best effort too, takes
        // the standby.
        assert_eq!(around.port.published().len(), 1);
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let second = &around.port.published()[1].frames[0];
        assert_eq!(second[24] & 0x07, 3, "the other TID went while UP0 waited");
        for _ in 0..2 {
            around.port.set_air(BEST_EFFORT, Air::Ack);
            let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        }
        // UP0's window moved at 1's end: 2 went last.
        assert_eq!(around.net.home, [1, 3, 2]);
    });
}

/// The deleted actor's `a_link_that_goes_returns_every_queued_msdu_once`:
/// a link that ends while its MSDUs wait (behind a window, behind the air)
/// sends each home exactly once.
#[test]
fn a_link_that_ends_sends_every_waiting_msdu_home_once() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        linked(&mut engine, |peer| {
            agree(peer, WmmUserPriority::UP0, 1, &mut around);
        });
        for id in 1..=4 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        around.port.set_air(BEST_EFFORT, Air::Hold);
        {
            let mut data = station_flowing(&mut engine);
            let PeerPlace::Linked(peer) = data.peer(ACCESS_POINT) else {
                panic!("the access point is linked");
            };
            let _ = peer.unlink(&mut around.turn());
        }
        // The queued ones went home at the unlink; the one on the air at
        // its end.
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let mut home = around.net.home.clone();
        home.sort_unstable();
        assert_eq!(home, [1, 2, 3, 4]);
        assert_eq!(engine.queued(), 0);
    });
}

#[test]
fn a_stop_with_an_arena_on_the_air_ends_once_it_ended_and_the_gate_closed() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        for id in 1..=3 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        around.port.set_air(BEST_EFFORT, Air::Hold);
        assert!(matches!(
            station_flowing(&mut engine).stop(&mut around.turn()),
            DataStop::Going
        ));
        // What waited and what the standby held went home at once.
        assert_eq!(around.net.home, [3, 2]);
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let DataPlace::Stopping(stopping) = engine.data() else {
            panic!("the data is stopping");
        };
        let DataStop::Stopped(stopped) = stopping.poll(&mut around.turn()) else {
            panic!("the air ended and the gate closed");
        };
        assert_eq!(around.net.home, [3, 2, 1]);
        assert!(around.told.gate.is_some());
        assert!(matches!(engine.data(), DataPlace::Stopped(_)));
        // The data flows again with its rights.
        let DataPlace::Stopped(data) = engine.data() else {
            panic!("the data is stopped");
        };
        let _ = data.flows(stopped);
        assert!(matches!(engine.data(), DataPlace::Flowing(_)));
    });
}

/// An access point's engine of `PEERS` flowing, its gate open, with two
/// clients linked.
fn access_point<'id, const PEERS: usize, const POOL: usize>(
    run: Running<'id, AP_SINGLES, DATA_SINGLES>,
    engine: &mut ApEngine<'id, PEERS, POOL>,
) -> Around<'id, AP_SINGLES, DATA_SINGLES> {
    let Running {
        port,
        data: rights,
        gate,
    } = run;
    let mut around = Around::new(port);
    let DataPlace::Stopped(data) = engine.data() else {
        panic!("a new engine's data is stopped");
    };
    let mut flowing = data.flows(rights);
    let GatePlace::Shut(shut) = flowing.gate() else {
        panic!("the gate is shut");
    };
    let _ = shut.open(gate, &mut around.turn());
    for (peer, client) in [(0, CLIENT_A), (1, CLIENT_B)] {
        let PeerPlace::Free(free) = flowing.peer(Peer::new(peer).unwrap()) else {
            panic!("the peer is free");
        };
        let _ = free.link(client_link(client), &mut around.turn());
    }
    around
}

/// The data place of `category` in an access point's layout: after the
/// controller's, in QUEUES' order.
fn data_place(category: WmmAccessCategory) -> u8 {
    let offset = oer_protocols_ieee80211_contracts_port::QUEUES
        .iter()
        .position(|queued| *queued == category)
        .unwrap();
    (AP_SINGLES + offset) as u8
}

#[test]
fn an_access_points_msdu_is_copied_into_its_queues_place_and_its_memory_goes_home_at_the_write() {
    running::<AP_SINGLES, DATA_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4, POLICY);
        let mut around = access_point(run, &mut engine);
        around.net.offer(7, WmmUserPriority::UP0, CLIENT_B);
        let _ = engine.offered(&mut around.turn());
        // Home at the write, before its end.
        assert_eq!(around.net.home, [7]);
        let single = around
            .port
            .single(data_place(BEST_EFFORT))
            .unwrap()
            .to_vec();
        // A QoS data frame from the DS: Address 1 the client, 2 the access
        // point, 3 the source; then LLC/SNAP and the payload, copied.
        assert_eq!(single[0], 0x88);
        assert_eq!(single[1] & 0x03, 0x02);
        assert_eq!(single[4..10], CLIENT_B);
        assert_eq!(single[10..16], ACCESS_POINT_ADDRESS);
        assert_eq!(single[16..22], STATION);
        let payload = &around.net.frames[0].2[ETHERNET..];
        assert_eq!(&single[26 + 8..], payload);
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.rates.lessons, 1);
    });
}

/// An access point's client's named TID is told its number once its single
/// on the air ended, and its next data MPDU carries it.
#[test]
fn an_access_points_named_tid_is_told_its_number_once_its_single_ended() {
    running::<AP_SINGLES, DATA_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4, POLICY);
        let mut around = access_point(run, &mut engine);
        let client = Peer::new(1).unwrap();
        around.net.offer(1, WmmUserPriority::UP0, CLIENT_B);
        let _ = engine.offered(&mut around.turn());
        {
            let mut data = ap_flowing(&mut engine);
            let PeerPlace::Linked(mut peer) = data.peer(client) else {
                panic!("the client is linked");
            };
            let NamePlace::Unnamed(unnamed) = peer.naming() else {
                panic!("no TID is named");
            };
            let _ = unnamed.name(WmmUserPriority::UP0, &mut around.turn());
        }
        around.net.offer(2, WmmUserPriority::UP0, CLIENT_B);
        let _ = engine.offered(&mut around.turn());
        assert!(around.told.named.is_empty(), "its single is on the air");
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(
            around.told.named,
            [(WmmUserPriority::UP0, SequenceNumber::new(1).unwrap())]
        );
        assert_eq!(around.told.named_peers, [1], "the client's number");
        assert_eq!(engine.queued(), 1, "2 waits for the name's end");
        {
            let mut data = ap_flowing(&mut engine);
            let PeerPlace::Linked(mut peer) = data.peer(client) else {
                panic!("the client is linked");
            };
            let NamePlace::Named(named) = peer.naming() else {
                panic!("its number was told");
            };
            let _ = named.release(&mut around.turn());
        }
        let single = around.port.single(data_place(BEST_EFFORT)).unwrap();
        let control = u16::from_le_bytes([single[22], single[23]]);
        assert_eq!(SequenceNumber::from_sequence_control(control).get(), 1);
        // The other client's TID, with nothing on the air, is told at once,
        // under its own peer.
        {
            let mut data = ap_flowing(&mut engine);
            let PeerPlace::Linked(mut peer) = data.peer(Peer::new(0).unwrap()) else {
                panic!("the client is linked");
            };
            let NamePlace::Unnamed(unnamed) = peer.naming() else {
                panic!("no TID is named");
            };
            let _ = unnamed.name(WmmUserPriority::UP0, &mut around.turn());
        }
        assert_eq!(around.told.named_peers, [1, 0]);
    });
}

#[test]
fn an_access_points_queue_carries_one_single_and_the_next_waits_for_its_end() {
    running::<AP_SINGLES, DATA_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4, POLICY);
        let mut around = access_point(run, &mut engine);
        around.net.offer(1, WmmUserPriority::UP0, CLIENT_A);
        around.net.offer(2, WmmUserPriority::UP0, CLIENT_B);
        around.net.offer(3, WmmUserPriority::UP6, CLIENT_B);
        let _ = engine.offered(&mut around.turn());
        // Voice and best effort each carry one; the second best effort
        // waits in its queue.
        assert_eq!(around.net.home, [3, 1]);
        assert_eq!(engine.queued(), 1);
        around.port.set_air(BEST_EFFORT, Air::NoAck);
        around.port.set_air(WmmAccessCategory::Voice, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.net.home, [3, 1, 2]);
        assert_eq!(engine.queued(), 0);
        assert!(around.seen.0.contains(&Event::Lost(DataLoss::RetryLimit)));
    });
}

#[test]
fn an_access_point_refuses_an_msdu_for_a_station_it_has_no_link_with() {
    running::<AP_SINGLES, DATA_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4, POLICY);
        let mut around = access_point(run, &mut engine);
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        assert_eq!(around.net.home, [1]);
        assert_eq!(around.seen.0, [Event::Refused(IntakeRefusal::NoLink)]);
    });
}

/// A link's accounts start anew at the next link of its peer: a single on
/// the air when the link ended charges them at its end, after the unlink.
#[test]
fn a_second_link_of_a_peer_starts_without_the_first_links_account() {
    running::<AP_SINGLES, DATA_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4, POLICY);
        let mut around = access_point(run, &mut engine);
        let queue = QueueId::new(0, WmmUserPriority::UP0).unwrap();
        around.net.offer(1, WmmUserPriority::UP0, CLIENT_A);
        around.port.set_air(BEST_EFFORT, Air::Hold);
        let _ = engine.offered(&mut around.turn());
        {
            let mut data = ap_flowing(&mut engine);
            let PeerPlace::Linked(linked) = data.peer(Peer::new(0).unwrap()) else {
                panic!("the client is linked");
            };
            let _ = linked.unlink(&mut around.turn());
        }
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert!(
            engine.queues.air(queue) > 0,
            "the end charged the ended link"
        );
        {
            let mut data = ap_flowing(&mut engine);
            let PeerPlace::Free(free) = data.peer(Peer::new(0).unwrap()) else {
                panic!("the client is free");
            };
            let _ = free.link(client_link(CLIENT_A), &mut around.turn());
        }
        assert_eq!(engine.queues.air(queue), 0);
    });
}

#[test]
fn an_access_points_stop_waits_for_its_singles_and_every_right_comes_home() {
    running::<AP_SINGLES, DATA_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4, POLICY);
        let mut around = access_point(run, &mut engine);
        around.net.offer(1, WmmUserPriority::UP0, CLIENT_A);
        around.net.offer(2, WmmUserPriority::UP0, CLIENT_A);
        around.port.set_air(BEST_EFFORT, Air::Hold);
        let _ = engine.offered(&mut around.turn());
        assert!(matches!(
            ap_flowing(&mut engine).stop(&mut around.turn()),
            DataStop::Going
        ));
        assert_eq!(around.net.home, [1, 2]);
        assert!(
            around.port.single(data_place(BEST_EFFORT)).is_some(),
            "1 is on the air"
        );
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let DataPlace::Stopping(stopping) = engine.data() else {
            panic!("the data is stopping");
        };
        let DataStop::Stopped(_rights) = stopping.poll(&mut around.turn()) else {
            panic!("the single ended and the gate closed");
        };
        assert!(matches!(engine.data(), DataPlace::Stopped(_)));
    });
}

/// The engine's host size for an access point's clients: a figure for the
/// report, not a gate (`tasks/114`, O8).
#[test]
fn the_access_points_engine_size_at_two_and_fifteen_clients() {
    let two = core::mem::size_of::<ApEngine<'static, 2, 32>>();
    let fifteen = core::mem::size_of::<ApEngine<'static, 15, 32>>();
    let station = core::mem::size_of::<StationEngine<'static, 32>>();
    std::println!(
        "engine size: access point at 2 clients {two}, at 15 {fifteen}; station {station}"
    );
    assert!(two < fifteen);
}

/// The host sizes of the values built into a variant, for the audit of
/// copies and frames (`tasks/126`): the link at an association, the plane
/// at the data's start, the running data at its stop.
#[test]
fn the_sizes_of_the_values_built_into_a_variant() {
    use core::mem::size_of;
    type Station = super::Running<
        'static,
        Arenas,
        ModelCaps,
        Key,
        STATION_SINGLES,
        MODEL_GROUPS,
        MODEL_AGREEMENTS,
    >;
    type Ap = super::Running<
        'static,
        Singles,
        ModelCaps,
        Key,
        AP_SINGLES,
        MODEL_GROUPS,
        MODEL_AGREEMENTS,
    >;
    std::println!(
        "copies: DataLink {}, StationLink {}, StationEntry {}, ClientLink {}, DataPlane {}, \
         station running {}, access point running {}",
        size_of::<DataLink>(),
        size_of::<StationLink<ModelLink>>(),
        size_of::<StationEntry<ModelLink>>(),
        size_of::<ClientLink>(),
        size_of::<DataPlane<'static, ModelCaps>>(),
        size_of::<Station>(),
        size_of::<Ap>(),
    );
    assert!(size_of::<DataPlane<'static, ModelCaps>>() <= size_of::<Station>());
}

/// The lifetime: the search leaves it out (its paths last microseconds, the
/// lifetime an hour), so these cases hold it. A station whose members live
/// a millisecond and may miss three aggregates, under an agreement of
/// four, its three MSDUs offered while its link was held and released
/// together: they go as one aggregate. A miss of one frame is its end
/// whatever its age (it went as one MPDU by its retry plan), so the cases
/// keep two frames to retry.
fn aggregating<'id>(
    run: Running<'id, STATION_SINGLES, 0>,
    engine: &mut StationEngine<'id, 8>,
    before: impl FnOnce(&mut Around<'id, STATION_SINGLES, 0>),
) -> Around<'id, STATION_SINGLES, 0> {
    let mut around = station(run, engine, 3, Duration::from_millis(1));
    linked(engine, |peer| {
        agree(peer, WmmUserPriority::UP0, 4, &mut around);
        let _ = peer.hold(&mut around.turn());
    });
    around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
    around.net.offer(2, WmmUserPriority::UP0, DESTINATION);
    around.net.offer(3, WmmUserPriority::UP0, DESTINATION);
    let _ = engine.offered(&mut around.turn());
    before(&mut around);
    linked(engine, |peer| {
        let _ = peer.release(&mut around.turn());
    });
    assert_eq!(around.port.published().len(), 1);
    assert_eq!(around.port.published()[0].frames.len(), 3);
    around
}

/// The BlockAck acknowledges the first frame only.
fn first_only<const S: usize, const D: usize>(around: &mut Around<'_, S, D>) {
    around.port.set_air(BEST_EFFORT, Air::Ack);
    around.port.acknowledge(FrameSet::prefix(1));
}

fn expired(around: &Around<'_, STATION_SINGLES, 0>) -> usize {
    around
        .seen
        .0
        .iter()
        .filter(|event| **event == Event::Lost(DataLoss::Expired))
        .count()
}

#[test]
fn a_member_missed_within_its_lifetime_is_kept_and_one_missed_after_it_is_dropped_expired() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = aggregating(run, &mut engine, |_| {});
        // Within their lifetime: the missed members are kept and go again.
        first_only(&mut around);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.net.home, [1]);
        assert_eq!(around.port.published().len(), 2);
        // Missed again after their lifetime: dropped, expired, home.
        around.pass(2_000);
        around.port.set_air(BEST_EFFORT, Air::NoAck);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.net.home, [1, 2, 3]);
        assert_eq!(expired(&around), 2);
        assert_eq!(around.port.published().len(), 2);
    });
}

/// A queued MSDU's lifetime starts when an arena takes it, not when the
/// network offered it: the time it waited in its queue is not counted.
#[test]
fn a_member_that_waited_in_its_queue_past_its_lifetime_still_has_its_lifetime_in_the_arena() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        // The three waited in their queue, held, longer than they live.
        let mut around = aggregating(run, &mut engine, |around| around.pass(5_000));
        first_only(&mut around);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        // The missed ones are kept for their retry, not dropped expired.
        assert_eq!(around.net.home, [1]);
        assert_eq!(expired(&around), 0);
        assert_eq!(around.port.published().len(), 2);
    });
}

/// A kept member that outlives its lifetime while the gate closes and stays
/// closed is not dropped then: the lifetime is read at a settlement. It
/// goes once more when the gate opens, and that miss drops it expired.
#[test]
fn a_kept_member_that_ages_while_the_gate_closes_goes_once_more_and_its_miss_expires_it() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4, POLICY);
        let mut around = aggregating(run, &mut engine, |_| {});
        // The gate's closing is asked while the aggregate is on the air.
        {
            let mut data = station_flowing(&mut engine);
            let GatePlace::Open(open) = data.gate() else {
                panic!("the gate is open");
            };
            let _ = open.close(&mut around.turn());
        }
        first_only(&mut around);
        // Its end settles; the closing's poll ends with the air quiet.
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let gate = around.told.gate.take().unwrap();
        assert_eq!(around.net.home, [1]);
        around.pass(5_000);
        let _ = engine.round(&mut around.turn());
        assert_eq!(around.net.home, [1]);
        {
            let mut data = station_flowing(&mut engine);
            let GatePlace::Shut(shut) = data.gate() else {
                panic!("the gate is shut");
            };
            let _ = shut.open(gate, &mut around.turn());
        }
        assert_eq!(around.port.published().len(), 2);
        around.port.set_air(BEST_EFFORT, Air::NoAck);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.net.home, [1, 2, 3]);
        assert_eq!(expired(&around), 2);
    });
}

/// A single that ends while the gate's closing is polled comes back through
/// the poll's sink: its place and its queue's right are home again, and the
/// stop gives the proof back.
#[test]
fn a_single_that_ends_during_the_commands_poll_gives_its_place_and_queue_back() {
    running::<AP_SINGLES, DATA_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4, POLICY);
        let mut around = access_point(run, &mut engine);
        around.port.set_air(BEST_EFFORT, Air::Hold);
        around.net.offer(1, WmmUserPriority::UP0, CLIENT_A);
        let _ = engine.offered(&mut around.turn());
        {
            let mut data = ap_flowing(&mut engine);
            let GatePlace::Open(open) = data.gate() else {
                panic!("the gate is open");
            };
            let _ = open.close(&mut around.turn());
        }
        // The single ends while the closing is polled, before the barrier.
        around.port.set_air(BEST_EFFORT, Air::Ack);
        around.port.serve_in_next_poll();
        let _ = engine.round(&mut around.turn());
        assert_eq!(around.rates.lessons, 1);
        assert!(around.told.gate.is_some());
        assert!(matches!(
            ap_flowing(&mut engine).stop(&mut around.turn()),
            DataStop::Stopped(_)
        ));
        assert!(matches!(engine.data(), DataPlace::Stopped(_)));
    });
}

/// Two rests together: a round before any wake, else the earlier instant.
#[test]
fn of_two_rests_the_round_or_the_earlier_wake_comes() {
    let at = |micros| Rest::Wake(Some(Instant::from_micros(micros)));
    assert_eq!(super::earliest(at(5), at(3)), at(3));
    assert_eq!(super::earliest(at(3), at(5)), at(3));
    assert_eq!(super::earliest(at(4), at(4)), at(4));
    assert_eq!(super::earliest(at(4), Rest::Wake(None)), at(4));
    assert_eq!(super::earliest(Rest::Wake(None), at(4)), at(4));
    assert_eq!(
        super::earliest(Rest::Wake(None), Rest::Wake(None)),
        Rest::Wake(None)
    );
    assert_eq!(super::earliest(at(4), Rest::Round), Rest::Round);
}
