//! The engine over the model port: its cases, its sizes and the search of
//! every order of its sections (`search`).

use super::*;
use crate::AggregateRetry;
use crate::ETHERNET_HEADER_LEN;
use crate::model::{Running, running};
use crate::plane::{AggregateRate, HtLimits, PpduKind};
use core::num::NonZeroU8;
use oer_portable_time::Duration;
use oer_protocols_ieee80211_contracts_port::{
    AmpduRate, FrameSet, HEADROOM, MpduStartSpacing, Ppdu, Protection, RetryStage,
};
use oer_protocols_ieee80211_core_upper_mac::ccmp::{InstalledCcmpTxKey, SoftwareCcmpKey};
use oer_protocols_ieee80211_core_upper_mac::qos::AcAdmission;
use oer_protocols_ieee80211_core_upper_mac::sequence::SequenceCounter;
use oer_protocols_ieee80211_testing_model::{
    Air, MODEL_AGREEMENTS, MODEL_GROUPS, ModelCaps, ModelLink, ModelMemory, ModelPort,
};
use oer_protocols_ieee80211_wire::address::{Bssid, OwnAddress};
use oer_protocols_ieee80211_wire::phy::{
    HtGuardInterval, HtMcs, HtRate, LegacyRate, PhyRate, PpduBandwidth,
};
use oer_protocols_ieee80211_wire::security::LinkProtection;
use oer_protocols_ieee80211_wire::station::StaDataHeaderTemplate;
use std::collections::VecDeque;
use std::vec::Vec;

mod search;

pub(super) const STATION: [u8; 6] = [2, 0, 0, 0, 0, 1];
pub(super) const ACCESS_POINT_ADDRESS: [u8; 6] = [2, 0, 0, 0, 0, 2];
pub(super) const CLIENT_A: [u8; 6] = [2, 0, 0, 0, 0, 0x0a];
pub(super) const CLIENT_B: [u8; 6] = [2, 0, 0, 0, 0, 0x0b];
pub(super) const DESTINATION: [u8; 6] = [2, 0, 0, 0, 0, 0x10];
pub(super) const PAYLOAD: usize = 100;
pub(super) const BEST_EFFORT: WmmAccessCategory = WmmAccessCategory::BestEffort;

/// A station's single's places in the tests.
pub(super) const STATION_SINGLES: usize = 8;
/// An access point's: seventeen of its controller's and its data's.
pub(super) const AP_SINGLES: usize = access_point_singles::<17>();

/// Key material of a test key, which software alone owns.
pub(super) struct TestKey;

impl SoftwareCcmpKey for TestKey {}

pub(super) type Key = InstalledCcmpTxKey<TestKey>;

/// A station's engine over the model port, with a pool of `POOL`.
pub(super) type StationEngine<'id, const POOL: usize> = Engine<
    'id,
    ModelPort<'id, STATION_SINGLES>,
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
    ModelPort<'id, AP_SINGLES>,
    Singles,
    Key,
    PEERS,
    POOL,
    AP_SINGLES,
    MODEL_GROUPS,
    MODEL_AGREEMENTS,
>;

/// The network's end: what it offers, each MSDU's Ethernet frame, and what
/// came home, in order.
#[derive(Default)]
pub(super) struct Net {
    pub offered: VecDeque<OutboundMsdu<ModelMemory>>,
    pub frames: Vec<(u32, Vec<u8>)>,
    pub home: Vec<u32>,
}

impl Net {
    /// Offer MSDU `id` of `priority` to `destination`.
    pub fn offer(&mut self, id: u32, priority: WmmUserPriority, destination: [u8; 6]) {
        let mut ethernet = Vec::new();
        ethernet.extend_from_slice(&destination);
        ethernet.extend_from_slice(&STATION);
        ethernet.extend_from_slice(&0x0800_u16.to_be_bytes());
        let mut front = [0; HEADROOM];
        front[HEADROOM - ETHERNET_HEADER_LEN..].copy_from_slice(&ethernet);
        let mut frame = ethernet;
        frame.extend((0..PAYLOAD).map(|octet| octet as u8 ^ id as u8));
        self.frames.push((id, frame));
        self.offered.push_back(OutboundMsdu {
            memory: ModelMemory {
                id,
                len: PAYLOAD as u16,
                front,
            },
            length: ETHERNET_HEADER_LEN + PAYLOAD,
            priority,
        });
    }
}

impl Network<ModelMemory> for Net {
    fn take(&mut self) -> Option<OutboundMsdu<ModelMemory>> {
        self.offered.pop_front()
    }

    fn frame<'m>(&'m self, memory: &'m ModelMemory, length: usize) -> &'m [u8] {
        self.frames
            .iter()
            .find(|(id, _)| *id == memory.id)
            .map_or(&[], |(_, frame)| &frame[..length.min(frame.len())])
    }

    fn home(&mut self, memory: ModelMemory) {
        self.home.push(memory.id);
    }
}

/// The rate policy of the tests: one rate a kind; it counts what it learned.
pub(super) struct Fixed<'id> {
    pub single: PortRate<'id, ModelCaps>,
    pub aggregate: AmpduRate<'id, ModelCaps>,
    pub lessons: usize,
}

/// One transmission at `rate`.
pub(super) fn once<R>(rate: R) -> RetryPlan<R> {
    RetryPlan::new(RetryStage {
        rate,
        transmissions: NonZeroU8::MIN,
        protection: Protection::None,
    })
}

impl<'id> Rates<'id, ModelCaps> for Fixed<'id> {
    fn arena(&mut self, _peer: u8, plan: PpduPlan) -> Option<Ppdu<'id, ModelCaps>> {
        Some(match plan.kind {
            PpduKind::Single => Ppdu::Single(once(self.single)),
            PpduKind::Aggregate { spacing } => Ppdu::Aggregate {
                rate: self.aggregate,
                spacing,
                protection: Protection::None,
                alone: once(self.single),
            },
        })
    }

    fn single(
        &mut self,
        _peer: u8,
        _tid: WmmUserPriority,
    ) -> Option<RetryPlan<PortRate<'id, ModelCaps>>> {
        Some(once(self.single))
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

/// What the engine told the controller.
#[derive(Default)]
pub(super) struct Controller(pub Vec<TbttEvent>);

impl Told for Controller {
    fn tbtt(&mut self, event: TbttEvent) {
        self.0.push(event);
    }
}

/// The events the engine reported.
#[derive(Default)]
pub(super) struct Seen(pub Vec<DataEvent>);

impl EventSink<DataEvent> for Seen {
    fn event(&mut self, event: DataEvent) {
        self.0.push(event);
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
pub(super) fn fixed<'id, const S: usize>(port: &ModelPort<'id, S>) -> Fixed<'id> {
    Fixed {
        single: port.rate(PhyRate::Legacy(LegacyRate::Ofdm24M)).unwrap(),
        aggregate: port.ampdu_rate(port.rate(ht()).unwrap()).unwrap(),
        lessons: 0,
    }
}

/// A station's open QoS link with `attempts` an aggregate member may miss.
pub(super) fn station_link(attempts: u8) -> StationLink<ModelLink, Key> {
    station_link_living(attempts, Duration::from_secs(3600))
}

/// The same, a member dropped once its MSDU is `lifetime` old.
pub(super) fn station_link_living(attempts: u8, lifetime: Duration) -> StationLink<ModelLink, Key> {
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
        numbers: TxNumbers::new(
            SequenceCounter::new(SequenceNumber::ZERO),
            Some([SequenceCounter::new(SequenceNumber::ZERO); 8]),
            None,
        ),
    }
}

/// An access point's open link to `client`.
pub(super) fn client_link(client: [u8; 6], qos: PeerQos) -> ClientLink<Key> {
    ClientLink {
        client: IndividualAddress::new(client).unwrap(),
        access_point: IndividualAddress::new(ACCESS_POINT_ADDRESS).unwrap(),
        qos,
        numbers: TxNumbers::new(
            SequenceCounter::new(SequenceNumber::ZERO),
            Some([SequenceCounter::new(SequenceNumber::ZERO); 8]),
            None,
        ),
    }
}

/// What a section is lent, held apart from the engine.
pub(super) struct Around<'id, const S: usize> {
    pub port: ModelPort<'id, S>,
    pub net: Net,
    pub rates: Fixed<'id>,
    pub seen: Seen,
    pub told: Controller,
    pub micros: u64,
}

impl<'id, const S: usize> Around<'id, S> {
    pub fn new(port: ModelPort<'id, S>) -> Self {
        let rates = fixed(&port);
        Self {
            port,
            net: Net::default(),
            rates,
            seen: Seen::default(),
            told: Controller::default(),
            micros: 1,
        }
    }

    /// One section's turn, a microsecond after the last.
    /// Let `micros` pass.
    pub fn pass(&mut self, micros: u64) {
        self.micros += micros;
    }

    pub fn turn(&mut self) -> Turn<'_, ModelPort<'id, S>, Net, Fixed<'id>, Seen, Controller> {
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

/// A station's engine flowing, its gate open, linked, aggregating at HT
/// MCS 7.
fn station<'id, const POOL: usize>(
    running: Running<'id, STATION_SINGLES>,
    engine: &mut StationEngine<'id, POOL>,
) -> Around<'id, STATION_SINGLES> {
    let Running { port, data, gate } = running;
    let mut around = Around::new(port);
    let _ = engine.flows(data);
    engine.open_gate(gate, &mut around.turn()).ok().unwrap();
    engine.aggregate_rate(Some(AggregateRate {
        rate: ht(),
        max_subframes: 8,
        max_length: 65_535,
    }));
    engine
        .link(ACCESS_POINT, station_link(2), &mut around.turn())
        .ok()
        .unwrap();
    around
}

#[test]
fn a_station_without_an_agreement_sends_one_mpdu_an_arena_and_delivers_in_order() {
    running::<STATION_SINGLES, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4);
        let mut around = station(run, &mut engine);
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
    running::<STATION_SINGLES, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8);
        let mut around = station(run, &mut engine);
        engine
            .asked(
                Request::TxAgreement(ACCESS_POINT, WmmUserPriority::UP0, 2),
                &mut around.turn(),
            )
            .unwrap();
        // The gate closed while the arena fills: nothing goes.
        for id in 1..=4 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        // The first went alone at once; the window of two lets one more
        // join the standby, the others wait for the window.
        let first = around.port.published().len();
        assert_eq!(first, 1);
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert!(
            around
                .port
                .published()
                .iter()
                .all(|went| went.frames.len() <= 2)
        );
        for _ in 0..4 {
            around.port.set_air(BEST_EFFORT, Air::Ack);
            let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        }
        assert_eq!(around.net.home, [1, 2, 3, 4]);
    });
}

#[test]
fn a_station_without_a_link_refuses_what_the_network_offers_at_once() {
    running::<STATION_SINGLES, _>(|run| {
        let Running { port, data, gate } = run;
        let mut engine = StationEngine::<'_, 8>::new(4);
        let mut around = Around::new(port);
        let _ = engine.flows(data);
        engine.open_gate(gate, &mut around.turn()).ok().unwrap();
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        assert_eq!(around.net.home, [1]);
        assert_eq!(
            around.seen.0,
            [DataEvent::MsduRefused(IntakeRefusal::NoLink)]
        );
    });
}

#[test]
fn a_held_link_keeps_its_data_until_released() {
    running::<STATION_SINGLES, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4);
        let mut around = station(run, &mut engine);
        engine
            .asked(Request::Hold(ACCESS_POINT), &mut around.turn())
            .unwrap();
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        assert!(around.port.published().is_empty());
        engine
            .asked(Request::Release(ACCESS_POINT), &mut around.turn())
            .unwrap();
        assert_eq!(around.port.published().len(), 1);
    });
}

#[test]
fn a_named_number_holds_its_tid_until_the_frame_that_names_it_went() {
    running::<STATION_SINGLES, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4);
        let mut around = station(run, &mut engine);
        let named = engine
            .name_next(ACCESS_POINT, LinkSpace::Tid(WmmUserPriority::UP0))
            .unwrap();
        assert_eq!(named, SequenceNumber::ZERO);
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        assert!(around.port.published().is_empty());
        engine
            .asked(Request::Named(ACCESS_POINT), &mut around.turn())
            .unwrap();
        assert_eq!(around.port.published().len(), 1);
        // The data drew the number after the named one.
        let front = around.port.front(1).unwrap();
        let control = u16::from_le_bytes([front[22], front[23]]);
        assert_eq!(SequenceNumber::from_sequence_control(control).get(), 1);
    });
}

#[test]
fn a_stop_with_an_arena_on_the_air_ends_once_it_ended_and_the_gate_closed() {
    running::<STATION_SINGLES, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4);
        let mut around = station(run, &mut engine);
        for id in 1..=3 {
            around.net.offer(id, WmmUserPriority::UP0, DESTINATION);
        }
        let _ = engine.offered(&mut around.turn());
        around.port.set_air(BEST_EFFORT, Air::Hold);
        assert!(matches!(
            engine.stop_data(&mut around.turn()),
            Ok(DataStop::Going)
        ));
        // What waited and what the standby held went home at once.
        assert_eq!(around.net.home, [3, 2]);
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let Ok(DataStop::Stopped(rights)) = engine.stop_data(&mut around.turn()) else {
            panic!("the air ended and the gate closed");
        };
        assert_eq!(around.net.home, [3, 2, 1]);
        assert!(engine.take_closed_gate().is_some());
        assert!(!engine.is_flowing());
        // The rights flow again.
        let _ = engine.flows(rights);
        assert!(engine.is_flowing());
    });
}

#[test]
fn a_stop_of_stopped_data_is_refused() {
    running::<STATION_SINGLES, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4);
        let mut around = Around::new(run.port);
        assert!(matches!(
            engine.stop_data(&mut around.turn()),
            Err(NotFlowing)
        ));
    });
}

/// An access point's engine of `PEERS` flowing, its gate open, with two
/// clients linked.
fn access_point<'id, const PEERS: usize, const POOL: usize>(
    running: Running<'id, AP_SINGLES>,
    engine: &mut ApEngine<'id, PEERS, POOL>,
) -> Around<'id, AP_SINGLES> {
    let Running { port, data, gate } = running;
    let mut around = Around::new(port);
    let _ = engine.flows(data);
    engine.open_gate(gate, &mut around.turn()).ok().unwrap();
    for (peer, client) in [(0, CLIENT_A), (1, CLIENT_B)] {
        engine
            .link(
                Peer::new(peer).unwrap(),
                client_link(client, PeerQos::Qos),
                &mut around.turn(),
            )
            .ok()
            .unwrap();
    }
    around
}

#[test]
fn an_access_points_msdu_is_copied_into_its_queues_place_and_its_memory_goes_home_at_the_write() {
    running::<AP_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4);
        let mut around = access_point(run, &mut engine);
        around.net.offer(7, WmmUserPriority::UP0, CLIENT_B);
        let _ = engine.offered(&mut around.turn());
        // Home at the write, before its end.
        assert_eq!(around.net.home, [7]);
        let place = ApEngine::<'_, 2, 8>::data_place(BEST_EFFORT);
        let single = around.port.single(place).unwrap().to_vec();
        // A QoS data frame from the DS: Address 1 the client, 2 the access
        // point, 3 the source; then LLC/SNAP and the payload, copied.
        assert_eq!(single[0], 0x88);
        assert_eq!(single[1] & 0x03, 0x02);
        assert_eq!(single[4..10], CLIENT_B);
        assert_eq!(single[10..16], ACCESS_POINT_ADDRESS);
        assert_eq!(single[16..22], STATION);
        let payload = &around.net.frames[0].1[ETHERNET_HEADER_LEN..];
        assert_eq!(&single[26 + 8..], payload);
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.rates.lessons, 1);
    });
}

#[test]
fn an_access_points_queue_carries_one_single_and_the_next_waits_for_its_end() {
    running::<AP_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4);
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
        assert!(
            around
                .seen
                .0
                .contains(&DataEvent::DataLost(DataLoss::RetryLimit))
        );
    });
}

#[test]
fn an_access_point_refuses_an_msdu_for_a_station_it_has_no_link_with() {
    running::<AP_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4);
        let mut around = access_point(run, &mut engine);
        around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
        let _ = engine.offered(&mut around.turn());
        assert_eq!(around.net.home, [1]);
        assert_eq!(
            around.seen.0,
            [DataEvent::MsduRefused(IntakeRefusal::NoLink)]
        );
        assert!(matches!(
            engine.asked(
                Request::TxAgreement(Peer::new(0).unwrap(), WmmUserPriority::UP0, 2),
                &mut around.turn()
            ),
            Err(Refused::NoAgreements)
        ));
    });
}

#[test]
fn an_access_points_stop_waits_for_its_singles_and_gives_every_right_back() {
    running::<AP_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4);
        let mut around = access_point(run, &mut engine);
        around.net.offer(1, WmmUserPriority::UP0, CLIENT_A);
        around.net.offer(2, WmmUserPriority::UP0, CLIENT_A);
        around.port.set_air(BEST_EFFORT, Air::Hold);
        let _ = engine.offered(&mut around.turn());
        assert!(matches!(
            engine.stop_data(&mut around.turn()),
            Ok(DataStop::Going)
        ));
        assert_eq!(around.net.home, [1, 2]);
        around.port.set_air(BEST_EFFORT, Air::Ack);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let Ok(DataStop::Stopped(rights)) = engine.stop_data(&mut around.turn()) else {
            panic!("the single ended and the gate closed");
        };
        assert_eq!(rights.singles.len(), AP_SINGLES);
        assert!(
            rights
                .singles
                .iter()
                .enumerate()
                .all(|(index, place)| usize::from(place.index()) == index)
        );
        assert_eq!(
            rights.queues.map(|queue| queue.category()),
            oer_protocols_ieee80211_contracts_port::QUEUES
        );
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

#[test]
fn an_access_points_data_place_is_its_layouts_last_one_a_queue() {
    let places = QUEUES.map(ApEngine::<'static, 2, 8>::data_place);
    let first = (AP_SINGLES - DATA_SINGLES) as u8;
    assert_eq!(places, [first, first + 1, first + 2, first + 3]);
    let _ = FrameSet::EMPTY;
}

/// The lifetime: the search leaves it out (its paths last microseconds, the
/// lifetime an hour), so these cases hold it. A station whose members live
/// a millisecond and may miss three aggregates, under an agreement of
/// four, its gate open, its three MSDUs offered while its link was held and
/// released together: they go as one aggregate. A miss of one frame is its
/// end whatever its age (it went as one MPDU by its retry plan), so the
/// cases keep two frames to retry.
fn aggregating<'id>(
    running: Running<'id, STATION_SINGLES>,
    engine: &mut StationEngine<'id, 8>,
    before: impl FnOnce(&mut Around<'id, STATION_SINGLES>),
) -> Around<'id, STATION_SINGLES> {
    let Running { port, data, gate } = running;
    let mut around = Around::new(port);
    let _ = engine.flows(data);
    engine.open_gate(gate, &mut around.turn()).ok().unwrap();
    engine
        .link(
            ACCESS_POINT,
            station_link_living(3, Duration::from_millis(1)),
            &mut around.turn(),
        )
        .ok()
        .unwrap();
    engine.aggregate_rate(Some(AggregateRate {
        rate: ht(),
        max_subframes: 8,
        max_length: 65_535,
    }));
    engine
        .asked(
            Request::TxAgreement(ACCESS_POINT, WmmUserPriority::UP0, 4),
            &mut around.turn(),
        )
        .unwrap();
    engine
        .asked(Request::Hold(ACCESS_POINT), &mut around.turn())
        .unwrap();
    around.net.offer(1, WmmUserPriority::UP0, DESTINATION);
    around.net.offer(2, WmmUserPriority::UP0, DESTINATION);
    around.net.offer(3, WmmUserPriority::UP0, DESTINATION);
    let _ = engine.offered(&mut around.turn());
    before(&mut around);
    engine
        .asked(Request::Release(ACCESS_POINT), &mut around.turn())
        .unwrap();
    assert_eq!(around.port.published().len(), 1);
    assert_eq!(around.port.published()[0].frames.len(), 3);
    around
}

/// The BlockAck acknowledges the first frame only.
fn first_only<const S: usize>(around: &mut Around<'_, S>) {
    around.port.set_air(BEST_EFFORT, Air::Ack);
    around.port.acknowledge(FrameSet::prefix(1));
}

#[test]
fn a_member_missed_within_its_lifetime_is_kept_and_one_missed_after_it_is_dropped_expired() {
    running::<STATION_SINGLES, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4);
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
        let expired = around
            .seen
            .0
            .iter()
            .filter(|event| **event == DataEvent::DataLost(DataLoss::Expired))
            .count();
        assert_eq!(expired, 2);
        assert_eq!(around.port.published().len(), 2);
    });
}

/// A queued MSDU's lifetime starts when an arena takes it, not when the
/// network offered it: the time it waited in its queue is not counted.
#[test]
fn a_member_that_waited_in_its_queue_past_its_lifetime_still_has_its_lifetime_in_the_arena() {
    running::<STATION_SINGLES, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4);
        // The two waited in their queue, held, longer than they live.
        let mut around = aggregating(run, &mut engine, |around| around.pass(5_000));
        first_only(&mut around);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        // The missed one is kept for its retry, not dropped expired.
        assert_eq!(around.net.home, [1]);
        assert!(
            !around
                .seen
                .0
                .contains(&DataEvent::DataLost(DataLoss::Expired))
        );
        assert_eq!(around.port.published().len(), 2);
    });
}

/// A kept member that outlives its lifetime while the gate closes and
/// stays closed is not dropped then: the lifetime is read at a settlement.
/// It goes once more when the gate opens, and that miss drops it expired.
#[test]
fn a_kept_member_that_ages_while_the_gate_closes_goes_once_more_and_its_miss_expires_it() {
    running::<STATION_SINGLES, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(4);
        let mut around = aggregating(run, &mut engine, |_| {});
        // The gate's closing is asked while the aggregate is on the air.
        engine
            .asked(Request::CloseGate, &mut around.turn())
            .unwrap();
        around.port.set_air(BEST_EFFORT, Air::Ack);
        around.port.acknowledge(FrameSet::prefix(1));
        // Its end settles; the closing's poll ends with the air quiet.
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        let gate = engine.take_closed_gate().unwrap();
        assert_eq!(around.net.home, [1]);
        around.pass(5_000);
        let _ = engine.round(&mut around.turn());
        assert_eq!(around.net.home, [1]);
        engine.open_gate(gate, &mut around.turn()).ok().unwrap();
        assert_eq!(around.port.published().len(), 2);
        around.port.set_air(BEST_EFFORT, Air::NoAck);
        let _ = engine.transmitted(TxFacts::DUE, Signal::Quiet, &mut around.turn());
        assert_eq!(around.net.home, [1, 2, 3]);
        let expired = around
            .seen
            .0
            .iter()
            .filter(|event| **event == DataEvent::DataLost(DataLoss::Expired))
            .count();
        assert_eq!(expired, 2);
    });
}

/// A single that ends while the gate's closing is polled comes back
/// through the poll's sink: its place and its queue's right are the
/// engine's again, and the stop gives every right back.
#[test]
fn a_single_that_ends_during_the_commands_poll_gives_its_place_and_queue_back() {
    running::<AP_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4);
        let mut around = access_point(run, &mut engine);
        around.port.set_air(BEST_EFFORT, Air::Hold);
        around.net.offer(1, WmmUserPriority::UP0, CLIENT_A);
        let _ = engine.offered(&mut around.turn());
        engine
            .asked(Request::CloseGate, &mut around.turn())
            .unwrap();
        // The single ends while the closing is polled, before the barrier.
        around.port.set_air(BEST_EFFORT, Air::Ack);
        around.port.serve_in_next_poll();
        let _ = engine.round(&mut around.turn());
        assert_eq!(around.rates.lessons, 1);
        assert!(engine.take_closed_gate().is_some());
        let Ok(DataStop::Stopped(rights)) = engine.stop_data(&mut around.turn()) else {
            panic!("nothing is on the air and the gate closed");
        };
        assert_eq!(rights.singles.len(), AP_SINGLES);
    });
}
