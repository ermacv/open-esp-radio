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
use oer_protocols_network_port::{EthernetFrameView, FramePort, FrameSource, Ready, SlotTooShort};
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::Ipv4Header;
use oer_protocols_network_wire::transport::UdpHeader;

const BROADCAST: [u8; 6] = [0xff; 6];
/// The account of frames to everyone.
const BROADCAST_KEY: PeerKey = (u8::MAX, 0);

/// An association: a peer's number and its generation.
pub type PeerKey = (u8, u16);

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
    ticket: u32,
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
    depth: usize,
    now: u64,
    busy_until: u64,
    tickets: u32,
    refusals: u32,
    refused: u64,
    received: VecDeque<Vec<u8>>,
    notices: VecDeque<RadioNotice<u32, PeerKey>>,
    waker: Option<Waker>,
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

    fn fly(&mut self, frame: &[u8]) -> u32 {
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
        self.tickets = self.tickets.wrapping_add(1);
        let ticket = self.tickets;
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
        self.notices.push_back(RadioNotice::Ended(Exchange {
            ticket: flight.ticket,
            charged,
        }));
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
            depth,
            now: 0,
            busy_until: 0,
            tickets: 0,
            refusals: 0,
            refused: 0,
            received: VecDeque::new(),
            notices: VecDeque::new(),
            waker: None,
        })))
    }

    /// Move to `now`: frames that have ended land, and the port's waker hears.
    pub fn advance(&self, now: Instant) {
        let mut state = self.0.borrow_mut();
        state.now = now.as_micros();
        let mut landed = false;
        while state
            .flights
            .front()
            .is_some_and(|flight| flight.ends_at <= state.now)
        {
            if let Some(flight) = state.flights.pop_front() {
                state.land(flight);
                landed = true;
            }
        }
        if landed {
            state.wake();
        }
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
    type Ticket = u32;

    fn publish(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), SlotTooShort>,
    ) -> Result<u32, NoRoom> {
        let mut state = self.0.borrow_mut();
        if state.refusals > 0 {
            state.refusals = state.refusals.saturating_sub(1);
            state.refused = state.refused.saturating_add(1);
            return Err(NoRoom);
        }
        if state.flights.len() >= state.depth {
            return Err(NoRoom);
        }
        let mut frame = vec![0; length];
        fill(&mut frame).map_err(|_| NoRoom)?;
        Ok(state.fly(&frame))
    }
}

impl FrameSource for Medium {
    type Notice = RadioNotice<u32, PeerKey>;

    fn take_frame(&mut self, read: impl FnOnce(EthernetFrameView<'_>)) -> bool {
        let frame = self.0.borrow_mut().received.pop_front();
        frame.is_some_and(|frame| {
            if let Ok((header, payload)) = EthernetHeader::parse(&frame) {
                read(EthernetFrameView {
                    destination: header.destination,
                    source: header.source,
                    ether_type: header.ether_type.value(),
                    payload,
                });
            }
            true
        })
    }

    fn take_notice(&mut self) -> Option<Self::Notice> {
        self.0.borrow_mut().notices.pop_front()
    }

    fn register(&mut self, cx: &mut Context<'_>) {
        let mut state = self.0.borrow_mut();
        state.waker = Some(cx.waker().clone());
        if !state.received.is_empty() || !state.notices.is_empty() {
            cx.waker().wake_by_ref();
        }
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
}
