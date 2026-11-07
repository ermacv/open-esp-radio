//! The core: sockets, frames in, work out.
//!
//! SOURCE: RFC 791 and RFC 1122 for IPv4; RFC 768 for UDP; RFC 826 for ARP.

use oer_portable_events::{EventSink, report};
use oer_portable_time::Instant;
use oer_portable_wire::Writer;
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::ethernet::{self, EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{
    DsField, IPV4_HEADER_LEN, Ipv4Fields, Ipv4Header, Ipv4VersionIhl,
};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UDP_HEADER_LEN, UdpHeader};

use crate::block::{Block, Pool};
use crate::interface::{BROADCAST_HARDWARE, Interface};
use crate::neighbor::{Lookup, Neighbors};

/// RFC 1122, 3.2.1.7: a default time to live large enough for the Internet.
const TIME_TO_LIVE: u8 = 64;
/// Headers before a datagram's data.
const HEADERS: usize = ethernet::HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN;
/// An ARP frame.
const ARP_FRAME: usize = ethernet::HEADER_LEN + arp::PACKET_LEN;

/// A peer's address and port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Remote {
    pub address: [u8; 4],
    pub port: u16,
}

/// How work is served. A reserve for control work is its own: data never takes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    Control,
    Data,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Work {
    Reply,
    Request {
        neighbor: u8,
    },
    Datagram {
        socket: u8,
        place: u8,
        generation: u16,
    },
}

/// Work that is ready, before any frame of it exists.
#[derive(Debug, Eq, PartialEq)]
pub struct Demand {
    pub class: Class,
    /// The frame's length: what a slot must hold.
    pub length: usize,
    work: Work,
}

impl Demand {
    /// Choose this work as it is: a direct interface has no other policy.
    pub const fn grant(self) -> Grant {
        Grant { work: self.work }
    }
}

/// Permission to materialize one piece of work.
#[derive(Debug, Eq, PartialEq)]
pub struct Grant {
    work: Work,
}

/// A frame in the port's slot, waiting for the port's answer. It borrows the
/// core, so no frame is received and no other work is chosen before that
/// answer: an acceptance is never behind a frame that follows it, and the core
/// has no state for work at the port. Nothing moved yet, so a refusal, or
/// dropping this, leaves the work ready.
///
/// ```compile_fail,E0499
/// # use oer_portable_time::Instant;
/// # use oer_protocols_network_core::{Core, Grant, Pool};
/// # fn f<'m>(
/// #     core: &mut Core<'m, 1536, 1, 1, 1, 1>,
/// #     pool: &mut Pool<'m, 1536, 1>,
/// #     grant: Grant,
/// #     slot: &mut [u8],
/// #     now: Instant,
/// # ) {
/// let frame = core.materialize(grant, slot, pool).unwrap();
/// core.poll_demand();
/// frame.accepted(now);
/// # }
/// ```
#[must_use = "the port's answer decides who owns the work"]
#[derive(Debug)]
pub struct Materialized<
    'c,
    'm,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
    const BLOCKS: usize,
> {
    /// The frame's length in the slot.
    pub length: usize,
    work: Work,
    core: &'c mut Core<'m, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS>,
    pool: &'c mut Pool<'m, SIZE, BLOCKS>,
}

impl<
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
    const BLOCKS: usize,
> Materialized<'_, '_, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS>
{
    /// The port accepted the frame. A datagram's block returns and its
    /// outcome is written here.
    pub fn accepted(self, now: Instant) {
        self.core.accepted(now, self.work, self.pool);
    }

    /// The port refused the frame. The work stays ready; the runtime asks
    /// again once the port has room.
    pub fn refused(self) {}
}

/// Why no frame was written. Nothing moved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unmaterialized {
    /// The work ended since it was demanded.
    Stale,
    /// The slot does not hold the frame.
    Slot,
}

/// A socket. Its handle is not a copy: closing consumes it.
#[derive(Debug, Eq, PartialEq)]
pub struct SocketId {
    index: u8,
    generation: u16,
}

/// A running command's right to its one result.
#[derive(Debug, Eq, PartialEq)]
pub struct CommandId {
    socket: u8,
    socket_generation: u16,
    place: u8,
    generation: u16,
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
pub struct Datagram<'m, const SIZE: usize> {
    pub remote: Remote,
    pub block: Block<'m, SIZE>,
}

/// Why a socket did not open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenRefused {
    /// Every socket is in use, until one closes.
    NoSocket,
    /// Another socket holds the port, until it closes.
    PortInUse,
}

/// The socket is not this core's any more. A final refusal.
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
pub struct SendRefused<'m, const SIZE: usize> {
    pub reason: SendRefusal,
    pub block: Block<'m, SIZE>,
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
enum Phase {
    /// Its next hop has not answered.
    Waiting { hop: [u8; 4] },
    /// It can be materialized.
    Ready { hardware: [u8; 6] },
}

#[derive(Debug)]
struct Sending<'m, const SIZE: usize> {
    remote: Remote,
    block: Block<'m, SIZE>,
    phase: Phase,
}

#[derive(Debug)]
enum Run<'m, const SIZE: usize> {
    Free,
    Sending(Sending<'m, SIZE>),
    Finished(Outcome),
}

/// One command's place: what it runs and, after, its result.
#[derive(Debug)]
struct Place<'m, const SIZE: usize> {
    generation: u16,
    /// Someone holds the right to the result.
    observed: bool,
    run: Run<'m, SIZE>,
}

impl<const SIZE: usize> Place<'_, SIZE> {
    const fn new() -> Self {
        Self {
            generation: 0,
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
        self.generation = self.generation.wrapping_add(1);
    }
}

#[derive(Debug)]
struct Socket<'m, const SIZE: usize, const CREDITS: usize, const RECEIVED: usize> {
    generation: u16,
    /// The bound port; a socket being closed keeps its slot, not its port.
    port: Option<u16>,
    open: bool,
    places: [Place<'m, SIZE>; CREDITS],
    /// Held from the socket's creation: a close always has a place.
    close: Place<'m, SIZE>,
    received: heapless::Deque<Datagram<'m, SIZE>, RECEIVED>,
}

impl<'m, const SIZE: usize, const CREDITS: usize, const RECEIVED: usize>
    Socket<'m, SIZE, CREDITS, RECEIVED>
{
    const fn new() -> Self {
        Self {
            generation: 0,
            port: None,
            open: false,
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
    fn place_mut(&mut self, place: u8) -> Option<&mut Place<'m, SIZE>> {
        let place = usize::from(place);
        if place == CREDITS {
            Some(&mut self.close)
        } else {
            self.places.get_mut(place)
        }
    }
}

/// The owner of the network state.
#[derive(Debug)]
pub struct Core<
    'm,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
> {
    interface: Interface,
    neighbors: Neighbors<NEIGHBORS>,
    /// Replies owed: the asker's addresses. Bounded by the table: an asker
    /// past it asks again.
    replies: heapless::Deque<([u8; 6], [u8; 4]), NEIGHBORS>,
    sockets: [Socket<'m, SIZE, CREDITS, RECEIVED>; SOCKETS],
    /// Where the next look for a datagram starts, so sockets take turns.
    cursor: usize,
    identification: u16,
}

impl<
    'm,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
> Core<'m, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS>
{
    const FITS: () = {
        if SOCKETS > 255 || CREDITS >= 255 || NEIGHBORS > 255 {
            panic!("sockets, places and neighbors are numbered in one octet");
        }
        if SIZE < ARP_FRAME {
            panic!("a block holds at least an ARP frame");
        }
    };

    pub const fn new(interface: Interface) -> Self {
        const { Self::FITS };
        Self {
            interface,
            neighbors: Neighbors::new(),
            replies: heapless::Deque::new(),
            sockets: [const { Socket::new() }; SOCKETS],
            cursor: 0,
            identification: 0,
        }
    }

    /// Open a socket on `port`.
    ///
    /// # Errors
    /// [`OpenRefused`]: every socket is in use, or the port is held.
    pub fn open(&mut self, port: u16) -> Result<SocketId, OpenRefused> {
        if self.sockets.iter().any(|socket| socket.port == Some(port)) {
            return Err(OpenRefused::PortInUse);
        }
        let (index, socket) = self
            .sockets
            .iter_mut()
            .enumerate()
            .find(|(_, socket)| !socket.in_use())
            .ok_or(OpenRefused::NoSocket)?;
        let index = u8::try_from(index).map_err(|_| OpenRefused::NoSocket)?;
        socket.generation = socket.generation.wrapping_add(1);
        socket.port = Some(port);
        socket.open = true;
        Ok(SocketId {
            index,
            generation: socket.generation,
        })
    }

    fn socket_mut(
        &mut self,
        index: u8,
        generation: u16,
    ) -> Option<&mut Socket<'m, SIZE, CREDITS, RECEIVED>> {
        self.sockets
            .get_mut(usize::from(index))
            .filter(|socket| socket.generation == generation)
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
        socket: &SocketId,
        remote: Remote,
        block: Block<'m, SIZE>,
    ) -> Result<CommandId, SendRefused<'m, SIZE>> {
        let refused = |reason, block| Err(SendRefused { reason, block });
        let datagram = IPV4_HEADER_LEN
            .saturating_add(UDP_HEADER_LEN)
            .saturating_add(block.octets().len());
        if datagram > usize::from(self.interface.mtu()) {
            return refused(SendRefusal::TooLong, block);
        }
        let Some(open) = self.socket_mut(socket.index, socket.generation) else {
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
        let phase = if self.interface.is_broadcast(remote.address) {
            Phase::Ready {
                hardware: BROADCAST_HARDWARE,
            }
        } else {
            let Some(hop) = self.interface.next_hop(remote.address) else {
                return refused(SendRefusal::NoRoute, block);
            };
            match self.neighbors.resolve(now, hop) {
                Lookup::Known(hardware) => Phase::Ready { hardware },
                Lookup::Asking => Phase::Waiting { hop },
                Lookup::Full => return refused(SendRefusal::NeighborsFull, block),
            }
        };
        let Some(place) = self
            .socket_mut(socket.index, socket.generation)
            .and_then(|open| open.places.get_mut(free))
        else {
            return refused(SendRefusal::Stale, block);
        };
        place.observed = true;
        place.run = Run::Sending(Sending {
            remote,
            block,
            phase,
        });
        Ok(CommandId {
            socket: socket.index,
            socket_generation: socket.generation,
            place: number,
            generation: place.generation,
        })
    }

    /// The command's result, once it ended: its place frees. A command still
    /// running gives its right back.
    ///
    /// # Errors
    /// The command runs; the right comes back.
    pub fn take_result(&mut self, command: CommandId) -> Result<Outcome, CommandId> {
        let finished = self
            .socket_mut(command.socket, command.socket_generation)
            .and_then(|socket| socket.place_mut(command.place))
            .filter(|place| place.generation == command.generation)
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
    pub fn drop_observer(&mut self, command: CommandId) {
        if let Some(place) = self
            .socket_mut(command.socket, command.socket_generation)
            .and_then(|socket| socket.place_mut(command.place))
            .filter(|place| place.generation == command.generation)
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
        socket: &SocketId,
    ) -> Result<Option<Datagram<'m, SIZE>>, StaleSocket> {
        let open = self
            .socket_mut(socket.index, socket.generation)
            .ok_or(StaleSocket)?;
        Ok(open.received.pop_front())
    }

    /// Close `socket`, from a place held for this since its creation: a full
    /// command queue never stands in its way. Received datagrams go back to
    /// `pool` and waiting datagrams end `Closed`.
    pub fn close<const BLOCKS: usize>(
        &mut self,
        socket: SocketId,
        pool: &mut Pool<'m, SIZE, BLOCKS>,
    ) -> Result<CommandId, StaleSocket> {
        let open = self
            .socket_mut(socket.index, socket.generation)
            .filter(|open| open.open)
            .ok_or(StaleSocket)?;
        open.open = false;
        open.port = None;
        while let Some(datagram) = open.received.pop_front() {
            let _foreign = pool.give(datagram.block);
        }
        for place in &mut open.places {
            if let Run::Sending(_) = place.run
                && let Run::Sending(sending) = core::mem::replace(&mut place.run, Run::Free)
            {
                let _foreign = pool.give(sending.block);
                place.finish(Outcome::Closed);
            }
        }
        open.close.observed = true;
        open.close.run = Run::Finished(Outcome::SocketClosed);
        let generation = open.close.generation;
        let place = u8::try_from(CREDITS).map_err(|_| StaleSocket)?;
        Ok(CommandId {
            socket: socket.index,
            socket_generation: socket.generation,
            place,
            generation,
        })
    }

    /// Take one received frame. Its octets are read here and copied only
    /// when delivered, so the port's buffer returns with the call.
    pub fn input<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        frame: &[u8],
        pool: &mut Pool<'m, SIZE, BLOCKS>,
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
        let asked =
            self.neighbors
                .learn(now, packet.sender_protocol, packet.sender_hardware, for_us);
        if asked {
            self.neighbor_known(packet.sender_protocol, packet.sender_hardware);
        }
        if for_us
            && packet.operation == Operation::Request
            && self
                .replies
                .push_back((packet.sender_hardware, packet.sender_protocol))
                .is_err()
        {
            report(events, || NetEvent::ReplyDropped);
        }
    }

    /// The datagrams that waited for `address` can go.
    fn neighbor_known(&mut self, address: [u8; 4], hardware: [u8; 6]) {
        for socket in &mut self.sockets {
            for place in &mut socket.places {
                if let Run::Sending(sending) = &mut place.run
                    && matches!(sending.phase, Phase::Waiting { hop } if hop == address)
                {
                    sending.phase = Phase::Ready { hardware };
                }
            }
        }
    }

    fn input_ipv4<const BLOCKS: usize>(
        &mut self,
        payload: &[u8],
        pool: &mut Pool<'m, SIZE, BLOCKS>,
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
            let _foreign = pool.give(block);
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
            let _foreign = pool.give(datagram.block);
            report(events, || NetEvent::ReceiveFull);
        }
    }

    /// Ask neighbors again, and end what waited for one that never answered.
    pub fn tick<const BLOCKS: usize>(&mut self, now: Instant, pool: &mut Pool<'m, SIZE, BLOCKS>) {
        let sockets = &mut self.sockets;
        self.neighbors.tick(now, |address| {
            for socket in sockets.iter_mut() {
                for place in &mut socket.places {
                    let waits = matches!(
                        &place.run,
                        Run::Sending(Sending { phase: Phase::Waiting { hop }, .. })
                            if *hop == address
                    );
                    if waits
                        && let Run::Sending(sending) = core::mem::replace(&mut place.run, Run::Free)
                    {
                        let _foreign = pool.give(sending.block);
                        place.finish(Outcome::Unreachable);
                    }
                }
            }
        });
    }

    /// When [`Self::tick`] has work.
    pub fn poll_deadline(&self) -> Option<Instant> {
        self.neighbors.deadline()
    }

    /// The next ready work, with no frame built. Control work comes first;
    /// sockets take turns.
    pub fn poll_demand(&mut self) -> Option<Demand> {
        if !self.replies.is_empty() {
            return Some(Demand {
                class: Class::Control,
                length: ARP_FRAME,
                work: Work::Reply,
            });
        }
        if let Some((neighbor, _)) = self.neighbors.owed_request() {
            return Some(Demand {
                class: Class::Control,
                length: ARP_FRAME,
                work: Work::Request { neighbor },
            });
        }
        for offset in 0..SOCKETS {
            let index = self.cursor.wrapping_add(offset).checked_rem(SOCKETS)?;
            let socket = self.sockets.get(index)?;
            for (number, place) in socket.places.iter().enumerate() {
                if let Run::Sending(sending) = &place.run
                    && matches!(sending.phase, Phase::Ready { .. })
                {
                    self.cursor = index.wrapping_add(1);
                    return Some(Demand {
                        class: Class::Data,
                        length: HEADERS.saturating_add(sending.block.octets().len()),
                        work: Work::Datagram {
                            socket: u8::try_from(index).ok()?,
                            place: u8::try_from(number).ok()?,
                            generation: place.generation,
                        },
                    });
                }
            }
        }
        None
    }

    /// Write the granted work into `slot`, the port's memory. The work stays
    /// the core's until the port answers: nothing has been accepted yet.
    ///
    /// # Errors
    /// [`Unmaterialized`]: nothing moved.
    pub fn materialize<'c, const BLOCKS: usize>(
        &'c mut self,
        grant: Grant,
        slot: &mut [u8],
        pool: &'c mut Pool<'m, SIZE, BLOCKS>,
    ) -> Result<
        Materialized<'c, 'm, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS>,
        Unmaterialized,
    > {
        let length = match grant.work {
            Work::Reply => self.materialize_reply(slot)?,
            Work::Request { neighbor } => self.materialize_request(neighbor, slot)?,
            Work::Datagram {
                socket,
                place,
                generation,
            } => self.materialize_datagram(socket, place, generation, slot)?,
        };
        Ok(Materialized {
            length,
            work: grant.work,
            core: self,
            pool,
        })
    }

    fn write_arp(
        &self,
        operation: Operation,
        destination: [u8; 6],
        target_hardware: [u8; 6],
        target_protocol: [u8; 4],
        slot: &mut [u8],
    ) -> Result<usize, Unmaterialized> {
        let (head, rest) = slot
            .split_at_mut_checked(ethernet::HEADER_LEN)
            .ok_or(Unmaterialized::Slot)?;
        if rest.len() < arp::PACKET_LEN {
            return Err(Unmaterialized::Slot);
        }
        let header = EthernetHeader {
            destination,
            source: self.interface.hardware(),
            ether_type: EtherType::ARP,
        };
        header
            .write(&mut Writer::new(head))
            .map_err(|_| Unmaterialized::Slot)?;
        Arp {
            operation,
            sender_hardware: self.interface.hardware(),
            sender_protocol: self.interface.address(),
            target_hardware,
            target_protocol,
        }
        .write(rest)
        .map_err(|_| Unmaterialized::Slot)?;
        Ok(ARP_FRAME)
    }

    fn materialize_reply(&mut self, slot: &mut [u8]) -> Result<usize, Unmaterialized> {
        let (hardware, protocol) = *self.replies.front().ok_or(Unmaterialized::Stale)?;
        self.write_arp(Operation::Reply, hardware, hardware, protocol, slot)
    }

    fn materialize_request(
        &mut self,
        neighbor: u8,
        slot: &mut [u8],
    ) -> Result<usize, Unmaterialized> {
        let (_, address) = self
            .neighbors
            .owed_request()
            .filter(|(owed, _)| *owed == neighbor)
            .ok_or(Unmaterialized::Stale)?;
        self.write_arp(
            Operation::Request,
            BROADCAST_HARDWARE,
            [0; 6],
            address,
            slot,
        )
    }

    fn materialize_datagram(
        &mut self,
        socket: u8,
        place: u8,
        generation: u16,
        slot: &mut [u8],
    ) -> Result<usize, Unmaterialized> {
        let hardware = self.interface.hardware();
        let address = self.interface.address();
        let identification = self.identification;
        let open = self
            .sockets
            .get_mut(usize::from(socket))
            .ok_or(Unmaterialized::Stale)?;
        let port = open.port.ok_or(Unmaterialized::Stale)?;
        let entry = open
            .places
            .get_mut(usize::from(place))
            .filter(|entry| entry.generation == generation)
            .ok_or(Unmaterialized::Stale)?;
        let Run::Sending(sending) = &mut entry.run else {
            return Err(Unmaterialized::Stale);
        };
        let Phase::Ready { hardware: next_hop } = sending.phase else {
            return Err(Unmaterialized::Stale);
        };
        let data = sending.block.octets();
        let total = HEADERS.saturating_add(data.len());
        let frame = slot.get_mut(..total).ok_or(Unmaterialized::Slot)?;
        let udp_length = UDP_HEADER_LEN.saturating_add(data.len());
        let wire_length = u16::try_from(udp_length).map_err(|_| Unmaterialized::Slot)?;
        let header = EthernetHeader {
            destination: next_hop,
            source: hardware,
            ether_type: EtherType::IPV4,
        };
        let ipv4 = Ipv4Fields {
            services: DsField::from_bytes([0]),
            total_length: u16::try_from(IPV4_HEADER_LEN.saturating_add(udp_length))
                .map_err(|_| Unmaterialized::Slot)?,
            identification,
            time_to_live: TIME_TO_LIVE,
            protocol: PROTOCOL_UDP,
            source: address,
            destination: sending.remote.address,
        };
        let ports = Ports {
            source: port,
            destination: sending.remote.port,
        };
        let pseudo = ipv4_pseudo_header(address, sending.remote.address, PROTOCOL_UDP, wire_length);
        let unfilled = UdpHeader {
            ports,
            length: wire_length,
            checksum: 0,
        };
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
        let mut writer = Writer::new(frame);
        header
            .write(&mut writer)
            .and_then(|()| writer.write(&ipv4.to_bytes()))
            .and_then(|()| writer.write(&udp.to_bytes()))
            .and_then(|()| writer.write(data))
            .map_err(|_| Unmaterialized::Slot)?;
        self.identification = identification.wrapping_add(1);
        Ok(total)
    }

    fn accepted<const BLOCKS: usize>(
        &mut self,
        now: Instant,
        work: Work,
        pool: &mut Pool<'m, SIZE, BLOCKS>,
    ) {
        match work {
            Work::Reply => {
                let _sent = self.replies.pop_front();
            }
            Work::Request { neighbor } => self.neighbors.request_accepted(neighbor, now),
            Work::Datagram { socket, place, .. } => {
                if let Some(entry) = self
                    .sockets
                    .get_mut(usize::from(socket))
                    .and_then(|open| open.places.get_mut(usize::from(place)))
                    && let Run::Sending(_) = entry.run
                    && let Run::Sending(sending) = core::mem::replace(&mut entry.run, Run::Free)
                {
                    let _foreign = pool.give(sending.block);
                    entry.finish(Outcome::Accepted);
                }
            }
        }
    }
}
