//! The station's composition on the model's data port: the types the
//! S31 facade names for its port (`chips/esp32s31/radio/ieee80211/station`),
//! here over `oer-protocols-ieee80211-testing-model`, and the network end's
//! transmit memory as a pool of the station's own transmit end
//! (`LocalTx`), as the application's (`chips/esp32s31/app/src/wifi/network.rs`)
//! holds the S31's.

use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use oer_portable_events::EventSink;
use oer_protocols_ieee80211_contracts_port::ReturnedMpdu;
use oer_protocols_ieee80211_core_roles_sta::data::MSDU_ETHERNET_OFFSET;
use oer_protocols_ieee80211_core_roles_sta::events::StaEvent;
use oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent;
use oer_protocols_ieee80211_runtime::datapath::{ActorEvent, DatapathPort};
use oer_protocols_ieee80211_runtime::lifecycle::{LifecyclePort, LifecycleTxBuffer};
use oer_protocols_ieee80211_runtime::station::{
    NetworkStop, NetworkStopLost, PoolRefused, ServiceEvent, TxPool,
};
use oer_protocols_ieee80211_testing_model::{
    MODEL_MAX_MPDU, ModelAck, ModelAnyBuffer, ModelBackend, ModelBlockAckEvidence, ModelCaps24,
    ModelLentRx, ModelNetworkMemory, ModelPort, ModelSmall, ModelTypes,
};
use oer_protocols_network_core::{NetEvent, NotLent};
use oer_protocols_network_port::TxStorage;
use oer_protocols_radio_port::{LocalId, SlotIdentity};

/// The model's contexts as the station takes them: its configuration
/// carries the link's epoch, no transmission carries a context.
pub struct Contexts;

impl oer_protocols_ieee80211_testing_model::ModelContext for Contexts {
    type Body = core::convert::Infallible;
    type OwnedContext<Id: SlotIdentity> = ();
    type Context<'p, Id: SlotIdentity + 'p> = ();
    type OwnedStationConfigureContext<Id: SlotIdentity> = oer_protocols_radio_port::ContextChoice<
        (),
        oer_protocols_ieee80211_core_roles_sta::epoch::EpochContext<
            oer_protocols_ieee80211_testing_model::ModelPool<Id, 4>,
            (),
        >,
    >;
    type StationConfigureContext<'p, Id: SlotIdentity + 'p> =
        oer_protocols_ieee80211_core_roles_sta::machine::StaConfigurationContext<
            'p,
            oer_protocols_ieee80211_testing_model::ModelPool<Id, 4>,
        >;
    type OwnedAccessPointConfigureContext<Id: SlotIdentity> = ();
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p> = ();
}

pub type Backend<'s> = ModelBackend<LocalId<'s>, ModelCaps24, Contexts>;
pub type Types<'s> = ModelTypes<Backend<'s>>;
pub type Memory = ModelNetworkMemory;

/// The model's port as the actor's and the lifecycle's.
pub struct StationPort<'s>(PhantomData<fn(&'s ()) -> &'s ()>);

impl<'s> DatapathPort<'s> for StationPort<'s> {
    type Port = ModelPort<'s, LocalId<'s>, ModelCaps24, Contexts>;
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

impl<'s> LifecyclePort<'s> for StationPort<'s> {
    fn tx_buffer(
        buffer: ModelAnyBuffer<'s, Backend<'s>>,
    ) -> Result<LifecycleTxBuffer<'s, Self>, ModelAnyBuffer<'s, Backend<'s>>> {
        match buffer {
            ModelAnyBuffer::Small(buffer) => Ok(ReturnedMpdu::Fresh(buffer)),
            ModelAnyBuffer::SmallSent(buffer) => Ok(ReturnedMpdu::Sent(buffer)),
            buffer @ (ModelAnyBuffer::Large(_) | ModelAnyBuffer::LargeSent(_)) => Err(buffer),
        }
    }
}

/// The events the actor, the lifecycle and the cores report, counted where a
/// case reads them.
#[derive(Clone, Default)]
pub struct Observed(Rc<RefCell<Counts>>);

/// What [`Observed`] counted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counts {
    /// The station lost a token: an owner the composition never gets back.
    pub tokens_lost: usize,
    /// Received frames the station dropped on their way to the network.
    pub rx_frames_dropped: usize,
    /// Received MSDUs the station's full queues to the network did not take.
    pub rx_queue_full: usize,
    /// Received MSDUs the station refused for pressure on its handoff: no
    /// lend, no room for a copy.
    pub rx_pressure: usize,
    /// A lending socket's datagrams the network dropped for want of credit.
    pub no_credit: usize,
    /// The network service's turns, and those of them that took only the
    /// frames.
    pub turns: usize,
    pub packet_turns: usize,
    /// Received frames the network's turns took in, and the turns that took
    /// their whole budget of them.
    pub frames_taken: usize,
    pub intake_full: usize,
    /// Frames the network's port accepted.
    pub accepted: usize,
    /// Passes of the network's wait that took returns home with no turn.
    pub returned_in_wait: usize,
}

impl Observed {
    pub fn counts(&self) -> Counts {
        *self.0.borrow()
    }
}

impl EventSink<ServiceEvent> for Observed {
    fn event(&mut self, event: ServiceEvent) {
        let mut counts = self.0.borrow_mut();
        match event {
            ServiceEvent::RxFrameDropped(_) => counts.rx_frames_dropped += 1,
            ServiceEvent::RxMsdusRefused(refused) => {
                counts.rx_queue_full += usize::from(refused.queue_full);
                counts.rx_pressure += usize::from(refused.pressure);
            }
            _ => {}
        }
    }
}
impl EventSink<NetEvent> for Observed {
    fn event(&mut self, event: NetEvent) {
        if event == NetEvent::NotLent(NotLent::NoCredit) {
            self.0.borrow_mut().no_credit += 1;
        }
    }
}
impl EventSink<oer_protocols_network_runtime::ServiceEvent> for Observed {
    fn event(&mut self, event: oer_protocols_network_runtime::ServiceEvent) {
        use oer_protocols_network_runtime::ServiceEvent as Event;
        let mut counts = self.0.borrow_mut();
        match event {
            Event::TurnBegan => counts.turns += 1,
            Event::PacketTurn => counts.packet_turns += 1,
            Event::FrameTaken => counts.frames_taken += 1,
            Event::IntakeFull => counts.intake_full += 1,
            Event::Accepted => counts.accepted += 1,
            Event::Returned { .. } => counts.returned_in_wait += 1,
            _ => {}
        }
    }
}
impl EventSink<StaEvent> for Observed {
    fn event(&mut self, event: StaEvent) {
        if event == StaEvent::TokenLost {
            self.0.borrow_mut().tokens_lost += 1;
        }
    }
}
impl EventSink<RxBlockAckEvent> for Observed {
    fn event(&mut self, _: RxBlockAckEvent) {}
}
impl EventSink<ActorEvent> for Observed {
    fn event(&mut self, _: ActorEvent) {}
}

/// The station's stop request to the network side and its answer, as the
/// facade's `NetworkStopState` and `NetworkStopper` keep them, on one hart.
#[derive(Default)]
pub struct NetworkStopState {
    requested: Cell<bool>,
    stopped: Cell<bool>,
    /// The participant went without reporting, as the facade's
    /// `NetworkStopper` dropped unreported.
    lost: Cell<bool>,
    network: Cell<Option<Waker>>,
    station: Cell<Option<Waker>>,
}

impl NetworkStopState {
    fn wake(cell: &Cell<Option<Waker>>) {
        if let Some(waker) = cell.take() {
            waker.wake();
        }
    }

    /// Ready once the station asked the network to stop.
    pub async fn requested(&self) {
        std::future::poll_fn(|cx: &mut Context<'_>| {
            self.network.set(Some(cx.waker().clone()));
            if self.requested.get() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }

    /// A new session's network: no stop asked, none answered.
    pub fn reset(&self) {
        self.requested.set(false);
        self.stopped.set(false);
        self.lost.set(false);
    }

    /// The participant ends without reporting its stop: it lost the
    /// confirmation, not the owners, which its composition still returns.
    pub fn lost(&self) {
        self.lost.set(true);
        Self::wake(&self.station);
    }

    /// The network stopped and holds no frame of the station.
    pub fn stopped(&self) {
        self.stopped.set(true);
        Self::wake(&self.station);
    }
}

impl NetworkStop for NetworkStopState {
    fn takes_frames(&self) -> bool {
        !self.requested.get()
    }
    fn request_stop(&self) {
        self.requested.set(true);
        Self::wake(&self.network);
    }
    fn poll_stopped(&self, cx: &mut Context<'_>) -> Poll<Result<(), NetworkStopLost>> {
        self.station.set(Some(cx.waker().clone()));
        if self.stopped.get() {
            Poll::Ready(Ok(()))
        } else if self.lost.get() {
            Poll::Ready(Err(NetworkStopLost))
        } else {
            Poll::Pending
        }
    }
}

/// The network's transmit memory as the model's station holds it: buffers
/// of an MPDU each, and what the harness reads of them.
pub struct FreeMemory {
    total: usize,
    /// Each buffer's address, at its place: as the S31 names a frame by
    /// its buffer's place among the network's.
    places: Vec<usize>,
    /// The buffers the station holds: published, not yet given back.
    out: Cell<usize>,
}

/// The name of a frame in transmit memory: its buffer's place, below the
/// buffers' count, which no other frame has while the station holds this
/// one and until its end is handed on.
pub type Ticket = u8;

/// What the port tells of a frame's end: nothing, the MAC charged its air.
pub type Notice = core::convert::Infallible;

impl FreeMemory {
    /// `total` buffers of an MPDU each, and the memory, each buffer at its
    /// place.
    pub fn new(
        total: usize,
    ) -> (
        Rc<Self>,
        heapless::Vec<ModelSlot, { super::NETWORK_BUFFERS }>,
    ) {
        let memory: Vec<Memory> = (0..total).map(|_| Box::new([0; MODEL_MAX_MPDU])).collect();
        let places = memory.iter().map(|memory| memory.as_ptr().addr()).collect();
        let free = Rc::new(Self {
            total,
            places,
            out: Cell::new(0),
        });
        let mut slots = heapless::Vec::new();
        for memory in memory {
            let slot = ModelSlot {
                place: free.ticket_of(&memory),
                memory,
            };
            assert!(slots.push(slot).is_ok(), "the network holds every buffer");
        }
        (free, slots)
    }

    /// The place of `memory`, a buffer of these; past every place for any
    /// other, which the policy refuses.
    fn ticket_of(&self, memory: &Memory) -> Ticket {
        let address = memory.as_ptr().addr();
        self.places
            .iter()
            .position(|place| *place == address)
            .and_then(|place| u8::try_from(place).ok())
            .unwrap_or(u8::MAX)
    }

    /// The memory no frame holds: free, or given back with an end the port
    /// has not handed on.
    pub fn free(&self) -> usize {
        self.total - self.out.get()
    }

    pub const fn total(&self) -> usize {
        self.total
    }
}

/// One buffer the network holds: its octets from where the data machine
/// reads a frame's Ethernet header.
pub struct ModelSlot {
    memory: Memory,
    place: Ticket,
}

impl TxStorage for ModelSlot {
    fn bytes_mut(&mut self) -> &mut [u8] {
        self.memory
            .get_mut(MSDU_ETHERNET_OFFSET..)
            .unwrap_or_default()
    }
}

/// The station's pool: a frame goes to the station in its own buffer, and
/// what the station gives back is the network's again.
pub struct ModelPool(pub Rc<FreeMemory>);

impl TxPool for ModelPool {
    type Slot = ModelSlot;
    type Memory = Memory;
    fn place(slot: &ModelSlot) -> u8 {
        slot.place
    }
    fn publish(&mut self, slot: ModelSlot, _: usize) -> Result<Memory, PoolRefused<ModelSlot>> {
        self.0.out.set(self.0.out.get() + 1);
        Ok(slot.memory)
    }
    fn slot(&mut self, memory: Memory) -> ModelSlot {
        self.0.out.set(self.0.out.get() - 1);
        ModelSlot {
            place: self.0.ticket_of(&memory),
            memory,
        }
    }
}
