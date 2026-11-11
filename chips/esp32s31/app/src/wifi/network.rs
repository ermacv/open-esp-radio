//! The network core over the station, in place of the IP service: the
//! station's transmit buffers as the port's memory, a channel socket, a local
//! producer, and the task on the group's executor.
//!
//! The network sends the first ready work: the station's MAC chooses which
//! of its queues goes on the air and charges each attempt, so the network
//! keeps no account of the air.
//!
//! How the channel socket receives is chosen by type too
//! ([`ReceiveMode`]): [`Copying`] copies each datagram into a block of the
//! network's memory; [`Lending`] keeps a datagram in the station's own
//! receive buffer, up to its credit, and copies only what the station
//! copied for its form (an A-MSDU's MSDUs, or one that shared its frame
//! with the EAPOL), dropping what it copied by pressure. Paths never
//! switch under pressure.
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
//! buffers back. Once the stop is confirmed, the network's task takes back
//! the loan of each buffer a lending socket still held as it returns, until
//! the pool's account is closed. The other owners stay parked for the rest
//! of the boot: nothing starts a second network, and no contract ends the
//! first one.

use super::*;
use oer_portable_memory::WakingSpscSender;
use oer_protocols_network_core::{
    ClaimRefused, Group, Interface, InterfaceError, NetworkOwner, OpenRefused, Retired, claim,
};
use oer_protocols_network_runtime::ConfigurationWatch;
use oer_protocols_network_runtime::{
    DevicePort, EndCause, Ended, LocalRefused, MemoryPolicy, Producer, Service, SocketLanes,
    StopLane,
};
use oer_protocols_network_wire::ethernet::HEADER_LEN;
use oer_protocols_network_wire::ip::{Dscp, IPV4_HEADER_LEN};
use oer_protocols_network_wire::transport::UDP_HEADER_LEN;
use static_cell::ConstStaticCell;

pub use oer_portable_memory::{AtomicWaker, ClaimCell};
/// The network's time, which a producer is called with.
pub use oer_portable_time::Instant as NetworkInstant;
pub use oer_protocols_network_core::{
    Block, Configuration, Datagram, Declined, DhcpState, Ipv4, Lease, Outcome, Payload, Remote,
    SendRefusal, WorkId,
};
/// Why a lent buffer no longer holds what the station showed.
pub use oer_protocols_network_port::FrameLengthError;
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
/// A received frame's buffer the station lends the network.
pub type LentBuffer = oer_protocols_network_port::LentFrame<wifi::ReceivedStorage<Notifies>>;
/// A lending channel socket ([`Lending`]).
pub type LendingSocket = oer_protocols_network_runtime::LendingSocket<
    'static,
    'static,
    Brand,
    BLOCK,
    DEPTH,
    LANE,
    CREDITS,
    LentBuffer,
>;
/// A datagram in the station's receive buffer, lent to the application.
pub type Lent = oer_protocols_network_runtime::Lent<'static, LentBuffer>;
/// What a lending socket receives: lent, or copied by the frame's form.
pub type Delivered =
    oer_protocols_network_runtime::Received<'static, 'static, Brand, BLOCK, LentBuffer>;
/// A block of the network.
pub type NetworkBlock = Block<'static, Brand, BLOCK>;

/// The port of the service over the station.
type Port = DevicePort<wifi::Network<'static, Notifies>, Transmit>;

/// Names a frame in the station's transmit buffers: its buffer's place
/// among the network's ([`wifi::TxSlot::place`]), below
/// [`wifi::NETWORK_BUFFERS`], which no other frame has until the station
/// gives the buffer back.
pub type TxTicket = u8;

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
    oer_protocols_network_runtime::Direct,
    Producer<'static, Brand, P, CREDITS>,
    Diagnosis,
>;

/// The service's switchable diagnostics: an observing image's (`observe`),
/// off until it switches them on; in any other image `()`, which leaves no
/// code.
#[cfg(feature = "observe")]
type Diagnosis = NetworkDiagnosis;
#[cfg(not(feature = "observe"))]
type Diagnosis = ();
#[cfg(feature = "observe")]
const DIAGNOSIS: Diagnosis = NetworkDiagnosis(());
#[cfg(not(feature = "observe"))]
const DIAGNOSIS: Diagnosis = ();

/// The network core's switchable diagnostics (docs/architecture.md,
/// "Switchable network-core diagnostics"), off until the observing image
/// switches them on; the network's task is their one writer.
#[cfg(feature = "observe")]
pub(crate) static DIAGNOSTICS: oer_chips_family_app::diagnostics::DiagnosticsCounter =
    oer_chips_family_app::diagnostics::DiagnosticsCounter::new();

/// The service's diagnostics: [`DIAGNOSTICS`]'s writer. Its clock is the
/// network hart's cycle counter, read only while they are on. Only this
/// module makes one; its type is named by the service's, which the receive
/// modes' sealed binds reach.
#[cfg(feature = "observe")]
#[doc(hidden)]
pub struct NetworkDiagnosis(());

#[cfg(feature = "observe")]
impl oer_protocols_network_runtime::Diagnostics for NetworkDiagnosis {
    fn on(&mut self) -> bool {
        DIAGNOSTICS.check()
    }

    fn clock(&mut self) -> u64 {
        oer_portable_riscv::cycle64()
    }

    // Out of the service's turn, as its sink is: it runs only while on.
    #[inline(never)]
    fn add(&mut self, counted: &oer_protocols_network_runtime::Diagnosis) {
        let oer_protocols_network_runtime::Diagnosis {
            turns,
            collections,
            walked,
            told,
            continued,
            stopped_spent,
            stopped_changed,
            stopped_grant,
            stopped_account,
            return_batches,
            blocks_returned,
            credits_restored,
            no_credit,
            no_credit_loans,
            no_credit_unreturned,
            materialised,
            stage_ticks,
        } = *counted;
        DIAGNOSTICS.add(&oer_chips_family_app::diagnostics::Counts {
            turns,
            collections,
            walked,
            told,
            continued,
            stopped_spent,
            stopped_changed,
            stopped_grant,
            stopped_account,
            return_batches,
            blocks_returned,
            credits_restored,
            no_credit,
            no_credit_loans,
            no_credit_unreturned,
            materialised,
            stage_ticks,
        });
    }
}

// The library counts the service's stages as the service names them.
#[cfg(feature = "observe")]
const _: () = {
    if oer_protocols_network_runtime::STAGES != oer_chips_family_app::diagnostics::STAGES {
        panic!("the diagnostics' stages are the service's");
    }
};
type Lanes = SocketLanes<'static, 'static, Brand, BLOCK, DEPTH, LANE, CREDITS>;
type LendingLanes = oer_protocols_network_runtime::LendingLanes<
    'static,
    'static,
    Brand,
    BLOCK,
    DEPTH,
    LANE,
    CREDITS,
    LentBuffer,
>;

mod receive {
    use super::{NetService, OpenRefused, Produce, ReceiveMode};

    /// Only this module's modes select how the channel socket receives.
    pub trait Sealed {
        /// Open the channel socket on `port`.
        fn bind<P: Produce + 'static>(
            service: &mut NetService<P>,
            port: u16,
            lanes: &'static mut <Self as ReceiveMode>::Lanes,
        ) -> Result<<Self as ReceiveMode>::Socket, OpenRefused>
        where
            Self: ReceiveMode;
    }
}

/// How the channel socket receives, by type: [`Copying`] or [`Lending`].
pub trait ReceiveMode: receive::Sealed + 'static {
    /// The channel socket.
    type Socket: 'static;
    /// Its lanes, in the image's storage.
    #[doc(hidden)]
    type Lanes: 'static;
    #[doc(hidden)]
    const LANES: Self::Lanes;
}

/// Each datagram is copied into a block of the network's memory.
pub enum Copying {}

/// Each datagram stays in the station's receive buffer, lent to the
/// application, and at most `CREDIT` at once: one more is dropped until a
/// lent one is dropped. The station's lend count holds [`wifi::LENT_RX`]
/// buffers; `CREDIT` of them are the socket's, and the rest stay for frames
/// on their way to the network. A copy the station made for the frame's form
/// comes in a block; one it made by pressure is dropped.
pub enum Lending<const CREDIT: usize> {}

impl ReceiveMode for Copying {
    type Socket = Socket;
    type Lanes = Lanes;
    const LANES: Lanes = SocketLanes::new();
}

impl receive::Sealed for Copying {
    fn bind<P: Produce + 'static>(
        service: &mut NetService<P>,
        port: u16,
        lanes: &'static mut <Self as ReceiveMode>::Lanes,
    ) -> Result<<Self as ReceiveMode>::Socket, OpenRefused> {
        service
            .bind(port, Dscp::default(), Group::shared(), lanes)
            .map_err(|refused| refused.reason)
    }
}

impl<const CREDIT: usize> Lending<CREDIT> {
    /// What the lend count keeps for frames on their way: all the socket's
    /// credit leaves.
    const RESERVE: usize = match wifi::LENT_RX.checked_sub(CREDIT) {
        Some(reserve) if CREDIT != 0 => reserve,
        _ => panic!("a lending socket's credit is at least one and fits the lend count"),
    };
}

impl<const CREDIT: usize> ReceiveMode for Lending<CREDIT> {
    type Socket = LendingSocket;
    type Lanes = LendingLanes;
    const LANES: LendingLanes = LendingLanes::new();
}

impl<const CREDIT: usize> receive::Sealed for Lending<CREDIT> {
    fn bind<P: Produce + 'static>(
        service: &mut NetService<P>,
        port: u16,
        lanes: &'static mut <Self as ReceiveMode>::Lanes,
    ) -> Result<<Self as ReceiveMode>::Socket, OpenRefused> {
        service
            .lend_from(wifi::LENT_RX, Self::RESERVE)
            .map_err(OpenRefused::Loans)?;
        service
            .bind_lending::<CREDIT>(port, Dscp::default(), Group::shared(), lanes)
            .map_err(|refused| refused.reason)
    }
}
/// The service, where it stays while it runs.
type Placed<P> = &'static mut Option<NetService<P>>;

/// What an image lends a network whose producer is `P`, in a static of its
/// own: where the service stays, the producer's end flag, which the service
/// sets at its end ([`LocalEnd`]), and the watch of the interface's
/// address.
pub struct NetworkStorage<P: Produce + 'static, R: ReceiveMode> {
    service: Option<NetService<P>>,
    end: LocalEnd,
    /// Where the service publishes the interface's address.
    watch: ConfigurationWatch,
    /// The channel socket's lanes.
    lanes: R::Lanes,
}

impl<P: Produce + 'static, R: ReceiveMode> NetworkStorage<P, R> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            service: None,
            end: LocalEnd::new(),
            watch: ConfigurationWatch::new(),
            lanes: R::LANES,
        }
    }
}

impl<P: Produce + 'static, R: ReceiveMode> Default for NetworkStorage<P, R> {
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
static STOP: ConstStaticCell<StopLane> = ConstStaticCell::new(StopLane::new());

/// The station's transmit end the network takes: it owns the network's
/// transmit buffers, and the network stack writes each frame in the buffer
/// it goes to the station in.
pub(super) type TxEnd = wifi::LocalEnd;

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

/// The group's end of the network whose producer is `P`, its channel
/// socket receiving as `R` says.
pub struct Network<P: Produce + 'static, R: ReceiveMode> {
    end: &'static LocalEnd,
    watch: &'static ConfigurationWatch,
    producer: core::marker::PhantomData<(P, R)>,
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

impl<P: Produce + 'static, R: ReceiveMode> Network<P, R> {
    /// Take `device` for the network and `storage` for its service, open the
    /// channel socket on `ports.receiving` and `producer`'s send-only socket
    /// on `ports.sending`: the service runs from the group's start, leases
    /// its address by DHCP and calls `producer` in its turns. A datagram for
    /// `ports.sending` is dropped; receiving and echoing stay on the channel
    /// socket, which receives as `R` says and comes back here.
    ///
    /// # Errors
    /// [`Error::AlreadyStarted`] for a second start; [`Error::Network`] when
    /// the service refuses its interface, its sockets or its watch.
    pub fn start(
        mut device: Device,
        boot: GroupBoot,
        storage: &'static mut NetworkStorage<P, R>,
        producer: P,
        ports: Ports,
    ) -> Result<(Self, R::Socket, GroupBoot<NetworkStart<P>>), Error> {
        let (network, transmit, stopper) = device.network.take().ok_or(Error::AlreadyStarted)?;
        let interface =
            Interface::dhcp(device.address, MTU, device.seed).map_err(NetworkError::Interface)?;
        let stop = STOP.try_take().ok_or(NetworkError::Storage)?;
        let memory = MEMORY.try_take().ok_or(NetworkError::Storage)?;
        let (pool, brand) = claim::<Brand, BLOCK, BLOCKS>(&mut memory.blocks)
            .map_err(|_: ClaimRefused<'_, BLOCK, BLOCKS>| NetworkError::Storage)?;
        let NetworkStorage {
            service,
            end,
            watch,
            lanes,
        } = storage;
        let (end, watch): (&'static LocalEnd, &'static ConfigurationWatch) = (end, watch);
        let (stop, stopping) = stop.split();
        let port = DevicePort::new(network, transmit);
        let owner = NetworkOwner::new(brand, interface, pool, port);
        let running = service.insert(
            Service::with_producer(
                owner,
                oer_protocols_network_runtime::Direct,
                MemoryPolicy::KEEP,
                stopping,
                producer,
                end,
            )
            .with_diagnostics(DIAGNOSIS),
        );
        running
            .bind_local(ports.sending, Dscp::default())
            .map_err(NetworkError::Local)?;
        let socket = <R as receive::Sealed>::bind::<P>(running, ports.receiving, lanes)
            .map_err(NetworkError::Port)?;
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
/// it still holds, before the station hears of it; the task then serves the
/// late returns of lent buffers' loans until none is out.
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
    let Some(ended) = service.take().map(Service::retire) else {
        stopper.stopped();
        return;
    };
    let Ended {
        retired,
        mut sockets,
        mut adopted,
        returns,
        producers,
        ..
    } = ended;
    let Retired {
        results,
        mut pool,
        brand,
        mut port,
    } = retired;
    // The queues outlive the port, so dropping it returns no frame and
    // takes back no memory: both happen here first.
    port.give_back();
    drop(port);
    // The stop is confirmed first: it waits for no lent buffer.
    stopper.stopped();
    // Then this task takes each lent buffer's loan back as it returns,
    // until the pool's account is closed. A buffer held for ever keeps it
    // waiting, as nothing expires.
    oer_protocols_network_runtime::serve_late_loans(
        &mut pool,
        [&mut sockets[..], &mut adopted[..]],
    )
    .await;
    // Memory, lanes and result places are all static, so what a late socket
    // or observer reaches stays.
    let _parked =
        core::mem::ManuallyDrop::new((results, pool, brand, sockets, adopted, returns, producers));
}
