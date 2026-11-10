//! The core: sockets, frames in, work out.
//!
//! SOURCE: RFC 791 and RFC 1122 for IPv4; RFC 768 for UDP; RFC 826 for ARP;
//! RFC 2131 for DHCP (`dhcp`); RFC 792 for ICMP echo (`echo`).

use core::marker::PhantomData;

use oer_portable_events::{EventSink, report};
use oer_portable_time::Instant;
use oer_portable_wire::Writer;
use oer_protocols_network_port::{
    Candidate, Class, CopyReason, EthernetFrameView, Holding, LinkSnapshot, LinkState, Ready,
};
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::dhcp;
use oer_protocols_network_wire::ethernet::{self, EtherType, EthernetHeader};
use oer_protocols_network_wire::icmp::{self, Echo, PROTOCOL_ICMP};
use oer_protocols_network_wire::ip::{
    Dscp, IPV4_HEADER_LEN, Ipv4Fields, Ipv4Header, Ipv4VersionIhl,
};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UDP_HEADER_LEN, UdpHeader};

use crate::block::{Block, CoreBrand, Group, Pool, Tag};
use crate::dhcp::{Change, Client, Destination, DhcpDrop, DhcpState, Lease, Outgoing};
use crate::diagnostics::{Count, counted};
use crate::echo::{Echoes, IcmpDrop, Owed};
use crate::interface::{
    Addressing, BROADCAST, BROADCAST_HARDWARE, Interface, Ipv4, UNSPECIFIED, is_unicast,
};
use crate::loans::{LoansRefused, NotLent};
use crate::neighbor::{Found, Lookup, Neighbors};
use crate::places::{Declared, Place, Places, Run, Sending};

/// RFC 1122, 3.2.1.7: a default time to live large enough for the Internet.
const TIME_TO_LIVE: u8 = 64;
/// Headers before a datagram's data.
const HEADERS: usize = ethernet::HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN;
/// An ARP frame.
const ARP_FRAME: usize = ethernet::HEADER_LEN + arp::PACKET_LEN;
/// Where the UDP header starts in a frame.
const UDP_AT: usize = ethernet::HEADER_LEN + IPV4_HEADER_LEN;
/// An echo reply's Ethernet and IPv4 headers.
const ECHO_HEADERS: usize = ethernet::HEADER_LEN + IPV4_HEADER_LEN;
/// What control work and a default socket carry.
const BEST_EFFORT: Dscp = Dscp::from_field(oer_portable_wire::U8::constant::<0>());

/// A peer's address and port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Remote {
    pub address: [u8; 4],
    pub port: u16,
}

/// One piece of work. A serial never comes back, so a grant names this work
/// and no later work in its place.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Work {
    Reply {
        serial: u64,
    },
    Request {
        neighbor: u8,
        serial: u64,
    },
    Datagram {
        socket: u8,
        place: u8,
        serial: u64,
        /// The station address it was chosen for: another is other work.
        peer: [u8; 6],
        /// What its demand found of the next hop, and while that holds:
        /// none for a broadcast.
        found: Option<Found>,
    },
    /// The DHCP client's owed message.
    Dhcp {
        serial: u64,
        peer: [u8; 6],
    },
    /// An echo reply owed.
    Echo {
        serial: u64,
        peer: [u8; 6],
        /// As a datagram's.
        found: Found,
    },
}

/// Work that is ready, before any frame of it exists: what a scheduler needs
/// to choose it.
#[derive(Debug, Eq, PartialEq)]
pub struct Demand<B> {
    /// What a scheduler may know of it.
    pub ready: Ready,
    work: Work,
    brand: Tag<B>,
}

impl<B> Candidate for Demand<B> {
    fn ready(&self) -> &Ready {
        &self.ready
    }
}

impl<B> Demand<B> {
    /// Choose this work.
    pub const fn grant(self) -> Grant<B> {
        Grant {
            work: self.work,
            brand: self.brand,
        }
    }
}

/// The right to send one piece of work, and no other.
#[derive(Debug, Eq, PartialEq)]
pub struct Grant<B> {
    work: Work,
    brand: Tag<B>,
}

/// A frame ready to write, as values.
#[derive(Debug)]
pub(crate) enum Frame<'a> {
    Arp([u8; ARP_FRAME]),
    /// A stored datagram's: its data is copied into the slot after the
    /// headers and summed in the same pass, the UDP checksum last.
    Datagram {
        /// The headers, the UDP checksum still zero.
        headers: [u8; HEADERS],
        udp: UdpHeader,
        pseudo: [u8; 12],
        data: &'a [u8],
    },
    /// A declared work's: its producer fills the data in the slot, between
    /// the headers written before and the checksum written after.
    Produced {
        /// The headers, the UDP checksum still zero.
        headers: [u8; HEADERS],
        udp: UdpHeader,
        pseudo: [u8; 12],
        length: usize,
        work: WorkId,
    },
    /// The DHCP client's message, written in the slot after its headers,
    /// the UDP checksum last.
    Dhcp {
        headers: [u8; HEADERS],
        udp: UdpHeader,
        pseudo: [u8; 12],
        message: Outgoing,
    },
    /// An echo reply: the request's identity and data under a new header.
    Echo {
        headers: [u8; ECHO_HEADERS],
        reply: [u8; icmp::ECHO_HEADER_LEN],
        data: &'a [u8],
    },
}

/// One declared work, unique for its brand's life: no two open works share
/// it, and a place's next work has another.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkId {
    serial: u64,
    place: u8,
}

impl WorkId {
    /// The command place it runs in, among its socket's.
    pub const fn place(self) -> usize {
        self.place as usize
    }
}

/// Why a producer did not fill a work's data. The work ends
/// [`Outcome::Declined`]: it did not go out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Declined {
    /// The data the work was declared for is gone or was overwritten: its
    /// source moved on.
    Stale,
    /// The producer cannot write its data into the region it was lent.
    Unwritable,
}

/// Why the core's own fill of a slot failed, which the port hands back as
/// [`Unpublished::Fill`](oer_protocols_network_port::Unpublished::Fill).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FillFailed {
    /// The room a port gave does not hold the frame.
    SlotTooShort,
    /// The producer declined to fill a declared work.
    Declined(Declined),
}

impl Frame<'_> {
    pub(crate) fn length(&self) -> usize {
        match self {
            Self::Arp(frame) => frame.len(),
            Self::Datagram { headers, data, .. } => headers.len().saturating_add(data.len()),
            Self::Produced {
                headers, length, ..
            } => headers.len().saturating_add(*length),
            Self::Dhcp {
                headers, message, ..
            } => headers.len().saturating_add(message.length()),
            Self::Echo {
                headers,
                reply,
                data,
            } => headers
                .len()
                .saturating_add(reply.len())
                .saturating_add(data.len()),
        }
    }

    /// Write the frame into `slot`. A produced frame's headers go first, then
    /// `produce` fills its data in place, and the UDP checksum, summed over
    /// that data in the slot, goes last; `produce` is called for no other.
    pub(crate) fn write(
        &self,
        slot: &mut [u8],
        produce: impl FnOnce(WorkId, &mut [u8]) -> Result<(), Declined>,
    ) -> Result<(), FillFailed> {
        let short = |_| FillFailed::SlotTooShort;
        match self {
            Self::Arp(frame) => Writer::new(slot).write(frame).map_err(short),
            Self::Datagram {
                headers,
                udp,
                pseudo,
                data,
            } => {
                let (head, rest) = slot
                    .split_at_mut_checked(HEADERS)
                    .ok_or(FillFailed::SlotTooShort)?;
                Writer::new(head).write(headers).map_err(short)?;
                // One read of the stored data: its copy and its sum.
                let sum = Checksum::new()
                    .with(pseudo)
                    .with(&udp.to_bytes())
                    .copied(data, rest)
                    .map_err(|_| FillFailed::SlotTooShort)?;
                checksum_last(head, udp, sum)
            }
            Self::Produced {
                headers,
                udp,
                pseudo,
                length,
                work,
            } => {
                let (head, rest) = slot
                    .split_at_mut_checked(HEADERS)
                    .ok_or(FillFailed::SlotTooShort)?;
                Writer::new(head).write(headers).map_err(short)?;
                let data = rest.get_mut(..*length).ok_or(FillFailed::SlotTooShort)?;
                produce(*work, data).map_err(FillFailed::Declined)?;
                let sum = Checksum::new()
                    .with(pseudo)
                    .with(&udp.to_bytes())
                    .with(data);
                checksum_last(head, udp, sum)
            }
            Self::Dhcp {
                headers,
                udp,
                pseudo,
                message,
            } => {
                let (head, rest) = slot
                    .split_at_mut_checked(HEADERS)
                    .ok_or(FillFailed::SlotTooShort)?;
                Writer::new(head).write(headers).map_err(short)?;
                let data = rest
                    .get_mut(..message.length())
                    .ok_or(FillFailed::SlotTooShort)?;
                message.write(&mut Writer::new(data)).map_err(short)?;
                let sum = Checksum::new()
                    .with(pseudo)
                    .with(&udp.to_bytes())
                    .with(data);
                checksum_last(head, udp, sum)
            }
            Self::Echo {
                headers,
                reply,
                data,
            } => {
                let mut writer = Writer::new(slot);
                writer
                    .write(headers)
                    .and_then(|()| writer.write(reply))
                    .and_then(|()| writer.write(data))
                    .map_err(short)
            }
        }
    }
}

/// Write `udp` with the checksum `sum` finishes into the headers at `head`,
/// once the data it covers is in the slot.
fn checksum_last(head: &mut [u8], udp: &UdpHeader, sum: Checksum) -> Result<(), FillFailed> {
    let checksum = udp_field(sum.finish());
    let field = head.get_mut(UDP_AT..).ok_or(FillFailed::SlotTooShort)?;
    Writer::new(field)
        .write(&UdpHeader { checksum, ..*udp }.to_bytes())
        .map_err(|_| FillFailed::SlotTooShort)
}

/// A socket. Its handle is not a copy: closing consumes it.
#[derive(Debug, Eq, PartialEq)]
pub struct SocketId<B> {
    index: u8,
    serial: u64,
    brand: Tag<B>,
}

impl<B> SocketId<B> {
    /// The flow this socket's work carries.
    pub const fn flow(&self) -> u8 {
        self.index
    }

    /// Where this socket's ends are told ([`Core::next_end`]): it outlives
    /// the handle's close, for the ends of what the socket ran.
    pub const fn ends(&self) -> Ends<B> {
        Ends {
            index: self.index,
            serial: self.serial,
            brand: PhantomData,
        }
    }
}

/// Where one socket's ended commands are told, for as long as the socket
/// holds its place in the core: a socket opened again there later is
/// another, and this tells none of its ends.
#[derive(Debug, Eq, PartialEq)]
pub struct Ends<B> {
    index: u8,
    serial: u64,
    brand: Tag<B>,
}

impl<B> Clone for Ends<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B> Copy for Ends<B> {}

/// A datagram accepted and not yet taken by the port, which only this can
/// evict. What the port took, an application's blocks and a socket's received
/// data are never here.
#[derive(Debug, Eq, PartialEq)]
pub struct Queued<B> {
    pub flow: u8,
    /// What its block counts toward, which need not be its socket's.
    pub group: Group<B>,
    /// When it was accepted.
    pub since: Instant,
    place: u8,
    serial: u64,
    brand: Tag<B>,
}

/// A running command's right to its one result.
#[derive(Debug, Eq, PartialEq)]
pub struct CommandId<B> {
    socket: u8,
    place: u8,
    serial: u64,
    brand: Tag<B>,
}

impl<B> CommandId<B> {
    /// The work this command runs.
    pub const fn work(&self) -> WorkId {
        WorkId {
            serial: self.serial,
            place: self.place,
        }
    }
}

/// How a command ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The port accepted the datagram. This says nothing of delivery.
    Accepted,
    /// No neighbor answered for the datagram's next hop.
    Unreachable,
    /// The socket closed before the port accepted it.
    Closed,
    /// The socket's close finished.
    SocketClosed,
    /// The port contradicted itself, so whether the datagram went out is not
    /// known. It is not sent again.
    Unknown,
    /// The core retired before the port accepted it.
    Retired,
    /// The stack took it back before the port accepted it.
    Evicted(Eviction),
    /// The port refused this frame, or the neighbor request it waited for,
    /// for good. It did not go out.
    Rejected,
    /// Its producer declined to fill it, or its source moved on before the
    /// port took it. It did not go out.
    Declined,
    /// The interface's address changed or went before the port took it: it
    /// was accepted for another address, and did not go out.
    Readdressed,
}

/// Why accepted work was taken back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Eviction {
    /// Others wait for memory that this was holding.
    MemoryPressure,
}

/// What became of a command.
#[derive(Debug, Eq, PartialEq)]
pub enum CommandState<B> {
    /// It ended; its place freed.
    Finished(Outcome),
    /// It runs; the right to its result comes back.
    Running(CommandId<B>),
    /// Not this core's: a retired core's result is with its
    /// [`RetiredResults`].
    Gone,
}

/// A datagram that was received.
#[derive(Debug)]
pub struct Datagram<'m, B, const SIZE: usize> {
    pub remote: Remote,
    pub block: Block<'m, B, SIZE>,
}

/// Why a socket did not open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenRefused {
    /// Every socket is in use, until one closes.
    NoSocket,
    /// Another socket holds the port, until it closes.
    PortInUse,
    /// The receive loans' account refused the lending socket's credit.
    Loans(LoansRefused),
}

/// The socket ended. A final refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaleSocket;

/// Why a datagram was not accepted, before anything moved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendRefusal {
    /// A final refusal.
    Stale,
    /// Every place of the socket runs a command, until one ends and its result is read.
    NoPlace,
    /// Longer than the interface's MTU. A final refusal.
    TooLong,
    /// No route leads there. A final refusal for this configuration.
    NoRoute,
    /// Every neighbor entry is asked for or holds a neighbor a datagram
    /// needs, until one answers, gives up or that datagram ends.
    NeighborsFull,
    /// The interface has no address, until one is leased: the address
    /// version ([`Core::address_version`]) moves then.
    NoAddress,
}

/// A datagram for a lending socket the core did not copy: its socket's
/// flow and where its data lies in the Ethernet payload of the frame, whose
/// buffer its holder lends.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LendTo {
    pub flow: u8,
    pub remote: Remote,
    pub data: core::ops::Range<usize>,
}

/// How a socket takes what it receives.
#[derive(Clone, Copy)]
enum Mode {
    /// Copied into blocks.
    Copy,
    /// Lent in the port's buffers.
    Lend,
    /// Nothing: it only sends.
    SendOnly,
}

/// A refused datagram with its block.
#[derive(Debug)]
pub struct SendRefused<'m, B, const SIZE: usize> {
    pub reason: SendRefusal,
    pub block: Block<'m, B, SIZE>,
}

/// What the core dropped, for a sink that counts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetEvent {
    /// A frame that is not what it claims.
    Malformed,
    /// An IPv4 or UDP checksum failed.
    Checksum,
    /// No socket holds the port.
    NoSocket,
    /// The socket's receive queue is full.
    ReceiveFull,
    /// No block for a received datagram.
    NoBlock,
    /// A socket's group is at its limit, so a received datagram was dropped.
    GroupLimit,
    /// A received datagram longer than a block.
    TooLong,
    /// A fragment, which this core does not reassemble.
    Fragment,
    /// A source address no host may have, RFC 1122, 3.2.1.3.
    BadSource,
    /// The replies owed are at their bound; the peer asks again.
    ReplyDropped,
    /// A datagram for a send-only socket's port, dropped before any block.
    SendOnly,
    /// A lending socket's datagram got no loan, for this reason: its buffer
    /// went back to the port.
    NotLent(NotLent),
    /// A lending socket's datagram the device had copied by pressure, not
    /// by its form: a lending socket takes no such copy.
    PressureCopy,
    /// A lending socket's lane had no room for a lent datagram: its buffer
    /// went back to the port.
    LentLaneFull,
    /// A server's DHCP message the client did not take.
    Dhcp(DhcpDrop),
    /// An ICMP message that got no reply.
    Icmp(IcmpDrop),
}

/// End every datagram that waits for `address` as `outcome`, its block home.
fn end_waiting<
    'm,
    B,
    const SIZE: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const BLOCKS: usize,
>(
    sockets: &mut [Socket<'m, B, SIZE, CREDITS, RECEIVED>],
    address: [u8; 4],
    outcome: Outcome,
    pool: &mut Pool<'m, B, SIZE, BLOCKS>,
) {
    for socket in sockets {
        let _released = socket.places.end_through(Some(address), outcome, pool);
    }
}

/// Whether a datagram or an echo reply goes through `address`: what data
/// protects in the neighbor table.
fn data_needs<'m, B, const SIZE: usize, const CREDITS: usize, const RECEIVED: usize>(
    sockets: &[Socket<'m, B, SIZE, CREDITS, RECEIVED>],
    echoes: &Echoes<'m, B, SIZE>,
    address: [u8; 4],
) -> bool {
    sockets.iter().any(|socket| socket.needs(address)) || echoes.needs(address)
}

/// The neighbor the DHCP client's owed unicast goes through.
fn dhcp_hop(dhcp: Option<&Client>, ipv4: Option<Ipv4>) -> Option<[u8; 4]> {
    let client = dhcp?;
    client.owed()?;
    match client.destination() {
        Destination::Broadcast => None,
        Destination::Server(server) => ipv4?.next_hop(server),
    }
}

/// What a socket's sends meet: the interface's address and how it is held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Configuration {
    /// The address in use, if any.
    pub ipv4: Option<Ipv4>,
    /// Moves at every change of `ipv4`.
    pub version: u32,
    /// The lease that holds the address, with a DHCP client.
    pub lease: Option<Lease>,
    /// The DHCP client's state; `None` for an address given by its owner.
    pub dhcp: Option<DhcpState>,
    /// The port's link as the core last observed it: an application that
    /// waits for an address over an up link, or for that link's loss, reads
    /// both from one publication.
    pub link: LinkSnapshot,
}

#[derive(Debug)]
struct Socket<'m, B, const SIZE: usize, const CREDITS: usize, const RECEIVED: usize> {
    serial: u64,
    /// The bound port; a socket being closed keeps its slot, not its port.
    port: Option<u16>,
    open: bool,
    /// It only sends: a datagram for its port is dropped before any block.
    send_only: bool,
    /// Its datagrams are lent in the port's own buffers, not copied.
    lends: bool,
    dscp: Dscp,
    /// What its received data counts toward.
    group: Group<B>,
    /// Its commands' places, the close's among them: held from the
    /// socket's creation, so a close always has a place.
    places: Places<'m, B, SIZE, CREDITS>,
    received: heapless::Deque<Datagram<'m, B, SIZE>, RECEIVED>,
}

impl<'m, B, const SIZE: usize, const CREDITS: usize, const RECEIVED: usize>
    Socket<'m, B, SIZE, CREDITS, RECEIVED>
{
    const fn new() -> Self {
        Self {
            serial: 0,
            port: None,
            open: false,
            send_only: false,
            lends: false,
            dscp: BEST_EFFORT,
            group: Group::shared(),
            places: Places::new(),
            received: heapless::Deque::new(),
        }
    }

    fn in_use(&self) -> bool {
        self.open || !self.places.idle()
    }

    /// Whether running work here goes through `address`.
    fn needs(&self, address: [u8; 4]) -> bool {
        self.places
            .running()
            .any(|(_, place)| place.run().hop() == Some(address))
    }
}

/// A free place a datagram may run in, and the neighbor it goes through.
#[derive(Clone, Copy)]
struct Admitted {
    number: u8,
    hop: Option<[u8; 4]>,
}

/// A reply owed to an asker.
#[derive(Clone, Copy, Debug)]
struct Reply {
    serial: u64,
    hardware: [u8; 6],
    address: [u8; 4],
}

/// The owner of the network state.
#[derive(Debug)]
pub struct Core<
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
> {
    interface: Interface,
    neighbors: Neighbors<NEIGHBORS>,
    /// Replies owed, oldest first. Bounded by the table: an asker past it
    /// asks again.
    replies: heapless::Vec<Reply, NEIGHBORS>,
    sockets: [Socket<'m, B, SIZE, CREDITS, RECEIVED>; SOCKETS],
    /// Where the next look for a datagram starts, so sockets take turns: every
    /// end at publication, a commit or a reject, moves it past its socket.
    cursor: usize,
    identification: u16,
    /// The last serial given: 2^64 of them outlast any device.
    serial: u64,
    /// Moves whenever a neighbor entry may have become free for a new send:
    /// see [`Self::neighbors_changed`].
    neighbor_changes: u32,
    /// The address in use.
    ipv4: Option<Ipv4>,
    /// Moves at every change of `ipv4`.
    version: u32,
    /// Moves at every change of [`Self::configuration`].
    configuration_changes: u32,
    /// The DHCP client, for an interface whose address it leases.
    dhcp: Option<Client>,
    echoes: Echoes<'m, B, SIZE>,
    /// The port's link, as last observed.
    link: LinkSnapshot,
    brand: Tag<B>,
}

impl<
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
> Core<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS>
{
    const FITS: () = {
        if SOCKETS > 255 || CREDITS >= 255 || NEIGHBORS > 255 {
            panic!("sockets, places and neighbors are numbered in one octet");
        }
        if SIZE < ARP_FRAME {
            panic!("a block holds at least an ARP frame");
        }
    };

    /// The core of `brand`. An interface whose address DHCP leases has none
    /// until a server grants one, and its client starts at the first link up.
    pub fn new(brand: CoreBrand<B>, interface: Interface) -> Self {
        const { Self::FITS };
        let (ipv4, dhcp) = match interface.addressing() {
            Addressing::Static(ipv4) => (Some(ipv4), None),
            Addressing::Dhcp { seed } => (
                None,
                Some(Client::new(interface.hardware(), interface.mtu(), seed)),
            ),
        };
        Self {
            interface,
            neighbors: Neighbors::new(),
            replies: heapless::Vec::new(),
            sockets: [const { Socket::new() }; SOCKETS],
            cursor: 0,
            identification: 0,
            serial: brand.serial,
            neighbor_changes: 0,
            ipv4,
            version: 0,
            configuration_changes: 0,
            dhcp,
            echoes: Echoes::new(),
            link: LinkSnapshot::DOWN,
            brand: brand.brand,
        }
    }

    /// The interface's address now, how it is held, and its version.
    pub fn configuration(&self) -> Configuration {
        Configuration {
            ipv4: self.ipv4,
            version: self.version,
            lease: self.dhcp.as_ref().and_then(Client::lease).copied(),
            dhcp: self.dhcp.as_ref().map(Client::state),
            link: self.link,
        }
    }

    /// A wrapping count that moves whenever [`Self::configuration`] may have
    /// changed: an address, a lease's times, the client's state, the link.
    pub const fn configuration_changes(&self) -> u32 {
        self.configuration_changes
    }

    /// A wrapping count that moves whenever the interface's address, prefix
    /// or gateway changes, or the address goes: work accepted under an
    /// earlier one ends [`Outcome::Readdressed`].
    pub const fn address_version(&self) -> u32 {
        self.version
    }

    fn configured(&mut self) {
        self.configuration_changes = self.configuration_changes.wrapping_add(1);
    }

    /// Take `ipv4` as the interface's address. Another than the one in use
    /// moves the version: every running datagram and echo reply ends
    /// [`Outcome::Readdressed`] with its block home, every neighbor and owed
    /// ARP reply is forgotten, as they were for the old network, and sockets
    /// stay open: they hold a port on the interface, not an address.
    fn readdress<const BLOCKS: usize>(
        &mut self,
        ipv4: Option<Ipv4>,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
        events: &mut impl EventSink<NetEvent>,
    ) {
        if self.ipv4 == ipv4 {
            return;
        }
        self.ipv4 = ipv4;
        self.version = self.version.wrapping_add(1);
        for socket in &mut self.sockets {
            let _released = socket.places.end_all(Outcome::Readdressed, pool);
        }
        self.echoes.take_all(|owed| {
            pool.give(owed.block);
            report(events, || NetEvent::Icmp(IcmpDrop::Readdressed));
        });
        self.neighbors.forget();
        self.replies.clear();
        self.neighbors_moved();
    }

    /// The port's link at `now`. A link that comes up anew, at another
    /// epoch, starts the DHCP client's next exchange; one that goes down
    /// stops its messages until it is up.
    pub fn link(&mut self, now: Instant, link: LinkSnapshot) {
        if link == self.link {
            return;
        }
        self.link = link;
        let mut serial = self.serial;
        if let Some(client) = &mut self.dhcp {
            match link.state {
                LinkState::Up => client.link_up(now, link.epoch, &mut serial),
                LinkState::Down => client.link_down(),
            }
        }
        self.serial = serial;
        self.configured();
    }

    /// The address a send may use at `now`: none past its lease's end, even
    /// before a tick has taken it away, RFC 2131, 4.4.5.
    fn usable(&self, now: Instant) -> Option<Ipv4> {
        self.ipv4
            .filter(|_| !self.dhcp.as_ref().is_some_and(|client| client.expired(now)))
    }

    /// The address a frame goes out from: 0 before the interface has one.
    fn source(&self) -> [u8; 4] {
        self.ipv4.map_or(UNSPECIFIED, |ipv4| ipv4.address())
    }

    /// The limited broadcast address, or the network's own.
    fn is_broadcast(&self, address: [u8; 4]) -> bool {
        address == BROADCAST
            || self
                .ipv4
                .is_some_and(|ipv4| ipv4.is_directed_broadcast(address))
    }

    /// Whether data may protect `hop`'s neighbor entry: with a DHCP client,
    /// data and echo replies protect at most all entries but one, so a
    /// renewal always finds one, RFC 2131, 4.4.5's unicast. What counts is
    /// the protection a send acquires, not the entry's presence: an entry
    /// data protects already stays its own, cached or expired, and one that
    /// is merely cached costs a place like any other.
    fn data_may_resolve(&self, hop: [u8; 4]) -> bool {
        let sockets = &self.sockets;
        let echoes = &self.echoes;
        if self.dhcp.is_none() || data_needs(sockets, echoes, hop) {
            return true;
        }
        self.neighbors
            .count(|address| data_needs(sockets, echoes, address))
            < NEIGHBORS.saturating_sub(1)
    }

    /// Resolve `hop` for data, within the control reserve.
    fn resolve_for_data(&mut self, now: Instant, hop: [u8; 4]) -> Lookup {
        if !self.data_may_resolve(hop) {
            return Lookup::Full;
        }
        let serial = self.next_serial();
        let (sockets, echoes) = (&self.sockets, &self.echoes);
        let dhcp = dhcp_hop(self.dhcp.as_ref(), self.ipv4);
        self.neighbors.resolve(now, hop, serial, |address| {
            data_needs(sockets, echoes, address) || dhcp == Some(address)
        })
    }

    fn next_serial(&mut self) -> u64 {
        self.serial = self.serial.wrapping_add(1);
        self.serial
    }

    /// Open a socket on `port`, whose datagrams carry `dscp`.
    ///
    /// # Errors
    /// [`OpenRefused`]: every socket is in use, or the port is held.
    pub fn open(&mut self, port: u16, dscp: Dscp) -> Result<SocketId<B>, OpenRefused> {
        self.open_in(port, dscp, Group::shared())
    }

    /// Open a socket on `port`, whose received data counts toward `group`.
    ///
    /// # Errors
    /// [`OpenRefused`].
    pub fn open_in(
        &mut self,
        port: u16,
        dscp: Dscp,
        group: Group<B>,
    ) -> Result<SocketId<B>, OpenRefused> {
        self.open_socket(port, dscp, group, Mode::Copy, |_| Ok(()))
    }

    /// Open a socket on `port` that receives in the port's own buffers, with
    /// a credit of `credit` loans in `pool`'s account: each datagram in a
    /// lendable buffer is lent to it ([`Self::input_view`] says so) rather
    /// than copied, one the device copied for its form goes into a block of
    /// `group`, and one it copied by pressure is dropped. Its close takes the
    /// credit back; what it holds counts until it returns.
    ///
    /// # Errors
    /// [`OpenRefused`]; [`OpenRefused::Loans`] when the account refuses the
    /// credit. A refusal changes nothing.
    pub fn open_lending<const BLOCKS: usize>(
        &mut self,
        port: u16,
        dscp: Dscp,
        group: Group<B>,
        credit: usize,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) -> Result<SocketId<B>, OpenRefused> {
        self.open_socket(port, dscp, group, Mode::Lend, |index| {
            pool.loans()
                .grant(index, credit)
                .map_err(OpenRefused::Loans)
        })
    }

    /// Open a send-only socket on `port`, whose work a local producer
    /// declares ([`Self::declare`]): a datagram for its port is dropped
    /// before any block is taken.
    ///
    /// # Errors
    /// [`OpenRefused`].
    pub fn open_local(&mut self, port: u16, dscp: Dscp) -> Result<SocketId<B>, OpenRefused> {
        self.open_socket(port, dscp, Group::shared(), Mode::SendOnly, |_| Ok(()))
    }

    fn open_socket(
        &mut self,
        port: u16,
        dscp: Dscp,
        group: Group<B>,
        mode: Mode,
        admit: impl FnOnce(usize) -> Result<(), OpenRefused>,
    ) -> Result<SocketId<B>, OpenRefused> {
        if self.sockets.iter().any(|socket| socket.port == Some(port)) {
            return Err(OpenRefused::PortInUse);
        }
        // A refusal below skips a serial, which only has to be new.
        let serial = self.next_serial();
        let (index, socket) = self
            .sockets
            .iter_mut()
            .enumerate()
            .find(|(_, socket)| !socket.in_use())
            .ok_or(OpenRefused::NoSocket)?;
        let index_byte = u8::try_from(index).map_err(|_| OpenRefused::NoSocket)?;
        // The last refusal: nothing is written before it.
        admit(index)?;
        let index = index_byte;
        socket.serial = serial;
        socket.port = Some(port);
        socket.open = true;
        socket.send_only = matches!(mode, Mode::SendOnly);
        socket.lends = matches!(mode, Mode::Lend);
        socket.dscp = dscp;
        socket.group = group;
        Ok(SocketId {
            index,
            serial,
            brand: PhantomData,
        })
    }

    fn socket_mut(
        &mut self,
        index: u8,
        serial: u64,
    ) -> Option<&mut Socket<'m, B, SIZE, CREDITS, RECEIVED>> {
        self.sockets
            .get_mut(usize::from(index))
            .filter(|socket| socket.serial == serial)
    }

    /// Commit `block` as a datagram to `remote`. This is the acceptance
    /// point: a refusal returns the block and moved nothing, and after it
    /// the block is the core's until the command's outcome.
    ///
    /// # Errors
    /// [`SendRefused`], with the block.
    pub fn send(
        &mut self,
        now: Instant,
        socket: &SocketId<B>,
        remote: Remote,
        block: Block<'m, B, SIZE>,
    ) -> Result<CommandId<B>, SendRefused<'m, B, SIZE>> {
        let length = block.octets().len();
        match self.admit(now, socket, remote, length) {
            Ok(admitted) => Ok(self.place(socket, admitted, |hop| {
                Run::Sending(Sending {
                    remote,
                    block,
                    since: now,
                    hop,
                })
            })),
            Err(reason) => Err(SendRefused { reason, block }),
        }
    }

    /// Declare `length` octets to `remote` that a local producer fills when
    /// the port gives them room. This is the acceptance point: it takes a
    /// command place as [`Self::send`] does, with the same refusals, and no
    /// block. A refusal moved nothing; after it the work has one outcome.
    ///
    /// # Errors
    /// [`SendRefusal`].
    pub fn declare(
        &mut self,
        now: Instant,
        socket: &SocketId<B>,
        remote: Remote,
        length: usize,
    ) -> Result<CommandId<B>, SendRefusal> {
        let admitted = self.admit(now, socket, remote, length)?;
        Ok(self.place(socket, admitted, |hop| {
            Run::Declared(Declared {
                remote,
                length,
                hop,
            })
        }))
    }

    /// Admit a datagram of `length` octets on `socket` to `remote`: its free
    /// place and the neighbor it goes through, asked for if unknown.
    fn admit(
        &mut self,
        now: Instant,
        socket: &SocketId<B>,
        remote: Remote,
        length: usize,
    ) -> Result<Admitted, SendRefusal> {
        let datagram = IPV4_HEADER_LEN
            .saturating_add(UDP_HEADER_LEN)
            .saturating_add(length);
        if datagram > usize::from(self.interface.mtu()) {
            return Err(SendRefusal::TooLong);
        }
        let open = self
            .socket_mut(socket.index, socket.serial)
            .filter(|open| open.open)
            .ok_or(SendRefusal::Stale)?;
        let number = open.places.vacant().ok_or(SendRefusal::NoPlace)?;
        let ipv4 = self.usable(now).ok_or(SendRefusal::NoAddress)?;
        let hop = if self.is_broadcast(remote.address) {
            None
        } else {
            let hop = ipv4.next_hop(remote.address).ok_or(SendRefusal::NoRoute)?;
            if self.resolve_for_data(now, hop) == Lookup::Full {
                return Err(SendRefusal::NeighborsFull);
            }
            Some(hop)
        };
        Ok(Admitted { number, hop })
    }

    /// Run what `run` makes in the place `admitted` found free.
    fn place(
        &mut self,
        socket: &SocketId<B>,
        admitted: Admitted,
        run: impl FnOnce(Option<[u8; 4]>) -> Run<'m, B, SIZE>,
    ) -> CommandId<B> {
        let serial = self.next_serial();
        // `admit` found this place free in this open socket, and nothing ran
        // since.
        if let Some(open) = self.socket_mut(socket.index, socket.serial) {
            let _started = open
                .places
                .start(admitted.number, serial, run(admitted.hop));
        }
        CommandId {
            socket: socket.index,
            place: admitted.number,
            serial,
            brand: PhantomData,
        }
    }

    /// Something that may have freed a neighbor entry for a new send.
    fn neighbors_moved(&mut self) {
        self.neighbor_changes = self.neighbor_changes.wrapping_add(1);
    }

    /// A wrapping count that moves whenever a neighbor entry may have become
    /// free for a new send: an ARP answer, an entry asked for again past its
    /// life, a resolution given up or refused, and every end of running work
    /// that held a neighbor (a commit, a reject, a decline, a withdrawal, a
    /// close, an eviction, `Unreachable`, an echo reply's end, an address
    /// change). A waiter for
    /// [`SendRefusal::NeighborsFull`] keeps the value it saw; reading it
    /// resets nothing.
    pub const fn neighbors_changed(&self) -> u32 {
        self.neighbor_changes
    }

    /// End declared `command` [`Outcome::Declined`], before any port saw it:
    /// its source moved on. Whether it was still declared.
    pub fn withdraw(&mut self, command: &CommandId<B>) -> bool {
        let Some(places) = self
            .sockets
            .get_mut(usize::from(command.socket))
            .map(|socket| &mut socket.places)
            .filter(|places| {
                places
                    .of(command.place, command.serial)
                    .is_some_and(|place| matches!(place.run(), Run::Declared(_)))
            })
        else {
            return false;
        };
        let protected = places.end_declared(command.place, Outcome::Declined);
        if protected {
            self.neighbors_moved();
        }
        self.forget_unneeded();
        true
    }

    fn forget_unneeded(&mut self) {
        let (sockets, echoes) = (&self.sockets, &self.echoes);
        let dhcp = dhcp_hop(self.dhcp.as_ref(), self.ipv4);
        self.neighbors.forget_unneeded(|address| {
            data_needs(sockets, echoes, address) || dhcp == Some(address)
        });
    }

    /// The command's result, once it ended: its place frees.
    pub fn take_result(&mut self, command: CommandId<B>) -> CommandState<B> {
        let taken = self
            .sockets
            .get_mut(usize::from(command.socket))
            .and_then(|socket| socket.places.take(command.place, command.serial));
        match taken {
            None => CommandState::Gone,
            Some(Some(outcome)) => CommandState::Finished(outcome),
            Some(None) => CommandState::Running(command),
        }
    }

    /// The next of `ends`' socket's commands to end since this last told it,
    /// oldest end first, the close's among them: each end is told once, as
    /// the work's place and generation. Its result stays in its place for
    /// whoever holds its [`CommandId`] ([`CommandId::work`] is the same
    /// value), so a told end is no right to anything. A socket reopened in
    /// the same place is another, and the ends of an earlier one tell it
    /// nothing. `count` hears each end told ([`Count::told`]).
    pub fn next_end<C: Count + ?Sized>(&mut self, ends: &Ends<B>, count: &C) -> Option<WorkId> {
        let (place, serial) = self.socket_mut(ends.index, ends.serial)?.places.tell()?;
        counted(count, Count::told);
        Some(WorkId { serial, place })
    }

    /// Every datagram [`Self::evict`] can take back.
    pub fn queued(&self) -> impl Iterator<Item = Queued<B>> + '_ {
        let brand = self.brand;
        (0_u8..)
            .zip(self.sockets.iter())
            .flat_map(move |(flow, socket)| {
                socket
                    .places
                    .running()
                    .filter_map(move |(place, entry)| match entry.run() {
                        Run::Sending(sending) => Some(Queued {
                            flow,
                            group: sending.block.group(),
                            since: sending.since,
                            place,
                            serial: entry.serial(),
                            brand,
                        }),
                        _ => None,
                    })
            })
    }

    /// Take `queued` back: its block returns to `pool`, and its command ends
    /// [`Outcome::Evicted`]. Whether it was still queued.
    pub fn evict<const BLOCKS: usize>(
        &mut self,
        queued: Queued<B>,
        eviction: Eviction,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) -> bool {
        let Some(places) = self
            .sockets
            .get_mut(usize::from(queued.flow))
            .map(|socket| &mut socket.places)
            .filter(|places| {
                places
                    .of(queued.place, queued.serial)
                    .is_some_and(|place| matches!(place.run(), Run::Sending(_)))
            })
        else {
            return false;
        };
        if places.end(queued.place, Outcome::Evicted(eviction), pool) {
            self.neighbors_moved();
        }
        self.forget_unneeded();
        true
    }

    /// Nobody will read this command's result. A finished command's place
    /// frees now; a running command keeps it until its end, so that end
    /// never reaches a later command.
    pub fn drop_observer(&mut self, command: CommandId<B>) {
        if let Some(socket) = self.sockets.get_mut(usize::from(command.socket)) {
            socket.places.unobserve(command.place, command.serial);
        }
    }

    /// The next received datagram of `socket`. The block is the caller's;
    /// it goes back to the pool by [`Pool::give`].
    ///
    /// # Errors
    /// [`StaleSocket`]: a final refusal.
    pub fn receive(
        &mut self,
        socket: &SocketId<B>,
    ) -> Result<Option<Datagram<'m, B, SIZE>>, StaleSocket> {
        let open = self
            .socket_mut(socket.index, socket.serial)
            .ok_or(StaleSocket)?;
        Ok(open.received.pop_front())
    }

    /// Close `socket`, from a place held for this since its creation: a full
    /// command queue never stands in its way. Received datagrams go back to
    /// `pool` and waiting datagrams end `Closed`.
    ///
    /// # Errors
    /// [`StaleSocket`]: a final refusal.
    pub fn close<const BLOCKS: usize>(
        &mut self,
        socket: SocketId<B>,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) -> Result<CommandId<B>, StaleSocket> {
        let serial = self.next_serial();
        let open = self
            .socket_mut(socket.index, socket.serial)
            .filter(|open| open.open)
            .ok_or(StaleSocket)?;
        open.open = false;
        open.port = None;
        if core::mem::replace(&mut open.lends, false) {
            pool.loans().revoke(usize::from(socket.index));
        }
        while let Some(datagram) = open.received.pop_front() {
            pool.give(datagram.block);
        }
        let released = open.places.end_all(Outcome::Closed, pool);
        open.places.close(serial);
        if released {
            self.neighbors_moved();
        }
        self.forget_unneeded();
        let place = u8::try_from(CREDITS).map_err(|_| StaleSocket)?;
        Ok(CommandId {
            socket: socket.index,
            place,
            serial,
            brand: PhantomData,
        })
    }

    /// Take one received frame. Its octets are read here and copied only
    /// when delivered, so the port's buffer returns with the call.
    pub fn input<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        frame: &[u8],
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
        events: &mut impl EventSink<NetEvent>,
    ) {
        let Ok((header, payload)) = EthernetHeader::parse(frame) else {
            report(events, || NetEvent::Malformed);
            return;
        };
        let view = EthernetFrameView {
            destination: header.destination,
            source: header.source,
            ether_type: header.ether_type.value(),
            payload,
        };
        // A frame given as octets is no port's buffer: it is copied.
        let _copied = self.input_view(now, view, Holding::Copied(CopyReason::Format), pool, events);
    }

    /// As [`Self::input`], for a frame whose fields a port read apart from its
    /// payload, held as `holding` says. A datagram for a lending socket in a
    /// lendable buffer is not copied: the answer names its socket and where
    /// its data lies in the frame's payload, for its holder to lend.
    pub fn input_view<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        frame: EthernetFrameView<'_>,
        holding: Holding,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
        events: &mut impl EventSink<NetEvent>,
    ) -> Option<LendTo> {
        if frame.destination != self.interface.hardware() && frame.destination != BROADCAST_HARDWARE
        {
            return None;
        }
        let ether_type = EtherType::new(frame.ether_type);
        if ether_type == EtherType::ARP {
            self.input_arp(now, frame.payload, events);
            None
        } else if ether_type == EtherType::IPV4 {
            self.input_ipv4(now, frame.payload, holding, pool, events)
        } else {
            None
        }
    }

    fn input_arp(&mut self, now: Instant, payload: &[u8], events: &mut impl EventSink<NetEvent>) {
        let packet = match Arp::parse(payload) {
            Ok(Some(packet)) => packet,
            Ok(None) => return,
            Err(_) => {
                report(events, || NetEvent::Malformed);
                return;
            }
        };
        let for_us = self
            .ipv4
            .is_some_and(|ipv4| packet.target_protocol == ipv4.address());
        if self
            .neighbors
            .learn(now, packet.sender_protocol, packet.sender_hardware, for_us)
        {
            self.neighbors_moved();
        }
        if for_us && packet.operation == Operation::Request {
            let reply = Reply {
                serial: self.next_serial(),
                hardware: packet.sender_hardware,
                address: packet.sender_protocol,
            };
            if self.replies.push(reply).is_err() {
                report(events, || NetEvent::ReplyDropped);
            }
        }
    }

    fn input_ipv4<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        payload: &[u8],
        holding: Holding,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
        events: &mut impl EventSink<NetEvent>,
    ) -> Option<LendTo> {
        let Ok(packet) = Ipv4Header::parse(payload) else {
            report(events, || NetEvent::Malformed);
            return None;
        };
        let [s0, s1, s2, s3, d0, d1, d2, d3] = packet.addresses();
        let (source, destination) = ([s0, s1, s2, s3], [d0, d1, d2, d3]);
        let ours = self.ipv4.is_some_and(|ipv4| ipv4.address() == destination)
            || self.is_broadcast(destination);
        // RFC 2131, 2 and 4.1: before its address, a client takes what a
        // server unicasts to the address it offers; the client's own checks
        // decide.
        let leasing = self.dhcp.is_some() && packet.protocol() == PROTOCOL_UDP;
        if !ours && !leasing {
            return None;
        }
        let intact = payload
            .first()
            .map(|first| usize::from(Ipv4VersionIhl::from_bytes([*first]).ihl()).saturating_mul(4))
            .and_then(|header| payload.get(..header))
            .is_some_and(|header| Checksum::new().with(header).verifies());
        if !intact {
            report(events, || NetEvent::Checksum);
            return None;
        }
        if self.is_broadcast(source) {
            report(events, || NetEvent::BadSource);
            return None;
        }
        let protocol = packet.protocol();
        if protocol != PROTOCOL_UDP && protocol != PROTOCOL_ICMP {
            return None;
        }
        if packet.is_fragment() {
            report(events, || NetEvent::Fragment);
            return None;
        }
        if protocol == PROTOCOL_ICMP {
            self.input_icmp(now, source, destination, packet.payload(), pool, events);
            return None;
        }
        let datagram = packet.payload();
        let Ok((header, data)) = UdpHeader::parse(datagram) else {
            report(events, || NetEvent::Malformed);
            return None;
        };
        let length = usize::from(header.length);
        // Zero says that the sender computed none, RFC 768.
        if header.checksum != 0
            && !datagram.get(..length).is_some_and(|whole| {
                Checksum::new()
                    .with(&ipv4_pseudo_header(
                        source,
                        destination,
                        PROTOCOL_UDP,
                        header.length,
                    ))
                    .with(whole)
                    .verifies()
            })
        {
            report(events, || NetEvent::Checksum);
            return None;
        }
        let port = header.ports.destination;
        if self.dhcp.is_some() && port == dhcp::CLIENT_PORT {
            if header.ports.source == dhcp::SERVER_PORT {
                self.input_dhcp(now, source, data, pool, events);
            } else {
                report(events, || NetEvent::NoSocket);
            }
            return None;
        }
        if !ours {
            return None;
        }
        let found = (0_u8..)
            .zip(self.sockets.iter_mut())
            .find(|(_, socket)| socket.open && socket.port == Some(port));
        let Some((index, socket)) = found else {
            report(events, || NetEvent::NoSocket);
            return None;
        };
        if socket.send_only {
            report(events, || NetEvent::SendOnly);
            return None;
        }
        if socket.lends {
            match holding {
                Holding::Lendable => {
                    // `data` lies inside `payload`, which the parse sliced.
                    let at = data.as_ptr().addr().checked_sub(payload.as_ptr().addr());
                    let Some((at, end)) = at.and_then(|at| Some((at, at.checked_add(data.len())?)))
                    else {
                        report(events, || NetEvent::Malformed);
                        return None;
                    };
                    return Some(LendTo {
                        flow: index,
                        remote: Remote {
                            address: source,
                            port: header.ports.source,
                        },
                        data: at..end,
                    });
                }
                Holding::Copied(CopyReason::Pressure) => {
                    report(events, || NetEvent::PressureCopy);
                    return None;
                }
                // The device's copy by the frame's form: into a block of the
                // socket's group, as a copying socket takes it.
                Holding::Copied(CopyReason::Format) => {}
            }
        }
        if socket.received.is_full() {
            report(events, || NetEvent::ReceiveFull);
            return None;
        }
        let Some(mut block) = pool.take_as(socket.group) else {
            report(events, || {
                if pool.free() == 0 {
                    NetEvent::NoBlock
                } else {
                    NetEvent::GroupLimit
                }
            });
            return None;
        };
        if block.fill(data).is_err() {
            pool.give(block);
            report(events, || NetEvent::TooLong);
            return None;
        }
        let datagram = Datagram {
            remote: Remote {
                address: source,
                port: header.ports.source,
            },
            block,
        };
        if let Err(datagram) = socket.received.push_back(datagram) {
            pool.give(datagram.block);
            report(events, || NetEvent::ReceiveFull);
        }
        None
    }

    /// A server's message to the client port.
    fn input_dhcp<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        source: [u8; 4],
        message: &[u8],
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
        events: &mut impl EventSink<NetEvent>,
    ) {
        let Some(client) = &mut self.dhcp else {
            return;
        };
        let mut serial = self.serial;
        let taken = client.input(now, source, message, &mut serial);
        self.serial = serial;
        match taken {
            Ok(change) => {
                self.configured();
                self.apply(change, pool, events);
            }
            Err(drop) => report(events, || NetEvent::Dhcp(drop)),
        }
    }

    fn apply<const BLOCKS: usize>(
        &mut self,
        change: Option<Change>,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
        events: &mut impl EventSink<NetEvent>,
    ) {
        match change {
            Some(Change::Leased(ipv4)) => self.readdress(Some(ipv4), pool, events),
            Some(Change::Lost) => self.readdress(None, pool, events),
            None => {}
        }
        self.forget_unneeded();
    }

    /// An ICMP message to `destination`: an echo for this interface's
    /// address owes a reply, RFC 792 and RFC 1122, 3.2.2.6.
    fn input_icmp<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        source: [u8; 4],
        destination: [u8; 4],
        message: &[u8],
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
        events: &mut impl EventSink<NetEvent>,
    ) {
        if let Err(drop) = self.owe_echo(now, source, destination, message, pool) {
            report(events, || NetEvent::Icmp(drop));
        }
    }

    fn owe_echo<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        source: [u8; 4],
        destination: [u8; 4],
        message: &[u8],
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) -> Result<(), IcmpDrop> {
        let echo = Echo::parse(message).map_err(|_| IcmpDrop::Malformed)?;
        if !Checksum::new().with(message).verifies() {
            return Err(IcmpDrop::Checksum);
        }
        if echo.kind != icmp::MessageType::Echo {
            return Err(IcmpDrop::Other);
        }
        if echo.code != 0 {
            return Err(IcmpDrop::Malformed);
        }
        let ipv4 = self
            .usable(now)
            .filter(|ipv4| ipv4.address() == destination)
            .ok_or(IcmpDrop::Broadcast)?;
        if !is_unicast(source) {
            return Err(IcmpDrop::NoRoute);
        }
        if message.len() > SIZE {
            return Err(IcmpDrop::TooLong);
        }
        self.echoes.admits(now)?;
        let hop = ipv4.next_hop(source).ok_or(IcmpDrop::NoRoute)?;
        // The pool's last free block stays for data.
        if pool.free() <= 1 {
            return Err(IcmpDrop::NoBlock);
        }
        if self.resolve_for_data(now, hop) == Lookup::Full {
            return Err(IcmpDrop::NeighborsFull);
        }
        let mut block = pool.take().ok_or(IcmpDrop::NoBlock)?;
        if block.fill(message).is_err() {
            pool.give(block);
            self.forget_unneeded();
            return Err(IcmpDrop::TooLong);
        }
        let owed = Owed {
            serial: self.next_serial(),
            block,
            to: source,
            hop,
        };
        if let Err(owed) = self.echoes.push(owed) {
            pool.give(owed.block);
            self.forget_unneeded();
            return Err(IcmpDrop::Busy);
        }
        Ok(())
    }

    /// Advance to `now`: ask again for neighbors that expired under a waiting
    /// datagram, and end what waited for one that never answered. This comes
    /// first at every time [`Self::poll_deadline`] names, and before
    /// [`Self::demands`] at a later `now`: an address past its life is no work
    /// until this has asked for it again.
    ///
    /// The DHCP client's times come here too: a lease past its end goes, and
    /// the work that waited on its address ends [`Outcome::Readdressed`].
    pub fn tick<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
        events: &mut impl EventSink<NetEvent>,
    ) {
        if let Some(client) = &mut self.dhcp {
            let mut serial = self.serial;
            let before = client.state();
            let change = client.tick(now, &mut serial);
            let moved = change.is_some() || client.state() != before;
            self.serial = serial;
            if moved {
                self.configured();
            }
            self.apply(change, pool, events);
        }
        let mut serial = self.serial;
        let mut changes = self.neighbor_changes;
        let dhcp = dhcp_hop(self.dhcp.as_ref(), self.ipv4);
        let hops = self
            .sockets
            .iter()
            .flat_map(|socket| socket.places.running())
            .filter_map(|(_, place)| place.run().hop())
            .chain(self.echoes.iter().map(|owed| owed.hop))
            .chain(dhcp);
        for hop in hops {
            let next = serial.wrapping_add(1);
            if self.neighbors.reask(now, hop, next) {
                serial = next;
                changes = changes.wrapping_add(1);
            }
        }
        // A renewal's neighbor that is neither known nor asked for is asked
        // for: the reserve keeps an entry for it.
        if let Some(hop) = dhcp
            && !self.neighbors.holds(hop)
        {
            serial = serial.wrapping_add(1);
            let (sockets, echoes) = (&self.sockets, &self.echoes);
            let _lookup = self.neighbors.resolve(now, hop, serial, |address| {
                data_needs(sockets, echoes, address) || address == hop
            });
        }
        self.serial = serial;
        let sockets = &mut self.sockets;
        let echoes = &mut self.echoes;
        let client = &mut self.dhcp;
        self.neighbors.tick(now, |address| {
            end_waiting(sockets, address, Outcome::Unreachable, pool);
            echoes.take_through(address, |owed| {
                pool.give(owed.block);
                report(events, || NetEvent::Icmp(IcmpDrop::Unreachable));
            });
            // The server's neighbor never answered: the renewal never went,
            // so it counts as refused, and its next try comes at its time.
            if dhcp == Some(address)
                && let Some(client) = client
                && let Some(owed) = client.owed()
            {
                client.refused(now, owed);
            }
            changes = changes.wrapping_add(1);
        });
        self.neighbor_changes = changes;
    }

    /// When [`Self::tick`] has work: a request's time, the end of the life
    /// of a neighbor a datagram or reply needs, or a DHCP time. Nothing else
    /// makes demand appear.
    pub fn poll_deadline(&self) -> Option<Instant> {
        let (sockets, echoes) = (&self.sockets, &self.echoes);
        let dhcp = dhcp_hop(self.dhcp.as_ref(), self.ipv4);
        let neighbors = self
            .neighbors
            .deadline(|address| data_needs(sockets, echoes, address) || dhcp == Some(address));
        let client = self.dhcp.as_ref().and_then(Client::deadline);
        [neighbors, client].into_iter().flatten().min()
    }

    /// Every piece of work ready now, with no frame built: control first,
    /// then datagrams from the socket whose turn it is. A scheduler chooses
    /// among these; a direct interface takes [`Self::poll_demand`]. `count`
    /// hears each running place the walk of the sockets' running chains
    /// visits ([`Count::walked`]), ready or not, on every pass a caller makes.
    pub fn demands<'s, C: Count + ?Sized>(
        &'s self,
        now: Instant,
        count: &'s C,
    ) -> impl Iterator<Item = Demand<B>> + Clone + 's {
        let brand = self.brand;
        let replies = self.replies.iter().map(move |reply| Demand {
            ready: Ready {
                class: Class::Control,
                length: ARP_FRAME,
                peer: reply.hardware,
                flow: None,
                dscp: BEST_EFFORT,
            },
            work: Work::Reply {
                serial: reply.serial,
            },
            brand,
        });
        let requests = self
            .neighbors
            .owed_requests()
            .map(move |(neighbor, _, serial)| Demand {
                ready: Ready {
                    class: Class::Control,
                    length: ARP_FRAME,
                    peer: BROADCAST_HARDWARE,
                    flow: None,
                    dscp: BEST_EFFORT,
                },
                work: Work::Request { neighbor, serial },
                brand,
            });
        let dhcp = self
            .dhcp
            .as_ref()
            .and_then(|client| {
                let serial = client.owed()?;
                let peer = self.dhcp_peer(now, client)?;
                let length = client.message(now, serial)?.length();
                Some((serial, peer, length))
            })
            .into_iter()
            .map(move |(serial, peer, length)| Demand {
                ready: Ready {
                    class: Class::Control,
                    length: HEADERS.saturating_add(length),
                    peer,
                    flow: None,
                    dscp: BEST_EFFORT,
                },
                work: Work::Dhcp { serial, peer },
                brand,
            });
        let echoes = self.echoes.iter().filter_map(move |owed| {
            let found = self.neighbors.find(now, owed.hop)?;
            let peer = found.hardware;
            Some(Demand {
                ready: Ready {
                    class: Class::Control,
                    length: ECHO_HEADERS.saturating_add(owed.block.octets().len()),
                    peer,
                    flow: None,
                    dscp: BEST_EFFORT,
                },
                work: Work::Echo {
                    serial: owed.serial,
                    peer,
                    found,
                },
                brand,
            })
        });
        let datagrams = (0..SOCKETS)
            .filter_map(|offset| self.cursor.wrapping_add(offset).checked_rem(SOCKETS))
            .filter_map(|index| Some((index, self.sockets.get(index)?)))
            .flat_map(move |(index, socket)| {
                socket.places.running().filter_map(move |(number, place)| {
                    counted(count, Count::walked);
                    self.datagram_demand(now, index, socket, number, place)
                })
            });
        replies
            .chain(requests)
            .chain(dhcp)
            .chain(echoes)
            .chain(datagrams)
    }

    /// The station address the client's owed message goes to.
    fn dhcp_peer(&self, now: Instant, client: &Client) -> Option<[u8; 6]> {
        match client.destination() {
            Destination::Broadcast => Some(BROADCAST_HARDWARE),
            Destination::Server(server) => {
                let hop = self.ipv4?.next_hop(server)?;
                self.neighbors.hardware(now, hop)
            }
        }
    }

    fn datagram_demand(
        &self,
        now: Instant,
        index: usize,
        socket: &Socket<'m, B, SIZE, CREDITS, RECEIVED>,
        number: u8,
        place: &Place<'m, B, SIZE>,
    ) -> Option<Demand<B>> {
        let (hop, length) = match place.run() {
            Run::Sending(sending) => (sending.hop, sending.block.octets().len()),
            Run::Declared(declared) => (declared.hop, declared.length),
            Run::Free | Run::Finished(_) => return None,
        };
        let found = match hop {
            Some(hop) => Some(self.neighbors.find(now, hop)?),
            None => None,
        };
        let peer = found.map_or(BROADCAST_HARDWARE, |found| found.hardware);
        Some(Demand {
            ready: Ready {
                class: Class::Data,
                length: HEADERS.saturating_add(length),
                peer,
                flow: u8::try_from(index).ok(),
                dscp: socket.dscp,
            },
            work: Work::Datagram {
                socket: u8::try_from(index).ok()?,
                place: number,
                serial: place.serial(),
                peer,
                found,
            },
            brand: self.brand,
        })
    }

    /// The next ready work, and the next socket's turn. A direct interface
    /// grants this at once; a port's refusal leaves it ready.
    pub fn poll_demand(&self, now: Instant) -> Option<Demand<B>> {
        self.demands(now, &()).next()
    }

    /// The granted work as a frame, while it still is that work: another
    /// neighbor address, a later reply or a retired command is `None`. Only
    /// [`NetworkOwner`](crate::NetworkOwner) reaches this, so the port's
    /// answer is always committed.
    pub(crate) fn frame(&self, now: Instant, grant: &Grant<B>) -> Option<Frame<'_>> {
        match grant.work {
            Work::Reply { serial } => {
                let reply = self.replies.iter().find(|reply| reply.serial == serial)?;
                self.arp_frame(
                    Operation::Reply,
                    reply.hardware,
                    reply.hardware,
                    reply.address,
                )
            }
            Work::Request { neighbor, serial } => {
                let (_, address, _) = self
                    .neighbors
                    .owed_requests()
                    .find(|owed| (owed.0, owed.2) == (neighbor, serial))?;
                self.arp_frame(Operation::Request, BROADCAST_HARDWARE, [0; 6], address)
            }
            Work::Datagram {
                socket,
                place,
                serial,
                peer,
                found,
            } => self.datagram_frame(now, socket, place, serial, (peer, found)),
            Work::Dhcp { serial, peer } => self.dhcp_frame(now, serial, peer),
            Work::Echo {
                serial,
                peer,
                found,
            } => self.echo_frame(now, serial, peer, &found),
        }
    }

    /// Ethernet and IPv4 headers to `peer`, `destination`, of `protocol`
    /// carrying `length` octets.
    fn ip_headers(
        &self,
        peer: [u8; 6],
        source: [u8; 4],
        destination: [u8; 4],
        protocol: u8,
        length: usize,
        writer: &mut Writer<'_>,
    ) -> Option<()> {
        EthernetHeader {
            destination: peer,
            source: self.interface.hardware(),
            ether_type: EtherType::IPV4,
        }
        .write(writer)
        .ok()?;
        writer
            .write(
                &Ipv4Fields {
                    services: BEST_EFFORT.field(),
                    total_length: u16::try_from(IPV4_HEADER_LEN.saturating_add(length)).ok()?,
                    identification: self.identification,
                    time_to_live: TIME_TO_LIVE,
                    protocol,
                    source,
                    destination,
                }
                .to_bytes(),
            )
            .ok()
    }

    fn dhcp_frame(&self, now: Instant, serial: u64, peer: [u8; 6]) -> Option<Frame<'_>> {
        let client = self.dhcp.as_ref()?;
        if self.dhcp_peer(now, client)? != peer {
            return None;
        }
        let message = client.message(now, serial)?;
        let udp_length = u16::try_from(UDP_HEADER_LEN.saturating_add(message.length())).ok()?;
        let udp = UdpHeader {
            ports: Ports {
                source: dhcp::CLIENT_PORT,
                destination: dhcp::SERVER_PORT,
            },
            length: udp_length,
            checksum: 0,
        };
        let mut headers = [0; HEADERS];
        let mut writer = Writer::new(&mut headers);
        self.ip_headers(
            peer,
            message.source,
            message.destination,
            PROTOCOL_UDP,
            usize::from(udp_length),
            &mut writer,
        )?;
        writer.write(&udp.to_bytes()).ok()?;
        Some(Frame::Dhcp {
            headers,
            udp,
            pseudo: ipv4_pseudo_header(
                message.source,
                message.destination,
                PROTOCOL_UDP,
                udp_length,
            ),
            message,
        })
    }

    fn echo_frame(
        &self,
        now: Instant,
        serial: u64,
        peer: [u8; 6],
        found: &Found,
    ) -> Option<Frame<'_>> {
        let owed = self.echoes.get(serial)?;
        if self.next_hop(now, Some(owed.hop), Some(found))? != peer {
            return None;
        }
        let request = Echo::parse(owed.block.octets()).ok()?;
        let mut headers = [0; ECHO_HEADERS];
        self.ip_headers(
            peer,
            self.usable(now)?.address(),
            owed.to,
            PROTOCOL_ICMP,
            owed.block.octets().len(),
            &mut Writer::new(&mut headers),
        )?;
        Some(Frame::Echo {
            headers,
            reply: request.header(icmp::MessageType::EchoReply),
            data: request.data,
        })
    }

    fn arp_frame(
        &self,
        operation: Operation,
        destination: [u8; 6],
        target_hardware: [u8; 6],
        target_protocol: [u8; 4],
    ) -> Option<Frame<'_>> {
        let mut frame = [0; ARP_FRAME];
        let (head, rest) = frame.split_at_mut_checked(ethernet::HEADER_LEN)?;
        EthernetHeader {
            destination,
            source: self.interface.hardware(),
            ether_type: EtherType::ARP,
        }
        .write(&mut Writer::new(head))
        .ok()?;
        Arp {
            operation,
            sender_hardware: self.interface.hardware(),
            sender_protocol: self.source(),
            target_hardware,
            target_protocol,
        }
        .write(rest)
        .ok()?;
        Some(Frame::Arp(frame))
    }

    /// The station address of `hop`, none for a broadcast: what the demand
    /// `found` while that holds, else a new look at the table.
    fn next_hop(
        &self,
        now: Instant,
        hop: Option<[u8; 4]>,
        found: Option<&Found>,
    ) -> Option<[u8; 6]> {
        let Some(hop) = hop else {
            return Some(BROADCAST_HARDWARE);
        };
        match found {
            Some(found) if self.neighbors.holds_still(found, now) => Some(found.hardware),
            _ => self.neighbors.hardware(now, hop),
        }
    }

    fn datagram_frame(
        &self,
        now: Instant,
        socket: u8,
        place: u8,
        serial: u64,
        (peer, found): ([u8; 6], Option<Found>),
    ) -> Option<Frame<'_>> {
        // No frame leaves from an address past its lease's end.
        let source = self.usable(now)?.address();
        let open = self.sockets.get(usize::from(socket))?;
        let source_port = open.port?;
        let entry = open.places.of(place, serial)?;
        let (remote, hop, data, length) = match entry.run() {
            Run::Sending(sending) => (
                sending.remote,
                sending.hop,
                Some(sending.block.octets()),
                sending.block.octets().len(),
            ),
            Run::Declared(declared) => (declared.remote, declared.hop, None, declared.length),
            Run::Free | Run::Finished(_) => return None,
        };
        let next_hop = self.next_hop(now, hop, found.as_ref())?;
        if next_hop != peer {
            return None;
        }
        let udp_length = UDP_HEADER_LEN.saturating_add(length);
        let wire_length = u16::try_from(udp_length).ok()?;
        // The checksum is summed over the data in the slot: a block's as it
        // is copied there, a producer's once it is written.
        let udp = UdpHeader {
            ports: Ports {
                source: source_port,
                destination: remote.port,
            },
            length: wire_length,
            checksum: 0,
        };
        let pseudo = ipv4_pseudo_header(source, remote.address, PROTOCOL_UDP, wire_length);
        let mut headers = [0; HEADERS];
        let mut writer = Writer::new(&mut headers);
        EthernetHeader {
            destination: next_hop,
            source: self.interface.hardware(),
            ether_type: EtherType::IPV4,
        }
        .write(&mut writer)
        .ok()?;
        writer
            .write(
                &Ipv4Fields {
                    services: open.dscp.field(),
                    total_length: u16::try_from(IPV4_HEADER_LEN.saturating_add(udp_length)).ok()?,
                    identification: self.identification,
                    time_to_live: TIME_TO_LIVE,
                    protocol: PROTOCOL_UDP,
                    source,
                    destination: remote.address,
                }
                .to_bytes(),
            )
            .ok()?;
        writer.write(&udp.to_bytes()).ok()?;
        Some(match data {
            Some(data) => Frame::Datagram {
                headers,
                udp,
                pseudo,
                data,
            },
            None => Frame::Produced {
                headers,
                udp,
                pseudo,
                length,
                work: WorkId { serial, place },
            },
        })
    }

    /// `grant`'s frame was refused for good, by the port or by its own fill:
    /// its work ends, and nothing offers it again. As a commit, this end moves
    /// the next socket's turn past a datagram's socket.
    pub(crate) fn reject<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        grant: &Grant<B>,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) {
        match grant.work {
            Work::Reply { serial } => {
                self.replies.retain(|reply| reply.serial != serial);
            }
            // No request left: what waits for this neighbor has its answer, and
            // nothing was accepted to wait for.
            Work::Request { neighbor, serial } => {
                if let Some(address) = self.neighbors.request_refused(neighbor, serial) {
                    end_waiting(&mut self.sockets, address, Outcome::Rejected, pool);
                    self.echoes
                        .take_through(address, |owed| pool.give(owed.block));
                    if dhcp_hop(self.dhcp.as_ref(), self.ipv4) == Some(address)
                        && let Some(client) = &mut self.dhcp
                        && let Some(owed) = client.owed()
                    {
                        client.refused(now, owed);
                    }
                    self.neighbors_moved();
                }
            }
            Work::Datagram { .. } => self.end_published(grant, Outcome::Rejected, pool),
            Work::Dhcp { serial, .. } => {
                if let Some(client) = &mut self.dhcp {
                    client.refused(now, serial);
                }
                self.forget_unneeded();
            }
            Work::Echo { serial, .. } => self.end_echo(serial, pool),
        }
    }

    /// The echo reply of `serial` ends at its publication, taken or refused
    /// for good: its block goes home, and the neighbor it protected may serve
    /// a new send, which [`Self::neighbors_changed`] tells.
    fn end_echo<const BLOCKS: usize>(&mut self, serial: u64, pool: &mut Pool<'m, B, SIZE, BLOCKS>) {
        if let Some(owed) = self.echoes.take(serial) {
            pool.give(owed.block);
            self.neighbors_moved();
        }
        self.forget_unneeded();
    }

    /// The port took the client's message of `serial`: its next try comes at
    /// its time.
    fn dhcp_sent(&mut self, now: Instant, serial: u64) {
        if let Some(client) = &mut self.dhcp {
            client.sent(now, serial);
        }
        self.forget_unneeded();
    }

    /// `grant`'s producer declined its fill: the work ends
    /// [`Outcome::Declined`], and, as a reject, the next socket's turn moves
    /// past its socket. Only a declared work's frame has a producer's fill.
    pub(crate) fn decline<const BLOCKS: usize>(
        &mut self,
        grant: &Grant<B>,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) {
        self.end_published(grant, Outcome::Declined, pool);
    }

    /// End `grant`'s datagram as `outcome` at its publication, which moves
    /// the next socket's turn past its socket.
    fn end_published<const BLOCKS: usize>(
        &mut self,
        grant: &Grant<B>,
        outcome: Outcome,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) {
        let Work::Datagram {
            socket,
            place,
            serial,
            ..
        } = grant.work
        else {
            return;
        };
        self.cursor = usize::from(socket).wrapping_add(1);
        let released = self
            .sockets
            .get_mut(usize::from(socket))
            .map(|open| &mut open.places)
            .filter(|places| places.of(place, serial).is_some())
            .is_some_and(|places| places.end(place, outcome, pool));
        if released {
            self.neighbors_moved();
        }
        self.forget_unneeded();
    }

    /// The port took the granted work, or said it did without a written
    /// frame: then its datagram ends `Unknown`, and nothing is sent again.
    pub(crate) fn commit<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        grant: Grant<B>,
        written: bool,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) {
        match grant.work {
            Work::Reply { serial } => {
                self.replies.retain(|reply| reply.serial != serial);
            }
            Work::Request { neighbor, .. } => self.neighbors.request_accepted(neighbor, now),
            Work::Dhcp { serial, .. } => {
                self.identification = self.identification.wrapping_add(1);
                self.dhcp_sent(now, serial);
            }
            Work::Echo { serial, .. } => {
                self.identification = self.identification.wrapping_add(1);
                self.end_echo(serial, pool);
            }
            Work::Datagram {
                socket,
                place,
                serial,
                ..
            } => {
                self.cursor = usize::from(socket).wrapping_add(1);
                self.identification = self.identification.wrapping_add(1);
                let outcome = if written {
                    Outcome::Accepted
                } else {
                    Outcome::Unknown
                };
                let released = self
                    .sockets
                    .get_mut(usize::from(socket))
                    .map(|open| &mut open.places)
                    .filter(|places| places.of(place, serial).is_some())
                    .is_some_and(|places| places.end(place, outcome, pool));
                if released {
                    self.neighbors_moved();
                }
            }
        }
    }

    /// End this core. Every block it holds returns to `pool`, and the brand
    /// comes back for the next core. Results move to [`RetiredResults`], so a
    /// confirmed one never depends on having been read in time; a datagram
    /// still running ends [`Outcome::Retired`]. What applications hold stays
    /// out ([`Pool::outstanding`]) and returns whenever they give it.
    pub fn retire<const BLOCKS: usize>(
        self,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) -> (RetiredResults<B, SOCKETS, CREDITS>, CoreBrand<B>) {
        // Loans outlive a core: what is out stays owed, to this pool.
        pool.loans().reinstall();
        let Self {
            sockets,
            serial,
            brand,
            mut echoes,
            ..
        } = self;
        echoes.take_all(|owed| pool.give(owed.block));
        let mut received_dropped = 0_u32;
        let mut index = 0;
        let mut results = [const { [Kept::NONE; CREDITS] }; SOCKETS];
        let mut closes = [Kept::NONE; SOCKETS];
        for mut socket in sockets {
            while let Some(datagram) = socket.received.pop_front() {
                pool.give(datagram.block);
                received_dropped = received_dropped.saturating_add(1);
            }
            let (places, close) = socket.places.into_places();
            let close = Kept::of(close);
            let kept = places.map(|place| {
                let (place_serial, observed, run) = place.into_parts();
                let outcome = match run {
                    Run::Sending(sending) => {
                        pool.give(sending.block);
                        Some(Outcome::Retired)
                    }
                    Run::Declared(_) => Some(Outcome::Retired),
                    Run::Finished(outcome) => Some(outcome),
                    Run::Free => None,
                };
                Kept {
                    serial: place_serial,
                    outcome: outcome.filter(|_| observed),
                }
            });
            if let Some(slot) = results.get_mut(index) {
                *slot = kept;
            }
            if let Some(slot) = closes.get_mut(index) {
                *slot = close;
            }
            index = index.wrapping_add(1);
        }
        (
            RetiredResults {
                results,
                closes,
                received_dropped,
                brand,
            },
            CoreBrand { brand, serial },
        )
    }
}

/// A result kept past its core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Kept {
    serial: u64,
    outcome: Option<Outcome>,
}

impl Kept {
    const NONE: Self = Self {
        serial: 0,
        outcome: None,
    };

    fn of<B, const SIZE: usize>(place: Place<'_, B, SIZE>) -> Self {
        match place.into_parts() {
            (serial, true, Run::Finished(outcome)) => Self {
                serial,
                outcome: Some(outcome),
            },
            _ => Self::NONE,
        }
    }
}

/// A retired core's results, each read once, whenever its observer comes.
#[derive(Debug)]
pub struct RetiredResults<B, const SOCKETS: usize, const CREDITS: usize> {
    results: [[Kept; CREDITS]; SOCKETS],
    closes: [Kept; SOCKETS],
    /// Received datagrams nobody read, which went back to the pool.
    pub received_dropped: u32,
    brand: Tag<B>,
}

impl<B, const SOCKETS: usize, const CREDITS: usize> RetiredResults<B, SOCKETS, CREDITS> {
    /// The command's result, once.
    pub fn take(&mut self, command: CommandId<B>) -> CommandState<B> {
        let kept = self
            .results
            .get_mut(usize::from(command.socket))
            .and_then(|places| places.get_mut(usize::from(command.place)))
            .or_else(|| self.closes.get_mut(usize::from(command.socket)))
            .filter(|kept| kept.serial == command.serial);
        kept.and_then(|kept| kept.outcome.take())
            .map_or(CommandState::Gone, CommandState::Finished)
    }

    /// How many results nobody has read yet.
    pub fn unread(&self) -> usize {
        self.results
            .iter()
            .flatten()
            .chain(self.closes.iter())
            .filter(|kept| kept.outcome.is_some())
            .count()
    }
}
