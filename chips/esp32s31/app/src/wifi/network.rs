//! The network core over the station, in place of the IP service: the
//! station's transmit buffers as the port's memory, its sockets and its task
//! on the group's executor.
//!
//! [`Network::start`] takes the station's device in the group's boot. The
//! address comes later, when the image has it: [`Network::configure`] builds
//! the service with its sockets and hands it to the task, once. A stop ends the
//! sockets and gives back every frame in the port's queue, so the station has
//! its received buffers back.
//! The other owners stay parked for the rest of the boot: nothing starts a
//! second network, and no contract ends the first one.

use super::*;
use core::task::{Context as TaskContext, Poll};
use oer_portable_memory::{AtomicWaker, ClaimCell, WakingSpscReceiver, WakingSpscSender};
use oer_protocols_ieee80211_runtime::station::{TxRefused, TxSupply};
use oer_protocols_network_core::{
    ClaimRefused, Group, Interface, InterfaceError, NetworkOwner, OpenRefused, Retired, claim,
};
use oer_protocols_network_port::{
    EthernetFrame, EthernetFrameView, FrameLengthError, SlotTooShort, TransmitError,
    TransmitRefused, TxStorage,
};
use oer_protocols_network_runtime::{
    DevicePort, Direct, EndCause, Ended, MemoryPolicy, PortRefusal, Service, SocketLanes, StopLane,
    TxBuffers,
};
use oer_protocols_network_wire::ethernet::{EthernetHeader, HEADER_LEN};
use oer_protocols_network_wire::ip::{Dscp, IPV4_HEADER_LEN};
use oer_protocols_network_wire::transport::UDP_HEADER_LEN;
use static_cell::ConstStaticCell;

pub use oer_protocols_network_core::{Block, Datagram, Remote};
pub use oer_protocols_network_runtime::{Observer, RESERVE_AHEAD, Refusal, Refused, SocketClosed};

/// The sockets a network holds.
pub const SOCKETS: usize = 2;
/// A block's octets: the longest UDP payload of an Ethernet MTU, RFC 768 over
/// RFC 791.
pub const BLOCK: usize = 1472;
/// The Ethernet MTU.
const MTU: u16 = 1500;
/// Blocks of the network's memory, for all sockets and both directions.
pub const BLOCKS: usize = 64;
/// Commands a socket has running.
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
type NetService = Service<
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
>;
type Lanes = SocketLanes<'static, 'static, Brand, BLOCK, DEPTH, LANE, CREDITS>;
/// The service, where it stays while it runs.
type Placed = &'static mut Option<NetService>;

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
static LANES: ConstStaticCell<[Lanes; SOCKETS]> =
    ConstStaticCell::new([const { SocketLanes::new() }; SOCKETS]);
static SERVICE: StaticCell<Option<NetService>> = StaticCell::new();
static STOP: ConstStaticCell<StopLane> = ConstStaticCell::new(StopLane::new());
static HANDOFF: ConstStaticCell<oer_portable_memory::WakingSpscQueue<Placed, 1>> =
    ConstStaticCell::new(oer_portable_memory::WakingSpscQueue::new());
/// The port and the service's lanes until the address comes. In a cell, as a
/// stop before that has to reach the port too.
static WAITING: ClaimCell<Waiting> = ClaimCell::new();

/// What [`Network::configure`] finds.
struct Waiting {
    port: Port,
    handoff: WakingSpscSender<'static, Placed, 1>,
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
    fn write(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), SlotTooShort>,
    ) -> Result<WrittenFrame, PortRefusal> {
        let mut slot = FREE.take().ok_or(PortRefusal::NoRoom)?;
        match slot.bytes_mut().get_mut(..length).map(fill) {
            Some(Ok(())) => Ok(WrittenFrame { slot, length }),
            Some(Err(_)) | None => {
                FREE.give(slot);
                Err(PortRefusal::BadFrame)
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
    /// The network's storage was taken.
    Storage,
}

/// The group's end of the network.
pub struct Network(());

/// What the network's task runs: not an API.
#[doc(hidden)]
pub struct NetworkStart {
    placed: WakingSpscReceiver<'static, Placed, 1>,
    stop: WakingSpscSender<'static, (), 1>,
    stopper: wifi::NetworkStopper<'static, Notifies>,
}

impl crate::NetTask for NetworkStart {
    fn serve(self) -> impl core::future::Future<Output = ()> {
        serve_network(self)
    }
}

impl Network {
    /// Take `device` for the network, whose task waits for its address from
    /// here on.
    ///
    /// # Errors
    /// The network started already.
    pub fn start(
        mut device: Device,
        boot: GroupBoot,
    ) -> Result<(Self, GroupBoot<NetworkStart>), Error> {
        let (network, transmit, stopper) = device.network.take().ok_or(Error::AlreadyStarted)?;
        let handoff = HANDOFF.try_take().ok_or(Error::Storage)?;
        let stop = STOP.try_take().ok_or(Error::Storage)?;
        let (handoff, placed) = handoff.split();
        let (stop, stopping) = stop.split();
        WAITING
            .install(Waiting {
                port: DevicePort::new(network, transmit, WifiBuffers),
                handoff,
                stopping,
                hardware: device.address,
            })
            .map_err(|_| Error::AlreadyStarted)?;
        Ok((
            Self(()),
            boot.with_net(NetworkStart {
                placed,
                stop,
                stopper,
            }),
        ))
    }

    /// Give the network its address, once, and open a socket on each of
    /// `ports`: the service runs from here on.
    ///
    /// # Errors
    /// [`NetworkError`]; an address it cannot take leaves it waiting.
    pub fn configure(
        &mut self,
        address: NetworkAddress,
        ports: [u16; SOCKETS],
    ) -> Result<[Socket; SOCKETS], NetworkError> {
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
        let memory = MEMORY.try_take().ok_or(NetworkError::Storage)?;
        let lanes = LANES.try_take().ok_or(NetworkError::Storage)?;
        let (pool, brand) = claim::<Brand, BLOCK, BLOCKS>(&mut memory.blocks)
            .map_err(|_: ClaimRefused<'_, BLOCK, BLOCKS>| NetworkError::Storage)?;
        let Waiting {
            port,
            handoff,
            stopping,
            ..
        } = WAITING.take().map_err(|_| NetworkError::NotWaiting)?;
        let owner = NetworkOwner::new(brand, interface, pool, port);
        let placed = SERVICE
            .try_uninit()
            .ok_or(NetworkError::Storage)?
            .write(Some(Service::new(
                owner,
                Direct,
                MemoryPolicy::KEEP,
                stopping,
            )));
        let [first, second] = lanes;
        let sockets = placed.as_mut().map(|service| {
            let [one, two] = ports;
            let open = |service: &mut NetService, port, lanes| {
                service
                    .bind(port, Dscp::default(), Group::shared(), lanes)
                    .map_err(|refused| NetworkError::Port(refused.reason))
            };
            open(service, one, first).and_then(|one| Ok([one, open(service, two, second)?]))
        });
        let sockets = sockets.ok_or(NetworkError::Storage)?;
        // The queue is the task's and holds this one value.
        let _sent = handoff.try_send(placed);
        sockets
    }
}

/// The network's task: its address first, then the service until a stop or
/// the port's fault, which records and resets. A stop drops the port, with the
/// frames of the station it still holds, before the station hears of it.
async fn serve_network(start: NetworkStart) {
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
        let _parked =
            core::mem::ManuallyDrop::new((results, pool, brand, sockets, adopted, returns));
    }
    stopper.stopped();
}

/// The service, once it has its address.
async fn configured(placed: &mut WakingSpscReceiver<'static, Placed, 1>) -> Placed {
    loop {
        if let Some(service) = placed.try_receive() {
            return service;
        }
        placed.ready().await;
    }
}
