//! The station's composition on the model's data port: the types the
//! S31 facade names for its port (`chips/esp32s31/radio/ieee80211/station`),
//! here over `oer-protocols-ieee80211-testing-model`, and the network end's
//! transmit memory, as the application's `FreeSlots`, `WifiTx` and
//! `WifiBuffers` (`chips/esp32s31/app/src/wifi/network.rs`) hold it, without
//! their statics, which parallel tests may not share.

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
    NetworkStop, NetworkStopLost, ServiceEvent, TxRefused, TxSupply,
};
use oer_protocols_ieee80211_testing_model::{
    MODEL_MAX_MPDU, ModelAck, ModelAnyBuffer, ModelBackend, ModelBlockAckEvidence, ModelCaps24,
    ModelLentRx, ModelNetworkMemory, ModelPort, ModelSmall, ModelTypes,
};
use oer_protocols_network_port::{EthernetFrame, EthernetFrameView, FrameLengthError, Unpublished};
use oer_protocols_network_runtime::{PortRefusal, TxBuffers};
use oer_protocols_network_wire::ethernet::EthernetHeader;
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
}

impl Observed {
    pub fn counts(&self) -> Counts {
        *self.0.borrow()
    }
}

impl EventSink<ServiceEvent> for Observed {
    fn event(&mut self, event: ServiceEvent) {
        if matches!(event, ServiceEvent::RxFrameDropped(_)) {
            self.0.borrow_mut().rx_frames_dropped += 1;
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
        } else {
            Poll::Pending
        }
    }
}

/// The network's transmit memory no frame holds: what the station gave back
/// and what the port did not use, shared by the station's end
/// ([`Supply`]) and the port's ([`Buffers`]), both on the network's task.
pub struct FreeMemory {
    free: RefCell<Vec<Memory>>,
    total: usize,
    waker: Cell<Option<Waker>>,
}

impl FreeMemory {
    /// `total` buffers of an MPDU each.
    pub fn new(total: usize) -> Rc<Self> {
        Rc::new(Self {
            free: RefCell::new((0..total).map(|_| Box::new([0; MODEL_MAX_MPDU])).collect()),
            total,
            waker: Cell::new(None),
        })
    }

    pub fn free(&self) -> usize {
        self.free.borrow().len()
    }

    pub const fn total(&self) -> usize {
        self.total
    }

    fn take(&self) -> Option<Memory> {
        self.free.borrow_mut().pop()
    }

    fn give(&self, memory: Memory) {
        self.free.borrow_mut().push(memory);
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

/// A frame written in transmit memory, its Ethernet header where the data
/// machine reads it.
pub struct WrittenFrame {
    memory: Memory,
    length: usize,
}

impl EthernetFrame for WrittenFrame {
    fn view(&mut self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        let end = MSDU_ETHERNET_OFFSET
            .checked_add(self.length)
            .ok_or(FrameLengthError::OutsideStorage)?;
        let bytes = self
            .memory
            .get(MSDU_ETHERNET_OFFSET..end)
            .ok_or(FrameLengthError::OutsideStorage)?;
        let (header, payload) =
            EthernetHeader::parse(bytes).map_err(|_| FrameLengthError::TooShort)?;
        Ok(EthernetFrameView {
            destination: header.destination,
            source: header.source,
            ether_type: header.ether_type.value(),
            payload,
        })
    }
}

/// The station's end of the memory: a frame's own memory goes to the
/// station, and what it gives back is free again.
pub struct Supply(pub Rc<FreeMemory>);

impl TxSupply<WrittenFrame> for Supply {
    type Memory = Memory;
    fn available(&self) -> bool {
        self.0.free() != 0
    }
    fn write(&mut self, frame: WrittenFrame) -> Result<Memory, TxRefused<WrittenFrame>> {
        Ok(frame.memory)
    }
    fn give(&mut self, memory: Memory) {
        self.0.give(memory);
    }
}

/// The port's end of the memory, where the network core writes its frames.
pub struct Buffers(pub Rc<FreeMemory>);

impl TxBuffers for Buffers {
    type Frame = WrittenFrame;
    fn available(&self) -> bool {
        self.0.free() != 0
    }
    fn poll_free(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.0.waker.set(Some(cx.waker().clone()));
        if self.available() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
    fn write<E>(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), E>,
    ) -> Result<WrittenFrame, Unpublished<PortRefusal, E>> {
        let mut memory = self
            .0
            .take()
            .ok_or(Unpublished::Port(PortRefusal::NoRoom))?;
        let written = match MSDU_ETHERNET_OFFSET
            .checked_add(length)
            .and_then(|end| memory.get_mut(MSDU_ETHERNET_OFFSET..end))
        {
            Some(bytes) => fill(bytes).map_err(Unpublished::Fill),
            None => Err(Unpublished::Port(PortRefusal::BadFrame)),
        };
        match written {
            Ok(()) => Ok(WrittenFrame { memory, length }),
            Err(unpublished) => {
                self.0.give(memory);
                Err(unpublished)
            }
        }
    }
    fn restore(&mut self, frame: WrittenFrame) {
        self.0.give(frame.memory);
    }
}
