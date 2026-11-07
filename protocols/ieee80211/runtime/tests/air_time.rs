//! The real actor and data plane on a model air with time: what a channel
//! allows, which limit binds, and how that changes with each input.
//!
//! The test plays the lifecycle and the network; the actor, the data plane,
//! the receive agreements and the link are the product's. Every software
//! turn takes no time, so these scenarios bound what the air allows and
//! say nothing of the CPU. They assert structure: which limit binds and
//! which way a number moves, not its value. [`validation`] holds the
//! comparison with the stand.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use core::cell::Cell;
use core::convert::Infallible;
use core::future::Future;
use core::marker::PhantomData;
use core::num::{NonZeroU8, NonZeroU32};
use core::pin::Pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Poll, Waker};
use std::boxed::Box;
use std::rc::Rc;
use std::sync::Arc;
use std::vec::Vec;

use oer_portable_memory::{
    SlotReceiver, SlotSender, WakingSlotQueue, WakingSpscQueue, WakingSpscReceiver,
    WakingSpscSender,
};
use oer_portable_time::{Clock, Duration, Instant};
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_contracts_port::{
    AggregateOutcome, Channel, ChannelWidth, InstalledPairwise, KeyTypes, LowerMacTerminal,
    PhyRate, PortCommands, PortTypes, Protection, RegulatedChannel, RetryPlan, RetryStage,
    SingleOutcome, SupportedChannel, Tid,
};
use oer_protocols_ieee80211_core_roles_sta::data::plane::{DataLink, HtLimits, TxWindow};
use oer_protocols_ieee80211_core_roles_sta::data::{
    AggregateRetry, MSDU_ETHERNET_OFFSET, WmmUserPriority,
};
use oer_protocols_ieee80211_core_roles_sta::epoch::LinkEpoch;
use oer_protocols_ieee80211_core_roles_sta::link::{LinkHandover, LinkReturn};
use oer_protocols_ieee80211_core_roles_sta::machine::PendingRxBlockAck;
use oer_protocols_ieee80211_core_upper_mac::ccmp::{CcmpPacketNumberStep, PendingCcmpTxKey};
use oer_protocols_ieee80211_core_upper_mac::qos::AcAdmission;
use oer_protocols_ieee80211_core_upper_mac::rate_control::{BlockAckTally, RatePeer};
use oer_protocols_ieee80211_core_upper_mac::rate_selection::{FrameRates, RateSelection};
use oer_protocols_ieee80211_runtime::datapath::{
    Actor, ActorEnd, ActorLink, DatapathPort, FromActor, GroupPending, LifecycleEnds, NetworkEnds,
    PARCELS, Parcel, Returned, ToActor,
};
use oer_protocols_ieee80211_runtime::station::{
    NetworkStop, NetworkStopLost, Outgoing, RX_COPIED_DEPTH, Received, RxBlockAckStorage, SameHart,
};
use oer_protocols_ieee80211_testing_model::{
    AirConfig, AirHolder, AirPeer, AirReport, AirTiming, BusyChannel, MODEL_KEY_SLOTS, ModelAck,
    ModelAir, ModelBackend, ModelBlockAckEvidence, ModelCaps24, ModelContext, ModelInstall,
    ModelLentRx, ModelNetworkMemory, ModelPort, ModelPortFault, ModelResources, ModelRxLimits,
    ModelRxMeta, ModelSmall, ModelTypes, Offered, PeerWindow,
};
use oer_protocols_ieee80211_wire::address::{Bssid, IndividualAddress, OwnAddress};
use oer_protocols_ieee80211_wire::block_ack::frame::BlockAckAction;
use oer_protocols_ieee80211_wire::phy::{HtMcs, HtRate, PpduBandwidth};
use oer_protocols_ieee80211_wire::security::{LinkProtection, Spp};
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;
use oer_protocols_ieee80211_wire::station::StaDataHeaderTemplate;
use oer_protocols_radio_port::{Installations, LocalId, SlotIdentity, with_local_slot};

const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 0x09];

const TO: usize = 16;
const RETURNS: usize = 40;
const FROM: usize = 64;
const TX_DEPTH: usize = 16;
/// The network queue's depth: the S31 composition's `RX_DEPTH`.
const RX_DEPTH: usize = 32;
/// The reorder frames of the receive agreements: the S31 composition's
/// `RX_BA_FRAMES`, its `RX_HELD`.
const REORDER_FRAMES: usize = 16;
const TID: u8 = 0;

/// The UDP payload of the stand's load, in octets.
const UDP_PAYLOAD: u64 = 1472;
/// The Ethernet payload that carries it: IP and UDP headers around it.
const IP_PACKET: usize = 1500;

/// The stand's PHY mode: HT40, short guard interval, MCS 7.
fn stand_rate() -> PhyRate {
    PhyRate::Ht(HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz40, true).unwrap())
}

fn channel() -> Channel {
    Channel::ghz2_4(9, ChannelWidth::Mhz40Below).unwrap()
}

fn supported() -> SupportedChannel<ModelCaps24> {
    SupportedChannel::new(
        oer_protocols_radio_regulatory::TEST_DOMAIN
            .permit(RegulatedChannel::from(channel()))
            .unwrap(),
    )
    .unwrap()
}

/// The access point's frames of the stand's load, QoS data of [`TID`]: an
/// open image, since a link for a test holds no replay state. The air
/// carries them under CCMP all the same ([`PEER_OVERHEAD`]).
fn peer_frames() -> impl FnMut(SequenceNumber) -> Vec<u8> {
    |sequence| {
        let mut frame = std::vec![0_u8; 26];
        frame[0..2].copy_from_slice(&0x0288_u16.to_le_bytes());
        frame[4..10].copy_from_slice(&LOCAL);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&PEER);
        frame[22..24].copy_from_slice(&sequence.sequence_control().to_le_bytes());
        frame[24] = TID;
        frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
        frame.resize(frame.len() + IP_PACKET, 0x45);
        frame
    }
}

/// What each of the access point's frames carries on the air beside its
/// image: the CCMP header, the MIC and the FCS.
const PEER_OVERHEAD: u32 = 8 + 8 + 4;

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

/// The model's port as the actor's, holding `HELD` units: `RX_HELD`.
struct AirPort<'s, const HELD: u8>(PhantomData<fn(&'s ()) -> &'s ()>);

impl<'s, const HELD: u8> DatapathPort<'s> for AirPort<'s, HELD> {
    type Port = ModelPort<'s, LocalId<'s>, ModelCaps24, Contexts>;
    type Caps = ModelCaps24;
    type Types = Types<'s>;
    type Small = ModelSmall;
    type TxMemory = ModelNetworkMemory;
    type LentRx = ModelLentRx;
    const RX_HELD: u8 = HELD;

    fn arena_ack(response: ModelBlockAckEvidence) -> Option<ModelAck> {
        match response {
            ModelBlockAckEvidence::Measured(snr_db) => Some(ModelAck { snr_db }),
            ModelBlockAckEvidence::Unmeasured => None,
        }
    }
}

/// One rate for every frame: an input here, not a rate selection under test.
struct FixedRate(PhyRate);

impl RateSelection<ModelRxMeta, ModelAck, ModelBlockAckEvidence> for FixedRate {
    fn packet_number_step(&self) -> CcmpPacketNumberStep {
        CcmpPacketNumberStep::ONE
    }

    fn plan<R>(
        &self,
        _: FrameRates,
        carried: impl Fn(PhyRate) -> Option<R>,
    ) -> Option<RetryPlan<R>> {
        Some(RetryPlan::new(RetryStage {
            rate: carried(self.0)?,
            transmissions: NonZeroU8::MIN,
            protection: Protection::None,
        }))
    }

    fn associated(&mut self, _: &RatePeer, _: &ModelRxMeta) {}

    fn disconnected(&mut self) {}

    fn completed<D>(&mut self, _: Instant, _: FrameRates, _: &SingleOutcome<ModelAck, D>) {}

    fn completed_aggregate<D>(
        &mut self,
        _: Instant,
        _: BlockAckTally,
        _: &AggregateOutcome<ModelBlockAckEvidence, D>,
    ) {
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

/// A network side that takes frames and never stops.
struct Network;

impl NetworkStop for Network {
    fn takes_frames(&self) -> bool {
        true
    }
    fn request_stop(&self) {}
    fn poll_stopped(&self, _: &mut core::task::Context<'_>) -> Poll<Result<(), NetworkStopLost>> {
        Poll::Pending
    }
}

/// How much a side of the network moves: without a limit, or one frame per
/// so many nanoseconds, an explicit input and no cost model.
#[derive(Clone, Copy, Debug)]
enum Pace {
    Unlimited,
    PerMillisecond(u32),
}

impl Pace {
    fn interval_nanos(self) -> Option<u64> {
        match self {
            Self::Unlimited => None,
            Self::PerMillisecond(frames) => Some(1_000_000 / u64::from(frames)),
        }
    }
}

/// One side's turn: the next instant it moves a frame.
#[derive(Clone, Copy, Debug)]
struct Turnstile {
    interval: Option<u64>,
    next_nanos: u64,
}

impl Turnstile {
    fn new(pace: Pace) -> Self {
        Self {
            interval: pace.interval_nanos(),
            next_nanos: 0,
        }
    }

    /// Whether a frame may move at `nanos`; if so, it moved.
    fn pass(&mut self, nanos: u64) -> bool {
        match self.interval {
            None => true,
            Some(_) if nanos < self.next_nanos => false,
            Some(interval) => {
                self.next_nanos = self.next_nanos.max(nanos.saturating_sub(interval)) + interval;
                true
            }
        }
    }

    fn next_micros(&self) -> Option<u64> {
        self.interval.map(|_| self.next_nanos.div_ceil(1_000))
    }
}

/// The inputs of one run.
#[derive(Clone, Copy)]
struct Inputs {
    seed: u64,
    /// The transmit agreement's window; `None` sends nothing.
    tx_window: Option<u16>,
    /// What the network offers the station.
    tx_pace: Pace,
    /// The access point's window; `None` sends nothing.
    rx_window: Option<PeerWindow>,
    /// What the access point offers.
    rx_offered: Offered,
    /// What the network takes.
    rx_pace: Pace,
    /// The subframes of a data arena.
    arena_frames: u8,
    /// The units the network may hold.
    lent: usize,
    /// The ring's slots.
    ring: usize,
    busy: Option<BusyChannel>,
    timing: AirTiming,
    /// The exchange that protects the access point's PPDUs.
    peer_protection: Protection,
    warm_up_ms: u64,
    measure_ms: u64,
}

impl Inputs {
    /// The stand's mode, one direction unset, in a quiet channel.
    fn stand() -> Self {
        Self {
            seed: 1,
            tx_window: None,
            tx_pace: Pace::Unlimited,
            rx_window: None,
            rx_offered: Offered::Saturated,
            rx_pace: Pace::Unlimited,
            arena_frames: 32,
            lent: 32,
            ring: 96,
            busy: None,
            timing: AirTiming::HT_2_4_GHZ,
            peer_protection: Protection::None,
            warm_up_ms: 30,
            measure_ms: 200,
        }
    }

    fn transmitting(window: u16) -> Self {
        Self {
            tx_window: Some(window),
            ..Self::stand()
        }
    }

    fn receiving(window: PeerWindow) -> Self {
        Self {
            rx_window: Some(window),
            ..Self::stand()
        }
    }
}

/// What one run measured after its warm-up.
#[derive(Clone, Copy, Debug)]
struct Outcome {
    air: AirReport,
    /// Frames the air acknowledged and the network got back.
    sent: u64,
    /// Frames the network took.
    taken: u64,
    measure_micros: u64,
    /// No two transmissions overlapped, a collision's two sides aside.
    one_at_a_time: bool,
    /// The window the port applied for [`TID`].
    agreed: Option<u16>,
}

impl Outcome {
    fn megabits(&self, frames: u64) -> f64 {
        (frames * UDP_PAYLOAD * 8) as f64 / self.measure_micros as f64
    }

    /// The UDP payload the network sent, in Mbit/s.
    fn tx_mbps(&self) -> f64 {
        self.megabits(self.sent)
    }

    /// The UDP payload the network took, in Mbit/s.
    fn rx_mbps(&self) -> f64 {
        self.megabits(self.taken)
    }

    /// The station's subframes per exchange.
    fn tx_aggregate(&self) -> f64 {
        self.air.station.subframes as f64 / self.air.station.exchanges.max(1) as f64
    }

    /// The access point's subframes per exchange.
    fn rx_aggregate(&self) -> f64 {
        self.air.peer.subframes as f64 / self.air.peer.exchanges.max(1) as f64
    }

    /// The subframes that landed per access point's exchange.
    fn rx_landed(&self) -> f64 {
        self.air.landed as f64 / self.air.peer.exchanges.max(1) as f64
    }

    /// The share of the time the channel held someone.
    fn occupancy(&self) -> f64 {
        (self.air.station.air_micros + self.air.peer.air_micros + self.air.busy_micros) as f64
            / self.measure_micros as f64
    }
}

type Run<'b> = Pin<&'b mut dyn Future<Output = ActorEnd<ModelPortFault>>>;
type P<'s, const HELD: u8> = AirPort<'s, HELD>;

/// The lifecycle's and the network's side of one actor.
struct Bench<'b, 's, const HELD: u8> {
    clock: &'b VirtualClock,
    air: Rc<ModelAir>,
    run: Run<'b>,
    counter: Arc<Counter>,
    end: Option<ActorEnd<ModelPortFault>>,
    to_actor: WakingSpscSender<'b, ToActor<'s, P<'s, HELD>>, TO>,
    from_actor: WakingSpscReceiver<'b, FromActor<'s, P<'s, HELD>>, FROM>,
    returns: WakingSpscReceiver<'b, Returned<'s, P<'s, HELD>>, RETURNS>,
    link: WakingSpscSender<'b, ActorLink<'s, P<'s, HELD>>, 1>,
    transmit: SlotSender<'b, Option<Outgoing<ModelNetworkMemory>>, TX_DEPTH>,
    returned: SlotReceiver<'b, Option<ModelNetworkMemory>, TX_DEPTH>,
    receive: WakingSpscReceiver<'b, Received<ModelLentRx>, RX_DEPTH>,
    copies: WakingSpscReceiver<
        'b,
        oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame,
        RX_COPIED_DEPTH,
    >,
    /// Memory the network has back.
    pool: Vec<ModelNetworkMemory>,
    transmits: bool,
    tx_turn: Turnstile,
    rx_turn: Turnstile,
    sent: u64,
    taken: u64,
}

impl<'s, const HELD: u8> Bench<'_, 's, HELD> {
    fn wakes(&self) -> usize {
        self.counter.0.load(Ordering::Relaxed)
    }

    fn poll(&mut self) {
        let waker = Waker::from(self.counter.clone());
        let mut cx = core::task::Context::from_waker(&waker);
        if let Poll::Ready(end) = self.run.as_mut().poll(&mut cx) {
            self.end = Some(end);
        }
    }

    /// Poll the actor while it was woken since its last poll.
    fn settle(&mut self) {
        let mut seen = self.wakes();
        self.poll();
        let mut polls = 0;
        while self.wakes() != seen && self.end.is_none() {
            seen = self.wakes();
            self.poll();
            polls += 1;
            assert!(polls < 1000, "the actor never sleeps");
        }
    }

    fn send(&mut self, message: ToActor<'s, P<'s, HELD>>) {
        assert!(
            self.to_actor.try_send(message).is_ok(),
            "the queue has room"
        );
        self.settle();
    }

    fn start(&mut self) {
        self.send(ToActor::Start(supported()));
        assert!(matches!(
            self.from_actor.try_receive(),
            Some(FromActor::Started(Ok(())))
        ));
    }

    fn connect(&mut self, link: ActorLink<'s, P<'s, HELD>>) {
        assert!(self.link.try_send(link).is_ok());
        self.send(ToActor::Connect);
        self.send(ToActor::Release);
    }

    fn now_micros(&self) -> u64 {
        self.clock.now().as_micros()
    }

    /// The network's memory of one more MSDU.
    fn memory(&mut self) -> ModelNetworkMemory {
        self.pool.pop().unwrap_or_else(|| {
            let mut memory: ModelNetworkMemory =
                Box::new([0; oer_protocols_ieee80211_testing_model::MODEL_MAX_MPDU]);
            memory[MSDU_ETHERNET_OFFSET..][..6].copy_from_slice(&PEER);
            memory[MSDU_ETHERNET_OFFSET + 6..][..6].copy_from_slice(&LOCAL);
            memory[MSDU_ETHERNET_OFFSET + 12..][..2].copy_from_slice(&0x0800_u16.to_be_bytes());
            memory
        })
    }

    /// The network's side at the current instant: memory back, MSDUs in
    /// and frames out, within their paces; whether it moved anything.
    fn network(&mut self) -> bool {
        let nanos = self.now_micros() * 1_000;
        let mut moved = false;
        while let Ok(Some(memory)) = self.returned.try_receive_with(Option::take) {
            self.sent += 1;
            self.pool.push(memory);
            moved = true;
        }
        while self.transmits && self.transmit.free_capacity() != 0 && self.tx_turn.pass(nanos) {
            let mut outgoing = Some(Outgoing {
                memory: self.memory(),
                length: u16::try_from(14 + IP_PACKET).unwrap(),
                priority: WmmUserPriority::new(TID).unwrap(),
            });
            assert!(
                self.transmit
                    .try_send_with(|slot| {
                        *slot = outgoing.take();
                        Ok::<_, Infallible>(())
                    })
                    .is_ok()
            );
            moved = true;
        }
        while !self.receive.is_empty() && self.rx_turn.pass(nanos) {
            let frame = self.receive.try_receive();
            if matches!(frame, Some(Received::Copied(_))) {
                drop(self.copies.try_receive());
            }
            drop(frame);
            self.taken += 1;
            moved = true;
        }
        moved
    }

    /// What the lifecycle hears: this bench answers none of it.
    fn drain_messages(&mut self) {
        while let Some(message) = self.from_actor.try_receive() {
            drop(message);
        }
        while let Some(returned) = self.returns.try_receive() {
            drop(returned);
        }
    }

    /// Everything that can happen at the current instant.
    fn at_this_instant(&mut self) {
        for round in 0.. {
            assert!(
                round < 10_000,
                "the world does not move at {:?}",
                self.clock.now()
            );
            let now = self.clock.now();
            let due = self.air.next_air_event().is_some_and(|at| at <= now);
            if due {
                self.air.clock_advanced();
            }
            let moved = self.network() || due;
            self.settle();
            self.drain_messages();
            assert!(
                self.end.is_none(),
                "the actor ended: {:?}, the air: {:?}",
                self.end,
                self.air.air_fault()
            );
            if !moved {
                return;
            }
        }
    }

    /// Run until `micros`.
    fn run_until(&mut self, micros: u64) {
        while self.now_micros() < micros {
            self.at_this_instant();
            let waiting = !self.receive.is_empty();
            let next = [
                self.air.next_air_event().map(Instant::as_micros),
                self.clock.next_deadline().map(Instant::as_micros),
                self.transmits.then(|| self.tx_turn.next_micros()).flatten(),
                waiting.then(|| self.rx_turn.next_micros()).flatten(),
            ]
            .into_iter()
            .flatten()
            .filter(|at| *at > self.now_micros())
            .min()
            .expect("the world has a next instant");
            self.clock
                .advance_to(Instant::from_micros(next.min(micros)));
            self.air.clock_advanced();
        }
    }
}

/// The station's link: its data under `key`, what it receives open
/// ([`peer_frames`]), the access point's HT limits on it.
fn link<'s, const HELD: u8>(
    key: <AirPort<'s, HELD> as Keyed<'s>>::Key,
) -> ActorLink<'s, P<'s, HELD>> {
    let address = OwnAddress::new(IndividualAddress::new(LOCAL).unwrap());
    let bssid = Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap();
    let data = DataLink {
        header: StaDataHeaderTemplate::new(address, bssid, true, LinkProtection::Ccmp).unwrap(),
        admission: AcAdmission::OPEN,
        retry: Some(AggregateRetry {
            lifetime: Duration::from_millis(1_500),
            attempts: 4,
        }),
        ht: Some(HtLimits {
            spacing: oer_protocols_ieee80211_contracts_port::MpduStartSpacing::new(0).unwrap(),
            max_length: 65_535,
        }),
        windows: [None; 8],
    };
    LinkHandover::for_test(1, address, bssid, false, true, Some(key), Some(data))
}

/// The link key's type on the port of `D`.
trait Keyed<'s> {
    type Key;
}

impl<'s, const HELD: u8> Keyed<'s> for AirPort<'s, HELD> {
    type Key = oer_protocols_ieee80211_runtime::datapath::ActorLinkKey<'s, Self>;
}

/// One run of `inputs` with `HELD` units.
fn run<const HELD: u8>(inputs: Inputs) -> Outcome {
    let result = Cell::new(None);
    embassy_futures::block_on(with_local_slot(async |claim| {
        let clock = Rc::new(VirtualClock::<8>::new());
        let mut installations = Installations::claim(claim);
        let (install, air) = ModelPort::install(
            &mut installations,
            channel(),
            [
                IndividualAddress::new(LOCAL).unwrap(),
                IndividualAddress::new(BSSID).unwrap(),
            ],
            clock.clone(),
        )
        .unwrap();
        let ModelInstall {
            port,
            mut commands,
            resources,
        } = install;
        let ModelResources {
            block_acks,
            station_association,
            station_pairwise,
            ..
        } = resources;
        air.set_air_time(AirConfig {
            seed: inputs.seed,
            timing: inputs.timing,
            arena_frames: inputs.arena_frames,
            rx_limits: ModelRxLimits {
                ring: inputs.ring,
                held: usize::from(HELD),
                lent: inputs.lent,
            },
            busy: inputs.busy,
            peer: inputs.rx_window.map(|window| {
                let mut peer = AirPeer::best_effort(
                    Box::new(peer_frames()),
                    ModelRxMeta::new(channel(), -40, -95),
                    stand_rate(),
                    inputs.rx_offered,
                    window,
                    Tid::new(TID).unwrap(),
                );
                peer.air_overhead_octets = PEER_OVERHEAD;
                peer.protection = inputs.peer_protection;
                peer
            }),
        })
        .unwrap();
        let storage = Box::leak(Box::new(
            RxBlockAckStorage::<Types<'_>, 2, REORDER_FRAMES>::new(
                block_acks,
                Duration::from_millis(100),
            ),
        ));
        let mut actor = Actor::<P<'_, HELD>, _, _>::init_in_place(
            Box::leak(Box::new(core::mem::MaybeUninit::uninit())),
            port,
            FixedRate(stand_rate()),
            storage.view(),
            (),
        );
        let mut to_queue = WakingSpscQueue::<ToActor<'_, P<'_, HELD>>, TO>::new();
        let mut parcel_queue = WakingSpscQueue::<Parcel<'_, P<'_, HELD>>, PARCELS>::new();
        let mut from_queue = WakingSpscQueue::<FromActor<'_, P<'_, HELD>>, FROM>::new();
        let mut return_queue = WakingSpscQueue::<Returned<'_, P<'_, HELD>>, RETURNS>::new();
        let mut link_queue = WakingSpscQueue::<ActorLink<'_, P<'_, HELD>>, 1>::new();
        let mut group_queue = WakingSpscQueue::<
            GroupPending<
                '_,
                oer_protocols_ieee80211_runtime::datapath::ActorEpochPath<'_, P<'_, HELD>>,
            >,
            1,
        >::new();
        let mut link_return_queue = WakingSpscQueue::<LinkReturn, 1>::new();
        let mut transmit_queue =
            WakingSlotQueue::<Option<Outgoing<ModelNetworkMemory>>, TX_DEPTH>::new();
        let mut returned_queue = WakingSlotQueue::<Option<ModelNetworkMemory>, TX_DEPTH>::new();
        let mut receive_queue = WakingSpscQueue::<Received<ModelLentRx>, RX_DEPTH>::new();
        let mut copies_queue = WakingSpscQueue::<
            oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame,
            RX_COPIED_DEPTH,
        >::new();
        let (to_actor, to_actor_end) = to_queue.split();
        let (_parcels, parcels_end) = parcel_queue.split();
        let (from_actor_end, from_actor) = from_queue.split();
        let (returns_end, returns) = return_queue.split();
        let (link_sender, link_end) = link_queue.split();
        let (groups_end, _groups) = group_queue.split();
        let (link_return_end, _link_return) = link_return_queue.split();
        let (transmit, transmit_end) = transmit_queue.split(|| None);
        let (returned_end, returned) = returned_queue.split(|| None);
        let (receive_end, receive) = receive_queue.split();
        let (copies_end, copies) = copies_queue.split();
        let mut lifecycle = LifecycleEnds {
            to_actor: to_actor_end,
            parcels: parcels_end,
            from_actor: from_actor_end,
            returns: returns_end,
            groups: groups_end,
            link: link_end,
            link_return: link_return_end,
        };
        let network_side = Network;
        let mut network = NetworkEnds::<_, _, _, TX_DEPTH, RX_DEPTH, SameHart> {
            transmit: transmit_end,
            returned: returned_end,
            receive: receive_end,
            receive_copied: copies_end,
            network: &network_side,
        };
        let timer = clock.clone();
        let run = actor.run(&mut lifecycle, &mut network, &*timer);
        let mut run = core::pin::pin!(run);
        let mut bench = Bench {
            clock: &clock,
            air,
            run: run.as_mut(),
            counter: Arc::new(Counter(AtomicUsize::new(0))),
            end: None,
            to_actor,
            from_actor,
            returns,
            link: link_sender,
            transmit,
            returned,
            receive,
            copies,
            pool: Vec::new(),
            transmits: false,
            tx_turn: Turnstile::new(inputs.tx_pace),
            rx_turn: Turnstile::new(inputs.rx_pace),
            sent: 0,
            taken: 0,
        };
        bench.settle();
        bench.start();
        let association =
            <<Types<'_> as PortTypes<'_>>::StationKeys as KeyTypes<'_>>::begin_association(
                station_association,
                Spp::NotNegotiated,
            );
        let [slot, _unused]: [_; MODEL_KEY_SLOTS] = station_pairwise;
        let (command, identity) = commands.install_pairwise(
            &association,
            slot,
            IndividualAddress::new(BSSID).unwrap(),
            oer_protocols_ieee80211_contracts_security::Ccmp128Key::new(&[7; 16]),
        );
        bench.send(ToActor::Port(command));
        let key = loop {
            match bench.from_actor.try_receive() {
                Some(FromActor::Returned) => break bench.returns.try_receive(),
                Some(_) => {}
                None => panic!("the key's terminal"),
            }
        };
        let Some(Returned::Terminal {
            terminal: LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key)),
            ..
        }) = key
        else {
            panic!("the key's installation terminal");
        };
        let key = PendingCcmpTxKey::issued(identity, CcmpPacketNumberStep::ONE)
            .installed::<<Types<'_> as PortTypes<'_>>::StationKeys>(key)
            .ok()
            .unwrap();
        bench.connect(link::<HELD>(key));
        bench.settle();
        if let Some(window) = inputs.tx_window {
            bench.send(ToActor::Agreement {
                tid: WmmUserPriority::new(TID).unwrap(),
                window: Some(TxWindow {
                    head: SequenceNumber::ZERO,
                    size: window,
                }),
            });
            bench.transmits = true;
        }
        if inputs.rx_window == Some(PeerWindow::Agreed) {
            bench.send(ToActor::RxAgreement(PendingRxBlockAck::for_test(
                LinkEpoch::for_test(1),
                Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap(),
                BlockAckAction::AddbaRequest {
                    dialog_token: 1,
                    tid: TID,
                    immediate: true,
                    amsdu: false,
                    window: 64,
                    timeout_tu: 0,
                    starting_sequence: SequenceNumber::ZERO,
                },
                true,
            )));
        }
        let start = bench.now_micros();
        let warm = start + inputs.warm_up_ms * 1_000;
        bench.run_until(warm);
        bench.air.reset_air_report();
        bench.sent = 0;
        bench.taken = 0;
        let measure_micros = inputs.measure_ms * 1_000;
        bench.run_until(warm + measure_micros);
        let spans = bench.air.air_spans();
        let one_at_a_time = spans
            .iter()
            .zip(spans.iter().skip(1))
            .all(|(first, next)| first.end_micros <= next.start_micros);
        let collided = spans
            .iter()
            .filter(|span| span.holder == AirHolder::Collision)
            .count();
        let air = bench.air.air_report().unwrap();
        assert_eq!(collided as u64, air.station.collisions);
        let agreed = bench
            .air
            .receive_agreements()
            .first()
            .map(|(_, agreement)| agreement.window);
        result.set(Some(Outcome {
            agreed,
            air,
            sent: bench.sent,
            taken: bench.taken,
            measure_micros,
            one_at_a_time,
        }));
    }));
    result.take().unwrap()
}

/// The station's frames at `window` with its 32-subframe arenas.
fn transmit(window: u16) -> Outcome {
    run::<16>(Inputs::transmitting(window))
}

/// The access point's frames under `window`.
fn receive(window: PeerWindow) -> Outcome {
    run::<16>(Inputs::receiving(window))
}

/// Whether `value` lies within `tolerance` of `expected`, as a share.
fn near(value: f64, expected: f64, tolerance: f64) -> bool {
    (value - expected).abs() <= expected * tolerance
}

fn agreed() -> PeerWindow {
    PeerWindow::Agreed
}

#[test]
fn more_offered_load_than_the_air_carries_raises_no_ceiling() {
    let ceiling = transmit(64);
    assert!(ceiling.one_at_a_time);
    assert!(ceiling.occupancy() > 0.9, "{}", ceiling.occupancy());
    assert!(ceiling.tx_mbps() < 150.0, "below the PHY's 150 Mbit/s");
    for frames in [13, 16, 32] {
        let offered = run::<16>(Inputs {
            tx_pace: Pace::PerMillisecond(frames),
            ..Inputs::transmitting(64)
        });
        assert!(
            near(offered.tx_mbps(), ceiling.tx_mbps(), 0.02),
            "{offered:?}"
        );
    }
    let ceiling = receive(agreed());
    assert!(ceiling.occupancy() > 0.9, "{}", ceiling.occupancy());
    for frames in [12_000, 16_000, 32_000] {
        let offered = run::<16>(Inputs {
            rx_offered: Offered::FramesPerSecond(NonZeroU32::new(frames).unwrap()),
            ..Inputs::receiving(agreed())
        });
        assert!(
            near(offered.rx_mbps(), ceiling.rx_mbps(), 0.02),
            "{offered:?}"
        );
    }
}

#[test]
fn offered_load_below_the_ceiling_goes_through_whole_and_keeps_the_channel_free() {
    let (tx, rx) = (transmit(64), receive(agreed()));
    for frames in [2, 4, 8] {
        let sent = run::<16>(Inputs {
            tx_pace: Pace::PerMillisecond(frames),
            ..Inputs::transmitting(64)
        });
        let offered = (u64::from(frames) * UDP_PAYLOAD * 8) as f64 / 1000.0;
        assert!(near(sent.tx_mbps(), offered, 0.01), "{sent:?}");
        assert!(sent.occupancy() < tx.occupancy());
        let landed = run::<16>(Inputs {
            rx_offered: Offered::FramesPerSecond(NonZeroU32::new(frames * 1000).unwrap()),
            ..Inputs::receiving(agreed())
        });
        assert!(near(landed.rx_mbps(), offered, 0.01), "{landed:?}");
        assert!(landed.occupancy() < rx.occupancy());
        assert_eq!(landed.air.bounds.offered, landed.air.peer.exchanges);
    }
}

#[test]
fn a_transmit_aggregate_stops_at_the_arena_or_at_half_a_window_above_one_intake() {
    // The window holds the arena on the air and the one that waits: from
    // two intakes on, it splits between them.
    for window in [32, 48, 64] {
        let outcome = transmit(window);
        assert!(
            near(outcome.tx_aggregate(), f64::from(window) / 2.0, 0.01),
            "{outcome:?}"
        );
    }
    // A window one intake fills goes out whole.
    assert!(near(transmit(16).tx_aggregate(), 16.0, 0.01));
    assert!(near(transmit(8).tx_aggregate(), 8.0, 0.01));
    // The arena binds below the window.
    let small = run::<16>(Inputs {
        arena_frames: 8,
        ..Inputs::transmitting(64)
    });
    assert!(near(small.tx_aggregate(), 8.0, 0.01), "{small:?}");
    assert!(small.tx_mbps() < transmit(64).tx_mbps());
}

#[test]
fn a_transmit_window_of_32_carries_no_more_than_one_of_16() {
    let (narrow, wide, widest) = (transmit(16), transmit(32), transmit(64));
    assert!(
        near(wide.tx_mbps(), narrow.tx_mbps(), 0.01),
        "{narrow:?} {wide:?}"
    );
    assert!(near(wide.tx_aggregate(), narrow.tx_aggregate(), 0.01));
    assert!(widest.tx_mbps() > wide.tx_mbps() * 1.03, "{widest:?}");
}

#[test]
fn the_agreed_window_sets_the_receive_aggregate_and_rx_held_does_not() {
    let (narrow, wide) = (
        receive(PeerWindow::Assumed(16)),
        receive(PeerWindow::Assumed(32)),
    );
    for (outcome, window) in [(narrow, 16.0), (wide, 32.0)] {
        assert_eq!(outcome.air.bounds.window, outcome.air.peer.exchanges);
        assert!(near(outcome.rx_aggregate(), window, 0.001), "{outcome:?}");
    }
    assert!(
        wide.rx_mbps() > narrow.rx_mbps() * 1.03,
        "{narrow:?} {wide:?}"
    );
    // The units the actor may hold move nothing while the network keeps up.
    let held = run::<32>(Inputs::receiving(PeerWindow::Assumed(16)));
    assert!(near(held.rx_mbps(), narrow.rx_mbps(), 0.005), "{held:?}");
    let held = run::<8>(Inputs::receiving(PeerWindow::Assumed(32)));
    assert!(near(held.rx_mbps(), wide.rx_mbps(), 0.005), "{held:?}");
}

#[test]
fn a_recipient_agrees_its_reorder_window_and_no_more_whatever_rx_held_says() {
    use oer_protocols_ieee80211_core_upper_mac::block_ack::RX_BLOCK_ACK_WINDOW;
    for outcome in [receive(agreed()), run::<32>(Inputs::receiving(agreed()))] {
        assert_eq!(
            outcome.agreed.map(usize::from),
            Some(RX_BLOCK_ACK_WINDOW.min(REORDER_FRAMES)),
            "the request asked 64"
        );
        assert!(near(outcome.rx_aggregate(), 16.0, 0.001), "{outcome:?}");
    }
}

#[test]
fn the_longest_aggregate_stops_at_the_stations_length_when_no_window_binds() {
    let wide = receive(PeerWindow::Assumed(64));
    assert_eq!(wide.air.bounds.length, wide.air.peer.exchanges);
    // 65 535 octets of 1560-octet subframes.
    assert!(near(wide.rx_aggregate(), 42.0, 0.001), "{wide:?}");
}

#[test]
fn a_busy_channel_lowers_both_ceilings_and_leaves_the_aggregates_alone() {
    let busy = |micros| {
        (micros != 0).then(|| BusyChannel {
            period_micros: NonZeroU32::new(2_000).unwrap(),
            busy_micros: micros,
        })
    };
    let mut last: Option<(Outcome, Outcome)> = None;
    for micros in [0, 200, 500, 800] {
        let tx = run::<16>(Inputs {
            busy: busy(micros),
            ..Inputs::transmitting(64)
        });
        let rx = run::<16>(Inputs {
            busy: busy(micros),
            ..Inputs::receiving(agreed())
        });
        assert!(tx.one_at_a_time && rx.one_at_a_time);
        assert!(near(tx.tx_aggregate(), 32.0, 0.001), "{tx:?}");
        assert!(near(rx.rx_aggregate(), 16.0, 0.001), "{rx:?}");
        if micros != 0 {
            assert!(tx.air.busy_micros > 0);
        }
        if let Some((earlier_tx, earlier_rx)) = last {
            assert!(tx.tx_mbps() < earlier_tx.tx_mbps(), "{micros}: {tx:?}");
            assert!(rx.rx_mbps() < earlier_rx.rx_mbps(), "{micros}: {rx:?}");
        }
        last = Some((tx, rx));
    }
    // What another network holds, nobody here has: 40 % of the channel
    // takes about 40 % of the ceiling.
    let (tx, _) = last.unwrap();
    assert!(
        near(tx.tx_mbps(), transmit(64).tx_mbps() * 0.6, 0.05),
        "{tx:?}"
    );
}

#[test]
fn a_network_that_feeds_slowly_sends_smaller_aggregates() {
    let mut last = 0.0;
    for frames in [4, 6, 8, 10, 11] {
        let outcome = run::<16>(Inputs {
            tx_pace: Pace::PerMillisecond(frames),
            ..Inputs::transmitting(64)
        });
        let offered = (u64::from(frames) * UDP_PAYLOAD * 8) as f64 / 1000.0;
        assert!(
            near(outcome.tx_mbps(), offered, 0.01),
            "{frames}: {outcome:?}"
        );
        assert!(outcome.tx_aggregate() > last, "{frames}: {outcome:?}");
        assert!(outcome.tx_aggregate() < 32.0, "{frames}: {outcome:?}");
        last = outcome.tx_aggregate();
    }
}

#[test]
fn a_network_that_takes_slowly_is_what_binds_and_what_lands_shrinks() {
    let full = receive(agreed());
    let mut last = 0.0;
    for frames in [2, 4, 6, 8] {
        let outcome = run::<16>(Inputs {
            rx_pace: Pace::PerMillisecond(frames),
            ..Inputs::receiving(agreed())
        });
        let taken = (u64::from(frames) * UDP_PAYLOAD * 8) as f64 / 1000.0;
        assert!(
            near(outcome.rx_mbps(), taken, 0.02),
            "{frames}: {outcome:?}"
        );
        assert!(outcome.air.ring_drops > 0, "{frames}: {outcome:?}");
        assert!(outcome.rx_landed() > last, "{frames}: {outcome:?}");
        assert!(
            outcome.rx_landed() < full.rx_landed(),
            "{frames}: {outcome:?}"
        );
        // The access point fills its window with what went missing, and the
        // air stays as busy as ever for less.
        assert!(outcome.rx_aggregate() > outcome.rx_landed(), "{outcome:?}");
        assert!(outcome.occupancy() > 0.85, "{outcome:?}");
        last = outcome.rx_landed();
    }
}

#[test]
fn the_window_binds_a_fast_network_and_the_network_binds_a_window_it_cannot_keep_up_with() {
    // A network that keeps up never meets a small ring or a small credit.
    let fast = run::<16>(Inputs {
        ring: 4,
        lent: 4,
        ..Inputs::receiving(agreed())
    });
    assert_eq!(fast.air.ring_drops, 0, "{fast:?}");
    assert!(near(fast.rx_mbps(), receive(agreed()).rx_mbps(), 0.002));
    assert_eq!(fast.air.bounds.window, fast.air.peer.exchanges);
    // A slow one sets the rate whatever the window.
    let taking = |window| {
        run::<16>(Inputs {
            rx_pace: Pace::PerMillisecond(6),
            ..Inputs::receiving(window)
        })
    };
    let (narrow, wide) = (
        taking(PeerWindow::Assumed(16)),
        taking(PeerWindow::Assumed(32)),
    );
    assert!(
        near(narrow.rx_mbps(), wide.rx_mbps(), 0.01),
        "{narrow:?} {wide:?}"
    );
    assert!(
        wide.air.ring_drops > narrow.air.ring_drops,
        "a wider window loses more"
    );
    // More ring and more credit only move where the frames wait.
    let deep = run::<16>(Inputs {
        rx_pace: Pace::PerMillisecond(6),
        ring: 512,
        lent: 256,
        warm_up_ms: 200,
        ..Inputs::receiving(PeerWindow::Assumed(16))
    });
    assert!(near(deep.rx_mbps(), narrow.rx_mbps(), 0.01), "{deep:?}");
}

#[test]
fn both_directions_take_turns_one_transmission_at_a_time() {
    let tx = transmit(64);
    let rx = receive(agreed());
    let both = run::<16>(Inputs {
        rx_window: Some(agreed()),
        ..Inputs::transmitting(64)
    });
    assert!(both.one_at_a_time, "{both:?}");
    assert!(
        both.tx_mbps() < tx.tx_mbps() && both.rx_mbps() < rx.rx_mbps(),
        "{both:?}"
    );
    assert!(both.tx_mbps() + both.rx_mbps() <= tx.tx_mbps(), "{both:?}");
    // Each side wins about every other access, whatever its aggregate.
    let turns = both.air.station.exchanges as f64 / both.air.peer.exchanges as f64;
    assert!((0.7..=1.4).contains(&turns), "{both:?}");
    assert!(both.air.station.collisions > 0, "{both:?}");
    assert_eq!(both.air.station.collisions, both.air.peer.collisions);
    // Nobody's frame was lost for good: the collided ones went again.
    assert!(both.sent > 0 && both.taken > 0);
}

#[test]
fn a_protection_exchange_costs_the_ceiling_its_time() {
    let plain = receive(agreed());
    let protected = run::<16>(Inputs {
        peer_protection: Protection::RtsCts {
            rate: oer_protocols_ieee80211_wire::phy::LegacyRate::Ofdm24M,
        },
        ..Inputs::receiving(agreed())
    });
    assert!(near(protected.rx_aggregate(), 16.0, 0.001));
    let lost = 1.0 - protected.rx_mbps() / plain.rx_mbps();
    // 88 us of a 1526-us cycle.
    assert!((0.04..=0.07).contains(&lost), "{lost}");
}

#[test]
fn a_slower_response_costs_the_ceiling_its_time() {
    let slow = run::<16>(Inputs {
        timing: AirTiming {
            response_rate: oer_protocols_ieee80211_wire::phy::LegacyRate::Ofdm6M,
            ..AirTiming::HT_2_4_GHZ
        },
        ..Inputs::receiving(agreed())
    });
    assert!(slow.rx_mbps() < receive(agreed()).rx_mbps());
}

#[test]
fn one_seed_replays_a_run_and_the_ceiling_holds_across_seeds() {
    let of = |seed| {
        run::<16>(Inputs {
            seed,
            rx_window: Some(agreed()),
            ..Inputs::transmitting(64)
        })
    };
    let (first, again) = (of(5), of(5));
    assert_eq!(first.sent, again.sent);
    assert_eq!(first.taken, again.taken);
    assert_eq!(first.air, again.air);
    let other = of(6);
    assert_ne!(first.air, other.air);
    assert!(near(
        transmit(64).tx_mbps(),
        run::<16>(Inputs {
            seed: 9,
            ..Inputs::transmitting(64)
        })
        .tx_mbps(),
        0.01
    ));
}

/// The stand's mode, against a number worked by hand and a table of what the
/// model predicts. The comparison with the stand lives with the stand's
/// numbers, outside this repository: nothing here is fitted to them.
mod validation {
    use super::*;

    /// One exchange: a PPDU of `ppdu` us, SIFS, a BlockAck at 24 Mb/s, AIFS
    /// and the mean backoff of CWmin 15, in Mbit/s of UDP payload.
    fn by_hand(subframes: u64, ppdu: f64) -> f64 {
        let cycle = ppdu + 10.0 + 38.0 + (10.0 + 3.0 * 9.0) + 7.5 * 9.0;
        (subframes * UDP_PAYLOAD * 8) as f64 / cycle
    }

    #[test]
    fn the_stand_mode_gives_the_ceiling_worked_by_hand() {
        let seconds = |seed| Inputs {
            seed,
            measure_ms: 1000,
            ..Inputs::stand()
        };
        // 32 subframes of 1554-octet MPDUs: 740 symbols, 2706 us.
        let tx = run::<16>(Inputs {
            tx_window: Some(64),
            ..seconds(3)
        });
        assert!(near(tx.tx_mbps(), by_hand(32, 2706.0), 0.01), "{tx:?}");
        // 16 of them: 370 symbols, 1374 us.
        let rx = run::<16>(Inputs {
            rx_window: Some(agreed()),
            ..seconds(3)
        });
        assert!(near(rx.rx_mbps(), by_hand(16, 1374.0), 0.01), "{rx:?}");
    }

    /// What the model predicts for the stand: `--nocapture` prints the
    /// table. It asserts only that every run stayed on the air's rules.
    #[test]
    fn prediction_for_the_stand() {
        let mean = |inputs: Inputs| {
            let runs: Vec<Outcome> = (1..=5)
                .map(|seed| {
                    run::<16>(Inputs {
                        seed,
                        measure_ms: 500,
                        ..inputs
                    })
                })
                .collect();
            let of = |value: fn(&Outcome) -> f64| {
                runs.iter().map(value).sum::<f64>() / runs.len() as f64
            };
            assert!(runs.iter().all(|run| run.one_at_a_time));
            (
                of(Outcome::tx_mbps),
                of(Outcome::rx_mbps),
                of(Outcome::tx_aggregate),
                of(Outcome::rx_aggregate),
            )
        };
        let rts = Protection::RtsCts {
            rate: oer_protocols_ieee80211_wire::phy::LegacyRate::Ofdm24M,
        };
        let both = Inputs {
            rx_window: Some(agreed()),
            ..Inputs::transmitting(64)
        };
        // A paced load of 8.49 frames per ms carries 100 Mbit/s.
        let cases = [
            ("TX unpaced", Inputs::transmitting(64)),
            ("RX saturated, agreed 16", Inputs::receiving(agreed())),
            (
                "RX saturated, assumed 32",
                Inputs::receiving(PeerWindow::Assumed(32)),
            ),
            ("RX+TX saturated", both),
            (
                "RX saturated, AP uses RTS/CTS",
                Inputs {
                    peer_protection: rts,
                    ..Inputs::receiving(agreed())
                },
            ),
        ];
        for (name, inputs) in cases {
            let (tx, rx, tx_subframes, rx_subframes) = mean(inputs);
            std::println!(
                "{name:32} TX {tx:7.2} RX {rx:7.2} TX subframes {tx_subframes:6.2} \
                 RX subframes {rx_subframes:6.2}"
            );
        }
    }
}
