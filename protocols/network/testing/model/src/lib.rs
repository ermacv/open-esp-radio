#![forbid(unsafe_code)]

//! One shared medium with several peers (see the crate's README).

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::task::{Context, Waker};

use oer_portable_time::Instant;
use oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeCandidate;
use oer_protocols_ieee80211_runtime::egress::{Exchange, PeerAir, RadioNotice};
use oer_protocols_network_port::{EthernetFrameView, FramePort, FrameSource, Ready, Unpublished};
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::Ipv4Header;
use oer_protocols_network_wire::transport::UdpHeader;

const BROADCAST: [u8; 6] = [0xff; 6];
/// The account of frames to everyone.
const BROADCAST_KEY: PeerKey = (u8::MAX, 0);

/// An association: a peer's number and its generation.
pub type PeerKey = (u8, u16);

/// Names a frame by its buffer's place at the port, below the port's depth,
/// as the station's buffers do: no other frame has it until the frame's
/// notice is taken. As wide as a place, so every place has its own.
pub type Ticket = usize;

/// One peer's rate and addresses.
#[derive(Clone, Copy, Debug)]
pub struct PeerSpec {
    pub hardware: [u8; 6],
    pub address: [u8; 4],
    /// The cost of any frame.
    pub overhead_micros: u32,
    /// The cost of each octet.
    pub micros_per_octet: u32,
    /// What the radio believes each octet costs.
    pub estimated_per_octet: u32,
    /// Extra attempts each frame takes, at full cost each.
    pub retries: u32,
    /// The most attempts past the first a frame can take.
    pub retry_limit: u32,
}

/// What the air carried.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Spent {
    pub air_micros: u64,
    pub frames: u64,
}

impl Spent {
    fn add(&mut self, air_micros: u32) {
        self.air_micros = self.air_micros.saturating_add(u64::from(air_micros));
        self.frames = self.frames.saturating_add(1);
    }

    /// What was spent since `earlier`.
    #[must_use]
    pub const fn since(self, earlier: Self) -> Self {
        Self {
            air_micros: self.air_micros.saturating_sub(earlier.air_micros),
            frames: self.frames.saturating_sub(earlier.frames),
        }
    }
}

/// The medium refused a frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NoRoom;

#[derive(Debug)]
struct Peer {
    spec: PeerSpec,
    generation: u16,
    asleep: bool,
    unknown_cost: bool,
    spent: Spent,
}

#[derive(Debug)]
struct Flight {
    ticket: Ticket,
    ends_at: u64,
    cost: u32,
    peer: Option<usize>,
    port: Option<u16>,
}

#[derive(Debug)]
struct State {
    local: [u8; 6],
    address: [u8; 4],
    peers: Vec<Peer>,
    broadcast: Spent,
    flows: BTreeMap<u16, Spent>,
    flights: VecDeque<Flight>,
    now: u64,
    busy_until: u64,
    /// Which places hold a frame whose notice was not taken: one place for
    /// each frame the port holds at once.
    held: Vec<bool>,
    refusals: u32,
    refused: u64,
    received: VecDeque<Vec<u8>>,
    /// Ends that landed, until a batch of them is told at once.
    landing: Vec<RadioNotice<Ticket, PeerKey>>,
    /// Ends told together: one, or a Block Ack's.
    batch: usize,
    notices: VecDeque<RadioNotice<Ticket, PeerKey>>,
    /// Notices taken.
    taken: u64,
    waker: Option<Waker>,
    /// A peer fell asleep or woke since the last registration: what the
    /// policy may choose changed, as a link change would.
    changed: bool,
}

impl State {
    fn wake(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }

    fn peer_of(&self, hardware: [u8; 6]) -> Option<usize> {
        self.peers
            .iter()
            .position(|peer| peer.spec.hardware == hardware)
    }

    /// A frame to everyone goes once, at the slowest peer's rate.
    fn slowest(&self) -> PeerSpec {
        let slowest = self
            .peers
            .iter()
            .map(|peer| peer.spec)
            .max_by_key(|spec| spec.micros_per_octet);
        slowest.unwrap_or(PeerSpec {
            hardware: BROADCAST,
            address: [0; 4],
            overhead_micros: 1,
            micros_per_octet: 1,
            estimated_per_octet: 1,
            retries: 0,
            retry_limit: 0,
        })
    }

    fn spec(&self, peer: Option<usize>) -> PeerSpec {
        let spec = peer.and_then(|peer| self.peers.get(peer));
        spec.map_or_else(
            || PeerSpec {
                retries: 0,
                retry_limit: 0,
                ..self.slowest()
            },
            |peer| peer.spec,
        )
    }

    /// What one attempt at `per_octet` costs.
    fn attempt(spec: &PeerSpec, per_octet: u32, length: usize) -> u32 {
        let octets = u32::try_from(length).unwrap_or(u32::MAX);
        spec.overhead_micros
            .saturating_add(octets.saturating_mul(per_octet))
    }

    /// What a frame really costs, its retries included.
    fn cost(&self, peer: Option<usize>, length: usize) -> u32 {
        let spec = self.spec(peer);
        Self::attempt(&spec, spec.micros_per_octet, length)
            .saturating_mul(spec.retries.saturating_add(1))
    }

    /// What the radio believes it will cost.
    fn estimate(&self, peer: Option<usize>, length: usize) -> u32 {
        let spec = self.spec(peer);
        Self::attempt(&spec, spec.estimated_per_octet, length)
    }

    /// The most it can cost.
    fn ceiling(&self, peer: Option<usize>, length: usize) -> u32 {
        let spec = self.spec(peer);
        let rate = spec.micros_per_octet.max(spec.estimated_per_octet);
        Self::attempt(&spec, rate, length).saturating_mul(spec.retry_limit.saturating_add(1))
    }

    /// Answer an ARP request for a peer's address, as that peer.
    fn answer(&mut self, payload: &[u8]) {
        let Some(request) = Arp::parse(payload).ok().flatten() else {
            return;
        };
        if request.operation != Operation::Request {
            return;
        }
        let peer = self
            .peers
            .iter()
            .find(|peer| peer.spec.address == request.target_protocol);
        let Some(peer) = peer.map(|peer| peer.spec) else {
            return;
        };
        let header = EthernetHeader {
            destination: self.local,
            source: peer.hardware,
            ether_type: EtherType::ARP,
        };
        let mut packet = [0; arp::PACKET_LEN];
        let reply = Arp {
            operation: Operation::Reply,
            sender_hardware: peer.hardware,
            sender_protocol: peer.address,
            target_hardware: self.local,
            target_protocol: self.address,
        };
        if reply.write(&mut packet).is_ok() {
            let mut frame = header.to_bytes().to_vec();
            frame.extend_from_slice(&packet);
            self.received.push_back(frame);
        }
    }

    /// The source port of a UDP frame: its flow.
    fn port_of(frame: &[u8]) -> Option<u16> {
        let (_, payload) = EthernetHeader::parse(frame).ok()?;
        let packet = Ipv4Header::parse(payload).ok()?;
        let (header, _) = UdpHeader::parse(packet.payload()).ok()?;
        Some(header.ports.source)
    }

    fn fly(&mut self, place: usize, frame: &[u8]) -> Ticket {
        let header = EthernetHeader::parse(frame).ok();
        let destination = header.as_ref().map(|(header, _)| header.destination);
        let peer = destination
            .filter(|hardware| *hardware != BROADCAST)
            .and_then(|hardware| self.peer_of(hardware));
        if let Some((header, payload)) = header
            && header.ether_type == EtherType::ARP
        {
            self.answer(payload);
        }
        let cost = self.cost(peer, frame.len());
        let start = self.now.max(self.busy_until);
        let ends_at = start.saturating_add(u64::from(cost));
        self.busy_until = ends_at;
        if let Some(held) = self.held.get_mut(place) {
            *held = true;
        }
        let ticket = place;
        self.flights.push_back(Flight {
            ticket,
            ends_at,
            cost,
            peer,
            port: Self::port_of(frame),
        });
        ticket
    }

    fn land(&mut self, flight: Flight) {
        let mut charged = NonZeroU32::new(flight.cost);
        match flight.peer.and_then(|peer| self.peers.get_mut(peer)) {
            Some(peer) => {
                peer.spent.add(flight.cost);
                if peer.unknown_cost {
                    charged = None;
                }
            }
            None => self.broadcast.add(flight.cost),
        }
        if let Some(port) = flight.port {
            self.flows.entry(port).or_default().add(flight.cost);
        }
        // The medium models no PHY rate.
        self.landing.push(RadioNotice::Ended(Exchange {
            ticket: flight.ticket,
            charged,
            rate: None,
        }));
    }

    /// Tell the landed ends once a batch of them landed or the air is idle:
    /// whether any were told.
    fn tell(&mut self) -> bool {
        if self.landing.is_empty() || (self.landing.len() < self.batch && !self.flights.is_empty())
        {
            return false;
        }
        self.notices.extend(self.landing.drain(..));
        true
    }
}

/// The port and the radio's view of its peers. Clones are one medium.
#[derive(Clone, Debug)]
pub struct Medium(Rc<RefCell<State>>);

impl Medium {
    /// A medium for a station of `local` and `address`, with `depth` frames at
    /// the port at most.
    #[must_use]
    pub fn new(local: [u8; 6], address: [u8; 4], depth: usize, peers: &[PeerSpec]) -> Self {
        Self(Rc::new(RefCell::new(State {
            local,
            address,
            peers: peers
                .iter()
                .map(|spec| Peer {
                    spec: *spec,
                    generation: 0,
                    asleep: false,
                    unknown_cost: false,
                    spent: Spent::default(),
                })
                .collect(),
            broadcast: Spent::default(),
            flows: BTreeMap::new(),
            flights: VecDeque::new(),
            now: 0,
            busy_until: 0,
            held: vec![false; depth],
            refusals: 0,
            refused: 0,
            received: VecDeque::new(),
            landing: Vec::new(),
            batch: 1,
            notices: VecDeque::new(),
            taken: 0,
            waker: None,
            changed: false,
        })))
    }

    /// Move to `now`: frames that have ended land, and the port's waker hears.
    pub fn advance(&self, now: Instant) {
        let mut state = self.0.borrow_mut();
        state.now = now.as_micros();
        while state
            .flights
            .front()
            .is_some_and(|flight| flight.ends_at <= state.now)
        {
            if let Some(flight) = state.flights.pop_front() {
                state.land(flight);
            }
        }
        if state.tell() {
            state.wake();
        }
    }

    /// From now on, the ends of `frames` landed frames are told together,
    /// or fewer once the air is idle: a Block Ack's settlements.
    pub fn batch(&self, frames: usize) {
        self.0.borrow_mut().batch = frames.max(1);
    }

    /// Notices the port handed on so far.
    #[must_use]
    pub fn notices_taken(&self) -> u64 {
        self.0.borrow().taken
    }

    /// When the frame on the air ends.
    #[must_use]
    pub fn next_landing(&self) -> Option<Instant> {
        let state = self.0.borrow();
        let flight = state.flights.front()?;
        Some(Instant::from_micros(flight.ends_at))
    }

    /// A peer falls asleep or wakes.
    pub fn set_asleep(&self, peer: usize, asleep: bool) {
        let mut state = self.0.borrow_mut();
        if let Some(peer) = state.peers.get_mut(peer) {
            peer.asleep = asleep;
        }
        state.changed = true;
        state.wake();
    }

    /// A peer's association ends and the next begins: its frames on the air
    /// still settle on the old one.
    pub fn leave(&self, peer: usize) {
        let mut state = self.0.borrow_mut();
        let number = u8::try_from(peer).unwrap_or(u8::MAX);
        if let Some(peer) = state.peers.get_mut(peer) {
            let key = (number, peer.generation);
            peer.generation = peer.generation.wrapping_add(1);
            state.notices.push_back(RadioNotice::Left(key));
        }
        state.wake();
    }

    /// From now on a peer's frames end with no known cost.
    pub fn set_unknown_cost(&self, peer: usize) {
        if let Some(peer) = self.0.borrow_mut().peers.get_mut(peer) {
            peer.unknown_cost = true;
        }
    }

    /// Refuse the next `count` frames.
    pub fn refuse(&self, count: u32) {
        self.0.borrow_mut().refusals = count;
    }

    /// What each peer's frames spent, in peer order.
    #[must_use]
    pub fn spent(&self) -> Vec<Spent> {
        let state = self.0.borrow();
        state.peers.iter().map(|peer| peer.spent).collect()
    }

    /// What frames to everyone spent.
    #[must_use]
    pub fn spent_on_everyone(&self) -> Spent {
        self.0.borrow().broadcast
    }

    /// What the frames of a source port spent.
    #[must_use]
    pub fn spent_by_flow(&self, port: u16) -> Spent {
        let state = self.0.borrow();
        state.flows.get(&port).copied().unwrap_or_default()
    }

    /// Frames refused so far.
    #[must_use]
    pub fn refused(&self) -> u64 {
        self.0.borrow().refused
    }
}

impl FramePort for Medium {
    type Refusal = NoRoom;
    type Ticket = Ticket;

    fn publish<E>(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), E>,
    ) -> Result<Ticket, Unpublished<NoRoom, E>> {
        let mut state = self.0.borrow_mut();
        if state.refusals > 0 {
            state.refusals = state.refusals.saturating_sub(1);
            state.refused = state.refused.saturating_add(1);
            return Err(Unpublished::Port(NoRoom));
        }
        // A buffer is free once its frame's notice was taken.
        let Some(place) = state.held.iter().position(|held| !*held) else {
            return Err(Unpublished::Port(NoRoom));
        };
        let mut frame = vec![0; length];
        fill(&mut frame).map_err(Unpublished::Fill)?;
        Ok(state.fly(place, &frame))
    }
}

impl FrameSource for Medium {
    type Notice = RadioNotice<Ticket, PeerKey>;
    /// The medium lends no buffer: each frame is its copy.
    type Held = oer_protocols_network_port::NeverHeld;

    fn take_frame(
        &mut self,
        read: impl FnOnce(
            EthernetFrameView<'_>,
            oer_protocols_network_port::Holding,
        ) -> oer_protocols_network_port::Keep,
    ) -> oer_protocols_network_port::Taken<Self::Held> {
        use oer_protocols_network_port::{CopyReason, Holding, Taken};
        let Some(frame) = self.0.borrow_mut().received.pop_front() else {
            return Taken::Empty;
        };
        if let Ok((header, payload)) = EthernetHeader::parse(&frame) {
            let _copied = read(
                EthernetFrameView {
                    destination: header.destination,
                    source: header.source,
                    ether_type: header.ether_type.value(),
                    payload,
                },
                Holding::Copied(CopyReason::Format),
            );
        }
        Taken::Read
    }

    fn take_notice(&mut self) -> Option<Self::Notice> {
        let mut state = self.0.borrow_mut();
        let notice = state.notices.pop_front()?;
        state.taken = state.taken.saturating_add(1);
        // The end frees its frame's buffer.
        if let RadioNotice::Ended(Exchange { ticket, .. }) | RadioNotice::Lost(ticket) = notice
            && let Some(held) = state.held.get_mut(ticket)
        {
            *held = false;
        }
        Some(notice)
    }

    /// The model's medium is up from the start, at one epoch.
    fn link(&self) -> oer_protocols_network_port::LinkSnapshot {
        oer_protocols_network_port::LinkSnapshot {
            state: oer_protocols_network_port::LinkState::Up,
            epoch: 1,
        }
    }

    fn register(&mut self, cx: &mut Context<'_>) -> bool {
        let mut state = self.0.borrow_mut();
        state.waker = Some(cx.waker().clone());
        let changed = core::mem::take(&mut state.changed);
        changed || !state.received.is_empty() || !state.notices.is_empty()
    }
}

impl PeerAir for Medium {
    type Key = PeerKey;

    fn candidate(&self, ready: &Ready) -> Option<AirtimeCandidate<PeerKey>> {
        let state = self.0.borrow();
        let (key, peer) = if ready.peer == BROADCAST {
            (BROADCAST_KEY, None)
        } else {
            let number = state.peer_of(ready.peer)?;
            let peer = state.peers.get(number)?;
            if peer.asleep {
                return None;
            }
            ((u8::try_from(number).ok()?, peer.generation), Some(number))
        };
        Some(AirtimeCandidate {
            key,
            minimum_micros: NonZeroU32::new(state.estimate(peer, ready.length))?,
        })
    }

    fn ceiling(&self, ready: &Ready) -> NonZeroU32 {
        let state = self.0.borrow();
        let ceiling = state.ceiling(state.peer_of(ready.peer), ready.length);
        NonZeroU32::new(ceiling).unwrap_or(NonZeroU32::MIN)
    }

    /// The medium's estimates do not depend on a rate, and it reports none.
    fn reported(&mut self, _key: PeerKey, _rate: oer_protocols_ieee80211_runtime::egress::PhyRate) {
    }
}

#[cfg(test)]
mod reviewer_ticket_width {
    use super::*;

    #[test]
    fn distinct_valid_buffer_places_have_distinct_tickets() {
        let last = usize::from(u16::MAX);
        let medium = Medium::new([2, 0, 0, 0, 0, 1], [192, 0, 2, 1], last + 2, &[]);
        let mut state = medium.0.borrow_mut();
        let frame = [0_u8; 14];
        let a = state.fly(last, &frame);
        let b = state.fly(last + 1, &frame);
        assert_ne!(a, b, "two distinct valid places must not alias one ticket");
    }
}
