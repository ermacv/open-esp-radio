//! The network core over a Wi-Fi role's network end: the role's transmit
//! buffers as the port's memory, a channel socket, a local producer, and the
//! task on the group's executor. A network is an instance over one role's
//! port: the image lends its storage ([`NetworkStorage`]) and names its
//! brand, a type of its own, so an image with both roles gives each its own
//! network.
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
//! [`Network::start`] takes a role's device in the group's boot, how its
//! interface is addressed ([`Addressing`]), the storage the image lends for
//! its service ([`NetworkStorage`]), its producer and the two ports: the
//! service's type holds the image's brand and producer, so no static of the
//! library can. The service starts at once: a leased interface's DHCP client
//! leases the address, RFC 2131, from the first link up, with a seed the
//! role drew at boot; a given one holds its address from the start; and
//! [`Network::configuration`] tells the group what it holds. The producer is called in the network task's own
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
    Interest, LocalEnd, Next, Observer, Offer, Produce, RESERVE_AHEAD, Refusal, Refused, Sent,
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

/// A socket of the network branded `B`.
pub type Socket<B> =
    oer_protocols_network_runtime::Socket<'static, 'static, B, BLOCK, DEPTH, LANE, CREDITS>;

/// A datagram of the network branded `B`.
pub type Received<B> = Datagram<'static, B, BLOCK>;
/// A received frame's buffer the station lends the network.
pub type LentBuffer = oer_protocols_network_port::LentFrame<wifi::ReceivedStorage<Notifies>>;
/// A lending channel socket ([`Lending`]).
pub type LendingSocket<B> = oer_protocols_network_runtime::LendingSocket<
    'static,
    'static,
    B,
    BLOCK,
    DEPTH,
    LANE,
    CREDITS,
    LentBuffer,
>;
/// A datagram in the station's receive buffer, lent to the application.
pub type Lent = oer_protocols_network_runtime::Lent<'static, LentBuffer>;
/// What a lending socket receives: lent, or copied by the frame's form.
pub type Delivered<B> =
    oer_protocols_network_runtime::Received<'static, 'static, B, BLOCK, LentBuffer>;
/// A block of the network branded `B`.
pub type NetworkBlock<B> = Block<'static, B, BLOCK>;

/// The port of the service over a role's network end: the station's and the
/// access point's are one type.
type Port = DevicePort<wifi::Network<'static, Notifies>, Transmit>;

/// Names a frame in the station's transmit buffers: its buffer's place
/// among the network's ([`wifi::TxSlot::place`]), below
/// [`wifi::NETWORK_BUFFERS`], which no other frame has until the station
/// gives the buffer back.
pub type TxTicket = u8;

type NetService<B, P> = Service<
    'static,
    'static,
    B,
    BLOCK,
    SOCKETS,
    CREDITS,
    RECEIVED,
    NEIGHBORS,
    BLOCKS,
    DEPTH,
    LANE,
    Port,
    Producer<'static, B, P, CREDITS>,
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
            walked,
            told,
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
            walked,
            told,
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
type Lanes<B> = SocketLanes<'static, 'static, B, BLOCK, DEPTH, LANE, CREDITS>;
type LendingLanes<B> = oer_protocols_network_runtime::LendingLanes<
    'static,
    'static,
    B,
    BLOCK,
    DEPTH,
    LANE,
    CREDITS,
    LentBuffer,
>;

mod receive {
    use super::{NetService, OpenRefused, Produce, ReceiveMode};

    /// Only this module's modes select how the channel socket receives.
    pub trait Sealed<B: 'static> {
        /// Open the channel socket on `port`.
        fn bind<P: Produce + 'static>(
            service: &mut NetService<B, P>,
            port: u16,
            lanes: &'static mut <Self as ReceiveMode<B>>::Lanes,
        ) -> Result<<Self as ReceiveMode<B>>::Socket, OpenRefused>
        where
            Self: ReceiveMode<B>;
    }
}

/// How the channel socket of a network branded `B` receives, by type:
/// [`Copying`] or [`Lending`].
pub trait ReceiveMode<B: 'static>: receive::Sealed<B> + 'static {
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

/// Each datagram stays in the role's receive buffer, lent to the
/// application, and at most `CREDIT` at once: one more is dropped until a
/// lent one is dropped. The station's lend count holds [`wifi::LENT_RX`]
/// buffers; `CREDIT` of them are the socket's, and the rest stay for frames
/// on their way to the network. A copy the role made for the frame's form
/// comes in a block (an access point's every MSDU is one); one it made by
/// pressure is dropped.
pub enum Lending<const CREDIT: usize> {}

impl<B: 'static> ReceiveMode<B> for Copying {
    type Socket = Socket<B>;
    type Lanes = Lanes<B>;
    const LANES: Lanes<B> = SocketLanes::new();
}

impl<B: 'static> receive::Sealed<B> for Copying {
    fn bind<P: Produce + 'static>(
        service: &mut NetService<B, P>,
        port: u16,
        lanes: &'static mut <Self as ReceiveMode<B>>::Lanes,
    ) -> Result<<Self as ReceiveMode<B>>::Socket, OpenRefused> {
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

impl<B: 'static, const CREDIT: usize> ReceiveMode<B> for Lending<CREDIT> {
    type Socket = LendingSocket<B>;
    type Lanes = LendingLanes<B>;
    const LANES: LendingLanes<B> = LendingLanes::new();
}

impl<B: 'static, const CREDIT: usize> receive::Sealed<B> for Lending<CREDIT> {
    fn bind<P: Produce + 'static>(
        service: &mut NetService<B, P>,
        port: u16,
        lanes: &'static mut <Self as ReceiveMode<B>>::Lanes,
    ) -> Result<<Self as ReceiveMode<B>>::Socket, OpenRefused> {
        service
            .lend_from(wifi::LENT_RX, Self::RESERVE)
            .map_err(OpenRefused::Loans)?;
        service
            .bind_lending::<CREDIT>(port, Dscp::default(), Group::shared(), lanes)
            .map_err(|refused| refused.reason)
    }
}
/// The service, where it stays while it runs.
type Placed<B, P> = &'static mut Option<NetService<B, P>>;

/// What an image lends a network branded `B` whose producer is `P`, in a
/// static of its own: its stop's lane, where the service stays, the
/// producer's end flag, which the service sets at its end ([`LocalEnd`]),
/// the watch of the interface's address and the channel socket's lanes. The
/// network's blocks are a zeroed static of their own ([`NetworkMemory`]).
/// Nothing of a network is the library's: a second instance is a second
/// storage of another brand.
pub struct NetworkStorage<B: 'static, P: Produce + 'static, R: ReceiveMode<B>> {
    stop: StopLane,
    service: Option<NetService<B, P>>,
    end: LocalEnd,
    /// Where the service publishes the interface's address.
    watch: ConfigurationWatch,
    /// The channel socket's lanes.
    lanes: R::Lanes,
}

impl<B: 'static, P: Produce + 'static, R: ReceiveMode<B>> NetworkStorage<B, P, R> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            stop: StopLane::new(),
            service: None,
            end: LocalEnd::new(),
            watch: ConfigurationWatch::new(),
            lanes: R::LANES,
        }
    }
}

impl<B: 'static, P: Produce + 'static, R: ReceiveMode<B>> Default for NetworkStorage<B, P, R> {
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
    if !BLOCK.is_multiple_of(4) {
        panic!("a block's length is a whole number of words");
    }
};

/// A network's block memory, for all its sockets and both directions, its
/// blocks at [`BLOCK_WORD_OFFSET`]: all zero, so the image's static of it
/// is zeroed memory, never initialized data. An image lends one per network
/// at its start ([`Network::start`]).
#[repr(C, align(4))]
pub struct NetworkMemory {
    before: [u8; BLOCK_WORD_OFFSET],
    blocks: [[u8; BLOCK]; BLOCKS],
}

impl NetworkMemory {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            before: [0; BLOCK_WORD_OFFSET],
            blocks: [[0; BLOCK]; BLOCKS],
        }
    }
}

impl Default for NetworkMemory {
    fn default() -> Self {
        Self::new()
    }
}

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

/// How a network's interface takes its IPv4 address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Addressing {
    /// Leased by the core's DHCP client from the first link up, RFC 2131,
    /// with the seed the role drew at boot: a station's.
    Dhcp,
    /// Given: the interface holds it from the start, an access point's.
    Given {
        address: [u8; 4],
        prefix: u8,
        gateway: Option<[u8; 4]>,
    },
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

/// The group's end of the network branded `B` whose producer is `P`, its
/// channel socket receiving as `R` says.
pub struct Network<B: 'static, P: Produce + 'static, R: ReceiveMode<B>> {
    end: &'static LocalEnd,
    watch: &'static ConfigurationWatch,
    producer: core::marker::PhantomData<(B, P, R)>,
}

/// What a network's task runs: not an API.
#[doc(hidden)]
pub struct NetworkStart<B: 'static, P: Produce + 'static> {
    service: Placed<B, P>,
    stop: WakingSpscSender<'static, (), 1>,
    stopper: wifi::NetworkStopper<'static, Notifies>,
}

impl<B: 'static, P: Produce + 'static> crate::NetTask for NetworkStart<B, P> {
    fn serve(self) -> impl core::future::Future<Output = ()> {
        serve_network(self)
    }
}

impl<B: 'static, P: Produce + 'static, R: ReceiveMode<B>> Network<B, P, R> {
    /// Take a role's `device` for the network, addressed as `addressing`
    /// says, `storage` for its service and `memory` for its blocks; open the channel socket on
    /// `ports.receiving` and `producer`'s send-only socket on
    /// `ports.sending`: the service runs from the group's start beside the
    /// group's other networks and calls `producer` in its turns. A datagram
    /// for `ports.sending` is dropped; receiving and echoing stay on the
    /// channel socket, which receives as `R` says and comes back here.
    ///
    /// # Errors
    /// [`Error::AlreadyStarted`] for a device a network took already;
    /// [`Error::Network`] when the service refuses its interface, its
    /// memory, its sockets or its watch.
    #[allow(
        clippy::type_complexity,
        reason = "the network, its socket and the group"
    )]
    pub fn start<N: crate::NetTask>(
        mut device: Device,
        addressing: Addressing,
        boot: GroupBoot<N>,
        storage: &'static mut NetworkStorage<B, P, R>,
        memory: &'static mut NetworkMemory,
        producer: P,
        ports: Ports,
    ) -> Result<(Self, R::Socket, GroupBoot<(N, NetworkStart<B, P>)>), Error> {
        let (network, transmit, stopper) = device.network.take().ok_or(Error::AlreadyStarted)?;
        let interface = match addressing {
            Addressing::Dhcp => Interface::dhcp(device.address, MTU, device.seed),
            Addressing::Given {
                address,
                prefix,
                gateway,
            } => Interface::new(device.address, address, prefix, gateway, MTU),
        }
        .map_err(NetworkError::Interface)?;
        let NetworkStorage {
            stop,
            service,
            end,
            watch,
            lanes,
        } = storage;
        let (pool, brand) = claim::<B, BLOCK, BLOCKS>(&mut memory.blocks)
            .map_err(|_: ClaimRefused<'_, BLOCK, BLOCKS>| NetworkError::Storage)?;
        let (end, watch): (&'static LocalEnd, &'static ConfigurationWatch) = (end, watch);
        let (stop, stopping) = stop.split();
        let port = DevicePort::new(network, transmit);
        let owner = NetworkOwner::new(brand, interface, pool, port);
        let running = service.insert(
            Service::with_producer(owner, MemoryPolicy::KEEP, stopping, producer, end)
                .with_diagnostics(DIAGNOSIS),
        );
        running
            .bind_local(ports.sending, Dscp::default())
            .map_err(NetworkError::Local)?;
        let socket = <R as receive::Sealed<B>>::bind::<P>(running, ports.receiving, lanes)
            .map_err(NetworkError::Port)?;
        running.watch(watch).map_err(NetworkError::Watch)?;
        Ok((
            Self {
                end,
                watch,
                producer: core::marker::PhantomData,
            },
            socket,
            boot.with_net(NetworkStart::<B, P> {
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
async fn serve_network<B: 'static, P: Produce + 'static>(start: NetworkStart<B, P>) {
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
    // Every frame the port holds goes back and every memory the role gave
    // back is taken, before the role hears of the stop.
    port.give_back();
    // The stop is confirmed first: it waits for no lent buffer.
    stopper.stopped();
    // Then this task takes each lent buffer's loan back as it returns,
    // until the pool's account is closed, and keeps the port's returns: a
    // memory the role gives back after the report (a frame the network
    // published before it saw the request) goes home to the port's
    // transmit owner. A buffer held for ever keeps the loans waiting, as
    // nothing expires; the task, with what it holds, lasts the boot.
    let mut loans = core::pin::pin!(oer_protocols_network_runtime::serve_late_loans(
        &mut pool,
        [&mut sockets[..], &mut adopted[..]],
    ));
    let _held = (&results, &brand, &returns, &producers);
    // The loans' future is polled until it ends and never after; the
    // port's returns are served for good.
    let mut loans_ended = false;
    let never = core::future::poll_fn(|cx| {
        if !loans_ended {
            loans_ended = loans.as_mut().poll(cx).is_ready();
        }
        port.serve_returns(cx);
        core::task::Poll::<core::convert::Infallible>::Pending
    })
    .await;
    match never {}
}
