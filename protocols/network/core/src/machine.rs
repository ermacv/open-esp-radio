//! The core: sockets, frames in, work out.
//!
//! SOURCE: RFC 791 and RFC 1122 for IPv4; RFC 768 for UDP; RFC 826 for ARP.

use core::marker::PhantomData;

use oer_portable_events::{EventSink, report};
use oer_portable_time::Instant;
use oer_portable_wire::Writer;
use oer_protocols_network_port::{Candidate, Class, EthernetFrameView, Ready, SlotTooShort};
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::ethernet::{self, EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{
    Dscp, IPV4_HEADER_LEN, Ipv4Fields, Ipv4Header, Ipv4VersionIhl,
};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UDP_HEADER_LEN, UdpHeader};

use crate::block::{Block, CoreBrand, Group, Pool, Tag};
use crate::interface::{BROADCAST_HARDWARE, Interface};
use crate::neighbor::{Lookup, Neighbors};

/// RFC 1122, 3.2.1.7: a default time to live large enough for the Internet.
const TIME_TO_LIVE: u8 = 64;
/// Headers before a datagram's data.
const HEADERS: usize = ethernet::HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN;
/// An ARP frame.
const ARP_FRAME: usize = ethernet::HEADER_LEN + arp::PACKET_LEN;
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
    Datagram {
        headers: [u8; HEADERS],
        data: &'a [u8],
    },
}

impl Frame<'_> {
    pub(crate) fn length(&self) -> usize {
        match self {
            Self::Arp(frame) => frame.len(),
            Self::Datagram { headers, data } => headers.len().saturating_add(data.len()),
        }
    }

    /// Write the frame into `slot`.
    pub(crate) fn write(&self, slot: &mut [u8]) -> Result<(), SlotTooShort> {
        let mut writer = Writer::new(slot);
        match self {
            Self::Arp(frame) => writer.write(frame),
            Self::Datagram { headers, data } => {
                writer.write(headers).and_then(|()| writer.write(data))
            }
        }
        .map_err(|_| SlotTooShort)
    }
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
}

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
}

#[derive(Debug)]
struct Sending<'m, B, const SIZE: usize> {
    remote: Remote,
    block: Block<'m, B, SIZE>,
    since: Instant,
    /// The neighbor it goes through; broadcast has none. Its station address
    /// is read when the frame is built, so a change reaches a waiting datagram.
    hop: Option<[u8; 4]>,
}

#[derive(Debug)]
enum Run<'m, B, const SIZE: usize> {
    Free,
    Sending(Sending<'m, B, SIZE>),
    Finished(Outcome),
}

/// One command's place: what it runs and, after, its result.
#[derive(Debug)]
struct Place<'m, B, const SIZE: usize> {
    /// The running command's, or 0.
    serial: u64,
    /// Someone holds the right to the result.
    observed: bool,
    run: Run<'m, B, SIZE>,
}

impl<B, const SIZE: usize> Place<'_, B, SIZE> {
    const fn new() -> Self {
        Self {
            serial: 0,
            observed: false,
            run: Run::Free,
        }
    }

    /// End the command. A place nobody observes frees here; otherwise the
    /// result waits in it.
    fn finish(&mut self, outcome: Outcome) {
        if self.observed {
            self.run = Run::Finished(outcome);
        } else {
            self.free();
        }
    }

    fn free(&mut self) {
        self.run = Run::Free;
        self.serial = 0;
    }
}

#[derive(Debug)]
struct Socket<'m, B, const SIZE: usize, const CREDITS: usize, const RECEIVED: usize> {
    serial: u64,
    /// The bound port; a socket being closed keeps its slot, not its port.
    port: Option<u16>,
    open: bool,
    dscp: Dscp,
    /// What its received data counts toward.
    group: Group<B>,
    places: [Place<'m, B, SIZE>; CREDITS],
    /// Held from the socket's creation: a close always has a place.
    close: Place<'m, B, SIZE>,
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
            dscp: BEST_EFFORT,
            group: Group::shared(),
            places: [const { Place::new() }; CREDITS],
            close: Place::new(),
            received: heapless::Deque::new(),
        }
    }

    fn in_use(&self) -> bool {
        self.open
            || !matches!(self.close.run, Run::Free)
            || self
                .places
                .iter()
                .any(|place| !matches!(place.run, Run::Free))
    }

    /// Whether a datagram here goes through `address`.
    fn needs(&self, address: [u8; 4]) -> bool {
        self.places.iter().any(|place| {
            matches!(&place.run, Run::Sending(Sending { hop: Some(hop), .. }) if *hop == address)
        })
    }

    /// A command's place by its number: `CREDITS` is the close's.
    fn place_mut(&mut self, place: u8) -> Option<&mut Place<'m, B, SIZE>> {
        let place = usize::from(place);
        if place == CREDITS {
            Some(&mut self.close)
        } else {
            self.places.get_mut(place)
        }
    }
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
    /// Where the next look for a datagram starts, so sockets take turns.
    cursor: usize,
    identification: u16,
    /// The last serial given: 2^64 of them outlast any device.
    serial: u64,
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

    /// The core of `brand`.
    pub const fn new(brand: CoreBrand<B>, interface: Interface) -> Self {
        const { Self::FITS };
        Self {
            interface,
            neighbors: Neighbors::new(),
            replies: heapless::Vec::new(),
            sockets: [const { Socket::new() }; SOCKETS],
            cursor: 0,
            identification: 0,
            serial: brand.serial,
            brand: brand.brand,
        }
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
        if self.sockets.iter().any(|socket| socket.port == Some(port)) {
            return Err(OpenRefused::PortInUse);
        }
        let serial = self.next_serial();
        let (index, socket) = self
            .sockets
            .iter_mut()
            .enumerate()
            .find(|(_, socket)| !socket.in_use())
            .ok_or(OpenRefused::NoSocket)?;
        let index = u8::try_from(index).map_err(|_| OpenRefused::NoSocket)?;
        socket.serial = serial;
        socket.port = Some(port);
        socket.open = true;
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
        let refused = |reason, block| Err(SendRefused { reason, block });
        let datagram = IPV4_HEADER_LEN
            .saturating_add(UDP_HEADER_LEN)
            .saturating_add(block.octets().len());
        if datagram > usize::from(self.interface.mtu()) {
            return refused(SendRefusal::TooLong, block);
        }
        let Some(open) = self.socket_mut(socket.index, socket.serial) else {
            return refused(SendRefusal::Stale, block);
        };
        if !open.open {
            return refused(SendRefusal::Stale, block);
        }
        let Some(free) = open
            .places
            .iter()
            .position(|place| matches!(place.run, Run::Free))
        else {
            return refused(SendRefusal::NoPlace, block);
        };
        let Ok(number) = u8::try_from(free) else {
            return refused(SendRefusal::NoPlace, block);
        };
        let hop = if self.interface.is_broadcast(remote.address) {
            None
        } else {
            let Some(hop) = self.interface.next_hop(remote.address) else {
                return refused(SendRefusal::NoRoute, block);
            };
            let serial = self.next_serial();
            let sockets = &self.sockets;
            let lookup = self.neighbors.resolve(now, hop, serial, |address| {
                sockets.iter().any(|socket| socket.needs(address))
            });
            if lookup == Lookup::Full {
                return refused(SendRefusal::NeighborsFull, block);
            }
            Some(hop)
        };
        let serial = self.next_serial();
        let Some(place) = self
            .socket_mut(socket.index, socket.serial)
            .and_then(|open| open.places.get_mut(free))
        else {
            return refused(SendRefusal::Stale, block);
        };
        place.observed = true;
        place.serial = serial;
        place.run = Run::Sending(Sending {
            remote,
            block,
            since: now,
            hop,
        });
        Ok(CommandId {
            socket: socket.index,
            place: number,
            serial,
            brand: PhantomData,
        })
    }

    /// The command's result, once it ended: its place frees.
    pub fn take_result(&mut self, command: CommandId<B>) -> CommandState<B> {
        let Some(place) = self
            .sockets
            .get_mut(usize::from(command.socket))
            .and_then(|socket| socket.place_mut(command.place))
            .filter(|place| place.serial == command.serial)
        else {
            return CommandState::Gone;
        };
        if let Run::Finished(outcome) = place.run {
            place.free();
            CommandState::Finished(outcome)
        } else {
            CommandState::Running(command)
        }
    }

    /// Every datagram [`Self::evict`] can take back.
    pub fn queued(&self) -> impl Iterator<Item = Queued<B>> + '_ {
        let brand = self.brand;
        (0_u8..)
            .zip(self.sockets.iter())
            .flat_map(move |(flow, socket)| {
                (0_u8..)
                    .zip(socket.places.iter())
                    .filter_map(move |(place, entry)| match &entry.run {
                        Run::Sending(sending) => Some(Queued {
                            flow,
                            group: sending.block.group(),
                            since: sending.since,
                            place,
                            serial: entry.serial,
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
        let Some(place) = self
            .sockets
            .get_mut(usize::from(queued.flow))
            .and_then(|socket| socket.places.get_mut(usize::from(queued.place)))
            .filter(|place| place.serial == queued.serial)
        else {
            return false;
        };
        if !matches!(place.run, Run::Sending(_)) {
            return false;
        }
        if let Run::Sending(sending) = core::mem::replace(&mut place.run, Run::Free) {
            pool.give(sending.block);
        }
        place.finish(Outcome::Evicted(eviction));
        let sockets = &self.sockets;
        self.neighbors
            .forget_unneeded(|address| sockets.iter().any(|socket| socket.needs(address)));
        true
    }

    /// Nobody will read this command's result. A finished command's place
    /// frees now; a running command keeps it until its end, so that end
    /// never reaches a later command.
    pub fn drop_observer(&mut self, command: CommandId<B>) {
        if let Some(place) = self
            .sockets
            .get_mut(usize::from(command.socket))
            .and_then(|socket| socket.place_mut(command.place))
            .filter(|place| place.serial == command.serial)
        {
            place.observed = false;
            if matches!(place.run, Run::Finished(_)) {
                place.free();
            }
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
        while let Some(datagram) = open.received.pop_front() {
            pool.give(datagram.block);
        }
        for place in &mut open.places {
            if let Run::Sending(_) = place.run
                && let Run::Sending(sending) = core::mem::replace(&mut place.run, Run::Free)
            {
                pool.give(sending.block);
                place.finish(Outcome::Closed);
            }
        }
        open.close.observed = true;
        open.close.serial = serial;
        open.close.run = Run::Finished(Outcome::SocketClosed);
        let sockets = &self.sockets;
        self.neighbors
            .forget_unneeded(|address| sockets.iter().any(|socket| socket.needs(address)));
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
        self.input_view(now, view, pool, events);
    }

    /// As [`Self::input`], for a frame whose fields a port read apart from its
    /// payload.
    pub fn input_view<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        frame: EthernetFrameView<'_>,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
        events: &mut impl EventSink<NetEvent>,
    ) {
        if frame.destination != self.interface.hardware() && frame.destination != BROADCAST_HARDWARE
        {
            return;
        }
        let ether_type = EtherType::new(frame.ether_type);
        if ether_type == EtherType::ARP {
            self.input_arp(now, frame.payload, events);
        } else if ether_type == EtherType::IPV4 {
            self.input_ipv4(frame.payload, pool, events);
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
        let for_us = packet.target_protocol == self.interface.address();
        self.neighbors
            .learn(now, packet.sender_protocol, packet.sender_hardware, for_us);
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
        payload: &[u8],
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
        events: &mut impl EventSink<NetEvent>,
    ) {
        let Ok(packet) = Ipv4Header::parse(payload) else {
            report(events, || NetEvent::Malformed);
            return;
        };
        let [s0, s1, s2, s3, d0, d1, d2, d3] = packet.addresses();
        let (source, destination) = ([s0, s1, s2, s3], [d0, d1, d2, d3]);
        if destination != self.interface.address() && !self.interface.is_broadcast(destination) {
            return;
        }
        let intact = payload
            .first()
            .map(|first| usize::from(Ipv4VersionIhl::from_bytes([*first]).ihl()).saturating_mul(4))
            .and_then(|header| payload.get(..header))
            .is_some_and(|header| Checksum::new().with(header).verifies());
        if !intact {
            report(events, || NetEvent::Checksum);
            return;
        }
        if self.interface.is_broadcast(source) {
            report(events, || NetEvent::BadSource);
            return;
        }
        if packet.protocol() != PROTOCOL_UDP {
            return;
        }
        if packet.is_fragment() {
            report(events, || NetEvent::Fragment);
            return;
        }
        let datagram = packet.payload();
        let Ok((header, data)) = UdpHeader::parse(datagram) else {
            report(events, || NetEvent::Malformed);
            return;
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
            return;
        }
        let port = header.ports.destination;
        let Some(socket) = self
            .sockets
            .iter_mut()
            .find(|socket| socket.open && socket.port == Some(port))
        else {
            report(events, || NetEvent::NoSocket);
            return;
        };
        if socket.received.is_full() {
            report(events, || NetEvent::ReceiveFull);
            return;
        }
        let Some(mut block) = pool.take_as(socket.group) else {
            report(events, || {
                if pool.free() == 0 {
                    NetEvent::NoBlock
                } else {
                    NetEvent::GroupLimit
                }
            });
            return;
        };
        if block.fill(data).is_err() {
            pool.give(block);
            report(events, || NetEvent::TooLong);
            return;
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
    }

    /// Advance to `now`: ask again for neighbors that expired under a waiting
    /// datagram, and end what waited for one that never answered. This comes
    /// first at every time [`Self::poll_deadline`] names, and before
    /// [`Self::demands`] at a later `now`: an address past its life is no work
    /// until this has asked for it again.
    pub fn tick<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) {
        let mut serial = self.serial;
        for place in self.sockets.iter().flat_map(|socket| socket.places.iter()) {
            if let Run::Sending(Sending { hop: Some(hop), .. }) = place.run {
                let next = serial.wrapping_add(1);
                if self.neighbors.reask(now, hop, next) {
                    serial = next;
                }
            }
        }
        self.serial = serial;
        let sockets = &mut self.sockets;
        self.neighbors.tick(now, |address| {
            for socket in sockets.iter_mut() {
                for place in &mut socket.places {
                    let waits = matches!(
                        &place.run,
                        Run::Sending(Sending { hop: Some(hop), .. }) if *hop == address
                    );
                    if waits
                        && let Run::Sending(sending) = core::mem::replace(&mut place.run, Run::Free)
                    {
                        pool.give(sending.block);
                        place.finish(Outcome::Unreachable);
                    }
                }
            }
        });
    }

    /// When [`Self::tick`] has work: a request's time, or the end of the life
    /// of a neighbor a datagram needs. Nothing else makes demand appear.
    pub fn poll_deadline(&self) -> Option<Instant> {
        let sockets = &self.sockets;
        self.neighbors
            .deadline(|address| sockets.iter().any(|socket| socket.needs(address)))
    }

    /// Every piece of work ready now, with no frame built: control first,
    /// then datagrams from the socket whose turn it is. A scheduler chooses
    /// among these; a direct interface takes [`Self::poll_demand`].
    pub fn demands(&self, now: Instant) -> impl Iterator<Item = Demand<B>> + Clone + '_ {
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
        let datagrams = (0..SOCKETS)
            .filter_map(|offset| self.cursor.wrapping_add(offset).checked_rem(SOCKETS))
            .filter_map(|index| Some((index, self.sockets.get(index)?)))
            .flat_map(move |(index, socket)| {
                socket
                    .places
                    .iter()
                    .enumerate()
                    .filter_map(move |(number, place)| {
                        self.datagram_demand(now, index, socket, number, place)
                    })
            });
        replies.chain(requests).chain(datagrams)
    }

    fn datagram_demand(
        &self,
        now: Instant,
        index: usize,
        socket: &Socket<'m, B, SIZE, CREDITS, RECEIVED>,
        number: usize,
        place: &Place<'m, B, SIZE>,
    ) -> Option<Demand<B>> {
        let Run::Sending(sending) = &place.run else {
            return None;
        };
        let peer = match sending.hop {
            Some(hop) => self.neighbors.hardware(now, hop)?,
            None => BROADCAST_HARDWARE,
        };
        Some(Demand {
            ready: Ready {
                class: Class::Data,
                length: HEADERS.saturating_add(sending.block.octets().len()),
                peer,
                flow: u8::try_from(index).ok(),
                dscp: socket.dscp,
            },
            work: Work::Datagram {
                socket: u8::try_from(index).ok()?,
                place: u8::try_from(number).ok()?,
                serial: place.serial,
                peer,
            },
            brand: self.brand,
        })
    }

    /// The next ready work, and the next socket's turn. A direct interface
    /// grants this at once; a port's refusal leaves it ready.
    pub fn poll_demand(&self, now: Instant) -> Option<Demand<B>> {
        self.demands(now).next()
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
            } => self.datagram_frame(now, socket, place, serial, peer),
        }
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
            sender_protocol: self.interface.address(),
            target_hardware,
            target_protocol,
        }
        .write(rest)
        .ok()?;
        Some(Frame::Arp(frame))
    }

    fn datagram_frame(
        &self,
        now: Instant,
        socket: u8,
        place: u8,
        serial: u64,
        peer: [u8; 6],
    ) -> Option<Frame<'_>> {
        let open = self.sockets.get(usize::from(socket))?;
        let source_port = open.port?;
        let entry = open
            .places
            .get(usize::from(place))
            .filter(|entry| entry.serial == serial)?;
        let Run::Sending(sending) = &entry.run else {
            return None;
        };
        let next_hop = match sending.hop {
            Some(hop) => self.neighbors.hardware(now, hop)?,
            None => BROADCAST_HARDWARE,
        };
        if next_hop != peer {
            return None;
        }
        let data = sending.block.octets();
        let udp_length = UDP_HEADER_LEN.saturating_add(data.len());
        let wire_length = u16::try_from(udp_length).ok()?;
        let unfilled = UdpHeader {
            ports: Ports {
                source: source_port,
                destination: sending.remote.port,
            },
            length: wire_length,
            checksum: 0,
        };
        let pseudo = ipv4_pseudo_header(
            self.interface.address(),
            sending.remote.address,
            PROTOCOL_UDP,
            wire_length,
        );
        let checksum = udp_field(
            Checksum::new()
                .with(&pseudo)
                .with(&unfilled.to_bytes())
                .with(data)
                .finish(),
        );
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
                    source: self.interface.address(),
                    destination: sending.remote.address,
                }
                .to_bytes(),
            )
            .ok()?;
        writer
            .write(
                &UdpHeader {
                    checksum,
                    ..unfilled
                }
                .to_bytes(),
            )
            .ok()?;
        Some(Frame::Datagram { headers, data })
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
            Work::Datagram { socket, place, .. } => {
                self.cursor = usize::from(socket).wrapping_add(1);
                self.identification = self.identification.wrapping_add(1);
                if let Some(entry) = self
                    .sockets
                    .get_mut(usize::from(socket))
                    .and_then(|open| open.places.get_mut(usize::from(place)))
                    && let Run::Sending(_) = entry.run
                    && let Run::Sending(sending) = core::mem::replace(&mut entry.run, Run::Free)
                {
                    pool.give(sending.block);
                    entry.finish(if written {
                        Outcome::Accepted
                    } else {
                        Outcome::Unknown
                    });
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
            ..
        } = self;
        let mut received_dropped = 0_u32;
        let mut index = 0;
        let mut results = [const { [Kept::NONE; CREDITS] }; SOCKETS];
        let mut closes = [Kept::NONE; SOCKETS];
        for mut socket in sockets {
            while let Some(datagram) = socket.received.pop_front() {
                pool.give(datagram.block);
                received_dropped = received_dropped.saturating_add(1);
            }
            let close = Kept::of(socket.close);
            let kept = socket.places.map(|place| {
                let observed = place.observed;
                let place_serial = place.serial;
                let outcome = match place.run {
                    Run::Sending(sending) => {
                        pool.give(sending.block);
                        Some(Outcome::Retired)
                    }
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
        match place.run {
            Run::Finished(outcome) if place.observed => Self {
                serial: place.serial,
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
