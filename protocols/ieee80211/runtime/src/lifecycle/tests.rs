//! The lifecycle and the actor joined over the model's data port, the test
//! playing the access point, the network stack and the application: the
//! join, the handshake's EAPOL through the actor, the station's frames on
//! the link's numbers, the data both ways without the lifecycle, the
//! group rekey, the end and the restart on the next installation.
//!
//! The submodules hold the cases by subject: the air and the rates
//! ([`air`]), the keys ([`keys`]), the data ([`data`]), the Block Ack
//! agreements ([`block_ack`]), the application's control ([`control`]),
//! the lifecycle's ends ([`ends`]) and the contracts of what the actor
//! returns ([`contracts`]).

use super::*;
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::task::Waker;
use std::boxed::Box;
use std::rc::Rc;
use std::sync::Arc;
use std::vec::Vec;

use oer_chips_family_radio_ieee80211_policy::rate_selection::EspressifRateSelection;
use oer_portable_events::EventSink;
use oer_portable_memory::{
    ClosingSpscQueue, SlotReceiver, SlotSender, WakingSlotQueue, WakingSpscQueue,
};
use oer_portable_time::Duration;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_contracts_port::ReturnedMpdu;
use oer_protocols_ieee80211_core_arbiter::ArbiterPolicy;
use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
use oer_protocols_ieee80211_core_roles_sta::events::StaEvent;
use oer_protocols_ieee80211_core_roles_sta::machine::{
    GroupCapacity, StaSecurity, StationResources,
};
use oer_protocols_ieee80211_core_security_rsn::{Pmk, PtkContext, frames::RsnTxFrame};
use oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent;
use oer_protocols_ieee80211_core_upper_mac::rate_selection::{FrameRates, RateSelection};
use oer_protocols_ieee80211_testing_model::{
    AirFrame, ModelAck, ModelAir, ModelAnyBuffer, ModelBackend, ModelBlockAckEvidence, ModelCaps24,
    ModelCommands, ModelInstall, ModelLentRx, ModelPort, ModelResources, ModelRxMeta, ModelSmall,
    ModelTypes,
};
use oer_protocols_ieee80211_wire::address::IndividualAddress;
use oer_protocols_ieee80211_wire::qos::{WmmAccessCategory, WmmUserPriority};
use oer_protocols_ieee80211_wire::security::rsn::Akm;
use oer_protocols_radio_port::{Installations, LocalId, with_local_slot};

use crate::datapath::{
    Actor, ActorEnd, ActorEvent, LifecycleEnds, NetworkEnds, PARCELS as ACTOR_PARCELS,
};
use crate::station::contract::{
    AccessPointCommand, Command as StaCommand, CommandOutcome as StaCommandOutcome,
    CommandRefusal as StaCommandRefusal, DisconnectReason as StaDisconnectReason,
    Notice as StaNotice,
};
use crate::station::{
    Control, ControlCommand, NetworkStop, NetworkStopLost, Outgoing, RX_COPIED_DEPTH, Received,
    RxBlockAckStorage, ServiceEvent, State, Status,
};
use crate::test_fixtures::{
    ANONCE, ASSOCIATION_REQUEST, AUTHENTICATION, BSSID, Callbacks, DATA, DEAUTHENTICATION, LOCAL,
    PMK, PROBE_REQUEST, SNONCE, SSID, ServiceContexts, TestConfig, association_response,
    authentication_response, beacon, channel6, config, decrypted, eapol_frame, kind, message3,
    meta, network_frame, protected_data, protected_group_message1,
};

mod air;
mod block_ack;
mod contracts;
mod control;
mod data;
mod ends;
mod keys;
mod pressure;

const TO: usize = 24;
const FROM: usize = 64;
const RETURNS: usize = 40;
const TX_DEPTH: usize = 4;
const RX_DEPTH: usize = 8;
const COMMANDS: usize = 4;

type Backend<'s> = ModelBackend<LocalId<'s>, ModelCaps24, ServiceContexts>;
type Types<'s> = ModelTypes<Backend<'s>>;
type Memory = oer_protocols_ieee80211_testing_model::ModelNetworkMemory;

/// The model's port as the actor's and the lifecycle's.
struct PairPort<'s>(PhantomData<fn(&'s ()) -> &'s ()>);

impl<'s> DatapathPort<'s> for PairPort<'s> {
    type Port = ModelPort<'s, LocalId<'s>, ModelCaps24, ServiceContexts>;
    type Caps = ModelCaps24;
    type Types = Types<'s>;
    type Small = ModelSmall;
    type TxMemory = Memory;
    type LentRx = ModelLentRx;
    const RX_HELD: u8 = 8;

    fn arena_ack(response: ModelBlockAckEvidence) -> Option<ModelAck> {
        match response {
            ModelBlockAckEvidence::Measured(snr_db) => Some(ModelAck { snr_db }),
            ModelBlockAckEvidence::Unmeasured => None,
        }
    }
}

impl<'s> LifecyclePort<'s> for PairPort<'s> {
    fn tx_buffer(
        buffer: ModelAnyBuffer<'s, Backend<'s>>,
    ) -> Result<LifecycleTxBuffer<'s, Self>, ModelAnyBuffer<'s, Backend<'s>>> {
        match buffer {
            ModelAnyBuffer::Small(buffer) => Ok(ReturnedMpdu::Fresh(buffer)),
            ModelAnyBuffer::SmallSent(buffer) => Ok(ReturnedMpdu::Sent(buffer)),
            buffer @ (ModelAnyBuffer::Large(_)
            | ModelAnyBuffer::LargeSent(_)
            | ModelAnyBuffer::Network(_)
            | ModelAnyBuffer::NetworkSent(_)) => Err(buffer),
        }
    }
}

type P<'s> = PairPort<'s>;
type Issuer<'s> = ModelCommands<'s, LocalId<'s>, ModelCaps24, ServiceContexts>;
type Contexts<'s> =
    [Option<LifecycleTxContext<'s, P<'s>>>; oer_protocols_ieee80211_testing_model::MODEL_TICKETS];
type TestLifecycle<'s, const G: usize> =
    Lifecycle<'s, P<'s>, Callbacks, Issuer<'s>, Contexts<'s>, G, Observed>;
type Completed<'s, const G: usize = 2> =
    LifecycleCompleted<'s, P<'s>, Callbacks, Issuer<'s>, Contexts<'s>, G, Observed>;

/// One event the actor, the lifecycle or a core reported.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Observation {
    Service(ServiceEvent),
    Station(StaEvent),
    BlockAck(RxBlockAckEvent),
    Actor(ActorEvent),
}

/// The events of one case, in order: the actor and the lifecycle share
/// the sink.
#[derive(Clone, Default)]
struct Observed(Rc<RefCell<Vec<Observation>>>);

impl EventSink<ServiceEvent> for Observed {
    fn event(&mut self, event: ServiceEvent) {
        self.0.borrow_mut().push(Observation::Service(event));
    }
}
impl EventSink<StaEvent> for Observed {
    fn event(&mut self, event: StaEvent) {
        self.0.borrow_mut().push(Observation::Station(event));
    }
}
impl EventSink<RxBlockAckEvent> for Observed {
    fn event(&mut self, event: RxBlockAckEvent) {
        self.0.borrow_mut().push(Observation::BlockAck(event));
    }
}
impl EventSink<ActorEvent> for Observed {
    fn event(&mut self, event: ActorEvent) {
        self.0.borrow_mut().push(Observation::Actor(event));
    }
}

impl Observed {
    fn contains(&self, observation: Observation) -> bool {
        self.0.borrow().contains(&observation)
    }

    /// The service's events that `pick` takes.
    fn service<T>(&self, pick: impl Fn(ServiceEvent) -> Option<T>) -> Vec<T> {
        self.0
            .borrow()
            .iter()
            .filter_map(|observation| match observation {
                Observation::Service(event) => pick(*event),
                _ => None,
            })
            .collect()
    }

    fn rx_frames_dropped(&self) -> usize {
        self.service(|event| matches!(event, ServiceEvent::RxFrameDropped(_)).then_some(()))
            .len()
    }

    fn tokens_lost(&self) -> usize {
        self.0
            .borrow()
            .iter()
            .filter(|observation| **observation == Observation::Station(StaEvent::TokenLost))
            .count()
    }
}

/// The actor's rates: the product's selection, with the case's refusal of
/// the next plan of one class and a record of every completion it learns.
struct TestRates {
    inner: EspressifRateSelection<ModelCaps24>,
    refuse: Rc<Cell<Option<FrameRates>>>,
    completions: Rc<RefCell<Vec<FrameRates>>>,
}

impl RateSelection<ModelRxMeta, ModelAck, ModelBlockAckEvidence> for TestRates {
    fn packet_number_step(
        &self,
    ) -> oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpPacketNumberStep {
        <EspressifRateSelection<ModelCaps24> as RateSelection<
            ModelRxMeta,
            ModelAck,
            ModelBlockAckEvidence,
        >>::packet_number_step(&self.inner)
    }

    fn plan<R>(
        &self,
        frame: FrameRates,
        carried: impl Fn(oer_protocols_ieee80211_wire::phy::PhyRate) -> Option<R>,
    ) -> Option<oer_protocols_ieee80211_contracts_port::RetryPlan<R>> {
        if self.refuse.get() == Some(frame) {
            self.refuse.set(None);
            return None;
        }
        <EspressifRateSelection<ModelCaps24> as RateSelection<
            ModelRxMeta,
            ModelAck,
            ModelBlockAckEvidence,
        >>::plan(&self.inner, frame, carried)
    }

    fn associated(
        &mut self,
        peer: &oer_protocols_ieee80211_core_upper_mac::rate_control::RatePeer,
        response: &ModelRxMeta,
    ) {
        <EspressifRateSelection<ModelCaps24> as RateSelection<
            ModelRxMeta,
            ModelAck,
            ModelBlockAckEvidence,
        >>::associated(&mut self.inner, peer, response);
    }

    fn disconnected(&mut self) {
        <EspressifRateSelection<ModelCaps24> as RateSelection<
            ModelRxMeta,
            ModelAck,
            ModelBlockAckEvidence,
        >>::disconnected(&mut self.inner);
    }

    fn completed<D>(
        &mut self,
        now: Instant,
        frame: FrameRates,
        completion: &oer_protocols_ieee80211_contracts_port::SingleOutcome<ModelAck, D>,
    ) {
        self.completions.borrow_mut().push(frame);
        <EspressifRateSelection<ModelCaps24> as RateSelection<
            ModelRxMeta,
            ModelAck,
            ModelBlockAckEvidence,
        >>::completed(&mut self.inner, now, frame, completion);
    }

    fn completed_aggregate<D>(
        &mut self,
        now: Instant,
        tally: oer_protocols_ieee80211_core_upper_mac::rate_control::BlockAckTally,
        completion: &oer_protocols_ieee80211_contracts_port::AggregateOutcome<
            ModelBlockAckEvidence,
            D,
        >,
    ) {
        self.completions.borrow_mut().push(FrameRates::Aggregate);
        <EspressifRateSelection<ModelCaps24> as RateSelection<
            ModelRxMeta,
            ModelAck,
            ModelBlockAckEvidence,
        >>::completed_aggregate(&mut self.inner, now, tally, completion);
    }
}

/// A network side that stops at once and takes every frame, unless the
/// case's composition has none.
struct Network {
    absent: Rc<Cell<bool>>,
}

impl NetworkStop for Network {
    fn takes_frames(&self) -> bool {
        !self.absent.get()
    }
    fn request_stop(&self) {}
    fn poll_stopped(&self, _: &mut core::task::Context<'_>) -> Poll<Result<(), NetworkStopLost>> {
        Poll::Ready(Ok(()))
    }
}

/// A task's waker: it marks the task woken and counts its wakes.
#[derive(Default)]
struct Flag {
    woken: AtomicBool,
    wakes: AtomicUsize,
}

impl std::task::Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::Relaxed);
        self.wakes.fetch_add(1, Ordering::Relaxed);
    }
}

/// One task of the test's executor.
struct Task<'a> {
    future: Pin<&'a mut dyn Future<Output = ()>>,
    flag: Arc<Flag>,
    done: bool,
}

/// Run `tasks` until each ended: a woken task is polled with its own
/// waker; when none is woken, the clock moves to its next deadline. The
/// first task, the actor, may outlive the others: an actor no message
/// retires (after a stop that proved nothing, or a broken contract) waits
/// for the image's reset, so a bounded number of rounds after the others
/// ended stops the run.
fn run_tasks(tasks: &mut [Task<'_>], clock: &VirtualClock, air: &ModelAir) {
    for task in tasks.iter() {
        task.flag.woken.store(true, Ordering::Relaxed);
    }
    let mut idle = 0;
    let mut outlived = 0;
    loop {
        if tasks.iter().all(|task| task.done) {
            return;
        }
        if tasks.iter().skip(1).all(|task| task.done) {
            outlived += 1;
            if outlived > 1_000 {
                return;
            }
        }
        let mut polled = false;
        for task in tasks.iter_mut() {
            if task.done || !task.flag.woken.swap(false, Ordering::Relaxed) {
                continue;
            }
            polled = true;
            let waker = Waker::from(task.flag.clone());
            let mut cx = Context::from_waker(&waker);
            if task.future.as_mut().poll(&mut cx).is_ready() {
                task.done = true;
            }
        }
        if polled {
            idle = 0;
            continue;
        }
        idle += 1;
        assert!(idle < 10_000, "the tasks never end");
        if clock.advance_to_next_deadline().is_none() {
            clock.advance(Duration::from_millis(1)).unwrap();
        }
        air.clock_advanced();
    }
}

/// What the application heard.
#[derive(Default)]
struct Heard {
    notices: RefCell<Vec<StaNotice>>,
    tx_refused: Cell<usize>,
    refusals: RefCell<Vec<crate::station::contract::TransmitRefusal>>,
    completed: Cell<usize>,
    /// Each request's unique terminal: its correlation and outcome.
    terminals: RefCell<Vec<(u64, StaCommandOutcome)>>,
    /// Each refused request: its correlation, command and reason.
    command_refusals: RefCell<Vec<(u64, StaCommand, StaCommandRefusal)>>,
    /// The arbiter's commands to the access point beside the station.
    access_point: RefCell<Vec<AccessPointCommand<ModelCaps24>>>,
    msdu_refused: Cell<usize>,
}

impl Heard {
    /// The terminals of the request `correlation`.
    fn terminals_of(&self, correlation: u64) -> Vec<StaCommandOutcome> {
        self.terminals
            .borrow()
            .iter()
            .filter(|(of, _)| *of == correlation)
            .map(|(_, outcome)| *outcome)
            .collect()
    }

    /// The Disconnected notices.
    fn disconnections(&self) -> usize {
        self.notices
            .borrow()
            .iter()
            .filter(|notice| matches!(notice, StaNotice::Disconnected(_)))
            .count()
    }
}

/// The world the script plays: the air, the clock, the application and
/// the network stack.
struct World<'b> {
    air: Rc<ModelAir>,
    clock: Rc<VirtualClock>,
    heard: Rc<Heard>,
    control: RefCell<Option<Control<'b, ModelCaps24, COMMANDS>>>,
    security: StaSecurity,
    stack: RefCell<SlotSender<'b, Option<Outgoing<Memory>>, TX_DEPTH>>,
    returned: RefCell<SlotReceiver<'b, Option<crate::station::TxEnded<Memory>>, TX_DEPTH>>,
    delivered: RefCell<WakingSpscReceiver<'b, Received<ModelLentRx>, RX_DEPTH>>,
    copies: RefCell<WakingSpscReceiver<'b, EthernetFrame, RX_COPIED_DEPTH>>,
    /// The lifecycle task's waker.
    lifecycle: Arc<Flag>,
    /// The events of the actor, the lifecycle and the cores.
    observed: Observed,
    /// The class of the next rate plan the actor's rates refuse.
    refuse_rate: Rc<Cell<Option<FrameRates>>>,
    /// The completions the actor's rates learned.
    completions: Rc<RefCell<Vec<FrameRates>>>,
    /// The composition has no network.
    network_absent: Rc<Cell<bool>>,
    /// How many delivered frames the port lent in place.
    lent: Cell<usize>,
    /// How much memory the actor gave back to the network.
    memory_back: Cell<usize>,
    /// Whether the connects' profiles ask for HE20.
    he20: Cell<bool>,
}

/// Let the other tasks run once.
async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

/// A copy of `security`, for one more profile of the same network.
fn duplicate(security: &StaSecurity) -> StaSecurity {
    match security {
        StaSecurity::Open => StaSecurity::Open,
        StaSecurity::Wpa2Personal { pmk } => StaSecurity::Wpa2Personal {
            pmk: pmk.duplicate(),
        },
        StaSecurity::Personal { password, pmk } => StaSecurity::Personal {
            password: oer_protocols_ieee80211_core_security_rsn::sae::SaePassword::new(
                password.as_bytes(),
            )
            .unwrap(),
            pmk: pmk.duplicate(),
        },
    }
}

impl<'b> World<'b> {
    fn profile(&self) -> oer_protocols_ieee80211_core_roles_sta::machine::StaProfile {
        oer_protocols_ieee80211_core_roles_sta::machine::StaProfile {
            ssid: heapless::Vec::from_slice(SSID).unwrap(),
            security: duplicate(&self.security),
            prefer_he20: self.he20.get(),
        }
    }

    /// Run `f` on the application's control.
    fn with_control<R>(&self, f: impl FnOnce(&mut Control<'b, ModelCaps24, COMMANDS>) -> R) -> R {
        let mut control = self.control.borrow_mut().take().unwrap();
        let result = f(&mut control);
        *self.control.borrow_mut() = Some(control);
        result
    }

    /// Admit a station request through its operation and detach the
    /// observer; the request's correlation names its terminal.
    async fn station_command(&self, command: StaCommand) -> u64 {
        let mut control = self.control.borrow_mut().take().unwrap();
        let operation = match command {
            StaCommand::Connect => control.connect(self.profile()).await,
            StaCommand::Disconnect => control.disconnect().await,
            StaCommand::BackgroundScan => control.scan().await,
        }
        .expect("the request is admitted");
        let correlation = operation.correlation();
        drop(operation);
        *self.control.borrow_mut() = Some(control);
        correlation
    }

    /// Send the application's `command`, which the lifecycle admits.
    async fn command(&self, command: ControlCommand<ModelCaps24>) {
        let mut control = self.control.borrow_mut().take().unwrap();
        assert!(control.command(command).await.is_ok());
        *self.control.borrow_mut() = Some(control);
    }

    /// Stop, the network taking back its memory meanwhile: a stop that
    /// stops the network waits for its memory to be home.
    async fn stop(&self) {
        let mut control = self.control.borrow_mut().take().unwrap();
        // A lifecycle that already ended refuses it.
        let _refused = control.command(ControlCommand::Stop).await;
        *self.control.borrow_mut() = Some(control);
        for _ in 0..2000 {
            self.take_back_memory();
            if self.status().stopped {
                return;
            }
            yield_now().await;
        }
    }

    fn status(&self) -> Status {
        self.with_control(|control| control.status())
    }

    /// The next frame of `subtype` the station sent after the first `seen`.
    async fn sent(&self, seen: &mut usize, subtype: u16) -> AirFrame {
        for round in 0..2000 {
            yield_now().await;
            if round % 20 == 19 {
                self.advance();
            }
            let sent = self.air.sent();
            while *seen < sent.len() {
                let frame = sent[*seen].clone();
                *seen += 1;
                if kind(&frame) == subtype {
                    return frame;
                }
            }
        }
        panic!("the station never sent {subtype:#06x}");
    }

    async fn notice(&self, notice: StaNotice) {
        for round in 0..2000 {
            yield_now().await;
            if round % 20 == 19 {
                self.advance();
            }
            if self.heard.notices.borrow().contains(&notice) {
                return;
            }
        }
        panic!("no {notice:?}");
    }

    /// Move the clock to the machines' next deadline.
    fn advance(&self) {
        if self.clock.advance_to_next_deadline().is_some() {
            self.air.clock_advanced();
        }
    }

    /// Move the clock by `duration`.
    fn advance_by(&self, duration: Duration) {
        self.clock.advance(duration).unwrap();
        self.air.clock_advanced();
    }

    async fn settle(&self) {
        for _ in 0..50 {
            yield_now().await;
        }
    }

    async fn associate(&self, seen: &mut usize, protected: bool) -> [AirFrame; 3] {
        self.associate_with(seen, beacon(protected), protected)
            .await
    }

    /// Scan, authenticate and associate after `beacon`: the probe, the
    /// Authentication and the Association Request as they went.
    async fn associate_with(
        &self,
        seen: &mut usize,
        beacon: Vec<u8>,
        protected: bool,
    ) -> [AirFrame; 3] {
        self.station_command(StaCommand::Connect).await;
        self.join(seen, beacon, protected).await
    }

    /// The join of an admitted Connect after `beacon`.
    async fn join(&self, seen: &mut usize, beacon: Vec<u8>, protected: bool) -> [AirFrame; 3] {
        let probe = self.sent(seen, PROBE_REQUEST).await;
        self.air.receive(&beacon, meta());
        let authentication = self.sent(seen, AUTHENTICATION).await;
        let response = authentication_response(&authentication.plaintext).unwrap();
        self.air.receive(&response, meta());
        let association = self.sent(seen, ASSOCIATION_REQUEST).await;
        self.air.receive(&association_response(protected), meta());
        [probe, authentication, association]
    }

    /// The station's connection to the access point on channel 6, in the
    /// PHY mode its configuration and the access point make.
    async fn connected(&self) {
        for round in 0..2000 {
            yield_now().await;
            if round % 20 == 19 {
                self.advance();
            }
            if self.heard.notices.borrow().iter().any(|notice| {
                matches!(
                    notice,
                    StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                        ..
                    }
                )
            }) {
                return;
            }
        }
        panic!("never connected");
    }

    async fn handshake(&self, seen: &mut usize) -> oer_protocols_ieee80211_core_security_rsn::Ptk {
        let ptk = self.start_handshake(seen).await;
        self.air.receive(&message3(&ptk), meta());
        let message4 = self.sent(seen, DATA).await;
        assert_eq!(message4.key, None, "Message 4 goes unprotected");
        assert_eq!(message4.plaintext[1] & 0x40, 0);
        self.connected().await;
        ptk
    }

    /// Message 1 and the station's Message 2: the derived key.
    async fn start_handshake(
        &self,
        seen: &mut usize,
    ) -> oer_protocols_ieee80211_core_security_rsn::Ptk {
        let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
        self.air.receive(&eapol_frame(message1.as_bytes()), meta());
        self.sent(seen, DATA).await;
        Pmk::from_bytes(PMK).derive_ptk(
            Akm::Psk,
            PtkContext {
                authenticator_address: BSSID,
                supplicant_address: LOCAL,
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            },
        )
    }

    fn send_msdu(&self, destination: [u8; 6]) -> bool {
        self.send_msdu_at(destination, 0)
    }

    /// Take back every memory the actor returned, counted.
    fn take_back_memory(&self) {
        while let Ok(Some(_memory)) = self.returned.borrow_mut().try_receive_with(Option::take) {
            self.memory_back.set(self.memory_back.get() + 1);
        }
    }

    /// The network stack sends an MSDU of user `priority`, if its queue
    /// has room.
    fn send_msdu_at(&self, destination: [u8; 6], priority: u8) -> bool {
        self.send_packet(
            oer_protocols_ieee80211_wire::data::MsduParts {
                destination,
                source: LOCAL,
                ether_type: 0x0800,
                payload: &[0xab; 100],
            },
            WmmUserPriority::new(priority).unwrap(),
        )
    }

    /// The network stack sends `parts` at `priority`, if its queue has room.
    fn send_packet(
        &self,
        parts: oer_protocols_ieee80211_wire::data::MsduParts<'_>,
        priority: WmmUserPriority,
    ) -> bool {
        self.take_back_memory();
        let packet = network_frame(parts, priority);
        self.stack
            .borrow_mut()
            .try_send_with(|slot| {
                *slot = Some(packet);
                Ok::<_, ()>(())
            })
            .is_ok()
    }

    /// The MSDUs the network stack received.
    fn delivered(&self) -> usize {
        self.delivered_frames().len()
    }

    /// The Ethernet frames the network stack received, each given back.
    fn delivered_frames(&self) -> Vec<Vec<u8>> {
        let bytes = |parts: oer_protocols_network_port::EthernetFrameView<'_>| {
            let mut bytes = Vec::from(parts.destination);
            bytes.extend_from_slice(&parts.source);
            bytes.extend_from_slice(&parts.ether_type.to_be_bytes());
            bytes.extend_from_slice(parts.payload);
            bytes
        };
        let mut frames = Vec::new();
        while let Some(received) = self.delivered.borrow().try_receive() {
            frames.push(match received {
                Received::Lent(frame) => {
                    self.lent.set(self.lent.get() + 1);
                    bytes(frame.view().unwrap())
                }
                Received::Copied(_) => {
                    let mut copy = self.copies.borrow().try_receive().unwrap();
                    bytes(oer_protocols_network_port::EthernetFrame::view(&mut copy).unwrap())
                }
            });
        }
        frames
    }

    /// The last payload octet of each frame the network stack received.
    async fn markers(&self) -> Vec<u8> {
        self.settle().await;
        self.delivered_frames()
            .iter()
            .map(|frame| *frame.last().unwrap())
            .collect()
    }

    fn lifecycle_wakes(&self) -> usize {
        self.lifecycle.wakes.load(Ordering::Relaxed)
    }
}

/// The script of a case: the access point, the network and the
/// application.
type Script<'w> = Pin<Box<dyn Future<Output = ()> + 'w>>;

/// A script that plays one installation.
type Scripted = Box<dyn for<'a, 'b> FnOnce(&'a World<'b>) -> Script<'a>>;

/// Box `script`, naming its borrow of the world.
fn script(script: impl for<'a, 'b> FnOnce(&'a World<'b>) -> Script<'a> + 'static) -> Scripted {
    Box::new(script)
}

/// A lifecycle's parts before it is built: the station, its arbiter, the
/// issuer, the first channel and the terminal access.
struct Parts<'s, const G: usize> {
    station: crate::lifecycle::LifecycleConstructed<'s, P<'s>, Callbacks, G>,
    arbiter: LifecycleArbiter<'s, P<'s>>,
    issuer: Issuer<'s>,
    channel: SupportedChannel<ModelCaps24>,
    terminal_access: oer_protocols_ieee80211_contracts_port::TerminalAccess<
        's,
        <Types<'s> as PortTypes<'s>>::TicketKind,
        <Types<'s> as PortTypes<'s>>::TicketPath,
    >,
}

impl<'s, const G: usize> Parts<'s, G> {
    /// Run the lifecycle to its end: placed, as a composition places it,
    /// when placement is built.
    async fn run(
        self,
        actor: &mut crate::lifecycle::ActorEnds<'_, 's, P<'s>, TO, FROM, RETURNS>,
        io: &mut LifecycleIo<
            '_,
            ModelCaps24,
            impl crate::station::ServiceOutputSink<ModelCaps24>,
            COMMANDS,
            SameHart,
        >,
        clock: &VirtualClock,
        events: Observed,
    ) -> Completed<'s, G> {
        let Self {
            station,
            arbiter,
            issuer,
            channel,
            terminal_access,
        } = self;
        #[cfg(feature = "placement")]
        {
            let (station, credits) = station;
            let placed = TestLifecycle::<'s, G>::init_in_place(
                Box::leak(Box::new(core::mem::MaybeUninit::uninit())),
                |place| (place.write(station), credits),
                arbiter,
                issuer,
                CcmpPacketNumberStep::ONE,
                channel,
                terminal_access,
                events,
            );
            let output = Box::leak(Box::new(core::mem::MaybeUninit::uninit()));
            TestLifecycle::<'s, G>::run_placed(placed, output, actor, io, clock)
                .await
                .read()
        }
        #[cfg(not(feature = "placement"))]
        {
            let lifecycle: TestLifecycle<'s, G> = Lifecycle::new(
                station,
                arbiter,
                issuer,
                CcmpPacketNumberStep::ONE,
                channel,
                terminal_access,
                events,
            );
            lifecycle.run(actor, io, clock).await
        }
    }
}

/// The lifecycle of a fresh installation's `resources`, and its receive
/// agreements' storage.
fn lifecycle<'s, const G: usize>(
    config: TestConfig,
    resources: ModelResources<'s, Backend<'s>>,
    issuer: Issuer<'s>,
) -> (Parts<'s, G>, RxBlockAckStorage<'s, Types<'s>, 2, 4>) {
    let (parts, storage, _spare) = lifecycle_spare(config, resources, issuer);
    (parts, storage)
}

/// The access point's group key keys of [`Spare`].
type AccessPointKeys<'s> = <Types<'s> as PortTypes<'s>>::AccessPointKeys;

/// The installation's owners the station does not take: the access
/// point's command permissions, one of its group key slots and its group
/// permission.
struct Spare<'s> {
    access_point: oer_protocols_ieee80211_contracts_port::InterfacePermissions<
        's,
        oer_protocols_ieee80211_contracts_port::AccessPoint,
        <Types<'s> as PortTypes<'s>>::AccessPointCommandPath,
    >,
    access_point_group: (
        <AccessPointKeys<'s> as KeyTypes<'s>>::GroupSlot,
        <AccessPointKeys<'s> as KeyTypes<'s>>::GroupPermission,
    ),
}

/// [`lifecycle`], and the owners the station does not take.
fn lifecycle_spare<'s, const G: usize>(
    config: TestConfig,
    resources: ModelResources<'s, Backend<'s>>,
    issuer: Issuer<'s>,
) -> (
    Parts<'s, G>,
    RxBlockAckStorage<'s, Types<'s>, 2, 4>,
    Spare<'s>,
) {
    let ModelResources {
        shared:
            oer_protocols_ieee80211_testing_model::ModelShared {
                block_acks,
                terminal_access,
                tickets,
                small,
                gate,
                arbiter_authority,
                ..
            },
        station:
            oer_protocols_ieee80211_testing_model::ModelStationPermissions {
                connection_epoch,
                commands: station_commands,
                association: station_association,
                pairwise: station_pairwise,
                group: station_group,
                group_permission: station_group_permission,
            },
        access_point:
            oer_protocols_ieee80211_testing_model::ModelAccessPointPermissions {
                commands: access_point_commands,
                group: [access_point_group, _],
                group_permission: access_point_group_permission,
                ..
            },
    } = resources;
    let mut tickets = tickets.into_iter();
    let station_tickets: [_; 4] = core::array::from_fn(|_| tickets.next().unwrap());
    let small: [_; 4] = small.try_into().ok().unwrap();
    let channel = config.0.channels[0];
    let (arbiter, place, absence) = LifecycleArbiter::<P<'s>>::new(
        ArbiterPolicy::DEFAULT
            .checked(oer_protocols_ieee80211_testing_model::MODEL_PUBLICATION_BOUND)
            .unwrap(),
        channel,
        arbiter_authority,
        gate,
    );
    let (sta_config, _security) = config;
    let resources = StationResources::<_, 4, 4, G> {
        configure: station_commands.configure,
        epochs: oer_protocols_ieee80211_core_roles_sta::epoch::LinkEpochSource::new(
            connection_epoch,
        ),
        tickets: station_tickets,
        buffers: small,
        place,
        absence_place: absence,
        keys: oer_protocols_ieee80211_contracts_port::association::reunite_vacant(
            station_association,
            oer_protocols_ieee80211_contracts_port::association::VacantKeyBank::new(
                station_pairwise,
                station_group,
            ),
        ),
        group_permission: station_group_permission,
    };
    let station = Station::new(sta_config, resources, GroupCapacity::new());
    let lifecycle = Parts {
        station,
        arbiter,
        issuer,
        channel,
        terminal_access,
    };
    let storage = RxBlockAckStorage::new(block_acks, Duration::from_millis(100));
    let spare = Spare {
        access_point: access_point_commands,
        access_point_group: (access_point_group, access_point_group_permission),
    };
    (lifecycle, storage, spare)
}

/// How a case's run ended.
struct Ends<'s, const G: usize = 2> {
    lifecycle: Option<Completed<'s, G>>,
    actor: Option<ActorEnd<oer_protocols_ieee80211_testing_model::ModelPortFault>>,
}

/// Run the station of `config` with `script` on a fresh model, for as many
/// installations as `script` runs: each script after the first plays the
/// next installation after a restart. `check` takes the ends of every
/// installation.
fn pair(
    config: impl Fn() -> TestConfig,
    scripts: Vec<Scripted>,
    check: impl for<'s> FnOnce(&[Ends<'s>], &ModelAir),
) {
    pair_with::<2>(config, |_| {}, scripts, check);
}

/// [`pair`] of a station with `G` group slots, on an air `setup` prepared
/// before the actor's first turn.
#[allow(clippy::type_complexity)]
fn pair_with<const G: usize>(
    config: impl Fn() -> TestConfig,
    setup: impl FnOnce(&ModelAir),
    scripts: Vec<Scripted>,
    check: impl for<'s> FnOnce(&[Ends<'s, G>], &ModelAir),
) {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let clock = Rc::new(VirtualClock::new());
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
        setup(&air);
        let ModelInstall {
            port,
            commands,
            resources,
        } = install;
        let (first, storage) = lifecycle::<G>(config(), resources, commands);
        let mut life = Some(first);
        let storage = Box::leak(Box::new(storage));
        let observed = Observed::default();
        let refuse_rate = Rc::new(Cell::new(None));
        let completions = Rc::new(RefCell::new(Vec::new()));
        let mut actor = Some(Actor::<P<'_>, _, _>::new(
            port,
            TestRates {
                inner: EspressifRateSelection::<ModelCaps24>::new(),
                refuse: refuse_rate.clone(),
                completions: completions.clone(),
            },
            storage.view(),
            observed.clone(),
        ));
        let network_absent = Rc::new(Cell::new(false));
        let network_side = Network {
            absent: network_absent.clone(),
        };
        let mut to_queue = ClosingSpscQueue::<ToActor<'_, P<'_>>, TO>::new();
        let mut parcel_queue = ClosingSpscQueue::<Parcel<'_, P<'_>>, ACTOR_PARCELS>::new();
        let mut from_queue = ClosingSpscQueue::<FromActor<'_, P<'_>>, FROM>::new();
        let mut return_queue = ClosingSpscQueue::<Returned<'_, P<'_>>, RETURNS>::new();
        let mut link_queue = ClosingSpscQueue::<ActorLink<'_, P<'_>>, 1>::new();
        let mut group_queue =
            ClosingSpscQueue::<GroupPending<'_, ActorEpochPath<'_, P<'_>>>, 1>::new();
        let mut link_return_queue = ClosingSpscQueue::<LinkReturn, 1>::new();
        let mut transmit_queue = WakingSlotQueue::<Option<Outgoing<Memory>>, TX_DEPTH>::new();
        let mut returned_queue =
            WakingSlotQueue::<Option<crate::station::TxEnded<Memory>>, TX_DEPTH>::new();
        let mut receive_queue = WakingSpscQueue::<Received<ModelLentRx>, RX_DEPTH>::new();
        let mut copies_queue = WakingSpscQueue::<EthernetFrame, RX_COPIED_DEPTH>::new();
        let mut command_queue =
            WakingSpscQueue::<crate::station::ServiceCommand<ModelCaps24>, COMMANDS>::new();
        let (to_actor, to_actor_end) = to_queue.split();
        let (parcels, parcels_end) = parcel_queue.split();
        let (from_actor_end, from_actor) = from_queue.split();
        let (returns_end, returns) = return_queue.split();
        let (link, link_end) = link_queue.split();
        let (groups_end, groups) = group_queue.split();
        let (link_return_end, link_return) = link_return_queue.split();
        let (stack, transmit_end) = transmit_queue.split(|| None);
        let (returned_end, returned) = returned_queue.split(|| None);
        let (receive_end, delivered) = receive_queue.split();
        let (copies_end, copies) = copies_queue.split();
        let (application, commands) = command_queue.split();
        let state = State::new();
        let (application, control) = Control::new(application, &state).unwrap();
        let mut actor_ends = LifecycleEnds {
            to_actor: to_actor_end,
            parcels: parcels_end,
            from_actor: from_actor_end,
            returns: returns_end,
            groups: groups_end,
            link: link_end,
            link_return: link_return_end,
        };
        let mut lifecycle_ends = ActorEnds {
            to_actor,
            parcels,
            from_actor,
            returns,
            groups,
            link,
            link_return,
        };
        let mut network = NetworkEnds::<_, _, _, TX_DEPTH, RX_DEPTH, SameHart> {
            transmit: transmit_end,
            returned: returned_end,
            receive: receive_end,
            receive_copied: copies_end,
            network: &network_side,
        };
        let heard = Rc::new(Heard::default());
        let lifecycle_flag = Arc::new(Flag::default());
        let world = World {
            air: air.clone(),
            clock: clock.clone(),
            heard: heard.clone(),
            control: RefCell::new(Some(application)),
            security: config().1,
            stack: RefCell::new(stack),
            returned: RefCell::new(returned),
            delivered: RefCell::new(delivered),
            copies: RefCell::new(copies),
            lifecycle: lifecycle_flag.clone(),
            observed: observed.clone(),
            refuse_rate,
            completions,
            network_absent,
            lent: Cell::new(0),
            memory_back: Cell::new(0),
            he20: Cell::new(false),
        };
        let record = {
            let heard = heard.clone();
            move |output: crate::station::ServiceOutput<ModelCaps24>| match output {
                crate::station::ServiceOutput::Notice(notice) => {
                    heard.notices.borrow_mut().push(notice)
                }
                crate::station::ServiceOutput::TxRefused { reason } => {
                    heard.tx_refused.set(heard.tx_refused.get() + 1);
                    heard.refusals.borrow_mut().push(reason);
                }
                crate::station::ServiceOutput::CommandCompleted(terminal) => {
                    heard.completed.set(heard.completed.get() + 1);
                    heard
                        .terminals
                        .borrow_mut()
                        .push((terminal.request.correlation(), terminal.outcome));
                }
                crate::station::ServiceOutput::CommandRefused(refused) => {
                    heard.command_refusals.borrow_mut().push((
                        refused.request.correlation(),
                        refused.request.command(),
                        refused.reason,
                    ));
                }
                crate::station::ServiceOutput::AccessPoint(command) => {
                    heard.access_point.borrow_mut().push(command)
                }
                crate::station::ServiceOutput::MsduRefused { .. } => {
                    heard.msdu_refused.set(heard.msdu_refused.get() + 1)
                }
                crate::station::ServiceOutput::CommandStarted(_) => {}
            }
        };
        let mut output = world.control.borrow().as_ref().unwrap().events(record);
        let mut io = LifecycleIo {
            commands,
            output: &mut output,
            control,
            stop: StopPolicy {
                drain: Duration::from_secs(1),
            },
        };
        let mut ends = Vec::new();
        let mut scripts = scripts.into_iter();
        let mut next = scripts.next();
        while let Some(script) = next.take() {
            let mut actor_now = actor.take().unwrap();
            let completed = RefCell::new(None);
            let actor_end = RefCell::new(None);
            {
                let actor_task = async {
                    *actor_end.borrow_mut() =
                        Some(actor_now.run(&mut actor_ends, &mut network, &*clock).await);
                };
                let lifecycle_task = async {
                    *completed.borrow_mut() = Some(
                        life.take()
                            .unwrap()
                            .run(&mut lifecycle_ends, &mut io, &clock, observed.clone())
                            .await,
                    );
                };
                let script_task = script(&world);
                let mut actor_task = core::pin::pin!(actor_task);
                let mut lifecycle_task = core::pin::pin!(lifecycle_task);
                let mut script_task = script_task;
                let mut tasks = [
                    Task {
                        future: actor_task.as_mut(),
                        flag: Arc::new(Flag::default()),
                        done: false,
                    },
                    Task {
                        future: lifecycle_task.as_mut(),
                        flag: lifecycle_flag.clone(),
                        done: false,
                    },
                    Task {
                        future: script_task.as_mut(),
                        flag: Arc::new(Flag::default()),
                        done: false,
                    },
                ];
                run_tasks(&mut tasks, &clock, &air);
            }
            let completed = completed.into_inner();
            let actor_end = actor_end.into_inner();
            let restart = matches!(
                completed.as_ref().map(|completed| &completed.end),
                Some(LifecycleEnd::Restart(_))
            ) && actor_end == Some(ActorEnd::Reinstall);
            ends.push(Ends {
                lifecycle: completed,
                actor: actor_end,
            });
            next = scripts.next();
            if restart && next.is_some() {
                // The next installation under the same actor, and the
                // service built again on its resources.
                let mut minted = None;
                #[expect(
                    clippy::result_large_err,
                    reason = "the model's refusal returns its port"
                )]
                let reinstalled = actor_now.reinstall(|port| {
                    port.reinstall(&mut installations)
                        .map(|install| {
                            let ModelInstall {
                                port,
                                commands,
                                resources,
                            } = install;
                            minted = Some((commands, resources));
                            port
                        })
                        .map_err(|refused| {
                            let (port, reason) = *refused;
                            (port, reason)
                        })
                });
                let actor_next = match reinstalled {
                    Ok(actor) => actor,
                    Err((_, reason)) => panic!("the stopped port reinstalls: {reason:?}"),
                };
                let (commands, resources) = minted.unwrap();
                let (next_life, storage) = lifecycle::<G>(config(), resources, commands);
                // The actor keeps its receive agreements' storage; the new
                // installation's stays owned.
                let _kept = Box::leak(Box::new(storage));
                life = Some(next_life);
                actor = Some(actor_next);
            }
        }
        assert_eq!(observed.tokens_lost(), 0, "no station token is lost");
        check(&ends, &air);
    }));
}

fn open() -> TestConfig {
    config(StaSecurity::Open)
}

fn wpa2() -> TestConfig {
    config(StaSecurity::Wpa2Personal {
        pmk: Pmk::from_bytes(PMK),
    })
}

/// An open data frame from the access point, from `source`.
fn open_data(sequence: u16, source: [u8; 6]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x0208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&source);
    frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    frame.extend_from_slice(&[0xcd; 40]);
    frame
}

/// Whether the last installation stopped as the application asked and its
/// actor retired.
fn stopped<const G: usize>(ends: &[Ends<'_, G>]) -> bool {
    matches!(
        ends.last()
            .and_then(|ends| ends.lifecycle.as_ref())
            .map(|completed| &completed.end),
        Some(LifecycleEnd::Stopped)
    ) && ends.last().and_then(|ends| ends.actor.as_ref()) == Some(&ActorEnd::Retired)
}

/// `config` with a second channel a scan visits.
fn two_channels(mut config: TestConfig) -> TestConfig {
    let channel1 = SupportedChannel::new(
        oer_protocols_radio_regulatory::TEST_DOMAIN
            .permit(
                oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(
                    oer_protocols_ieee80211_contracts_port::Channel::ghz2_4(
                        1,
                        oer_protocols_ieee80211_contracts_port::ChannelWidth::Mhz20,
                    )
                    .unwrap(),
                ),
            )
            .unwrap(),
    )
    .unwrap();
    config.0.channels.push(channel1).unwrap();
    config
}

#[test]
fn an_open_station_joins_through_the_actor_carries_data_both_ways_and_stops() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world
                    .notice(StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                        phy: crate::station::contract::LinkPhy::Legacy,
                    })
                    .await;
                // The management frames went to the lifecycle and back.
                world.settle().await;
                assert_eq!(world.air.pending_rx(), 0, "every frame recycled");
                // TX: the network's MSDU goes on the link.
                let destination = [0x02, 0, 0, 0, 0, 0x07];
                assert!(world.send_msdu(destination));
                let data = world.sent(&mut seen, DATA).await;
                assert_eq!(data.plaintext[4..10], BSSID);
                assert_eq!(data.plaintext[16..22], destination);
                assert_eq!(data.key, None);
                // RX: the access point's data reaches the network.
                world
                    .air
                    .receive(&open_data(1, [0x02, 0, 0, 0, 0, 0x08]), meta());
                world.settle().await;
                assert_eq!(world.delivered(), 1);
                world.stop().await;
            })
        })],
        |ends, air| {
            assert!(stopped(ends), "an ordinary stop retires the actor");
            assert!(
                ends[0]
                    .lifecycle
                    .as_ref()
                    .unwrap()
                    .retired
                    .rejected
                    .is_none()
            );
            assert_eq!(air.pending_rx(), 0);
        },
    );
}

#[test]
fn a_wpa2_station_handshakes_through_the_actor_and_its_pairwise_key_comes_back_at_the_end() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let _ptk = world.handshake(&mut seen).await;
                // The link's data goes under the pairwise key the actor
                // holds.
                let destination = [0x02, 0, 0, 0, 0, 0x07];
                assert!(world.send_msdu(destination));
                let first = world.sent(&mut seen, DATA).await;
                assert!(matches!(first.key, Some((_, true))), "{first:?}");
                assert!(world.send_msdu(destination));
                let second = world.sent(&mut seen, DATA).await;
                assert_eq!(
                    second.packet_number,
                    first.packet_number.map(|number| number + 1)
                );
                // RX: a decrypted MPDU reaches the stack; its replay does
                // not.
                let source = [0x02, 0, 0, 0, 0, 0x08];
                world
                    .air
                    .receive(&protected_data(1, source, &[0xcd; 40]), decrypted());
                world
                    .air
                    .receive(&protected_data(1, source, &[0xcd; 40]), decrypted());
                world.settle().await;
                assert_eq!(world.delivered(), 1);
                // The disconnection: the link comes back, the station's
                // Deauthentication goes under the key the actor kept, then
                // the key comes back for its removal.
                world.station_command(StaCommand::Disconnect).await;
                let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
                assert_eq!(deauthentication.plaintext[4..10], BSSID);
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                    .await;
                world.settle().await;
                world.stop().await;
            })
        })],
        |ends, air| {
            assert!(stopped(ends));
            assert!(
                ends[0]
                    .lifecycle
                    .as_ref()
                    .unwrap()
                    .retired
                    .rejected
                    .is_none()
            );
            assert_eq!(air.pending_rx(), 0);
        },
    );
}

#[test]
fn a_station_whose_radio_cannot_move_restarts_on_the_next_installation_without_a_reset() {
    pair(
        || two_channels(open()),
        std::vec![
            script(|world| {
                Box::pin(async move {
                    let mut seen = 0;
                    world.station_command(StaCommand::Connect).await;
                    world.sent(&mut seen, PROBE_REQUEST).await;
                    // The scan's move to the next channel and the return
                    // home are refused: the arbiter fails the station.
                    world.air.refuse_enables(2);
                })
            }),
            script(|world| {
                Box::pin(async move {
                    // The next installation's service joins on the same
                    // air and carries data.
                    let mut seen = world.air.sent().len();
                    world.associate(&mut seen, false).await;
                    world
                        .notice(StaNotice::Connected {
                            bssid: BSSID,
                            channel: 6,
                            phy: crate::station::contract::LinkPhy::Legacy,
                        })
                        .await;
                    assert!(world.send_msdu([0x02, 0, 0, 0, 0, 0x07]));
                    world.sent(&mut seen, DATA).await;
                    world.stop().await;
                })
            }),
        ],
        |ends, air| {
            assert_eq!(ends.len(), 2);
            let first = ends[0].lifecycle.as_ref().unwrap();
            assert!(
                matches!(
                    first.end,
                    LifecycleEnd::Restart(StationFailure::RetuneRefused(_))
                ),
                "{:?}",
                first.end
            );
            assert!(!first.end.is_terminal(), "a failed station is no reset");
            assert_eq!(ends[0].actor, Some(ActorEnd::Reinstall));
            assert!(first.retired.rejected.is_none());
            assert!(stopped(&ends[1..]));
            assert_eq!(air.pending_rx(), 0);
        },
    );
}

#[test]
fn steady_transmission_does_not_wake_the_lifecycle_per_ppdu() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world
                    .notice(StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                        phy: crate::station::contract::LinkPhy::Legacy,
                    })
                    .await;
                world.settle().await;
                let wakes = world.lifecycle_wakes();
                let before = world
                    .air
                    .sent()
                    .iter()
                    .filter(|frame| kind(frame) == DATA)
                    .count();
                for _ in 0..20 {
                    assert!(world.send_msdu([0x02, 0, 0, 0, 0, 0x07]));
                    world.settle().await;
                    world.clock.advance(Duration::from_micros(500)).unwrap();
                    world.air.clock_advanced();
                    world.settle().await;
                }
                let data = world
                    .air
                    .sent()
                    .iter()
                    .filter(|frame| kind(frame) == DATA)
                    .count();
                assert_eq!(data - before, 20, "every MSDU went");
                let woken = world.lifecycle_wakes() - wakes;
                // The first MSDU tells the TID's traffic once; the later
                // ones within the refresh span tell nothing.
                assert!(woken <= 2, "{woken} lifecycle wakes for 20 PPDUs");
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn message4_waits_for_the_group_installations_terminal_through_the_actor() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.start_handshake(&mut seen).await;
                let before = world.air.sent().len();
                world.air.hold_group_commands(true);
                world.air.receive(&message3(&ptk), meta());
                world.settle().await;
                assert_eq!(world.air.group_writes(), 0);
                assert_eq!(world.air.sent().len(), before, "no M4 before the terminal");
                assert!(world.heard.notices.borrow().is_empty());
                world.air.hold_group_commands(false);
                let message4 = world.sent(&mut seen, DATA).await;
                assert_eq!(message4.key, None);
                assert_eq!(world.air.group_writes(), 1);
                world
                    .notice(StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                        phy: crate::station::contract::LinkPhy::Legacy,
                    })
                    .await;
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn a_group_rekey_installs_by_its_terminal_and_answers_under_the_links_key() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.handshake(&mut seen).await;
                assert_eq!(world.air.group_writes(), 1);
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                let response = world.sent(&mut seen, DATA).await;
                assert!(matches!(response.key, Some((_, true))), "{response:?}");
                assert!(response.packet_number.is_some());
                assert_eq!(world.air.group_writes(), 2);
                world.stop().await;
            })
        })],
        |ends, _| {
            assert!(stopped(ends));
            assert!(
                ends[0]
                    .lifecycle
                    .as_ref()
                    .unwrap()
                    .retired
                    .rejected
                    .is_none()
            );
        },
    );
}

#[test]
fn the_actor_numbers_the_stations_frames_and_the_data_of_one_tid_without_a_gap() {
    use crate::test_fixtures::{QOS_DATA, WMM, carries_eapol, qos_sequence};
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                // Voice is closed, so the station's EAPOL goes as Video, in
                // TID 5, as data of priority 5 does.
                let mut wmm = WMM;
                wmm[22] |= 0x10;
                let mut beacon = beacon(true);
                beacon.extend_from_slice(&wmm);
                let mut seen = 0;
                world.associate_with(&mut seen, beacon, true).await;
                let ptk = world.handshake(&mut seen).await;
                assert!(world.send_msdu_at([2, 0, 0, 0, 0, 9], 5));
                let first = world.sent(&mut seen, QOS_DATA).await;
                assert!(!carries_eapol(&first));
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                let response = world.sent(&mut seen, QOS_DATA).await;
                assert!(carries_eapol(&response));
                assert!(world.send_msdu_at([2, 0, 0, 0, 0, 9], 5));
                let second = world.sent(&mut seen, QOS_DATA).await;
                assert!(!carries_eapol(&second));
                for frame in [&first, &response, &second] {
                    assert_eq!(frame.plaintext[24] & 0x0f, 5, "TID 5");
                    assert!(matches!(frame.key, Some((_, true))), "{frame:?}");
                }
                assert_eq!(qos_sequence(&response), qos_sequence(&first) + 1);
                assert_eq!(qos_sequence(&second), qos_sequence(&response) + 1);
                let numbers =
                    [&first, &response, &second].map(|frame| frame.packet_number.unwrap());
                assert!(
                    numbers.windows(2).all(|pair| pair[0] < pair[1]),
                    "{numbers:?}"
                );
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn a_receive_agreement_answers_through_the_station_and_reorders_in_the_actor() {
    use crate::test_fixtures::block_ack::{addba, data, ht_beacon, ht_config, response};
    use oer_protocols_ieee80211_wire::block_ack::frame::BlockAckAction;
    pair(
        ht_config,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate_with(&mut seen, ht_beacon(), false).await;
                world
                    .notice(StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                        phy: crate::station::contract::LinkPhy::Ht20,
                    })
                    .await;
                world.air.receive(&addba(7, 0, 4094, 0), meta());
                let ack = world.sent(&mut seen, 0xd0).await;
                assert!(matches!(
                    response(&ack),
                    BlockAckAction::AddbaResponse {
                        dialog_token: 7,
                        status: 0,
                        tid: 0,
                        window: 4,
                        amsdu: true,
                        ..
                    }
                ));
                assert_eq!(world.air.receive_agreements().len(), 1);
                world.air.receive(&data(4095, 0, 2), meta());
                world.settle().await;
                assert_eq!(world.delivered(), 0, "the hole holds the later frame");
                world.air.receive(&data(4094, 0, 1), meta());
                assert_eq!(world.markers().await, [1, 2], "in order");
                // An old retry only goes back to the port; the window wraps.
                world.air.receive(&data(4095, 0, 2), meta());
                assert!(world.markers().await.is_empty());
                world.air.receive(&data(0, 0, 3), meta());
                assert_eq!(world.markers().await, [3]);
                assert_eq!(world.observed.rx_frames_dropped(), 0);
                assert!(
                    world
                        .observed
                        .service(|event| match event {
                            ServiceEvent::RxMsdusRefused(refused) => Some(refused),
                            _ => None,
                        })
                        .is_empty()
                );
                world.stop().await;
            })
        })],
        |ends, air| {
            assert!(stopped(ends));
            assert_eq!(air.pending_rx(), 0);
        },
    );
}

#[test]
fn a_transmit_agreement_names_the_exact_next_number_and_ends_idle() {
    use crate::test_fixtures::block_ack::{
        addba_response, ht_beacon, ht_config, response, tx_block_ack_config,
    };
    use oer_protocols_ieee80211_wire::block_ack::frame::BlockAckAction;
    pair(
        || tx_block_ack_config(ht_config()),
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate_with(&mut seen, ht_beacon(), false).await;
                world
                    .notice(StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                        phy: crate::station::contract::LinkPhy::Ht20,
                    })
                    .await;
                assert!(world.send_msdu(BSSID));
                let request = world.sent(&mut seen, 0xd0).await;
                let BlockAckAction::AddbaRequest {
                    dialog_token,
                    tid: 0,
                    immediate: true,
                    window: 32,
                    timeout_tu: 0,
                    starting_sequence,
                    ..
                } = response(&request)
                else {
                    panic!("an ADDBA Request of TID 0: {:?}", response(&request));
                };
                // The MSDU that started it took Sequence Number 0.
                assert_eq!(starting_sequence.get(), 1);
                world
                    .air
                    .receive(&addba_response(dialog_token, 0, 32), meta());
                // The next MSDU takes the number the request named.
                assert!(world.send_msdu(BSSID));
                let next = world.sent(&mut seen, crate::test_fixtures::QOS_DATA).await;
                assert_eq!(crate::test_fixtures::qos_sequence(&next), 1);
                // No more data: one idle second later the agreement ends.
                let delba = world.sent(&mut seen, 0xd0).await;
                assert_eq!(
                    response(&delba),
                    BlockAckAction::Delba {
                        tid: 0,
                        initiator: true,
                        reason: 39,
                    }
                );
                // New data negotiates again.
                assert!(world.send_msdu(BSSID));
                assert!(matches!(
                    response(&world.sent(&mut seen, 0xd0).await),
                    BlockAckAction::AddbaRequest { tid: 0, .. }
                ));
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn a_station_frame_the_port_refuses_comes_back_withdrawn_and_the_station_goes_on() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world
                    .notice(StaNotice::Connected {
                        bssid: BSSID,
                        channel: 6,
                        phy: crate::station::contract::LinkPhy::Legacy,
                    })
                    .await;
                world.settle().await;
                // The port closes its admission: the Deauthentication ends
                // withdrawn at its submission, and the station hears it as
                // any withdrawn frame's end.
                world.air.fail_after_terminals();
                let before = world.air.sent().len();
                world.station_command(StaCommand::Disconnect).await;
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                    .await;
                assert_eq!(world.air.sent().len(), before, "nothing went on the air");
                // Its retry, numbered on the link the station gave back
                // meanwhile, is refused before the port as before.
                assert!(
                    world
                        .heard
                        .refusals
                        .borrow()
                        .iter()
                        .all(|reason| *reason
                            == crate::station::contract::TransmitRefusal::LinkChanged),
                    "{:?}",
                    world.heard.refusals.borrow()
                );
                world.stop().await;
            })
        })],
        |ends, _| {
            let completed = ends[0].lifecycle.as_ref().unwrap();
            assert!(completed.retired.rejected.is_none());
            assert!(
                !matches!(
                    completed.end,
                    LifecycleEnd::Contract(_) | LifecycleEnd::Faulted(_)
                ),
                "{:?}",
                completed.end
            );
        },
    );
}

/// An HE Capabilities element with HE-MCS 0..9 both ways on one stream.
const HE_CAPABILITY: [u8; 24] = [
    255, 22, 35, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x80, 0x00, 0xfd, 0xff, 0xfd, 0xff,
];
/// An HE Operation element: BSS color 27, default PE 4.
const HE_OPERATION: [u8; 9] = [255, 7, 36, 0x04, 0x00, 0x00, 0x1b, 0xfc, 0xff];

/// The HT Capabilities of an HT20 station or access point on channel 6.
const fn ht_capability() -> [u8; 28] {
    match oer_protocols_ieee80211_contracts_port::Channel::ghz2_4(
        6,
        oer_protocols_ieee80211_contracts_port::ChannelWidth::Mhz20,
    ) {
        Ok(channel) => oer_protocols_ieee80211_wire::ht::ht_capability_ie(
            oer_protocols_ieee80211_wire::ht::HtLocalCapabilities::new(0, 0, 0xff, 1),
            channel,
        ),
        Err(_) => panic!("channel 6"),
    }
}

static HE_CAPABILITIES: oer_protocols_ieee80211_wire::station::AssociationCapabilities =
    oer_protocols_ieee80211_wire::station::AssociationCapabilities {
        ht20: ht_capability(),
        ht40: ht_capability(),
        he20_ht: ht_capability(),
        he20: HE_CAPABILITY,
        he20_extended: None,
        wmm: [221, 7, 0, 0x50, 0xf2, 2, 0, 1, 0],
    };

/// An open station that prefers HE20 with an access point that admits it.
fn he_open() -> TestConfig {
    use oer_protocols_ieee80211_wire::station::{HeUlMuPowerCapability, StaPowerCapability};
    let mut config = open();
    config.0.association = &HE_CAPABILITIES;
    config.0.phy = oer_protocols_ieee80211_core_roles_sta::association::StaPhyRequest::PreferHe20 {
        ht40: false,
        power: oer_protocols_ieee80211_core_roles_sta::association::StaHePower {
            power_capability: StaPowerCapability::new(-11, 20).unwrap(),
            ul_mu_power: HeUlMuPowerCapability::from_rate_power_indices([20; 10]).unwrap(),
        },
    };
    config
}

/// `frame` with `elements` appended.
fn with_elements(frame: &[u8], elements: &[&[u8]]) -> Vec<u8> {
    let mut frame = frame.to_vec();
    for element in elements {
        frame.extend_from_slice(element);
    }
    frame
}

/// The open network's beacon with HT, and with HE when `he`.
fn ht_beacon(he: bool) -> Vec<u8> {
    let operation = oer_protocols_ieee80211_wire::ht::ht_operation_ie(
        oer_protocols_ieee80211_contracts_port::Channel::ghz2_4(
            6,
            oer_protocols_ieee80211_contracts_port::ChannelWidth::Mhz20,
        )
        .unwrap(),
        Default::default(),
    );
    let ht = with_elements(&beacon(false), &[&ht_capability(), &operation]);
    if he {
        with_elements(&ht, &[&HE_CAPABILITY, &HE_OPERATION])
    } else {
        ht
    }
}

/// The open network's Association Response with HT, and with HE when `he`.
fn ht_association_response(he: bool) -> Vec<u8> {
    let ht = with_elements(&association_response(false), &[&ht_capability()]);
    if he {
        with_elements(&ht, &[&HE_CAPABILITY, &HE_OPERATION])
    } else {
        ht
    }
}

#[test]
fn a_refused_he_association_restarts_the_station_and_its_next_ht_join_ends_the_bss() {
    use oer_protocols_ieee80211_contracts_port::StationHe;
    pair(
        he_open,
        std::vec![
            script(|world| {
                Box::pin(async move {
                    // The BSS goes before authentication; the port refuses
                    // the association after the response.
                    world.he20.set(true);
                    world.air.refuse_he_associations(1);
                    let mut seen = 0;
                    world.station_command(StaCommand::Connect).await;
                    world.sent(&mut seen, PROBE_REQUEST).await;
                    world.air.receive(&ht_beacon(true), meta());
                    let authentication = world.sent(&mut seen, AUTHENTICATION).await;
                    assert!(
                        matches!(world.air.he(), StationHe::Bss(_)),
                        "the BSS before authentication: {:?}",
                        world.air.he()
                    );
                    let response = authentication_response(&authentication.plaintext).unwrap();
                    world.air.receive(&response, meta());
                    world.sent(&mut seen, ASSOCIATION_REQUEST).await;
                    world.air.receive(&ht_association_response(true), meta());
                    world.settle().await;
                })
            }),
            script(|world| {
                Box::pin(async move {
                    // The next installation joins an access point without
                    // HE: its join ends the BSS the refused one left.
                    let mut seen = world.air.sent().len();
                    assert!(matches!(world.air.he(), StationHe::Bss(_)));
                    world.station_command(StaCommand::Connect).await;
                    world.sent(&mut seen, PROBE_REQUEST).await;
                    world.air.receive(&ht_beacon(false), meta());
                    let authentication = world.sent(&mut seen, AUTHENTICATION).await;
                    assert_eq!(world.air.he(), StationHe::Off);
                    let response = authentication_response(&authentication.plaintext).unwrap();
                    world.air.receive(&response, meta());
                    world.sent(&mut seen, ASSOCIATION_REQUEST).await;
                    world.air.receive(&ht_association_response(false), meta());
                    world
                        .notice(StaNotice::Connected {
                            bssid: BSSID,
                            channel: 6,
                            phy: crate::station::contract::LinkPhy::Ht20,
                        })
                        .await;
                    assert_eq!(world.air.he(), StationHe::Off);
                    assert!(world.send_msdu([0x02, 0, 0, 0, 0, 0x07]));
                    world.sent(&mut seen, DATA).await;
                    world.stop().await;
                })
            }),
        ],
        |ends, air| {
            assert_eq!(ends.len(), 2);
            let first = ends[0].lifecycle.as_ref().unwrap();
            assert!(
                matches!(
                    first.end,
                    LifecycleEnd::Restart(StationFailure::CommandRefused {
                        kind: oer_protocols_ieee80211_contracts_port::CommandKind::SetHe,
                        reason: oer_protocols_ieee80211_contracts_port::CommandRefusal::Busy,
                    })
                ),
                "{:?}",
                first.end
            );
            assert!(!first.end.is_terminal(), "a refused setting is no reset");
            assert_eq!(ends[0].actor, Some(ActorEnd::Reinstall));
            assert!(stopped(&ends[1..]));
            assert_eq!(air.he(), StationHe::Off);
            assert_eq!(air.pending_rx(), 0);
        },
    );
}
