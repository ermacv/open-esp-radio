#![forbid(unsafe_code)]

//! One shared medium with several peers (see the crate's README).

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::task::{Context, Waker};

use oer_portable_time::Instant;
use oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeCandidate;
use oer_protocols_ieee80211_runtime::egress::{Exchange, PeerAir};
use oer_protocols_network_port::{FramePort, FrameSource, Ready, SlotTooShort};
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::Ipv4Header;
use oer_protocols_network_wire::transport::UdpHeader;

const BROADCAST: [u8; 6] = [0xff; 6];
/// The account of frames to everyone.
const BROADCAST_KEY: u8 = u8::MAX;

/// One peer's rate and addresses.
#[derive(Clone, Copy, Debug)]
pub struct PeerSpec {
    pub hardware: [u8; 6],
    pub address: [u8; 4],
    /// The cost of any frame.
    pub overhead_micros: u32,
    /// The cost of each octet.
    pub micros_per_octet: u32,
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
    ended: VecDeque<Exchange<u32>>,
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

    fn cost(&self, peer: Option<usize>, length: usize) -> u32 {
        // A frame to everyone goes at the slowest peer's rate.
        let (overhead, per_octet) = peer.and_then(|peer| self.peers.get(peer)).map_or_else(
            || {
                let slowest = self
                    .peers
                    .iter()
                    .map(|peer| (peer.spec.overhead_micros, peer.spec.micros_per_octet))
                    .max_by_key(|(_, per_octet)| *per_octet);
                slowest.unwrap_or((1, 1))
            },
            |peer| (peer.spec.overhead_micros, peer.spec.micros_per_octet),
        );
        let octets = u32::try_from(length).unwrap_or(u32::MAX);
        overhead.saturating_add(octets.saturating_mul(per_octet))
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
        self.ended.push_back(Exchange {
            ticket: flight.ticket,
            charged,
        });
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
            ended: VecDeque::new(),
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
    type Completion = Exchange<u32>;

    fn take_frame(&mut self, read: impl FnOnce(&[u8])) -> bool {
        let frame = self.0.borrow_mut().received.pop_front();
        frame.is_some_and(|frame| {
            read(&frame);
            true
        })
    }

    fn take_completion(&mut self) -> Option<Exchange<u32>> {
        self.0.borrow_mut().ended.pop_front()
    }

    fn register(&mut self, cx: &mut Context<'_>) {
        let mut state = self.0.borrow_mut();
        state.waker = Some(cx.waker().clone());
        if !state.received.is_empty() || !state.ended.is_empty() {
            cx.waker().wake_by_ref();
        }
    }
}

impl PeerAir for Medium {
    type Key = u8;

    fn candidate(&self, ready: &Ready) -> Option<AirtimeCandidate<u8>> {
        let state = self.0.borrow();
        let (key, peer) = if ready.peer == BROADCAST {
            (BROADCAST_KEY, None)
        } else {
            let peer = state.peer_of(ready.peer)?;
            if state.peers.get(peer)?.asleep {
                return None;
            }
            (u8::try_from(peer).ok()?, Some(peer))
        };
        Some(AirtimeCandidate {
            key,
            minimum_micros: NonZeroU32::new(state.cost(peer, ready.length))?,
        })
    }
}
