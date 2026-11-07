//! The core: sockets, frames in, work out.
//!
//! SOURCE: RFC 791 and RFC 1122 for IPv4; RFC 768 for UDP; RFC 826 for ARP.

use core::marker::PhantomData;

use oer_portable_events::{EventSink, report};
use oer_portable_time::Instant;
use oer_portable_wire::Writer;
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::ethernet::{self, EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{
    Dscp, IPV4_HEADER_LEN, Ipv4Fields, Ipv4Header, Ipv4VersionIhl,
};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UDP_HEADER_LEN, UdpHeader};

use crate::block::{Block, Brand, CoreBrand, Pool};
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

/// How work is served.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    Control,
    Data,
}

/// One piece of work. A serial never comes back, so a grant names this work
/// and no later work in its place.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Work {
    Reply { serial: u64 },
    Request { neighbor: u8, address: [u8; 4] },
    Datagram { socket: u8, place: u8, serial: u64 },
}

/// Work that is ready, before any frame of it exists: what a scheduler needs
/// to choose it.
#[derive(Debug, Eq, PartialEq)]
pub struct Demand {
    pub class: Class,
    /// The frame's length: what a slot must hold.
    pub length: usize,
    /// The next hop's station address, whose budget this work spends.
    pub peer: [u8; 6],
    /// The socket that wants it; control work has none.
    pub flow: Option<u8>,
    /// Its priority.
    pub dscp: Dscp,
    work: Work,
}

impl Demand {
    /// Choose this work.
    pub const fn grant(self) -> Grant {
        Grant { work: self.work }
    }
}

/// The right to send one piece of work, and no other.
#[derive(Debug, Eq, PartialEq)]
pub struct Grant {
    work: Work,
}

/// A frame's way out. One call writes the frame and is the port's answer, so
/// a frame the port accepted is always one the core knows it accepted.
pub trait FramePort {
    /// Why the port took nothing, which names what it waits for.
    type Refusal;

    /// Take room for `length` octets, let `fill` write the frame there and
    /// accept it. `Ok` says the frame is the port's.
    ///
    /// # Errors
    /// The port accepted nothing: it did not have the room, or `fill` failed.
    fn publish(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), SlotTooShort>,
    ) -> Result<(), Self::Refusal>;
}

/// The room a port gave does not hold the frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlotTooShort;

/// Why a grant sent nothing. The work stays as it was.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unsent<R> {
    /// The granted work ended, or no longer holds: a final refusal of this
    /// grant, not of the work's successor.
    Stale,
    /// The port refused.
    Port(R),
    /// The port answered `Ok` though the frame was not written, which breaks
    /// [`FramePort::publish`]. Nothing is known to have gone out.
    NotFilled,
}

/// A socket. Its handle is not a copy: closing consumes it.
#[derive(Debug, Eq, PartialEq)]
pub struct SocketId<'id> {
    index: u8,
    serial: u64,
    brand: Brand<'id>,
}

/// A running command's right to its one result.
#[derive(Debug, Eq, PartialEq)]
pub struct CommandId<'id> {
    socket: u8,
    place: u8,
    serial: u64,
    brand: Brand<'id>,
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
}

/// A datagram that was received.
#[derive(Debug)]
pub struct Datagram<'id, const SIZE: usize> {
    pub remote: Remote,
    pub block: Block<'id, SIZE>,
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
    /// Every neighbor entry is being asked for, until one answers or gives up.
    NeighborsFull,
}

/// A refused datagram with its block.
#[derive(Debug)]
pub struct SendRefused<'id, const SIZE: usize> {
    pub reason: SendRefusal,
    pub block: Block<'id, SIZE>,
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
struct Sending<'id, const SIZE: usize> {
    remote: Remote,
    block: Block<'id, SIZE>,
    /// The neighbor it goes through; broadcast has none. Its station address
    /// is read when the frame is built, so a change reaches a waiting datagram.
    hop: Option<[u8; 4]>,
}

#[derive(Debug)]
enum Run<'id, const SIZE: usize> {
    Free,
    Sending(Sending<'id, SIZE>),
    Finished(Outcome),
}

/// One command's place: what it runs and, after, its result.
#[derive(Debug)]
struct Place<'id, const SIZE: usize> {
    /// The running command's, or 0.
    serial: u64,
    /// Someone holds the right to the result.
    observed: bool,
    run: Run<'id, SIZE>,
}

impl<const SIZE: usize> Place<'_, SIZE> {
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
struct Socket<'id, const SIZE: usize, const CREDITS: usize, const RECEIVED: usize> {
    serial: u64,
    /// The bound port; a socket being closed keeps its slot, not its port.
    port: Option<u16>,
    open: bool,
    dscp: Dscp,
    places: [Place<'id, SIZE>; CREDITS],
    /// Held from the socket's creation: a close always has a place.
    close: Place<'id, SIZE>,
    received: heapless::Deque<Datagram<'id, SIZE>, RECEIVED>,
}

impl<'id, const SIZE: usize, const CREDITS: usize, const RECEIVED: usize>
    Socket<'id, SIZE, CREDITS, RECEIVED>
{
    const fn new() -> Self {
        Self {
            serial: 0,
            port: None,
            open: false,
            dscp: BEST_EFFORT,
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

    /// A command's place by its number: `CREDITS` is the close's.
    fn place_mut(&mut self, place: u8) -> Option<&mut Place<'id, SIZE>> {
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
    'id,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
> {
    interface: Interface,
    neighbors: Neighbors<NEIGHBORS>,
    /// Replies owed. Bounded by the table: an asker past it asks again.
    replies: heapless::Deque<Reply, NEIGHBORS>,
    sockets: [Socket<'id, SIZE, CREDITS, RECEIVED>; SOCKETS],
    /// Where the next look for a datagram starts, so sockets take turns.
    cursor: usize,
    identification: u16,
    /// The last serial given: 2^64 of them outlast any device.
    serial: u64,
    brand: Brand<'id>,
}

impl<
    'id,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
> Core<'id, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS>
{
    const FITS: () = {
        if SOCKETS > 255 || CREDITS >= 255 || NEIGHBORS > 255 {
            panic!("sockets, places and neighbors are numbered in one octet");
        }
        if SIZE < ARP_FRAME {
            panic!("a block holds at least an ARP frame");
        }
    };

    /// The core of `brand`'s [`scope`](crate::scope).
    pub const fn new(brand: CoreBrand<'id>, interface: Interface) -> Self {
        const { Self::FITS };
        Self {
            interface,
            neighbors: Neighbors::new(),
            replies: heapless::Deque::new(),
            sockets: [const { Socket::new() }; SOCKETS],
            cursor: 0,
            identification: 0,
            serial: 0,
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
    pub fn open(&mut self, port: u16, dscp: Dscp) -> Result<SocketId<'id>, OpenRefused> {
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
    ) -> Option<&mut Socket<'id, SIZE, CREDITS, RECEIVED>> {
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
        socket: &SocketId<'id>,
        remote: Remote,
        block: Block<'id, SIZE>,
    ) -> Result<CommandId<'id>, SendRefused<'id, SIZE>> {
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
            if self.neighbors.resolve(now, hop) == Lookup::Full {
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
        place.run = Run::Sending(Sending { remote, block, hop });
        Ok(CommandId {
            socket: socket.index,
            place: number,
            serial,
            brand: PhantomData,
        })
    }

    /// The command's result, once it ended: its place frees. A command still
    /// running gives its right back.
    ///
    /// # Errors
    /// The command runs; the right comes back.
    pub fn take_result(&mut self, command: CommandId<'id>) -> Result<Outcome, CommandId<'id>> {
        let finished = self
            .sockets
            .get_mut(usize::from(command.socket))
            .and_then(|socket| socket.place_mut(command.place))
            .filter(|place| place.serial == command.serial)
            .and_then(|place| match place.run {
                Run::Finished(outcome) => {
                    place.free();
                    Some(outcome)
                }
                _ => None,
            });
        finished.ok_or(command)
    }

    /// Nobody will read this command's result. A finished command's place
    /// frees now; a running command keeps it until its end, so that end
    /// never reaches a later command.
    pub fn drop_observer(&mut self, command: CommandId<'id>) {
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
        socket: &SocketId<'id>,
    ) -> Result<Option<Datagram<'id, SIZE>>, StaleSocket> {
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
        socket: SocketId<'id>,
        pool: &mut Pool<'id, SIZE, BLOCKS>,
    ) -> Result<CommandId<'id>, StaleSocket> {
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
        pool: &mut Pool<'id, SIZE, BLOCKS>,
        events: &mut impl EventSink<NetEvent>,
    ) {
        let Ok((header, payload)) = EthernetHeader::parse(frame) else {
            report(events, || NetEvent::Malformed);
            return;
        };
        if header.destination != self.interface.hardware()
            && header.destination != BROADCAST_HARDWARE
        {
            return;
        }
        if header.ether_type == EtherType::ARP {
            self.input_arp(now, payload, events);
        } else if header.ether_type == EtherType::IPV4 {
            self.input_ipv4(payload, pool, events);
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
            if self.replies.push_back(reply).is_err() {
                report(events, || NetEvent::ReplyDropped);
            }
        }
    }

    fn input_ipv4<const BLOCKS: usize>(
        &mut self,
        payload: &[u8],
        pool: &mut Pool<'id, SIZE, BLOCKS>,
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
        let Some(mut block) = pool.take() else {
            report(events, || NetEvent::NoBlock);
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

    /// Ask neighbors again, and end what waited for one that never answered.
    pub fn tick<const BLOCKS: usize>(&mut self, now: Instant, pool: &mut Pool<'id, SIZE, BLOCKS>) {
        self.ask_for_hops(now);
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

    /// A datagram's neighbor may have expired while it waited: ask again, so
    /// it either goes or ends unreachable.
    fn ask_for_hops(&mut self, now: Instant) {
        let neighbors = &mut self.neighbors;
        for place in self.sockets.iter().flat_map(|socket| socket.places.iter()) {
            if let Run::Sending(Sending { hop: Some(hop), .. }) = place.run {
                let _resolved = neighbors.resolve(now, hop);
            }
        }
    }

    /// When [`Self::tick`] has work.
    pub fn poll_deadline(&self) -> Option<Instant> {
        self.neighbors.deadline()
    }

    /// Every piece of work ready now, with no frame built: control first,
    /// then datagrams from the socket whose turn it is. A scheduler chooses
    /// among these; a direct interface takes [`Self::poll_demand`].
    pub fn demands(&self, now: Instant) -> impl Iterator<Item = Demand> + '_ {
        let reply = self.replies.front().map(|reply| Demand {
            class: Class::Control,
            length: ARP_FRAME,
            peer: reply.hardware,
            flow: None,
            dscp: BEST_EFFORT,
            work: Work::Reply {
                serial: reply.serial,
            },
        });
        let requests = self
            .neighbors
            .owed_requests()
            .map(|(neighbor, address)| Demand {
                class: Class::Control,
                length: ARP_FRAME,
                peer: BROADCAST_HARDWARE,
                flow: None,
                dscp: BEST_EFFORT,
                work: Work::Request { neighbor, address },
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
        reply.into_iter().chain(requests).chain(datagrams)
    }

    fn datagram_demand(
        &self,
        now: Instant,
        index: usize,
        socket: &Socket<'id, SIZE, CREDITS, RECEIVED>,
        number: usize,
        place: &Place<'id, SIZE>,
    ) -> Option<Demand> {
        let Run::Sending(sending) = &place.run else {
            return None;
        };
        let peer = match sending.hop {
            Some(hop) => self.neighbors.hardware(now, hop)?,
            None => BROADCAST_HARDWARE,
        };
        Some(Demand {
            class: Class::Data,
            length: HEADERS.saturating_add(sending.block.octets().len()),
            peer,
            flow: u8::try_from(index).ok(),
            dscp: socket.dscp,
            work: Work::Datagram {
                socket: u8::try_from(index).ok()?,
                place: u8::try_from(number).ok()?,
                serial: place.serial,
            },
        })
    }

    /// The next ready work, and the next socket's turn. A direct interface
    /// grants this at once; the port's refusal leaves it ready.
    pub fn poll_demand(&mut self, now: Instant) -> Option<Demand> {
        self.ask_for_hops(now);
        let demand = self.demands(now).next()?;
        if let Work::Datagram { socket, .. } = demand.work {
            self.cursor = usize::from(socket).wrapping_add(1);
        }
        Some(demand)
    }

    /// Send the granted work: the port writes the frame in its own room and
    /// answers in one call, and what it accepted is committed here, so no
    /// frame leaves unknown to the core. A refusal moves nothing.
    ///
    /// # Errors
    /// [`Unsent`]: the work stays as it was.
    pub fn transmit<P: FramePort, const BLOCKS: usize>(
        &mut self,
        now: Instant,
        grant: Grant,
        port: &mut P,
        pool: &mut Pool<'id, SIZE, BLOCKS>,
    ) -> Result<(), Unsent<P::Refusal>> {
        match grant.work {
            Work::Reply { serial } => {
                let reply = self
                    .replies
                    .front()
                    .filter(|reply| reply.serial == serial)
                    .copied()
                    .ok_or(Unsent::Stale)?;
                let frame = self.arp_frame(
                    Operation::Reply,
                    reply.hardware,
                    reply.hardware,
                    reply.address,
                )?;
                publish_frame(port, &frame)?;
                let _sent = self.replies.pop_front();
                Ok(())
            }
            Work::Request { neighbor, address } => {
                if !self
                    .neighbors
                    .owed_requests()
                    .any(|owed| owed == (neighbor, address))
                {
                    return Err(Unsent::Stale);
                }
                let frame =
                    self.arp_frame(Operation::Request, BROADCAST_HARDWARE, [0; 6], address)?;
                publish_frame(port, &frame)?;
                self.neighbors.request_accepted(neighbor, now);
                Ok(())
            }
            Work::Datagram {
                socket,
                place,
                serial,
            } => self.transmit_datagram(now, socket, place, serial, port, pool),
        }
    }

    fn arp_frame<R>(
        &self,
        operation: Operation,
        destination: [u8; 6],
        target_hardware: [u8; 6],
        target_protocol: [u8; 4],
    ) -> Result<[u8; ARP_FRAME], Unsent<R>> {
        let mut frame = [0; ARP_FRAME];
        let (head, rest) = frame
            .split_at_mut_checked(ethernet::HEADER_LEN)
            .ok_or(Unsent::Stale)?;
        EthernetHeader {
            destination,
            source: self.interface.hardware(),
            ether_type: EtherType::ARP,
        }
        .write(&mut Writer::new(head))
        .map_err(|_| Unsent::Stale)?;
        Arp {
            operation,
            sender_hardware: self.interface.hardware(),
            sender_protocol: self.interface.address(),
            target_hardware,
            target_protocol,
        }
        .write(rest)
        .map_err(|_| Unsent::Stale)?;
        Ok(frame)
    }

    fn transmit_datagram<P: FramePort, const BLOCKS: usize>(
        &mut self,
        now: Instant,
        socket: u8,
        place: u8,
        serial: u64,
        port: &mut P,
        pool: &mut Pool<'id, SIZE, BLOCKS>,
    ) -> Result<(), Unsent<P::Refusal>> {
        let open = self.sockets.get(usize::from(socket)).ok_or(Unsent::Stale)?;
        let source_port = open.port.ok_or(Unsent::Stale)?;
        let entry = open
            .places
            .get(usize::from(place))
            .filter(|entry| entry.serial == serial)
            .ok_or(Unsent::Stale)?;
        let Run::Sending(sending) = &entry.run else {
            return Err(Unsent::Stale);
        };
        let next_hop = match sending.hop {
            Some(hop) => self.neighbors.hardware(now, hop).ok_or(Unsent::Stale)?,
            None => BROADCAST_HARDWARE,
        };
        let data = sending.block.octets();
        let udp_length = UDP_HEADER_LEN.saturating_add(data.len());
        let wire_length = u16::try_from(udp_length).map_err(|_| Unsent::Stale)?;
        let header = EthernetHeader {
            destination: next_hop,
            source: self.interface.hardware(),
            ether_type: EtherType::IPV4,
        };
        let ipv4 = Ipv4Fields {
            services: open.dscp.field(),
            total_length: u16::try_from(IPV4_HEADER_LEN.saturating_add(udp_length))
                .map_err(|_| Unsent::Stale)?,
            identification: self.identification,
            time_to_live: TIME_TO_LIVE,
            protocol: PROTOCOL_UDP,
            source: self.interface.address(),
            destination: sending.remote.address,
        };
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
        let udp = UdpHeader {
            checksum,
            ..unfilled
        };
        let total = HEADERS.saturating_add(data.len());
        publish(port, total, |slot| {
            let mut writer = Writer::new(slot);
            header
                .write(&mut writer)
                .and_then(|()| writer.write(&ipv4.to_bytes()))
                .and_then(|()| writer.write(&udp.to_bytes()))
                .and_then(|()| writer.write(data))
                .map_err(|_| SlotTooShort)
        })?;
        self.identification = self.identification.wrapping_add(1);
        if let Some(entry) = self
            .sockets
            .get_mut(usize::from(socket))
            .and_then(|open| open.places.get_mut(usize::from(place)))
            && let Run::Sending(_) = entry.run
            && let Run::Sending(sending) = core::mem::replace(&mut entry.run, Run::Free)
        {
            pool.give(sending.block);
            entry.finish(Outcome::Accepted);
        }
        Ok(())
    }
}

/// Publish through `port`, and tell a port that said `Ok` without writing.
fn publish<P: FramePort>(
    port: &mut P,
    length: usize,
    fill: impl FnOnce(&mut [u8]) -> Result<(), SlotTooShort>,
) -> Result<(), Unsent<P::Refusal>> {
    let mut filled = false;
    port.publish(length, |slot| {
        let written = fill(slot);
        filled = written.is_ok();
        written
    })
    .map_err(Unsent::Port)?;
    if filled {
        Ok(())
    } else {
        Err(Unsent::NotFilled)
    }
}

fn publish_frame<P: FramePort>(port: &mut P, frame: &[u8]) -> Result<(), Unsent<P::Refusal>> {
    publish(port, frame.len(), |slot| {
        Writer::new(slot).write(frame).map_err(|_| SlotTooShort)
    })
}
