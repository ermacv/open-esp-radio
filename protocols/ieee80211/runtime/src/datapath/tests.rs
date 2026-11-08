//! The actor on the model's data port, the test playing the lifecycle, the
//! network stack and the access point: each turn's work, its wakes and its
//! one clock read, the data both ways, the station's frames on the link's
//! numbers, refusals, faults, the stop and the reinstall.

use super::*;
use core::cell::{Cell, RefCell};
use core::convert::Infallible;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::Waker;
use std::boxed::Box;
use std::rc::Rc;
use std::sync::Arc;
use std::vec::Vec;

use oer_chips_family_radio_ieee80211_policy::rate_selection::EspressifRateSelection;
use oer_portable_memory::{WakingSlotQueue, WakingSpscQueue};
use oer_portable_time::Clock;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_contracts_port::{
    Channel, ChannelWidth, InstalledPairwise, PortCommands, RegulatedChannel,
};
use oer_protocols_ieee80211_core_roles_sta::data::plane::DataLink;
use oer_protocols_ieee80211_core_roles_sta::data::{MSDU_ETHERNET_OFFSET, MSDU_PAYLOAD_OFFSET};
use oer_protocols_ieee80211_core_roles_sta::link::LinkSpace;
use oer_protocols_ieee80211_core_roles_sta::machine::sta_frame;
use oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpPacketNumberStep;
use oer_protocols_ieee80211_core_upper_mac::qos::AcAdmission;
use oer_protocols_ieee80211_testing_model::{
    AirFrame, MODEL_KEY_SLOTS, ModelAck, ModelAir, ModelBackend, ModelBlockAckEvidence,
    ModelCaps24, ModelCommands, ModelContext, ModelInstall, ModelLentRx, ModelNetworkMemory,
    ModelPool, ModelPort, ModelPortFault, ModelResources, ModelRxMeta, ModelSmall, ModelTypes,
};
use oer_protocols_ieee80211_wire::address::{Bssid, IndividualAddress, OwnAddress};
use oer_protocols_ieee80211_wire::security::LinkProtection;
use oer_protocols_ieee80211_wire::station::StaDataHeaderTemplate;
use oer_protocols_radio_port::{Installations, LocalId, SlotIdentity, with_local_slot};

const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 0x09];

const TO: usize = 16;
const RETURNS: usize = 40;
const FROM: usize = 64;
const TX_DEPTH: usize = 16;
const RX_DEPTH: usize = 8;

/// The model's contexts of a station whose frames carry no external body.
struct Contexts;

impl ModelContext for Contexts {
    type Body = Infallible;
    type OwnedContext<Id: SlotIdentity> = ();
    type Context<'p, Id: SlotIdentity + 'p> = ();
    type OwnedStationConfigureContext<Id: SlotIdentity> = ();
    type StationConfigureContext<'p, Id: SlotIdentity + 'p> = ();
    type OwnedAccessPointConfigureContext<Id: SlotIdentity> = ();
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p> = ();
}

type Backend<'s> = ModelBackend<LocalId<'s>, ModelCaps24, Contexts>;
type Types<'s> = ModelTypes<Backend<'s>>;

/// The model's port as the actor's.
struct TestPort<'s>(PhantomData<fn(&'s ()) -> &'s ()>);

impl<'s> DatapathPort<'s> for TestPort<'s> {
    type Port = ModelPort<'s, LocalId<'s>, ModelCaps24, Contexts>;
    type Caps = ModelCaps24;
    type Types = Types<'s>;
    type Small = ModelSmall;
    type TxMemory = ModelNetworkMemory;
    type LentRx = ModelLentRx;
    const RX_HELD: u8 = 8;

    fn arena_ack(response: ModelBlockAckEvidence) -> Option<ModelAck> {
        match response {
            ModelBlockAckEvidence::Measured(snr_db) => Some(ModelAck { snr_db }),
            ModelBlockAckEvidence::Unmeasured => None,
        }
    }

    fn received(frame: &[u8], _: &ModelRxMeta) {
        OBSERVED.with(|observed| observed.borrow_mut().push(frame.to_vec()));
    }
}

std::thread_local! {
    /// The units the actor's observation saw on this test's thread.
    static OBSERVED: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}

type P<'s> = TestPort<'s>;

/// The test's clock: the model's virtual time, counting the actor's reads.
struct Clocked {
    inner: Rc<VirtualClock>,
    reads: Cell<usize>,
}

impl Clock for Clocked {
    fn now(&self) -> Instant {
        self.reads.set(self.reads.get() + 1);
        self.inner.now()
    }
}

impl Timer for Clocked {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        self.inner.wait_until(deadline)
    }
}

/// A waker that counts its wakes.
struct Counter(AtomicUsize);

impl std::task::Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// The actor's own events of a case.
#[derive(Clone, Default)]
struct Recorder(Rc<RefCell<Vec<ActorEvent>>>);

impl EventSink<ActorEvent> for Recorder {
    fn event(&mut self, event: ActorEvent) {
        self.0.borrow_mut().push(event);
    }
}
impl EventSink<crate::station::ServiceEvent> for Recorder {
    const ENABLED: bool = false;
    fn event(&mut self, _: crate::station::ServiceEvent) {}
}
impl EventSink<oer_protocols_ieee80211_core_roles_sta::events::StaEvent> for Recorder {
    const ENABLED: bool = false;
    fn event(&mut self, _: oer_protocols_ieee80211_core_roles_sta::events::StaEvent) {}
}
impl EventSink<oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent> for Recorder {
    const ENABLED: bool = false;
    fn event(&mut self, _: oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent) {}
}

/// A network side that takes every frame and counts its stop requests.
/// `answer` is what its stop says; none yet leaves it running.
#[derive(Default)]
struct Network {
    asked: Cell<u32>,
    answer: Cell<Option<Result<(), crate::station::NetworkStopLost>>>,
}

impl NetworkStop for Network {
    fn takes_frames(&self) -> bool {
        true
    }
    fn request_stop(&self) {
        self.asked.set(self.asked.get() + 1);
    }
    fn poll_stopped(
        &self,
        _: &mut core::task::Context<'_>,
    ) -> Poll<Result<(), crate::station::NetworkStopLost>> {
        self.answer.get().map_or(Poll::Pending, Poll::Ready)
    }
}

type Run<'b> = Pin<&'b mut dyn Future<Output = ActorEnd<ModelPortFault>>>;

/// The lifecycle's, the network's and the air's side of one actor.
struct Bench<'b, 's> {
    clock: &'b Clocked,
    network: &'b Network,
    air: Rc<ModelAir>,
    run: Run<'b>,
    counter: Arc<Counter>,
    events: Recorder,
    end: Option<ActorEnd<ModelPortFault>>,
    polls: usize,
    to_actor: WakingSpscSender<'b, ToActor<'s, P<'s>>, TO>,
    from_actor: WakingSpscReceiver<'b, FromActor<'s, P<'s>>, FROM>,
    returns: WakingSpscReceiver<'b, Returned<'s, P<'s>>, RETURNS>,
    link: WakingSpscSender<'b, ActorLink<'s, P<'s>>, 1>,
    link_return: WakingSpscReceiver<'b, LinkReturn, 1>,
    transmit: SlotSender<'b, Option<Outgoing<ModelNetworkMemory>>, TX_DEPTH>,
    returned: SlotReceiver<'b, Option<TxEnded<ModelNetworkMemory>>, TX_DEPTH>,
    receive: WakingSpscReceiver<'b, Received<ModelLentRx>, RX_DEPTH>,
    tickets: Vec<ActorTicket<'s, P<'s>>>,
    small: Vec<ActorStationBuffer<'s, P<'s>>>,
    commands: ModelCommands<'s, LocalId<'s>, ModelCaps24, Contexts>,
    association: Option<Association<'s>>,
    pairwise: Option<[PairwiseSlot<'s>; MODEL_KEY_SLOTS]>,
}

type Association<'s> = oer_protocols_ieee80211_contracts_port::association::AssociationStart<
    's,
    oer_protocols_ieee80211_contracts_port::Station,
    MODEL_KEY_SLOTS,
    MODEL_KEY_SLOTS,
    ModelPool<LocalId<'s>, 5>,
>;
type PairwiseSlot<'s> = oer_protocols_ieee80211_contracts_port::association::PairwiseKeySlot<
    's,
    oer_protocols_ieee80211_contracts_port::Station,
    MODEL_KEY_SLOTS,
    MODEL_KEY_SLOTS,
    ModelPool<LocalId<'s>, 5>,
>;

fn channel6() -> Channel {
    Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
}

fn supported() -> SupportedChannel<ModelCaps24> {
    SupportedChannel::new(
        oer_protocols_radio_regulatory::TEST_DOMAIN
            .permit(RegulatedChannel::from(channel6()))
            .unwrap(),
    )
    .unwrap()
}

fn meta() -> ModelRxMeta {
    ModelRxMeta::new(channel6(), -40, -95)
}

/// A data frame from the access point to the station, `payload` after its
/// LLC header, Sequence Number `sequence`.
fn data_from_ap(sequence: u16, payload: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x0208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&PEER);
    frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    frame.extend_from_slice(payload);
    frame
}

/// A data frame of another BSS: the link refuses it.
fn foreign_data(sequence: u16) -> Vec<u8> {
    let mut frame = data_from_ap(sequence, &[1, 2, 3, 4]);
    frame[10..16].copy_from_slice(&[0x30, 0x05, 0x5c, 0, 0, 0x01]);
    frame
}

/// A management frame from the access point (an Action, category 127).
fn management(sequence: u16) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x00d0_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
    frame.extend_from_slice(&[127, 0, 0, 0]);
    frame
}

/// An MSDU of the network to the peer at user `priority`.
fn msdu(priority: u8, payload: &[u8]) -> Outgoing<ModelNetworkMemory> {
    let mut memory: ModelNetworkMemory =
        Box::new([0; oer_protocols_ieee80211_testing_model::MODEL_MAX_MPDU]);
    memory[MSDU_ETHERNET_OFFSET..][..6].copy_from_slice(&PEER);
    memory[MSDU_ETHERNET_OFFSET + 6..][..6].copy_from_slice(&LOCAL);
    memory[MSDU_ETHERNET_OFFSET + 12..][..2].copy_from_slice(&0x0800_u16.to_be_bytes());
    memory[MSDU_PAYLOAD_OFFSET..][..payload.len()].copy_from_slice(payload);
    Outgoing {
        memory,
        length: u16::try_from(14 + payload.len()).unwrap(),
        priority: WmmUserPriority::new(priority).unwrap(),
    }
}

/// The station's link, open or under `key`, on a QoS BSS when `qos`.
fn link<'s>(qos: bool, key: Option<ActorLinkKey<'s, P<'s>>>) -> ActorLink<'s, P<'s>> {
    let address = OwnAddress::new(IndividualAddress::new(LOCAL).unwrap());
    let bssid = Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap();
    let protection = if key.is_some() {
        LinkProtection::Ccmp
    } else {
        LinkProtection::Open
    };
    let data = DataLink {
        header: StaDataHeaderTemplate::new(address, bssid, qos, protection).unwrap(),
        admission: AcAdmission::OPEN,
        retry: None,
        ht: None,
        windows: [None; 8],
        protection: Default::default(),
    };
    LinkHandover::for_test(1, address, bssid, key.is_some(), qos, key, Some(data))
}

/// The Sequence Number of a frame the air saw.
fn sequence(frame: &AirFrame) -> u16 {
    u16::from_le_bytes([frame.plaintext[22], frame.plaintext[23]]) >> 4
}

fn is_data(frame: &AirFrame) -> bool {
    frame.plaintext[0] & 0x0c == 0x08
}

impl<'s> Bench<'_, 's> {
    fn wakes(&self) -> usize {
        self.counter.0.load(Ordering::Relaxed)
    }

    /// Poll the actor once.
    fn poll(&mut self) {
        let waker = Waker::from(self.counter.clone());
        let mut cx = core::task::Context::from_waker(&waker);
        self.polls += 1;
        if let Poll::Ready(end) = self.run.as_mut().poll(&mut cx) {
            self.end = Some(end);
        }
    }

    /// Poll the actor while it was woken since its last poll; the number
    /// of polls.
    fn settle(&mut self) -> usize {
        let mut polls = 0;
        let mut seen = self.wakes();
        self.poll();
        polls += 1;
        while self.wakes() != seen && self.end.is_none() {
            seen = self.wakes();
            self.poll();
            polls += 1;
            assert!(polls < 1000, "the actor never sleeps");
        }
        polls
    }

    /// Tell the actor `message` and let it run.
    fn send(&mut self, message: ToActor<'s, P<'s>>) {
        assert!(
            self.to_actor.try_send(message).is_ok(),
            "the queue has room"
        );
        self.settle();
    }

    /// Start the radio and take its answer.
    fn start(&mut self) {
        self.send(ToActor::Start(supported()));
        assert!(matches!(self.next(), Some(FromActor::Started(Ok(())))));
    }

    /// Hand the actor `link` and release its data.
    fn connect(&mut self, link: ActorLink<'s, P<'s>>) {
        assert!(self.link.try_send(link).is_ok());
        self.send(ToActor::Connect);
        self.send(ToActor::Release);
    }

    fn next(&mut self) -> Option<FromActor<'s, P<'s>>> {
        self.from_actor.try_receive()
    }

    /// The next owner the actor gave back, after its announcement; the
    /// facts before it are passed over.
    fn returned(&mut self) -> Option<Returned<'s, P<'s>>> {
        loop {
            match self.next()? {
                FromActor::Returned => return self.returns.try_receive(),
                FromActor::TxTraffic { .. } | FromActor::BarOwed { .. } | FromActor::Tbtt(_) => {}
                _ => return None,
            }
        }
    }

    /// Hand the network's MSDU to the actor's queue.
    fn send_msdu(&mut self, outgoing: Outgoing<ModelNetworkMemory>) {
        let mut outgoing = Some(outgoing);
        assert!(
            self.transmit
                .try_send_with(|slot| {
                    *slot = outgoing.take();
                    Ok::<_, Infallible>(())
                })
                .is_ok()
        );
    }

    fn data_sent(&self) -> Vec<AirFrame> {
        self.air.sent().into_iter().filter(is_data).collect()
    }

    /// Advance virtual time by `micros`.
    fn advance(&mut self, micros: u64) {
        self.clock
            .inner
            .advance(oer_portable_time::Duration::from_micros(micros))
            .unwrap();
        self.air.clock_advanced();
    }
}

/// Run `test` against a fresh actor on the model.
fn bench(test: impl for<'b, 's> FnOnce(&mut Bench<'b, 's>)) {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let clock = Clocked {
            inner: Rc::new(VirtualClock::new()),
            reads: Cell::new(0),
        };
        let mut installations = Installations::claim(claim);
        let (install, air) = ModelPort::install(
            &mut installations,
            channel6(),
            [
                IndividualAddress::new(LOCAL).unwrap(),
                IndividualAddress::new(BSSID).unwrap(),
            ],
            clock.inner.clone(),
        )
        .unwrap();
        let ModelInstall {
            port,
            commands,
            resources,
        } = install;
        let ModelResources {
            shared:
                oer_protocols_ieee80211_testing_model::ModelShared {
                    tickets,
                    small,
                    block_acks,
                    ..
                },
            station:
                oer_protocols_ieee80211_testing_model::ModelStationPermissions {
                    association: station_association,
                    pairwise: station_pairwise,
                    ..
                },
            ..
        } = resources;
        let storage = Box::leak(Box::new(
            crate::station::RxBlockAckStorage::<Types<'_>, 2, 4>::new(
                block_acks,
                oer_portable_time::Duration::from_millis(100),
            ),
        ));
        let events = Recorder::default();
        // The actor is placed, as a composition places it.
        let mut actor = Actor::<P<'_>, _, _>::init_in_place(
            Box::leak(Box::new(core::mem::MaybeUninit::uninit())),
            port,
            EspressifRateSelection::<ModelCaps24>::new(),
            storage.view(),
            events.clone(),
        );
        let mut to_queue = WakingSpscQueue::<ToActor<'_, P<'_>>, TO>::new();
        let mut parcel_queue = WakingSpscQueue::<Parcel<'_, P<'_>>, PARCELS>::new();
        let mut from_queue = WakingSpscQueue::<FromActor<'_, P<'_>>, FROM>::new();
        let mut return_queue = WakingSpscQueue::<Returned<'_, P<'_>>, RETURNS>::new();
        let mut link_queue = WakingSpscQueue::<ActorLink<'_, P<'_>>, 1>::new();
        let mut group_queue =
            WakingSpscQueue::<GroupPending<'_, ActorEpochPath<'_, P<'_>>>, 1>::new();
        let mut link_return_queue = WakingSpscQueue::<LinkReturn, 1>::new();
        let mut transmit_queue =
            WakingSlotQueue::<Option<Outgoing<ModelNetworkMemory>>, TX_DEPTH>::new();
        let mut returned_queue =
            WakingSlotQueue::<Option<TxEnded<ModelNetworkMemory>>, TX_DEPTH>::new();
        let mut receive_queue = WakingSpscQueue::<Received<ModelLentRx>, RX_DEPTH>::new();
        let mut copies_queue = WakingSpscQueue::<EthernetFrame, RX_COPIED_DEPTH>::new();
        let (to_actor, to_actor_end) = to_queue.split();
        let (_parcels, parcels_end) = parcel_queue.split();
        let (from_actor_end, from_actor) = from_queue.split();
        let (returns_end, returns) = return_queue.split();
        let (link, link_end) = link_queue.split();
        let (groups_end, _groups) = group_queue.split();
        let (link_return_end, link_return) = link_return_queue.split();
        let (transmit, transmit_end) = transmit_queue.split(|| None);
        let (returned_end, returned) = returned_queue.split(|| None);
        let (receive_end, receive) = receive_queue.split();
        let (copies_end, _copies) = copies_queue.split();
        let mut lifecycle = LifecycleEnds {
            to_actor: to_actor_end,
            parcels: parcels_end,
            from_actor: from_actor_end,
            returns: returns_end,
            groups: groups_end,
            link: link_end,
            link_return: link_return_end,
        };
        let network_side = Network::default();
        let mut network = NetworkEnds::<_, _, _, TX_DEPTH, RX_DEPTH, SameHart> {
            transmit: transmit_end,
            returned: returned_end,
            receive: receive_end,
            receive_copied: copies_end,
            network: &network_side,
        };
        let run = actor.run(&mut lifecycle, &mut network, &clock);
        let mut run = core::pin::pin!(run);
        let mut bench = Bench {
            clock: &clock,
            network: &network_side,
            air,
            run: run.as_mut(),
            counter: Arc::new(Counter(AtomicUsize::new(0))),
            events,
            end: None,
            polls: 0,
            to_actor,
            from_actor,
            returns,
            link,
            link_return,
            transmit,
            returned,
            receive,
            tickets,
            small,
            commands,
            association: Some(station_association),
            pairwise: Some(station_pairwise),
        };
        bench.settle();
        test(&mut bench);
    }));
}

#[test]
fn an_idle_actor_sleeps_without_waking_itself() {
    bench(|bench| {
        bench.start();
        let wakes = bench.wakes();
        bench.poll();
        bench.poll();
        assert_eq!(bench.wakes(), wakes, "no self-wake while idle");
        assert!(bench.next().is_none());
    });
}

#[test]
fn each_turn_reads_the_clock_once() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        for _ in 0..4 {
            let reads = bench.clock.reads.get();
            bench.send_msdu(msdu(0, &[1, 2, 3, 4]));
            bench.poll();
            assert_eq!(bench.clock.reads.get(), reads + 1, "one read per turn");
        }
        // The last arena ends at the next service; then a poll finds no
        // work, runs no turn and reads nothing.
        bench.poll();
        let reads = bench.clock.reads.get();
        bench.poll();
        assert_eq!(bench.clock.reads.get(), reads);
    });
}

#[test]
fn forty_received_units_take_a_full_turn_one_repost_and_a_short_one() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        let wakes = bench.wakes();
        for sequence in 0..40 {
            bench.air.receive(&foreign_data(sequence), meta());
        }
        let polls = bench.settle();
        assert_eq!(polls, 2, "32 units, then the other 8 after one repost");
        assert_eq!(bench.wakes() - wakes, 2, "the air's wake and one repost");
        // Every unit went back to the ring: the next ones come.
        bench.air.receive(&foreign_data(41), meta());
        assert_eq!(bench.settle(), 1);
    });
}

#[test]
fn one_poll_carries_each_msdu_with_no_lifecycle_message() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        // The first MSDU tells the lifecycle its TID's traffic once.
        bench.send_msdu(msdu(0, &[0; 8]));
        bench.settle();
        assert!(matches!(bench.next(), Some(FromActor::TxTraffic { .. })));
        let polls = bench.polls;
        for round in 1..=8 {
            // The network's next MSDU comes while the last one is on the
            // air; its completion and the next publication take one turn.
            bench.send_msdu(msdu(0, &[round; 8]));
            bench.poll();
            assert_eq!(bench.data_sent().len(), usize::from(round) + 1);
        }
        assert_eq!(bench.polls - polls, 8, "one actor poll per PPDU");
        assert!(bench.next().is_none(), "the lifecycle never runs per PPDU");
        // The network's memory came back once each frame ended.
        let mut back = 0;
        while bench.returned.try_receive_with(Option::take).is_ok() {
            back += 1;
        }
        assert!(back >= 8, "{back}");
    });
}

#[test]
fn a_completion_publishes_the_sealed_standby_in_the_same_turn() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        // The first MSDU's arena stays on the air.
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, true);
        bench.send_msdu(msdu(0, &[1; 8]));
        bench.settle();
        let _traffic = bench.next();
        bench.send_msdu(msdu(0, &[2; 8]));
        bench.settle();
        let first = bench.data_sent().len();
        assert_eq!(first, 1, "the second MSDU waits in the standby");
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, false);
        let polls = bench.settle();
        assert_eq!(polls, 1, "completion and publication in one turn");
        assert_eq!(bench.data_sent().len(), first + 1);
        assert_eq!(bench.air.relayed_arenas(), 1, "ahead of the port's service");
    });
}

#[test]
fn a_waiting_event_stops_intake_at_the_next_frame() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, true);
        bench.send_msdu(msdu(0, &[1; 8]));
        bench.settle();
        let _traffic = bench.next();
        bench.air.set_event_waiting(true);
        for tag in 2..5 {
            bench.send_msdu(msdu(0, &[tag; 8]));
        }
        bench.poll();
        assert_eq!(
            bench.transmit.free_capacity(),
            TX_DEPTH - 2,
            "one frame taken, the rest left queued for the event"
        );
    });
}

#[test]
fn received_data_reaches_the_network_without_the_lifecycle() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench.air.receive(&data_from_ap(1, &[9, 9, 9, 9]), meta());
        bench.settle();
        assert!(matches!(
            bench.receive.try_receive(),
            Some(Received::Lent(_))
        ));
        assert!(bench.next().is_none(), "no lifecycle message");
    });
}

#[test]
fn a_turn_s_received_data_wakes_a_waiting_network() {
    struct Woken(AtomicUsize);
    impl std::task::Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        let woken = Arc::new(Woken(AtomicUsize::new(0)));
        let waker = Waker::from(woken.clone());
        let mut cx = Context::from_waker(&waker);
        assert!(
            core::pin::pin!(bench.receive.ready())
                .poll(&mut cx)
                .is_pending()
        );
        for sequence in 1..4 {
            bench.air.receive(&data_from_ap(sequence, &[9; 4]), meta());
        }
        bench.settle();
        assert_eq!(woken.0.load(Ordering::Relaxed), 1);
        assert!(matches!(
            bench.receive.try_receive(),
            Some(Received::Lent(_))
        ));
    });
}

#[test]
fn a_unit_landing_behind_the_queued_handover_is_routed_under_the_link() {
    bench(|bench| {
        bench.start();
        // Commands the message budget takes one per turn queue ahead of the
        // handover; the access point's data lands before the actor's turn.
        for message in [ToActor::Disassociated, ToActor::Disassociated] {
            assert!(bench.to_actor.try_send(message).is_ok());
        }
        assert!(bench.link.try_send(link(true, None)).is_ok());
        assert!(bench.to_actor.try_send(ToActor::Connect).is_ok());
        assert!(bench.to_actor.try_send(ToActor::Release).is_ok());
        bench.air.receive(&data_from_ap(1, &[9, 9, 9, 9]), meta());
        bench.settle();
        assert!(matches!(
            bench.receive.try_receive(),
            Some(Received::Lent(_))
        ));
        assert!(bench.next().is_none(), "nothing went to the lifecycle");
    });
}

#[test]
fn the_networks_lend_limit_holds_frames_until_it_returns_one() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        for sequence in 0..6 {
            bench
                .air
                .receive(&data_from_ap(sequence, &[sequence as u8; 4]), meta());
        }
        bench.settle();
        // The model lends four units: the others wait in order.
        let mut lent = Vec::new();
        while let Some(frame) = bench.receive.try_receive() {
            lent.push(frame);
        }
        assert_eq!(lent.len(), 4);
        // The network returns one: the next frame follows.
        drop(lent.pop());
        bench.settle();
        assert!(bench.receive.try_receive().is_some());
        assert!(bench.events.0.borrow().is_empty(), "nothing dropped");
    });
}

#[test]
fn frames_for_the_station_wait_in_the_ring_while_the_lifecycle_holds_its_lane() {
    bench(|bench| {
        bench.start();
        for sequence in 0..6 {
            bench.air.receive(&management(sequence), meta());
        }
        bench.settle();
        let mut frames = Vec::new();
        while let Some(message) = bench.next() {
            match message {
                FromActor::Received { frame, .. } => frames.push(frame),
                _ => panic!("only frames"),
            }
        }
        assert_eq!(frames.len(), LIFECYCLE_FRAMES);
        assert!(bench.events.0.borrow().is_empty(), "nothing dropped");
        // Frames sent back free the lane: the ring's other frames follow.
        for frame in frames {
            assert!(bench.to_actor.try_send(ToActor::Recycle(frame)).is_ok());
        }
        bench.settle();
        let mut more = 0;
        while let Some(FromActor::Received { frame, .. }) = bench.next() {
            more += 1;
            assert!(bench.to_actor.try_send(ToActor::Recycle(frame)).is_ok());
        }
        assert_eq!(more, 2);
        bench.settle();
    });
}

/// A protected QoS Data frame of the station, an EAPOL packet in it, that
/// the link numbers in TID `tid`.
fn station_frame<'s>(bench: &mut Bench<'_, 's>, tid: u8) -> StationTx<'s, P<'s>> {
    station_frame_under(bench, tid, true)
}

/// The station's QoS Data frame of TID `tid`, protected under the link's
/// key or open, numbered by the station.
fn station_frame_under<'s>(
    bench: &mut Bench<'_, 's>,
    tid: u8,
    protected: bool,
) -> StationTx<'s, P<'s>> {
    use oer_protocols_ieee80211_contracts_port::TxBufferMemory;
    let ticket = bench.tickets.pop().unwrap();
    let mut buffer = bench.small.pop().unwrap();
    let mut frame = std::vec![0_u8; 26];
    let control: u16 = if protected { 0x4188 } else { 0x0188 };
    frame[0..2].copy_from_slice(&control.to_le_bytes());
    frame[4..10].copy_from_slice(&BSSID);
    frame[10..16].copy_from_slice(&LOCAL);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24] = tid;
    if protected {
        frame.extend_from_slice(&[0; 8]);
    }
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e, 2, 3, 0, 0]);
    let bytes = buffer.bytes_mut().unwrap();
    bytes[..frame.len()].copy_from_slice(&frame);
    let description = sta_frame(&bytes[..frame.len()], StaRatePlan::Management).unwrap();
    StationTx {
        ticket,
        buffer,
        frame: description,
        class: StaFrameClass::Normal,
        plan: StaRatePlan::Management,
        access_category: WmmUserPriority::new(tid).unwrap().access_category(),
        number: if protected {
            FrameNumber::Link {
                space: LinkSpace::Tid(WmmUserPriority::new(tid).unwrap()),
                protected: true,
            }
        } else {
            FrameNumber::Open
        },
        names: None,
    }
}

/// Install the station's pairwise key through the actor; its transmit half.
fn install_key<'s>(bench: &mut Bench<'_, 's>) -> ActorLinkKey<'s, P<'s>> {
    let association =
        <<Types<'s> as PortTypes<'s>>::StationKeys as KeyTypes<'s>>::begin_association(
            bench.association.take().unwrap(),
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
    let [slot, _] = bench.pairwise.take().unwrap();
    let (command, identity) = bench.commands.install_pairwise(
        &association,
        slot,
        IndividualAddress::new(BSSID).unwrap(),
        oer_protocols_ieee80211_contracts_security::Ccmp128Key::new(&[7; 16]),
    );
    let _unused_after_install = association;
    bench.send(ToActor::Port(command));
    match bench.returned() {
        Some(Returned::Terminal {
            terminal: LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key)),
            ..
        }) => oer_protocols_ieee80211_core_upper_mac::ccmp::PendingCcmpTxKey::issued(
            identity,
            CcmpPacketNumberStep::ONE,
        )
        .installed::<<Types<'s> as PortTypes<'s>>::StationKeys>(key)
        .ok()
        .unwrap(),
        _ => panic!("the key's installation terminal"),
    }
}

#[test]
fn station_frames_and_data_of_one_tid_number_on_without_a_gap() {
    bench(|bench| {
        bench.start();
        let key = install_key(bench);
        bench.connect(link(true, Some(key)));
        bench.send_msdu(msdu(5, &[1; 8]));
        bench.settle();
        let transmit = station_frame(bench, 5);
        bench.send(ToActor::StationTx(transmit));
        bench.send_msdu(msdu(5, &[2; 8]));
        bench.settle();
        bench.advance(1);
        bench.settle();
        let sent = bench.data_sent();
        assert_eq!(sent.len(), 3, "{sent:?}");
        for frame in &sent {
            assert_eq!(frame.plaintext[24] & 0x0f, 5, "TID 5");
            assert!(matches!(frame.key, Some((_, true))), "{frame:?}");
        }
        let sequences: Vec<u16> = sent.iter().map(sequence).collect();
        assert!(
            sequences.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "{sequences:?}"
        );
        let numbers: Vec<u64> = sent
            .iter()
            .map(|frame| frame.packet_number.unwrap())
            .collect();
        assert!(
            numbers.windows(2).all(|pair| pair[0] < pair[1]),
            "{numbers:?}"
        );
    });
}

#[test]
fn a_station_frame_without_a_link_comes_back_with_its_owners() {
    bench(|bench| {
        bench.start();
        let tickets = bench.tickets.len();
        let transmit = station_frame(bench, 0);
        bench.send(ToActor::StationTx(transmit));
        match bench.returned() {
            Some(Returned::Station { ticket, reason, .. }) => {
                assert_eq!(reason, TxRefusal::LinkChanged);
                bench.tickets.push(ticket);
            }
            _ => panic!("the frame's owners"),
        }
        assert_eq!(bench.tickets.len(), tickets);
        assert!(bench.air.sent().is_empty());
    });
}

#[test]
fn a_command_before_the_start_comes_back_refused() {
    bench(|bench| {
        // Nothing runs: a setting before the start comes back unchanged.
        let color = oer_protocols_ieee80211_contracts_port::BssColor::new(1).unwrap();
        bench.send(ToActor::Port(PortCommand::SetBssColor(color)));
        assert!(matches!(
            bench.returned(),
            Some(Returned::Command {
                reason: CommandRefusal::NotEnabled,
                ..
            })
        ));
    });
}

#[test]
fn a_port_fault_ends_the_actor_and_tells_the_lifecycle() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        // An arena stays on the air: the next turn serves the port.
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, true);
        bench.send_msdu(msdu(0, &[1; 8]));
        bench.settle();
        bench.air.poison();
        bench.advance(1);
        bench.settle();
        assert!(matches!(
            bench.end,
            Some(ActorEnd::Faulted(ActorFault::Port(_)))
        ));
        let mut faulted = false;
        while let Some(message) = bench.next() {
            faulted |= matches!(message, FromActor::Faulted(ActorFault::Port(_)));
        }
        assert!(faulted);
    });
}

#[test]
fn a_fault_the_port_stops_past_asks_for_a_restart_and_the_next_ends_the_actor() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, true);
        bench.send_msdu(msdu(0, &[1; 8]));
        bench.settle();
        bench.air.allow_recovery();
        bench.air.poison();
        bench.advance(1);
        bench.settle();
        assert!(bench.end.is_none(), "the actor stays for the stop");
        let mut recovering = false;
        while let Some(message) = bench.next() {
            recovering |= matches!(message, FromActor::Recovering(ActorFault::Port(_)));
        }
        assert!(recovering);
        // A second broken contract finds no second restart.
        bench.air.poison();
        bench.advance(1);
        bench.settle();
        assert!(matches!(bench.end, Some(ActorEnd::Faulted(_))));
    });
}

#[test]
fn a_port_recovering_sends_nothing_new_however_late_the_stop_comes() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, true);
        bench.send_msdu(msdu(0, &[1; 8]));
        bench.settle();
        let sent = bench.air.sent().len();
        bench.air.allow_recovery();
        bench.air.poison();
        bench.advance(1);
        bench.settle();
        let mut recovering = false;
        while let Some(message) = bench.next() {
            recovering |= matches!(message, FromActor::Recovering(ActorFault::Port(_)));
        }
        assert!(recovering);
        // The lifecycle has not asked for the stop: the port waits for it,
        // with room on the air for a new frame.
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, false);
        bench.send_msdu(msdu(0, &[2; 8]));
        bench.settle();
        bench.advance(1_000);
        bench.settle();
        assert_eq!(bench.air.sent().len(), sent, "nothing new went out");
        assert!(bench.end.is_none(), "and nothing faulted again");
    });
}

#[test]
fn a_stop_returns_the_networks_frames_and_confirms() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, true);
        bench.send_msdu(msdu(0, &[1; 8]));
        bench.settle();
        bench.send_msdu(msdu(0, &[2; 8]));
        bench.settle();
        while bench.next().is_some() {}
        // The second MSDU waits in the standby: the stop gives it back.
        let deadline = bench
            .clock
            .inner
            .now()
            .saturating_add(oer_portable_time::Duration::from_millis(50));
        bench.send(ToActor::Stop {
            deadline,
            network: NetworkEnd::Stays,
        });
        assert!(bench.returned.try_receive_with(Option::take).is_ok());
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, false);
        bench.settle();
        bench.advance(1);
        bench.settle();
        assert!(matches!(
            bench.next(),
            Some(FromActor::Stopped(StopOutcome::Confirmed))
        ));
        bench.send(ToActor::Retire);
        assert_eq!(bench.end, Some(ActorEnd::Retired));
    });
}

#[test]
fn a_restarts_stop_leaves_the_network_running() {
    bench(|bench| {
        bench.start();
        let deadline = bench.clock.inner.now();
        bench.send(ToActor::Stop {
            deadline,
            network: NetworkEnd::Stays,
        });
        bench.settle();
        assert_eq!(bench.network.asked.get(), 0);
        assert!(matches!(
            bench.next(),
            Some(FromActor::Stopped(StopOutcome::Confirmed))
        ));
    });
}

/// A stop that retires, with `window` milliseconds to end in.
fn retire(bench: &mut Bench<'_, '_>, window: u32) {
    let deadline = bench
        .clock
        .inner
        .now()
        .saturating_add(oer_portable_time::Duration::from_millis(window));
    bench.send(ToActor::Stop {
        deadline,
        network: NetworkEnd::Stops,
    });
}

#[test]
fn a_retiring_stop_waits_for_the_network_and_not_for_its_lent_units() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench.air.receive(&data_from_ap(1, &[9, 9, 9, 9]), meta());
        bench.settle();
        let held = bench.receive.try_receive();
        assert!(matches!(held, Some(Received::Lent(_))));
        retire(bench, 50);
        bench.advance(1);
        bench.settle();
        assert_eq!(bench.network.asked.get(), 1);
        assert!(bench.next().is_none(), "the network has not stopped");
        bench.network.answer.set(Some(Ok(())));
        bench.settle();
        // A lent unit held for ever does not hold the stop: it stays
        // counted, and returns whenever its holder drops it.
        assert!(matches!(
            bench.next(),
            Some(FromActor::Stopped(StopOutcome::Confirmed))
        ));
        drop(held);
        bench.settle();
        assert!(bench.next().is_none(), "a late return answers nothing");
    });
}

#[test]
fn a_stopped_radio_still_waits_for_its_network() {
    bench(|bench| {
        let deadline = bench
            .clock
            .inner
            .now()
            .saturating_add(oer_portable_time::Duration::from_millis(50));
        bench.send(ToActor::Stop {
            deadline,
            network: NetworkEnd::Stops,
        });
        assert!(bench.next().is_none());
        bench.network.answer.set(Some(Ok(())));
        bench.settle();
        assert!(matches!(
            bench.next(),
            Some(FromActor::Stopped(StopOutcome::Confirmed))
        ));
    });
}

/// A participant lost before the deadline ends the stop at once with the
/// radio's confirmed stop and the loss: nothing of the radio is unconfirmed,
/// and the actor retires.
#[test]
fn a_lost_network_participant_ends_the_stop_with_the_loss_and_the_radio_stopped() {
    bench(|bench| {
        bench.start();
        retire(bench, 50);
        bench
            .network
            .answer
            .set(Some(Err(crate::station::NetworkStopLost)));
        bench.advance(1);
        bench.settle();
        match bench.next() {
            Some(FromActor::Stopped(StopOutcome::NetworkLost(observed))) => {
                assert_eq!((observed.memory_held, observed.lent), (0, 0));
            }
            _ => panic!("a lost participant ends the stop with the loss"),
        }
        bench.send(ToActor::Retire);
        assert_eq!(bench.end, Some(ActorEnd::Retired));
    });
}

/// The radio stopped, but the network has not stopped by the deadline: the
/// stop's wait ends there as returning, not unconfirmed. A retirement waits
/// for the network, which the actor still follows, and ends the actor once
/// it stopped.
#[test]
fn a_network_that_stops_late_ends_the_wait_returning_and_retires_after_it() {
    bench(|bench| {
        bench.start();
        retire(bench, 1);
        bench.advance(2_000);
        bench.settle();
        match bench.next() {
            Some(FromActor::Stopped(StopOutcome::Returning(observed))) => {
                assert!(observed.network_running);
                assert_eq!((observed.memory_held, observed.lent), (0, 0));
            }
            _ => panic!("a stopped radio whose network is late is returning"),
        }
        bench.send(ToActor::Retire);
        bench.settle();
        assert!(bench.end.is_none(), "the retirement waits for the network");
        bench.network.answer.set(Some(Ok(())));
        bench.settle();
        assert_eq!(bench.end, Some(ActorEnd::Retired));
    });
}

/// A participant lost after the deadline, while the actor still followed
/// the network's late return: the station's share is home, so the actor
/// retires and tells the composition the loss.
#[test]
fn a_network_lost_after_the_deadline_retires_the_actor_with_the_loss() {
    bench(|bench| {
        bench.start();
        retire(bench, 1);
        bench.advance(2_000);
        bench.settle();
        assert!(matches!(
            bench.next(),
            Some(FromActor::Stopped(StopOutcome::Returning(_)))
        ));
        bench.send(ToActor::Retire);
        bench.settle();
        assert!(bench.end.is_none());
        bench
            .network
            .answer
            .set(Some(Err(crate::station::NetworkStopLost)));
        bench.settle();
        assert_eq!(bench.end, Some(ActorEnd::Retired));
    });
}

#[test]
fn a_stop_past_its_deadline_is_unconfirmed() {
    bench(|bench| {
        bench.start();
        // The lifecycle keeps a frame it was handed past the stop's deadline.
        bench.air.receive(&management(1), meta());
        bench.settle();
        let Some(FromActor::Received { frame, .. }) = bench.next() else {
            panic!("the station's frame");
        };
        let deadline = bench
            .clock
            .inner
            .now()
            .saturating_add(oer_portable_time::Duration::from_millis(1));
        bench.send(ToActor::Stop {
            deadline,
            network: NetworkEnd::Stays,
        });
        assert!(bench.next().is_none(), "the stop waits for the frame");
        bench.advance(2_000);
        bench.settle();
        match bench.next() {
            Some(FromActor::Stopped(StopOutcome::Unconfirmed(observed))) => {
                assert!(observed.deadline);
                assert_eq!(observed.frames_out, 1);
                assert_eq!(observed.port, None);
            }
            _ => panic!("an unconfirmed stop"),
        }
        drop(frame);
    });
}

#[test]
fn a_stop_aborts_the_arena_on_the_air_and_confirms() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, true);
        bench.send_msdu(msdu(0, &[1; 8]));
        bench.settle();
        while bench.next().is_some() {}
        let deadline = bench
            .clock
            .inner
            .now()
            .saturating_add(oer_portable_time::Duration::from_millis(1));
        bench.send(ToActor::Stop {
            deadline,
            network: NetworkEnd::Stays,
        });
        bench.advance(2_000);
        bench.settle();
        assert!(matches!(
            bench.next(),
            Some(FromActor::Stopped(StopOutcome::Confirmed))
        ));
    });
}

#[test]
fn hold_stops_the_data_and_release_carries_it_again() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench.send(ToActor::Hold);
        bench.send_msdu(msdu(0, &[1; 8]));
        bench.settle();
        assert!(bench.data_sent().is_empty(), "a held link carries no data");
        bench.send(ToActor::Release);
        bench.advance(1);
        bench.settle();
        assert_eq!(bench.data_sent().len(), 1);
    });
}

#[test]
fn a_new_pairwise_key_withdraws_the_frames_written_under_the_old_one() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, true);
        bench.send_msdu(msdu(0, &[1; 8]));
        bench.settle();
        bench.send_msdu(msdu(0, &[2; 8]));
        bench.settle();
        let returned = |bench: &mut Bench<'_, '_>| {
            let mut back = 0;
            while bench.returned.try_receive_with(Option::take).is_ok() {
                back += 1;
            }
            back
        };
        let before = returned(bench);
        let _key = install_key(bench);
        assert_eq!(returned(bench), before + 1, "the standby's frame came back");
    });
}

#[test]
fn a_link_comes_back_through_its_mailbox() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench.send(ToActor::Disconnect);
        assert!(matches!(bench.next(), Some(FromActor::LinkReturned)));
        assert!(bench.link_return.try_receive().is_some());
    });
}

#[test]
fn a_protected_station_frame_after_the_links_return_draws_the_kept_keys_next_packet_number() {
    bench(|bench| {
        bench.start();
        let key = install_key(bench);
        bench.connect(link(true, Some(key)));
        bench.send_msdu(msdu(0, &[1; 8]));
        bench.settle();
        bench.advance(1);
        bench.settle();
        let data = bench.data_sent();
        let last = data.last().unwrap().packet_number.unwrap();
        bench.send(ToActor::Disconnect);
        while !matches!(bench.next().unwrap(), FromActor::LinkReturned) {}
        assert!(bench.link_return.try_receive().is_some());
        // The station numbered it; the kept key numbers its packet.
        let mut transmit = station_frame_under(bench, 0, true);
        transmit.number = FrameNumber::Keyed;
        bench.send(ToActor::StationTx(transmit));
        bench.advance(1);
        bench.settle();
        let sent = bench.data_sent();
        assert_eq!(sent.len(), data.len() + 1, "{sent:?}");
        assert_eq!(sent.last().unwrap().packet_number, Some(last + 1));
        // The key goes back only when asked.
        while let Some(message) = bench.next() {
            assert!(!matches!(message, FromActor::PairwiseReturned(_)));
        }
        bench.send(ToActor::ReturnPairwise);
        assert!(matches!(bench.next(), Some(FromActor::PairwiseReturned(_))));
    });
}

#[test]
fn the_addba_starting_sequence_is_the_first_data_mpdu_numbered_after_it() {
    let tid = WmmUserPriority::new(5).unwrap();
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        // Traffic flows on the TID.
        for round in 0..3_u8 {
            bench.send_msdu(msdu(5, &[round; 8]));
            bench.settle();
            bench.advance(1);
            bench.settle();
        }
        let before = bench.data_sent().len();
        bench.send(ToActor::Sequence(tid));
        let next = loop {
            match bench.next() {
                Some(FromActor::TxSequence { tid: told, next }) => {
                    assert_eq!(told, tid);
                    break next;
                }
                Some(_) => {}
                None => panic!("the actor tells the TID's next number"),
            }
        };
        // The TID's data waits for the request that names the number.
        bench.send_msdu(msdu(5, &[9; 8]));
        bench.settle();
        bench.advance(1);
        bench.settle();
        assert_eq!(bench.data_sent().len(), before, "the TID's data waits");
        let mut request = station_frame_under(bench, 0, false);
        request.names = Some(tid);
        bench.send(ToActor::StationTx(request));
        bench.advance(1);
        bench.settle();
        bench.advance(1);
        bench.settle();
        let first = bench.data_sent()[before..]
            .iter()
            .find(|frame| frame.plaintext[24] & 0x0f == 5)
            .map(sequence)
            .expect("the TID's data went");
        assert_eq!(first, next.get());
    });
}

#[test]
fn messages_stay_small() {
    assert!(core::mem::size_of::<FromActor<'static, P<'static>>>() <= FROM_ACTOR_BYTES);
    std::println!(
        "ToActor {} FromActor {} Returned {} StationTx {} Link {}",
        core::mem::size_of::<ToActor<'static, P<'static>>>(),
        core::mem::size_of::<FromActor<'static, P<'static>>>(),
        core::mem::size_of::<Returned<'static, P<'static>>>(),
        core::mem::size_of::<StationTx<'static, P<'static>>>(),
        core::mem::size_of::<ActorLink<'static, P<'static>>>(),
    );
    std::println!(
        "Command {} Group {} Peer {} Outcome {} Rx {} Terminal {}",
        core::mem::size_of::<ActorCommand<'static, P<'static>>>(),
        core::mem::size_of::<GroupPending<'static, ActorEpochPath<'static, P<'static>>>>(),
        core::mem::size_of::<Peer<ModelRxMeta>>(),
        core::mem::size_of::<ActorSingleOutcome<'static, P<'static>>>(),
        core::mem::size_of::<PendingRxBlockAck<'static, ActorEpochPath<'static, P<'static>>>>(),
        core::mem::size_of::<ActorTerminal<'static, P<'static>>>(),
    );
}

#[test]
fn a_stopped_port_reinstalls_under_the_same_actor_without_a_reset() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let clock = Rc::new(VirtualClock::<4>::new());
        let mut installations = Installations::claim(claim);
        let (install, air) = ModelPort::install(
            &mut installations,
            channel6(),
            [
                IndividualAddress::new(LOCAL).unwrap(),
                IndividualAddress::new(BSSID).unwrap(),
            ],
            clock.clone(),
        )
        .unwrap();
        let ModelInstall {
            port, resources, ..
        } = install;
        let storage = Box::leak(Box::new(
            crate::station::RxBlockAckStorage::<Types<'_>, 2, 4>::new(
                resources.shared.block_acks,
                oer_portable_time::Duration::from_millis(100),
            ),
        ));
        let actor = Actor::<P<'_>, _, _>::new(
            port,
            EspressifRateSelection::<ModelCaps24>::new(),
            storage.view(),
            Recorder::default(),
        );
        let generation = actor.port().generation();
        // The radio never ran; the network keeps no unit: the installation
        // retires and the next one takes its place under the same actor.
        #[expect(
            clippy::result_large_err,
            reason = "the model's refusal returns its port"
        )]
        let actor = actor
            .reinstall(|port| {
                port.reinstall(&mut installations)
                    .map(|install| install.port)
                    .map_err(|refused| *refused)
            })
            .ok()
            .unwrap();
        assert_eq!(actor.port().generation(), generation + 1);
        assert_eq!(air.disable_calls(), 0, "no stop, no reset");
    }));
}

#[test]
fn a_frame_the_running_port_refuses_ends_withdrawn_and_the_actor_runs_on() {
    bench(|bench| {
        bench.start();
        // The port closes its admission while the actor still runs it.
        bench.air.poison();
        let transmit = station_frame_under(bench, 0, false);
        assert!(
            bench
                .to_actor
                .try_send(ToActor::StationTx(transmit))
                .is_ok()
        );
        bench.poll();
        match bench.returned() {
            Some(Returned::Terminal {
                terminal:
                    LowerMacTerminal::TxCompleted(
                        oer_protocols_ieee80211_contracts_port::TxCompleted::Single(_),
                    ),
                grouped: false,
            }) => {}
            _ => panic!("the frame's withdrawn terminal"),
        }
        assert!(bench.air.sent().is_empty(), "never on the air");
        assert!(bench.end.is_none(), "a refusal ends nothing");
    });
}

#[test]
fn a_due_port_deadline_reposts_a_bounded_number_of_turns_then_the_actor_sleeps() {
    bench(|bench| {
        bench.start();
        // A queued station frame makes the model's deadline due at once,
        // and again while its publication waits for the next service.
        let transmit = station_frame_under(bench, 0, false);
        assert!(
            bench
                .to_actor
                .try_send(ToActor::StationTx(transmit))
                .is_ok()
        );
        let polls = bench.settle();
        assert!(
            polls <= 1 + usize::from(DUE_REPOSTS),
            "{polls} polls for one frame"
        );
        assert_eq!(bench.air.sent().len(), 1);
        // Asleep: a poll finds nothing and wakes nothing.
        let wakes = bench.wakes();
        let reads = bench.clock.reads.get();
        bench.poll();
        assert_eq!(bench.wakes(), wakes);
        assert_eq!(bench.clock.reads.get(), reads);
    });
}

#[test]
fn a_reinstall_in_place_runs_on_the_placed_port_and_a_refusal_leaves_the_actor() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let clock = Rc::new(VirtualClock::<4>::new());
        let mut installations = Installations::claim(claim);
        let (install, _air) = ModelPort::install(
            &mut installations,
            channel6(),
            [
                IndividualAddress::new(LOCAL).unwrap(),
                IndividualAddress::new(BSSID).unwrap(),
            ],
            clock,
        )
        .unwrap();
        let ModelInstall {
            port, resources, ..
        } = install;
        let storage = Box::leak(Box::new(
            crate::station::RxBlockAckStorage::<Types<'_>, 2, 4>::new(
                resources.shared.block_acks,
                oer_portable_time::Duration::from_millis(100),
            ),
        ));
        let mut actor = Actor::<P<'_>, _, _>::init_in_place(
            Box::leak(Box::new(core::mem::MaybeUninit::uninit())),
            port,
            EspressifRateSelection::<ModelCaps24>::new(),
            storage.view(),
            Recorder::default(),
        );
        let mut seen = 0;
        assert_eq!(
            actor.reinstall_in_place(|_| {
                seen += 1;
                Err::<(), _>("refused")
            }),
            Err("refused")
        );
        assert_eq!(actor.reinstall_in_place(|_| Ok::<(), ()>(())), Ok(()));
        assert_eq!(seen, 1);
    }));
}

#[test]
fn ring_work_beyond_one_pull_reposts_the_actor() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        // The port's pull ended at its own bound with the ring's work left:
        // it names the turn's instant, which no alarm can keep.
        let now = bench.clock.inner.now();
        bench.air.defer_receive(now);
        bench.air.receive(&data_from_ap(1, &[9, 9, 9, 9]), meta());
        bench.settle();
        assert!(
            matches!(bench.receive.try_receive(), Some(Received::Lent(_))),
            "the next turn pulled the ring on"
        );
    });
}

#[test]
fn a_passed_port_deadline_hides_no_later_receive_continuation() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        // The port's deadline stays passed: the actor reposts it a bounded
        // number of turns, then sleeps.
        let now = bench.clock.inner.now();
        bench.air.keep_due(Some(now));
        bench.settle();
        // The ring continues only later, at its own instant.
        let at = now.saturating_add(oer_portable_time::Duration::from_millis(1));
        bench.air.defer_receive(at);
        bench.air.receive(&data_from_ap(1, &[9, 9, 9, 9]), meta());
        bench.settle();
        assert!(bench.receive.try_receive().is_none());
        assert_eq!(
            bench.clock.inner.next_deadline(),
            Some(at),
            "the alarm keeps the ring's continuation"
        );
        // Only the clock moves: the alarm alone wakes the actor.
        let wakes = bench.wakes();
        bench
            .clock
            .inner
            .advance(oer_portable_time::Duration::from_millis(1))
            .unwrap();
        assert!(bench.wakes() > wakes, "the alarm woke the actor");
        bench.settle();
        assert!(matches!(
            bench.receive.try_receive(),
            Some(Received::Lent(_))
        ));
    });
}

#[test]
fn a_unit_that_lands_during_a_turn_takes_a_repost() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench.air.receive(&data_from_ap(1, &[1, 1, 1, 1]), meta());
        // The second unit lands after the turn found the ring empty: the
        // actor's own poll of the facts after the turn takes its signal,
        // so no wake comes for it.
        bench
            .air
            .receive_during_pull(&data_from_ap(2, &[2, 2, 2, 2]), meta());
        bench.settle();
        let mut lent = 0;
        while let Some(Received::Lent(_)) = bench.receive.try_receive() {
            lent += 1;
        }
        assert_eq!(lent, 2, "the repost pulled the landed unit");
    });
}

#[test]
fn every_unit_the_port_gives_is_observed_once_before_the_reordering() {
    bench(|bench| {
        OBSERVED.with(|observed| observed.borrow_mut().clear());
        bench.start();
        bench.connect(link(true, None));
        let frames = [data_from_ap(1, &[1; 4]), foreign_data(2), management(3)];
        for frame in &frames {
            bench.air.receive(frame, meta());
        }
        bench.settle();
        let observed = OBSERVED.with(|observed| observed.take());
        assert_eq!(observed, frames.to_vec());
    });
}

#[test]
fn standing_ring_work_the_actor_cannot_take_reposts_a_bounded_number_of_turns() {
    bench(|bench| {
        bench.start();
        // The lifecycle keeps the frames it got: the port holds the rest
        // and keeps giving the ring's work as a fact.
        for sequence in 0..6 {
            bench.air.receive(&management(sequence), meta());
        }
        let polls = bench.settle();
        assert!(polls <= 1 + usize::from(DUE_REPOSTS), "{polls} polls");
        // Asleep: a poll serves the standing fact and wakes nothing.
        let wakes = bench.wakes();
        bench.poll();
        assert_eq!(bench.wakes(), wakes);
    });
}

/// A TBTT fires during a turn of an actor that already slept on standing
/// ring work it cannot take: the facts after that turn take its wake, and
/// the actor's next turn still serves it and tells the lifecycle.
#[test]
fn a_tbtt_after_turns_of_standing_work_takes_a_repost() {
    bench(|bench| {
        bench.start();
        for sequence in 0..6 {
            bench.air.receive(&management(sequence), meta());
        }
        bench.settle();
        // Asleep past its reposts: a poll serves the standing fact and
        // wakes nothing.
        let wakes = bench.wakes();
        bench.poll();
        assert_eq!(bench.wakes(), wakes);
        bench.air.tbtt_during_receive();
        bench.poll();
        assert!(bench.wakes() > wakes, "the TBTT takes a repost");
        bench.settle();
        let mut told = 0;
        while let Some(message) = bench.next() {
            if matches!(message, FromActor::Tbtt(_)) {
                told += 1;
            }
        }
        assert_eq!(told, 1, "the lifecycle hears the TBTT once");
    });
}

/// The port fails before the stop: the stop answers once, unconfirmed with
/// the port's report, the actor keeps running until the image's reset and
/// sleeps meanwhile.
#[test]
fn a_stop_of_a_failed_port_answers_unconfirmed_once_and_the_actor_sleeps() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench.air.poison();
        let deadline = bench
            .clock
            .inner
            .now()
            .saturating_add(oer_portable_time::Duration::from_millis(50));
        bench.send(ToActor::Stop {
            deadline,
            network: NetworkEnd::Stays,
        });
        let mut stopped = Vec::new();
        while let Some(message) = bench.next() {
            if let FromActor::Stopped(outcome) = message {
                stopped.push(outcome);
            }
        }
        match stopped.as_slice() {
            [StopOutcome::Unconfirmed(observed)] => {
                assert!(observed.port.is_some(), "{observed:?}");
                assert!(!observed.deadline);
            }
            other => panic!("one unconfirmed stop: {other:?}"),
        }
        assert!(bench.end.is_none(), "only the image ends it");
        let wakes = bench.wakes();
        bench.poll();
        bench.poll();
        assert_eq!(bench.wakes(), wakes, "asleep");
        assert!(bench.next().is_none());
    });
}

/// The network queued its MSDUs before the actor ran: one turn takes every
/// one the data machine holds.
#[test]
fn the_network_wakes_the_actor_per_batch_while_an_arena_is_on_the_air() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        // The first MSDU's arena stays on the air.
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, true);
        bench.send_msdu(msdu(0, &[0; 8]));
        bench.settle();
        let _traffic = bench.next();
        assert_eq!(bench.data_sent().len(), 1);
        // Below half the queue the network's MSDUs wake nothing.
        let batch = TX_DEPTH / 2;
        let wakes = bench.wakes();
        for round in 1..batch {
            bench.send_msdu(msdu(0, &[round as u8; 8]));
        }
        assert_eq!(bench.wakes(), wakes, "no wake below the batch");
        bench.send_msdu(msdu(0, &[batch as u8; 8]));
        assert_eq!(bench.wakes(), wakes + 1, "one wake for the batch");
        bench.settle();
        assert_eq!(bench.data_sent().len(), 1, "the standby waits");
        // The completion goes on with every queued MSDU, each in turn.
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, false);
        bench.settle();
        for _ in 0..batch {
            bench.advance(1_000);
            bench.settle();
        }
        assert_eq!(bench.data_sent().len(), batch + 1);
    });
}

#[test]
fn a_completion_takes_what_the_network_queued_below_its_batch_first() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, true);
        bench.send_msdu(msdu(0, &[1; 8]));
        bench.settle();
        let _traffic = bench.next();
        // One MSDU below the batch: no wake, it waits in the queue.
        let wakes = bench.wakes();
        bench.send_msdu(msdu(0, &[2; 8]));
        assert_eq!(bench.wakes(), wakes);
        bench
            .air
            .hold_completion(WmmAccessCategory::BestEffort, false);
        let polls = bench.settle();
        assert_eq!(polls, 1, "taken, completed and published in one turn");
        assert_eq!(bench.data_sent().len(), 2);
        assert_eq!(bench.transmit.free_capacity(), TX_DEPTH);
    });
}

#[test]
fn one_turn_takes_every_msdu_the_network_queued() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        // Nothing leaves for the air: the arenas keep what they take.
        bench.air.hold(WmmAccessCategory::BestEffort, true);
        for round in 0..2_u8 {
            bench.send_msdu(msdu(0, &[round; 8]));
        }
        let polls = bench.polls;
        bench.poll();
        assert_eq!(bench.polls - polls, 1);
        assert_eq!(
            bench.transmit.free_capacity(),
            TX_DEPTH,
            "one turn took every queued MSDU"
        );
    });
}

#[test]
fn every_actor_span_is_found_by_its_number_and_no_other_number_is() {
    for span in ActorSpan::ALL {
        assert_eq!(ActorSpan::from_number(span as u8), Some(span));
    }
    assert_eq!(ActorSpan::from_number(ActorSpan::ALL.len() as u8), None);
}

/// A participant lost while the station still holds the network's memory:
/// the return queue is full and the last ended frame's memory waits in the
/// actor. The stop does not settle while it does; once the queue has room
/// again the memory goes home, and only then the stop ends with the loss,
/// holding nothing, and the actor retires.
#[test]
fn a_lost_network_settles_only_once_every_ended_frame_s_memory_went_home() {
    bench(|bench| {
        bench.start();
        bench.connect(link(true, None));
        let total = TX_DEPTH + 1;
        for n in 0..total {
            bench.send_msdu(msdu(0, &[n as u8; 8]));
            bench.settle();
        }
        bench.advance(1);
        bench.settle();
        while bench.next().is_some() {}
        retire(bench, 50);
        bench
            .network
            .answer
            .set(Some(Err(crate::station::NetworkStopLost)));
        bench.settle();
        assert!(bench.next().is_none(), "the actor still holds memory");
        let mut home = 0;
        while let Ok(Some(_memory)) = bench.returned.try_receive_with(Option::take) {
            home += 1;
        }
        bench.settle();
        let mut answer = None;
        while let Some(message) = bench.next() {
            if let FromActor::Stopped(outcome) = message {
                answer = Some(outcome);
            }
        }
        match answer {
            Some(StopOutcome::NetworkLost(observed)) => assert_eq!(observed.memory_held, 0),
            other => panic!("the lost stop settles with nothing held: {other:?}"),
        }
        while let Ok(Some(_memory)) = bench.returned.try_receive_with(Option::take) {
            home += 1;
        }
        assert_eq!(home, total, "every MSDU's memory is home");
        bench.send(ToActor::Retire);
        assert_eq!(bench.end, Some(ActorEnd::Retired));
    });
}
