//! The network core over the station, in place of the IP service: the
//! station's transmit buffers as the port's memory, a channel socket, a local
//! producer, and the task on the group's executor.
//!
//! The egress is chosen by type ([`TransmitMode`]): [`Queued`] chooses among
//! the waiting work by the station's airtime accounts, one per association,
//! each frame's reservation settled by the cost the station's completion
//! reports; [`Direct`] sends the first ready work with no tables and no
//! account. How an application provides data, a producer or a socket's
//! blocks, does not depend on it.
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
use core::task::{Context as TaskContext, Poll};
use oer_portable_memory::WakingSpscSender;
use oer_protocols_ieee80211_contracts_port::TxEnd;
use oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeStorage;
use oer_protocols_ieee80211_runtime::egress::{AirtimeEgress, RadioNotice};
use oer_protocols_ieee80211_runtime::station::{Completions, StationKey, TxRefused, TxSupply};
use oer_protocols_network_core::{
    ClaimRefused, Group, Interface, InterfaceError, NetworkOwner, OpenRefused, Retired, claim,
};
use oer_protocols_network_port::{
    EthernetFrame, EthernetFrameView, TransmitError, TransmitRefused, TxStorage, Unpublished,
};
use oer_protocols_network_runtime::ConfigurationWatch;
use oer_protocols_network_runtime::{
    DevicePort, EndCause, Ended, LocalRefused, MemoryPolicy, PortRefusal, Producer, Service,
    SocketLanes, StopLane, TxBuffers,
};
use oer_protocols_network_wire::ethernet::{EthernetHeader, HEADER_LEN};
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
use oer_protocols_network_port::{Held, Slot};
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
type Port = DevicePort<wifi::Network<'static, Notifies>, Transmit, WifiBuffers>;

/// Names a frame in the station's transmit buffers: its buffer's place
/// among the network's ([`wifi::TxSlot::place`]), below
/// [`wifi::NETWORK_BUFFERS`], which no other frame has until the station
/// gives the buffer back and its end is handed on.
pub type TxTicket = u8;
/// What the port tells the egress policy.
pub type TxNotice = RadioNotice<TxTicket, StationKey>;

/// Accounts the queued egress keeps: the association's, and earlier ones
/// whose frames have not settled.
const AIR_PEERS: usize = 4;
/// What each account earns a round, in microseconds: one long frame.
const AIR_QUANTUM: core::num::NonZeroU32 = match core::num::NonZeroU32::new(2_000) {
    Some(quantum) => quantum,
    None => panic!("a quantum is not zero"),
};

/// The queued egress's account of the station's air.
pub type AirStorage = AirtimeStorage<StationKey, AIR_PEERS, { wifi::NETWORK_BUFFERS }>;
type Airtime = AirtimeEgress<
    'static,
    wifi::StationAir<'static, Notifies>,
    TxTicket,
    AIR_PEERS,
    { wifi::NETWORK_BUFFERS },
>;

mod mode {
    /// Only this module's modes select an egress.
    pub trait Sealed {}
}

/// How the network chooses what goes out, by type: [`Queued`] or
/// [`Direct`].
pub trait TransmitMode: mode::Sealed + 'static {
    #[doc(hidden)]
    type Egress: oer_protocols_network_port::Egress<TxTicket, TxNotice> + 'static;
    /// What the egress keeps, in the image's storage.
    #[doc(hidden)]
    type Storage: 'static;
    #[doc(hidden)]
    const STORAGE: Self::Storage;
    #[doc(hidden)]
    fn egress(
        storage: &'static mut Self::Storage,
        link: &'static wifi::NetworkLinkSource<Notifies>,
    ) -> Self::Egress;
}

/// Waiting work stays outside the station's buffers; each frame is chosen
/// when the port has room, by the station's airtime accounts, one per
/// association; at most one frame per transmit buffer is reserved, and each
/// reservation settles by its frame's cost when the station gives the
/// buffer back.
pub enum Queued {}

/// The first ready work goes, with no account.
pub enum Direct {}

impl mode::Sealed for Queued {}
impl mode::Sealed for Direct {}

impl TransmitMode for Queued {
    type Egress = Airtime;
    type Storage = AirStorage;
    const STORAGE: AirStorage = AirtimeStorage::new(AIR_QUANTUM);
    fn egress(
        storage: &'static mut AirStorage,
        link: &'static wifi::NetworkLinkSource<Notifies>,
    ) -> Airtime {
        AirtimeEgress::new(
            storage,
            wifi::StationAir::<Notifies>::new(link, wifi::S31Ceiling::new()),
        )
    }
}

impl TransmitMode for Direct {
    type Egress = oer_protocols_network_runtime::Direct;
    type Storage = ();
    const STORAGE: () = ();
    fn egress(
        (): &'static mut (),
        _link: &'static wifi::NetworkLinkSource<Notifies>,
    ) -> oer_protocols_network_runtime::Direct {
        oer_protocols_network_runtime::Direct
    }
}

type NetService<P, M> = Service<
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
    <M as TransmitMode>::Egress,
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
    use super::{NetService, OpenRefused, Produce, ReceiveMode, TransmitMode};

    /// Only this module's modes select how the channel socket receives.
    pub trait Sealed {
        /// Open the channel socket on `port`.
        fn bind<P: Produce + 'static, M: TransmitMode>(
            service: &mut NetService<P, M>,
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
    fn bind<P: Produce + 'static, M: TransmitMode>(
        service: &mut NetService<P, M>,
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
    fn bind<P: Produce + 'static, M: TransmitMode>(
        service: &mut NetService<P, M>,
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
type Placed<P, M> = &'static mut Option<NetService<P, M>>;

/// What an image lends a network whose producer is `P` and whose egress is
/// `M`'s, in a static of its own: where the service stays, the producer's
/// end flag, which the service sets at its end ([`LocalEnd`]), the watch of
/// the interface's address, and what the egress keeps.
pub struct NetworkStorage<P: Produce + 'static, M: TransmitMode, R: ReceiveMode> {
    service: Option<NetService<P, M>>,
    end: LocalEnd,
    /// Where the service publishes the interface's address.
    watch: ConfigurationWatch,
    egress: M::Storage,
    /// The channel socket's lanes.
    lanes: R::Lanes,
}

impl<P: Produce + 'static, M: TransmitMode, R: ReceiveMode> NetworkStorage<P, M, R> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            service: None,
            end: LocalEnd::new(),
            watch: ConfigurationWatch::new(),
            egress: M::STORAGE,
            lanes: R::LANES,
        }
    }
}

impl<P: Produce + 'static, M: TransmitMode, R: ReceiveMode> Default for NetworkStorage<P, M, R> {
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

/// The transmit buffers no frame holds, shared by the supply, which takes
/// back what the station gave back, and the port, which writes them. Both run
/// in the network's task, so a lend is never refused for another's.
struct FreeSlots {
    slots: ClaimCell<heapless::Vec<wifi::TxSlot, { wifi::NETWORK_BUFFERS }>>,
    /// What the station gave back, each with its frame's end, until the port
    /// hands the end on: only then is the buffer free again.
    ended: ClaimCell<Completions<wifi::TxSlot, TxTicket, { wifi::NETWORK_BUFFERS }>>,
    freed: AtomicWaker,
}

impl FreeSlots {
    fn any(&self) -> bool {
        self.slots.lend(|slots| !slots.is_empty()).unwrap_or(false)
    }

    fn take(&self) -> Option<wifi::TxSlot> {
        self.slots.lend(heapless::Vec::pop).ok().flatten()
    }

    /// The station gave `slot` back, its frame ended so (`end`): it waits
    /// with that end for the port.
    fn ended(&self, slot: wifi::TxSlot, end: TxEnd) {
        let ticket = slot.place();
        let mut slot = Some(slot);
        // Only the network's task lends this, so a lend is never refused.
        let refused = self
            .ended
            .lend(|ended| {
                let slot = slot.take()?;
                ended.returned(slot, ticket, end).err()
            })
            .ok()
            .flatten();
        // One end a buffer, and this holds an end for every buffer: no end
        // is refused. Were one, its buffer would be free again at once.
        if let Some(slot) = refused {
            self.give(slot);
        }
        self.freed.wake();
    }

    /// The station took the frame of `ticket` and lost its buffer.
    fn lost(&self, ticket: TxTicket) {
        let _held = self.ended.lend(|ended| ended.lost(ticket));
        self.freed.wake();
    }

    /// The next end for the egress policy, the link now `link`: its buffer is
    /// free again from here.
    fn take_notice(&self, link: oer_protocols_network_port::LinkSnapshot) -> Option<TxNotice> {
        let (notice, slot) = self
            .ended
            .lend(|ended| ended.take_notice(link))
            .ok()
            .flatten()?;
        if let Some(slot) = slot {
            self.give(slot);
        }
        Some(notice)
    }

    /// Whether an end waits for the port.
    fn notice_waiting(&self, link: oer_protocols_network_port::LinkSnapshot) -> bool {
        self.ended
            .lend(|ended| ended.waiting(link))
            .unwrap_or(false)
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
    ended: ClaimCell::new(),
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
        let _ends = FREE.ended.install(Completions::new());
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
        let ticket = slot.place();
        self.publisher
            .publish(slot, 0, length)
            .map_err(|refused| match refused {
                wifi::TxPublishRefused::Range(slot) => TxRefused::Kept(TransmitRefused {
                    frame: WrittenFrame { slot, length },
                    reason: TransmitError::InvalidLength(FrameLengthError::TooLong),
                }),
                wifi::TxPublishRefused::Lost => {
                    // The port took it: its reservation settles as lost,
                    // and its buffer stays out of reach for good.
                    FREE.lost(ticket);
                    TxRefused::Lost
                }
            })
    }
    fn give(&mut self, memory: wifi::TxMemory, end: TxEnd) {
        FREE.ended(self.publisher.slot(memory), end);
    }
}

/// The port's end of the transmit buffers, and the link the station
/// publishes, whose epoch keys its air. Its name is public only for the
/// sealed bind of a [`ReceiveMode`]; nothing outside makes one.
#[doc(hidden)]
pub struct WifiBuffers {
    link: &'static wifi::NetworkLinkSource<Notifies>,
    /// The run of one octet each buffer holds from the last frame written
    /// into it, by its place, which runs to that frame's end. Nothing else
    /// writes a run's octets: the station writes its MPDU header in front of
    /// the frame's payload, and on a shorter header moves the payload down
    /// in place, which moves octets of a run ending at the frame's end onto
    /// octets of it (`write_mpdu`,
    /// `protocols/ieee80211/core/roles/sta/src/data/plane.rs`); the
    /// hardware's MIC and FCS room lies behind the frame, and a retry goes
    /// from the octets as they are.
    held: [Option<Held>; wifi::NETWORK_BUFFERS],
}

impl TxBuffers for WifiBuffers {
    type Frame = WrittenFrame;
    type Ticket = TxTicket;
    type Notice = TxNotice;
    fn ticket(&self, frame: &WrittenFrame) -> TxTicket {
        frame.slot.place()
    }
    fn take_notice(&mut self) -> Option<TxNotice> {
        FREE.take_notice(self.link.snapshot())
    }
    fn notice_waiting(&self) -> bool {
        FREE.notice_waiting(self.link.snapshot())
    }
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
        fill: impl FnOnce(&mut Slot<'_>) -> Result<(), E>,
    ) -> Result<WrittenFrame, Unpublished<PortRefusal, E>> {
        let mut slot = FREE.take().ok_or(Unpublished::Port(PortRefusal::NoRoom))?;
        let held = self.held.get_mut(usize::from(slot.place()));
        // A frame longer than a buffer is the port's refusal, before any
        // fill; a fill's own failure stays the fill's.
        let written = match (slot.bytes_mut().get_mut(..length), held) {
            (Some(bytes), Some(held)) => {
                let mut room = Slot::new(bytes, held.take());
                let filled = fill(&mut room).map_err(Unpublished::Fill);
                // Whatever the fill did, the buffer holds what the room says.
                *held = room.held();
                filled
            }
            (Some(bytes), None) => fill(&mut Slot::new(bytes, None)).map_err(Unpublished::Fill),
            (None, _) => Err(Unpublished::Port(PortRefusal::BadFrame)),
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

/// The group's end of the network whose producer is `P`, its egress `M`'s,
/// its channel socket receiving as `R` says.
pub struct Network<P: Produce + 'static, M: TransmitMode, R: ReceiveMode> {
    end: &'static LocalEnd,
    watch: &'static ConfigurationWatch,
    producer: core::marker::PhantomData<(P, M, R)>,
}

/// What the network's task runs: not an API.
#[doc(hidden)]
pub struct NetworkStart<P: Produce + 'static, M: TransmitMode> {
    service: Placed<P, M>,
    stop: WakingSpscSender<'static, (), 1>,
    stopper: wifi::NetworkStopper<'static, Notifies>,
}

impl<P: Produce + 'static, M: TransmitMode> crate::NetTask for NetworkStart<P, M> {
    fn serve(self) -> impl core::future::Future<Output = ()> {
        serve_network(self)
    }
}

impl<P: Produce + 'static, M: TransmitMode, R: ReceiveMode> Network<P, M, R> {
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
        storage: &'static mut NetworkStorage<P, M, R>,
        producer: P,
        ports: Ports,
    ) -> Result<(Self, R::Socket, GroupBoot<NetworkStart<P, M>>), Error> {
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
            egress,
            lanes,
        } = storage;
        let (end, watch): (&'static LocalEnd, &'static ConfigurationWatch) = (end, watch);
        let (stop, stopping) = stop.split();
        let link = network.link_source();
        let port = DevicePort::new(
            network,
            transmit,
            WifiBuffers {
                link,
                held: [None; wifi::NETWORK_BUFFERS],
            },
        );
        let owner = NetworkOwner::new(brand, interface, pool, port);
        let running = service.insert(
            Service::with_producer(
                owner,
                M::egress(egress, link),
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
        let socket = <R as receive::Sealed>::bind::<P, M>(running, ports.receiving, lanes)
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
async fn serve_network<P: Produce + 'static, M: TransmitMode>(start: NetworkStart<P, M>) {
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
