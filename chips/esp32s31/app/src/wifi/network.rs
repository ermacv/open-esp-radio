//! The network core over the station, in place of the IP service: the
//! station's transmit buffers as the port's memory, a channel socket, a local
//! producer, and the task on the group's executor.
//!
//! [`Network::start`] takes the station's device in the group's boot, and the
//! storage the image lends for its service ([`NetworkStorage`]): the service's
//! type holds the image's producer, so no static of the library can. The
//! address comes later, when the image has it: [`Network::configure`] builds
//! the service with its channel socket and its producer, and hands it to the
//! task, once. The producer is called in the network task's own turns, on the
//! group's hart ([`Produce`]): it fills each datagram's data in the station's
//! transmit buffer itself. A stop ends the sockets, ends each of the
//! producer's open works `Retired` and sets its end flag, and gives back
//! every frame in the port's queue, so the station has its received buffers
//! back. The other owners stay parked for the rest of the boot: nothing
//! starts a second network, and no contract ends the first one.

use super::*;
use core::task::{Context as TaskContext, Poll};
use oer_portable_memory::{WakingSpscReceiver, WakingSpscSender};
use oer_protocols_ieee80211_runtime::station::{TxRefused, TxSupply};
use oer_protocols_network_core::{
    ClaimRefused, Group, Interface, InterfaceError, NetworkOwner, OpenRefused, Retired, claim,
};
use oer_protocols_network_port::{
    EthernetFrame, EthernetFrameView, FrameLengthError, TransmitError, TransmitRefused, TxStorage,
    Unpublished,
};
use oer_protocols_network_runtime::{
    DevicePort, Direct, EndCause, Ended, LocalRefused, MemoryPolicy, PortRefusal, Producer,
    Service, SocketLanes, StopLane, TxBuffers,
};
use oer_protocols_network_wire::ethernet::{EthernetHeader, HEADER_LEN};
use oer_protocols_network_wire::ip::{Dscp, IPV4_HEADER_LEN};
use oer_protocols_network_wire::transport::UDP_HEADER_LEN;
use static_cell::ConstStaticCell;

pub use oer_portable_memory::{AtomicWaker, ClaimCell};
/// The network's time, which a producer is called with.
pub use oer_portable_time::Instant as NetworkInstant;
pub use oer_protocols_network_core::{
    Block, Datagram, Declined, Outcome, Remote, SendRefusal, WorkId,
};
pub use oer_protocols_network_runtime::{
    Interest, LocalEnd, Next, Observer, Offer, Produce, RESERVE_AHEAD, Refusal, Refused,
    SocketClosed, Work,
};

/// The sockets a network holds: the channel socket and the producer's.
pub const SOCKETS: usize = 2;
/// A block's octets: the longest UDP payload of an Ethernet MTU, RFC 768 over
/// RFC 791.
pub const BLOCK: usize = 1472;
/// The Ethernet MTU.
const MTU: u16 = 1500;
/// Blocks of the network's memory, for all sockets and both directions.
pub const BLOCKS: usize = 64;
/// Commands a socket has running; a producer's works too.
const CREDITS: usize = 32;
/// Datagrams a socket's queue holds in the core, and in its lane.
const RECEIVED: usize = 8;
const LANE: usize = 32;
/// Requests a socket's lane holds.
const DEPTH: usize = 32;
const NEIGHBORS: usize = 4;

/// The type that names this network's blocks.
pub struct Brand;

/// A socket of the network.
pub type Socket =
    oer_protocols_network_runtime::Socket<'static, 'static, Brand, BLOCK, DEPTH, LANE, CREDITS>;

/// A datagram of the network.
pub type Received = Datagram<'static, Brand, BLOCK>;
/// A block of the network.
pub type NetworkBlock = Block<'static, Brand, BLOCK>;

/// The port of the service over the station.
type Port = DevicePort<wifi::Network<'static, Notifies>, Transmit, WifiBuffers>;
type NetService<P> = Service<
    'static,
    'static,
    Brand,
    BLOCK,
    SOCKETS,
    CREDITS,
    RECEIVED,
    NEIGHBORS,
    BLOCKS,
    DEPTH,
    LANE,
    Port,
    Direct,
    Producer<'static, Brand, P, CREDITS>,
>;
type Lanes = SocketLanes<'static, 'static, Brand, BLOCK, DEPTH, LANE, CREDITS>;
/// The service, where it stays while it runs.
type Placed<P> = &'static mut Option<NetService<P>>;

/// What an image lends a network whose producer is `P`, in a static of its
/// own: where the service stays, the lane that hands it to the network's
/// task, and the producer's end flag, which the service sets at its end
/// ([`LocalEnd`]).
pub struct NetworkStorage<P: Produce + 'static> {
    service: Option<NetService<P>>,
    handoff: oer_portable_memory::WakingSpscQueue<Placed<P>, 1>,
    end: LocalEnd,
}

impl<P: Produce + 'static> NetworkStorage<P> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            service: None,
            handoff: oer_portable_memory::WakingSpscQueue::new(),
            end: LocalEnd::new(),
        }
    }
}

impl<P: Produce + 'static> Default for NetworkStorage<P> {
    fn default() -> Self {
        Self::new()
    }
}

/// How far a block starts into a 32-bit word: as far as a UDP payload of a
/// frame does in a transmit buffer, so that a datagram's copy moves whole words.
const BLOCK_WORD_OFFSET: usize =
    (wifi::FRAME_WORD_OFFSET + HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN) % 4;
const _: () = {
    // Every block starts as the first does.
    if BLOCK % 4 != 0 {
        panic!("a block's length is a whole number of words");
    }
};

/// The network's memory, its blocks at [`BLOCK_WORD_OFFSET`].
#[repr(C, align(4))]
struct Memory {
    before: [u8; BLOCK_WORD_OFFSET],
    blocks: [[u8; BLOCK]; BLOCKS],
}

/// The blocks: far larger than a value the image may move.
static MEMORY: ConstStaticCell<Memory> = ConstStaticCell::new(Memory {
    before: [0; BLOCK_WORD_OFFSET],
    blocks: [[0; BLOCK]; BLOCKS],
});
/// The channel socket's lanes.
static LANES: ConstStaticCell<Lanes> = ConstStaticCell::new(SocketLanes::new());
static STOP: ConstStaticCell<StopLane> = ConstStaticCell::new(StopLane::new());
/// The port and the service's stop lane until the address comes. In a cell,
/// as a stop before that has to reach the port too.
static WAITING: ClaimCell<Waiting> = ClaimCell::new();

/// What [`Network::configure`] finds.
struct Waiting {
    port: Port,
    stopping: WakingSpscReceiver<'static, (), 1>,
    hardware: [u8; 6],
}

/// The transmit buffers no frame holds, shared by the supply, which takes
/// back what the station gave back, and the port, which writes them. Both run
/// in the network's task, so a lend is never refused for another's.
struct FreeSlots {
    slots: ClaimCell<heapless::Vec<wifi::TxSlot, { wifi::NETWORK_BUFFERS }>>,
    freed: AtomicWaker,
}

impl FreeSlots {
    fn any(&self) -> bool {
        self.slots.lend(|slots| !slots.is_empty()).unwrap_or(false)
    }

    fn take(&self) -> Option<wifi::TxSlot> {
        self.slots.lend(heapless::Vec::pop).ok().flatten()
    }

    /// A slot a refusal here drops goes back to the station's pool.
    fn give(&self, slot: wifi::TxSlot) {
        let mut slot = Some(slot);
        let _held = self.slots.lend(|slots| {
            if let Some(slot) = slot.take() {
                let _more_than_the_pool_holds = slots.push(slot);
            }
        });
        self.freed.wake();
    }
}

static FREE: FreeSlots = FreeSlots {
    slots: ClaimCell::new(),
    freed: AtomicWaker::new(),
};

/// A frame written in a transmit buffer, from its start.
pub struct WrittenFrame {
    slot: wifi::TxSlot,
    length: usize,
}

impl EthernetFrame for WrittenFrame {
    fn view(&mut self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        let bytes = self
            .slot
            .bytes_mut()
            .get(..self.length)
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

/// The packet a transmission hands the station.
pub(super) type TxPacket = WrittenFrame;

/// The station's end of the transmit buffers.
pub struct WifiTx {
    publisher: wifi::TxPublisher,
}

impl wifi::NetworkTxSupply for WifiTx {
    type Packet = TxPacket;
    fn new(tx: wifi::NetworkTx) -> Self {
        let (publisher, slots) = tx.into_parts();
        let mut free = heapless::Vec::new();
        for slot in slots {
            // The vector holds every buffer the network does.
            let _more_than_the_pool_holds = free.push(slot);
        }
        // One station's start per boot: a second keeps the first's list.
        let _installed = FREE.slots.install(free);
        Self { publisher }
    }
}

impl TxSupply<TxPacket> for WifiTx {
    type Memory = wifi::TxMemory;
    /// A frame comes in its own buffer, so this says whether the next one has
    /// any: the transmit end's room covers the buffers.
    fn available(&self) -> bool {
        FREE.any()
    }
    fn write(&mut self, frame: TxPacket) -> Result<wifi::TxMemory, TxRefused<TxPacket>> {
        let WrittenFrame { slot, length } = frame;
        self.publisher
            .publish(slot, 0, length)
            .map_err(|refused| match refused {
                wifi::TxPublishRefused::Range(slot) => TxRefused::Kept(TransmitRefused {
                    frame: WrittenFrame { slot, length },
                    reason: TransmitError::InvalidLength(FrameLengthError::TooLong),
                }),
                wifi::TxPublishRefused::Lost => TxRefused::Lost,
            })
    }
    fn give(&mut self, memory: wifi::TxMemory) {
        FREE.give(self.publisher.slot(memory));
    }
}

/// The port's end of the transmit buffers.
struct WifiBuffers;

impl TxBuffers for WifiBuffers {
    type Frame = WrittenFrame;
    fn available(&self) -> bool {
        FREE.any()
    }
    fn poll_free(&mut self, cx: &mut TaskContext<'_>) -> Poll<()> {
        FREE.freed.register(cx.waker());
        if FREE.any() {
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
        let mut slot = FREE.take().ok_or(Unpublished::Port(PortRefusal::NoRoom))?;
        // A frame longer than a buffer is the port's refusal, before any
        // fill; a fill's own failure stays the fill's.
        let written = match slot.bytes_mut().get_mut(..length) {
            Some(bytes) => fill(bytes).map_err(Unpublished::Fill),
            None => Err(Unpublished::Port(PortRefusal::BadFrame)),
        };
        match written {
            Ok(()) => Ok(WrittenFrame { slot, length }),
            Err(unpublished) => {
                FREE.give(slot);
                Err(unpublished)
            }
        }
    }
    fn restore(&mut self, frame: WrittenFrame) {
        FREE.give(frame.slot);
    }
}

/// An interface's IPv4 addressing, as the host names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkAddress {
    pub address: [u8; 4],
    pub prefix: u8,
    pub gateway: Option<[u8; 4]>,
}

/// Why [`Network::configure`] gave no sockets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkError {
    /// The network has its address, or a stop ended it first.
    NotWaiting,
    /// The address is not one an interface can have; the network still waits.
    Address(InterfaceError),
    /// A port cannot open.
    Port(OpenRefused),
    /// The producer's port cannot open.
    Local(LocalRefused),
    /// The network's storage was taken.
    Storage,
}

/// The group's end of the network whose producer is `P`.
pub struct Network<P: Produce + 'static> {
    /// Where the service goes, until it does.
    service: Option<Placed<P>>,
    handoff: WakingSpscSender<'static, Placed<P>, 1>,
    end: &'static LocalEnd,
}

/// What the network's task runs: not an API.
#[doc(hidden)]
pub struct NetworkStart<P: Produce + 'static> {
    placed: WakingSpscReceiver<'static, Placed<P>, 1>,
    stop: WakingSpscSender<'static, (), 1>,
    stopper: wifi::NetworkStopper<'static, Notifies>,
}

impl<P: Produce + 'static> crate::NetTask for NetworkStart<P> {
    fn serve(self) -> impl core::future::Future<Output = ()> {
        serve_network(self)
    }
}

impl<P: Produce + 'static> Network<P> {
    /// Take `device` for the network, whose task waits for its address from
    /// here on, and `storage` for its service.
    ///
    /// # Errors
    /// The network started already.
    pub fn start(
        mut device: Device,
        boot: GroupBoot,
        storage: &'static mut NetworkStorage<P>,
    ) -> Result<(Self, GroupBoot<NetworkStart<P>>), Error> {
        let (network, transmit, stopper) = device.network.take().ok_or(Error::AlreadyStarted)?;
        let stop = STOP.try_take().ok_or(Error::Storage)?;
        let NetworkStorage {
            service,
            handoff,
            end,
        } = storage;
        let (handoff, placed) = handoff.split();
        let (stop, stopping) = stop.split();
        WAITING
            .install(Waiting {
                port: DevicePort::new(network, transmit, WifiBuffers),
                stopping,
                hardware: device.address,
            })
            .map_err(|_| Error::AlreadyStarted)?;
        Ok((
            Self {
                service: Some(service),
                handoff,
                end,
            },
            boot.with_net(NetworkStart {
                placed,
                stop,
                stopper,
            }),
        ))
    }

    /// The producer's end flag: set once the service calls it no more, after
    /// every outcome it heard.
    pub const fn end(&self) -> &'static LocalEnd {
        self.end
    }

    /// Give the network its address, once, open the channel socket on
    /// `receiving` and `producer`'s send-only socket on `sending`: the
    /// service runs from here on, and calls `producer` in its turns. A
    /// datagram for `sending` is dropped; receiving and echoing stay on the
    /// channel socket.
    ///
    /// # Errors
    /// [`NetworkError`]; an address it cannot take leaves it waiting.
    pub fn configure(
        &mut self,
        address: NetworkAddress,
        receiving: u16,
        sending: u16,
        producer: P,
    ) -> Result<Socket, NetworkError> {
        let hardware = WAITING
            .lend(|waiting| waiting.hardware)
            .map_err(|_| NetworkError::NotWaiting)?;
        let interface = Interface::new(
            hardware,
            address.address,
            address.prefix,
            address.gateway,
            MTU,
        )
        .map_err(NetworkError::Address)?;
        let placed = self.service.take().ok_or(NetworkError::NotWaiting)?;
        let memory = MEMORY.try_take().ok_or(NetworkError::Storage)?;
        let lanes = LANES.try_take().ok_or(NetworkError::Storage)?;
        let (pool, brand) = claim::<Brand, BLOCK, BLOCKS>(&mut memory.blocks)
            .map_err(|_: ClaimRefused<'_, BLOCK, BLOCKS>| NetworkError::Storage)?;
        let Waiting { port, stopping, .. } =
            WAITING.take().map_err(|_| NetworkError::NotWaiting)?;
        let owner = NetworkOwner::new(brand, interface, pool, port);
        *placed = Some(Service::with_producer(
            owner,
            Direct,
            MemoryPolicy::KEEP,
            stopping,
            producer,
            self.end,
        ));
        let socket = placed.as_mut().map(|service| {
            service
                .bind_local(sending, Dscp::default())
                .map_err(NetworkError::Local)?;
            service
                .bind(receiving, Dscp::default(), Group::shared(), lanes)
                .map_err(|refused| NetworkError::Port(refused.reason))
        });
        let socket = socket.ok_or(NetworkError::Storage)?;
        // The queue is the task's and holds this one value.
        let _sent = self.handoff.try_send(placed);
        socket
    }
}

/// The network's task: its address first, then the service until a stop or
/// the port's fault, which records and resets. A stop drops the port, with the
/// frames of the station it still holds, before the station hears of it.
async fn serve_network<P: Produce + 'static>(start: NetworkStart<P>) {
    let NetworkStart {
        mut placed,
        stop,
        stopper,
    } = start;
    let service = match select(configured(&mut placed), stopper.requested()).await {
        Either::First(service) => service,
        Either::Second(()) => {
            if let Ok(mut waiting) = WAITING.take() {
                waiting.port.give_back_received();
            }
            stopper.stopped();
            return;
        }
    };
    let cause = match service.as_mut() {
        Some(running) => {
            // A measuring image counts the turns; a product lends `()`.
            #[cfg(feature = "observe")]
            let mut events = crate::observed::NetworkSink::default();
            #[cfg(not(feature = "observe"))]
            let mut events = ();
            let mut run = core::pin::pin!(running.run(&EmbassyClock, &mut events));
            match select(run.as_mut(), stopper.requested()).await {
                Either::First(cause) => cause,
                Either::Second(()) => {
                    // The lane holds this one word.
                    let _sent = stop.try_send(());
                    run.await
                }
            }
        }
        None => EndCause::PortContract,
    };
    if cause != EndCause::Stopped {
        crate::fail(oer_portable_diagnostics::name!("network-ended"; 32));
    }
    if let Some(ended) = service.take().map(Service::retire) {
        let Ended {
            retired,
            sockets,
            adopted,
            returns,
            producers,
            ..
        } = ended;
        let Retired {
            results,
            pool,
            brand,
            mut port,
        } = retired;
        // The queue outlives the port, so dropping it returns no frame.
        port.give_back_received();
        drop(port);
        // Memory, lanes and result places are all static, so what a late
        // socket or observer reaches stays.
        let _parked = core::mem::ManuallyDrop::new((
            results, pool, brand, sockets, adopted, returns, producers,
        ));
    }
    stopper.stopped();
}

/// The service, once it has its address.
async fn configured<P: Produce + 'static>(
    placed: &mut WakingSpscReceiver<'static, Placed<P>, 1>,
) -> Placed<P> {
    loop {
        if let Some(service) = placed.try_receive() {
            return service;
        }
        placed.ready().await;
    }
}
