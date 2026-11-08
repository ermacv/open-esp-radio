//! The network core over the station, in place of the IP service: the
//! station's transmit buffers as the port's memory, a channel socket, a local
//! producer, and the task on the group's executor.
//!
//! [`Network::start`] takes the station's device in the group's boot, the
//! storage the image lends for its service ([`NetworkStorage`]), its producer
//! and the two ports: the service's type holds the image's producer, so no
//! static of the library can. The service starts at once: its DHCP client
//! leases the interface's address, RFC 2131, from the first link up, with a
//! seed the station drew at boot, and [`Network::configuration`] tells the
//! group what it holds. The producer is called in the network task's own
//! turns, on the group's hart ([`Produce`]): it fills each datagram's data in
//! the station's transmit buffer itself. A stop ends the sockets, ends each
//! of the producer's open works `Retired` and sets its end flag, and gives
//! back every frame in the port's queue, so the station has its received
//! buffers back. The other owners stay parked for the rest of the boot:
//! nothing starts a second network, and no contract ends the first one.

use super::*;
use core::task::{Context as TaskContext, Poll};
use oer_portable_memory::WakingSpscSender;
use oer_protocols_ieee80211_runtime::station::{TxRefused, TxSupply};
use oer_protocols_network_core::{
    ClaimRefused, Group, Interface, InterfaceError, NetworkOwner, OpenRefused, Retired, claim,
};
use oer_protocols_network_port::{
    EthernetFrame, EthernetFrameView, FrameLengthError, TransmitError, TransmitRefused, TxStorage,
    Unpublished,
};
use oer_protocols_network_runtime::ConfigurationWatch;
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
    Block, Configuration, Datagram, Declined, DhcpState, Ipv4, Lease, Outcome, Remote, SendRefusal,
    WorkId,
};
pub use oer_protocols_network_runtime::{
    Interest, LocalEnd, Next, Observer, Offer, Produce, RESERVE_AHEAD, Refusal, Refused,
    SocketClosed, WatchRefused, Work,
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
/// own: where the service stays, the producer's end flag, which the service
/// sets at its end ([`LocalEnd`]), and the watch of the interface's address.
pub struct NetworkStorage<P: Produce + 'static> {
    service: Option<NetService<P>>,
    end: LocalEnd,
    /// Where the service publishes the interface's address.
    watch: ConfigurationWatch,
}

impl<P: Produce + 'static> NetworkStorage<P> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            service: None,
            end: LocalEnd::new(),
            watch: ConfigurationWatch::new(),
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

/// The ports a network's sockets hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ports {
    /// The channel socket's: it receives, and sends what it echoes.
    pub receiving: u16,
    /// The producer's send-only socket's.
    pub sending: u16,
}

/// Why [`Network::start`] gave no network.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkError {
    /// The interface cannot be.
    Interface(InterfaceError),
    /// A port cannot open.
    Port(OpenRefused),
    /// The producer's port cannot open.
    Local(LocalRefused),
    /// The watch of the address refused its first publication.
    Watch(WatchRefused),
    /// The network's storage was taken.
    Storage,
}

impl From<NetworkError> for Error {
    fn from(error: NetworkError) -> Self {
        Self::Network(error)
    }
}

/// The group's end of the network whose producer is `P`.
pub struct Network<P: Produce + 'static> {
    end: &'static LocalEnd,
    watch: &'static ConfigurationWatch,
    producer: core::marker::PhantomData<P>,
}

/// What the network's task runs: not an API.
#[doc(hidden)]
pub struct NetworkStart<P: Produce + 'static> {
    service: Placed<P>,
    stop: WakingSpscSender<'static, (), 1>,
    stopper: wifi::NetworkStopper<'static, Notifies>,
}

impl<P: Produce + 'static> crate::NetTask for NetworkStart<P> {
    fn serve(self) -> impl core::future::Future<Output = ()> {
        serve_network(self)
    }
}

impl<P: Produce + 'static> Network<P> {
    /// Take `device` for the network and `storage` for its service, open the
    /// channel socket on `ports.receiving` and `producer`'s send-only socket
    /// on `ports.sending`: the service runs from the group's start, leases
    /// its address by DHCP and calls `producer` in its turns. A datagram for
    /// `ports.sending` is dropped; receiving and echoing stay on the channel
    /// socket, which comes back here.
    ///
    /// # Errors
    /// [`Error::AlreadyStarted`] for a second start; [`Error::Network`] when
    /// the service refuses its interface, its sockets or its watch.
    pub fn start(
        mut device: Device,
        boot: GroupBoot,
        storage: &'static mut NetworkStorage<P>,
        producer: P,
        ports: Ports,
    ) -> Result<(Self, Socket, GroupBoot<NetworkStart<P>>), Error> {
        let (network, transmit, stopper) = device.network.take().ok_or(Error::AlreadyStarted)?;
        let interface =
            Interface::dhcp(device.address, MTU, device.seed).map_err(NetworkError::Interface)?;
        let stop = STOP.try_take().ok_or(NetworkError::Storage)?;
        let memory = MEMORY.try_take().ok_or(NetworkError::Storage)?;
        let lanes = LANES.try_take().ok_or(NetworkError::Storage)?;
        let (pool, brand) = claim::<Brand, BLOCK, BLOCKS>(&mut memory.blocks)
            .map_err(|_: ClaimRefused<'_, BLOCK, BLOCKS>| NetworkError::Storage)?;
        let NetworkStorage {
            service,
            end,
            watch,
        } = storage;
        let (end, watch): (&'static LocalEnd, &'static ConfigurationWatch) = (end, watch);
        let (stop, stopping) = stop.split();
        let port = DevicePort::new(network, transmit, WifiBuffers);
        let owner = NetworkOwner::new(brand, interface, pool, port);
        let running = service.insert(Service::with_producer(
            owner,
            Direct,
            MemoryPolicy::KEEP,
            stopping,
            producer,
            end,
        ));
        running
            .bind_local(ports.sending, Dscp::default())
            .map_err(NetworkError::Local)?;
        let socket = running
            .bind(ports.receiving, Dscp::default(), Group::shared(), lanes)
            .map_err(|refused| NetworkError::Port(refused.reason))?;
        running.watch(watch).map_err(NetworkError::Watch)?;
        Ok((
            Self {
                end,
                watch,
                producer: core::marker::PhantomData,
            },
            socket,
            boot.with_net(NetworkStart {
                service,
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

    /// The interface's address, its version and its lease, as the service
    /// last published them; `None` once it ended.
    ///
    /// # Errors
    /// [`WatchRefused`]: a publication holds the value at this moment.
    pub fn configuration(&self) -> Result<Option<Configuration>, WatchRefused> {
        Ok(self.watch.read()?.configuration)
    }

    /// A wrapping count of the service's publications.
    pub fn changes(&self) -> u32 {
        self.watch.changes()
    }

    /// Wait until the count of publications is no longer `seen`, and return
    /// it. One task waits at a time.
    pub async fn changed(&self, seen: u32) -> u32 {
        self.watch.changed(seen).await
    }
}

/// The network's task: the service until a stop or the port's fault, which
/// records and resets. A stop drops the port, with the frames of the station
/// it still holds, before the station hears of it.
async fn serve_network<P: Produce + 'static>(start: NetworkStart<P>) {
    let NetworkStart {
        service,
        stop,
        stopper,
    } = start;
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
