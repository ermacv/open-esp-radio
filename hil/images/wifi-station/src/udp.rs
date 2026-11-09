//! The load server's UDP over the network core: a channel socket that
//! receives and echoes, kept across runs, and a local producer that sends.
//! Both are bound at the group's start, on [`PORT`] and the one after it; the
//! network leases its address by DHCP, and a run starts once it holds one.
//!
//! The sender hands each datagram's header to the producer's [`Source`]; the
//! network's task asks the producer for it in its own turn and the producer
//! writes the whole payload, header and filler, straight into the transmit
//! buffer ([`Header::write_datagram`]). Each run is a generation of the
//! source: a datagram of an earlier run still waiting is declined, never
//! sent with a later run's data.

use core::cell::{Cell, RefCell, RefMut};
use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::task::{Context, Poll};

use embassy_futures::select::{Either, select};
use heapless::Vec;
use oer::wifi::{
    self, AtomicWaker, ClaimCell, Declined, Interest, Ipv4, LocalEnd, NetworkBlock, NetworkInstant,
    Next, Offer, Outcome, Produce, Received, Refusal, Refused, Remote, SendRefusal, Socket,
    WatchRefused, Work,
};
use oer_hil_services_load::header::{self, Header};
use oer_hil_services_load::{LoadError, Start};

/// The load port the channel socket holds; the producer sends from the one
/// after it. The image binds them at the group's start, before any run, so a
/// run names this one.
pub(crate) const PORT: u16 = 5001;

/// The ports of the network's sockets.
pub(crate) const PORTS: wifi::Ports = wifi::Ports {
    receiving: PORT,
    sending: PORT + 1,
};

/// Where a datagram goes.
#[derive(Clone, Copy)]
pub(crate) struct Peer {
    pub address: [u8; 4],
    pub port: u16,
}

impl From<Peer> for Remote {
    fn from(peer: Peer) -> Self {
        Self {
            address: peer.address,
            port: peer.port,
        }
    }
}

/// What became of a datagram offered now.
pub(crate) enum Attempt {
    Taken,
    /// No room: the datagram can wait for it.
    NoRoom,
    /// The datagram is lost.
    Lost,
}

/// Blocks no run could give back. A full lane can keep refusing across runs, so
/// no one run's end bounds them; the pool does, as a block is affine and this
/// has room for all of it.
type Parked = RefCell<Vec<NetworkBlock, { wifi::BLOCKS }>>;

/// Give `block` back, or park it for the next run: a full lane or a stop
/// refuses it, and a run's end has no time to wait.
fn home(socket: &mut Socket, parked: &Parked, block: NetworkBlock) {
    if let Err(Refused { block, .. }) = socket.release(block)
        && let Ok(mut parked) = parked.try_borrow_mut()
    {
        // Room for every block there is.
        let _kept = parked.push(block);
    }
}

/// Datagrams the sender handed over and the network has not ended: as many
/// as the network has works for the producer at once.
const QUEUE: usize = 32;

/// Names one datagram: its run's generation and its sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Tag {
    generation: u32,
    sequence: u64,
}

/// A datagram the sender handed over: what the producer writes, where.
#[derive(Clone, Copy)]
struct Pending {
    header: Header,
    size: usize,
    peer: Peer,
    /// The producer offered it, and has its answer to wait for.
    offered: bool,
}

/// What the sender and the producer share: the run's datagrams, oldest
/// first, until the network ends each. One whose end is not known
/// ([`Outcome::Unknown`]) leaves too, and is never sent again.
struct Datagrams {
    pending: Vec<Pending, QUEUE>,
}

/// The load generator's source of datagrams, which the sender writes and the
/// network's producer reads, each in one synchronous call at a time on the
/// group's hart: no lend is ever refused for another's.
pub(crate) struct Source {
    datagrams: ClaimCell<Datagrams>,
    /// The run's generation: a new run moves it, so a datagram of an earlier
    /// one still waiting is no longer current.
    generation: AtomicU32,
    /// The network's task waits here, for new datagrams while it can
    /// declare one ([`Interest::Offers`]) and for a new run always.
    network: AtomicWaker,
    /// The network last registered for offers, not only for changes.
    wants_offers: AtomicBool,
    /// A sender waits here for room.
    room: AtomicWaker,
    /// Wakes the source gave the network's task, for a trace's report.
    wakes: AtomicU32,
    /// Datagrams whose end is not known ([`Outcome::Unknown`]): they leave
    /// and are never sent again.
    unknown: AtomicU32,
}

impl Source {
    pub(crate) const fn new() -> Self {
        Self {
            datagrams: ClaimCell::new(),
            generation: AtomicU32::new(0),
            network: AtomicWaker::new(),
            wants_offers: AtomicBool::new(false),
            room: AtomicWaker::new(),
            wakes: AtomicU32::new(0),
            unknown: AtomicU32::new(0),
        }
    }

    /// Wake the network's task, if it waits.
    fn wake_network(&self) {
        if let Some(waker) = self.network.take() {
            self.wakes.fetch_add(1, Ordering::Relaxed);
            waker.wake();
        }
    }

    /// The wakes the source gave the network's task.
    pub(crate) fn wakes(&self) -> u32 {
        self.wakes.load(Ordering::Relaxed)
    }

    /// The datagrams whose end is not known.
    pub(crate) fn unknown(&self) -> u32 {
        self.unknown.load(Ordering::Relaxed)
    }

    /// Lend the datagrams to `use_them`; `None` before they are installed.
    fn lend<R>(&self, use_them: impl FnOnce(&mut Datagrams) -> R) -> Option<R> {
        self.datagrams.lend(use_them).ok()
    }

    /// Begin a run: its datagrams are a new generation, and what an earlier
    /// run left is not sent.
    fn begin(&self) {
        let _cleared = self.lend(|datagrams| datagrams.pending.clear());
        self.generation.fetch_add(1, Ordering::AcqRel);
        // The network's open works of the earlier run are stale now, whatever
        // it waits for.
        self.wake_network();
        self.room.wake();
    }

    /// Hand a datagram to the producer; whether there was room.
    fn push(&self, pending: Pending) -> bool {
        let taken = self
            .lend(|datagrams| datagrams.pending.push(pending).is_ok())
            .unwrap_or(false);
        // A datagram the network cannot declare yet is no reason to wake it:
        // what frees it comes in a turn of its own.
        if taken && self.wants_offers.load(Ordering::Acquire) {
            self.wake_network();
        }
        taken
    }
}

/// The network's producer of the load: it writes each datagram the sender
/// handed over into the slot the network grants it.
pub(crate) struct LoadProducer {
    source: &'static Source,
    /// The generation at the last registration.
    registered: u32,
}

impl LoadProducer {
    pub(crate) const fn new(source: &'static Source) -> Self {
        Self {
            source,
            registered: 0,
        }
    }

    /// The datagram of `tag` leaves, and its room is a sender's.
    fn forget(&self, tag: Tag) {
        if tag.generation == self.generation() {
            let _lent = self.source.lend(|datagrams| {
                datagrams
                    .pending
                    .retain(|pending| pending.header.sequence != tag.sequence);
            });
        }
        self.source.room.wake();
    }

    /// The datagram of `tag`, in its run.
    fn find(&self, tag: Tag, found: impl FnOnce(&mut Datagrams, usize)) -> bool {
        if tag.generation != self.generation() {
            return false;
        }
        self.source
            .lend(|datagrams| {
                let at = datagrams
                    .pending
                    .iter()
                    .position(|pending| pending.header.sequence == tag.sequence);
                at.map(|at| found(datagrams, at)).is_some()
            })
            .unwrap_or(false)
    }
}

impl Produce for LoadProducer {
    type Tag = Tag;

    fn generation(&self) -> u32 {
        self.source.generation.load(Ordering::Acquire)
    }

    fn current(&self, tag: Tag) -> bool {
        self.find(tag, |_, _| {})
    }

    fn offer(&mut self, _now: NetworkInstant) -> Result<Offer<Tag>, Next> {
        let generation = self.generation();
        self.source
            .lend(|datagrams| {
                let pending = datagrams
                    .pending
                    .iter_mut()
                    .find(|pending| !pending.offered)?;
                pending.offered = true;
                Some(Offer {
                    remote: Remote::from(pending.peer),
                    length: pending.size,
                    tag: Tag {
                        generation,
                        sequence: pending.header.sequence,
                    },
                })
            })
            .flatten()
            .ok_or(Next::Idle)
    }

    fn fill(
        &mut self,
        _now: NetworkInstant,
        work: Work<Tag>,
        data: &mut [u8],
    ) -> Result<(), Declined> {
        let mut written = None;
        let found = self.find(work.tag, |datagrams, at| {
            written = datagrams
                .pending
                .get(at)
                .map(|pending| pending.header.write_datagram(data).is_some());
        });
        match (found, written) {
            (true, Some(true)) => Ok(()),
            (true, _) => Err(Declined::Unwritable),
            (false, _) => Err(Declined::Stale),
        }
    }

    fn refused(&mut self, tag: Tag, reason: SendRefusal) {
        match reason {
            // It waits for what it lacks, and is offered again.
            SendRefusal::NoPlace | SendRefusal::NeighborsFull | SendRefusal::NoAddress => {
                let _found = self.find(tag, |datagrams, at| {
                    if let Some(pending) = datagrams.pending.get_mut(at) {
                        pending.offered = false;
                    }
                });
            }
            SendRefusal::TooLong | SendRefusal::NoRoute | SendRefusal::Stale => self.forget(tag),
        }
    }

    fn ended(&mut self, work: Work<Tag>, outcome: Outcome) {
        if outcome == Outcome::Unknown {
            self.source.unknown.fetch_add(1, Ordering::Relaxed);
        }
        self.forget(work.tag);
    }

    fn register(&mut self, cx: &mut Context<'_>, interest: Interest) {
        let offers = interest == Interest::Offers;
        self.source.wants_offers.store(offers, Ordering::Release);
        self.source.network.register(cx.waker());
        // Check again after registering: a new run since the last
        // registration, or, while the network can declare one, a datagram
        // not yet offered.
        let generation = self.generation();
        let moved = core::mem::replace(&mut self.registered, generation) != generation;
        let waiting = offers
            && self
                .source
                .lend(|datagrams| datagrams.pending.iter().any(|pending| !pending.offered))
                .unwrap_or(false);
        if moved || waiting {
            self.source.wake_network();
        }
    }
}

/// The network the load server opens its runs on.
pub(crate) struct Network {
    network: wifi::Network<LoadProducer, wifi::Queued>,
    /// The channel socket, which receives and echoes.
    socket: RefCell<Socket>,
    source: &'static Source,
    end: &'static LocalEnd,
    /// Datagrams the sockets delivered: this core counts no frames.
    delivered: Cell<u32>,
    /// Blocks an ended run could not give back. They outlive its adapters,
    /// which a deadline cancels at any await.
    parked: Parked,
    /// Where the network core's turns are kept ([`Turns`]), once the group
    /// gave it. The chip counts them; the host's tests of this facade have
    /// none.
    #[cfg(target_arch = "riscv32")]
    turns: Cell<Option<&'static mut Turns>>,
}

impl Network {
    /// The network, its channel socket, and the source its producer reads.
    pub(crate) fn new(
        network: wifi::Network<LoadProducer, wifi::Queued>,
        socket: Socket,
        source: &'static Source,
    ) -> Self {
        let _installed = source.datagrams.install(Datagrams {
            pending: Vec::new(),
        });
        Self {
            end: network.end(),
            network,
            socket: RefCell::new(socket),
            source,
            delivered: Cell::new(0),
            parked: RefCell::new(Vec::new()),
            #[cfg(target_arch = "riscv32")]
            turns: Cell::new(None),
        }
    }
}

/// The address the network holds now, as the service last published it.
///
/// # Errors
/// [`WatchRefused`]: a publication holds it at this moment, which no reader
/// on the network's hart meets: the service publishes within its own turn.
pub(crate) fn address(net: &Network) -> Result<Option<Ipv4>, WatchRefused> {
    Ok(net
        .network
        .configuration()?
        .and_then(|configuration| configuration.ipv4))
}

/// The network's publications, which the station services' waits read
/// (`crate::waits`).
#[cfg(target_arch = "riscv32")]
pub(crate) const fn published(net: &Network) -> &wifi::Network<LoadProducer, wifi::Queued> {
    &net.network
}

/// A run's sockets, once the network holds an address.
pub(crate) fn open<'a>(net: &'a Network, start: &Start) -> Result<Run<'a>, LoadError> {
    if start.port != PORT {
        return Err(LoadError::InvalidParameters);
    }
    address(net)
        .map_err(|_| LoadError::Busy)?
        .ok_or(LoadError::NoAddress)?;
    let socket = net.socket.try_borrow_mut().map_err(|_| LoadError::Busy)?;
    net.source.begin();
    Ok(Run {
        socket,
        source: net.source,
        end: net.end,
        delivered: &net.delivered,
        parked: &net.parked,
    })
}

/// The datagrams the sockets delivered.
pub(crate) fn received_frames(net: &Network) -> u32 {
    net.delivered.get()
}

/// No window of the network core's turns.
#[cfg(target_arch = "riscv32")]
const TURNS_NONE: oer_hil_services_load::NetworkTurns = oer_hil_services_load::NetworkTurns {
    available: false,
    turns: 0,
    reserves: 0,
    releases: 0,
    sends: 0,
    closes: 0,
    budget_spent: 0,
    budget_turns: 0,
    intake_full: 0,
    accepted: 0,
    accepted_by_turn: [0; oer_hil_services_load::ACCEPTED_BINS],
    publish_chose: 0,
    publish_refused: 0,
    publish_budget: 0,
    publish_deferred: 0,
    publish_fault: 0,
    blocks_out: 0,
    credits_out: 0,
    yields: 0,
    yields_with_work: 0,
    waits_pending: 0,
    waits_ready: 0,
    reserve_blocks: 0,
    reserve_by_size: [0; oer_hil_services_load::RESERVE_BINS],
    local_offered: 0,
    local_refused: 0,
    local_held: 0,
    local_declined: 0,
    local_withdrawn: 0,
    local_waits_offers: 0,
    local_waits_changes: 0,
    producer_wakes: 0,
    producer_unknown: 0,
};

/// The network core's turns at the open window's start, and the last closed
/// window, kept until the next one closes: in a static the group takes once,
/// as neither a run's futures, whose polls would copy them, nor the group's
/// stack should hold them.
#[cfg(target_arch = "riscv32")]
pub(crate) struct Turns {
    start: oer_hil_services_load::NetworkTurns,
    window: oer_hil_services_load::NetworkTurns,
}

/// The one place of the turns.
#[cfg(target_arch = "riscv32")]
pub(crate) static TURNS: static_cell::ConstStaticCell<Turns> =
    static_cell::ConstStaticCell::new(Turns {
        start: TURNS_NONE,
        window: TURNS_NONE,
    });

#[cfg(target_arch = "riscv32")]
impl Network {
    /// Keep the network core's turns in `turns`.
    pub(crate) fn keep_turns(&self, turns: &'static mut Turns) {
        self.turns.set(Some(turns));
    }

    /// Lend the place of the turns to `use_turns`, if the group gave one.
    fn turns<R: Default>(&self, use_turns: impl FnOnce(&mut Turns) -> R) -> R {
        let Some(turns) = self.turns.take() else {
            return R::default();
        };
        let result = use_turns(turns);
        self.turns.set(Some(turns));
        result
    }
}

/// Open a window of the network core's turns.
#[cfg(target_arch = "riscv32")]
pub(crate) fn open_turns(net: &Network) {
    net.turns(|turns| turns.start = network_turns(net.source));
}

/// Close the open window of the network core's turns, which
/// [`network_window`] answers from here on.
#[cfg(target_arch = "riscv32")]
pub(crate) fn close_turns(net: &Network) {
    net.turns(|turns| {
        turns.window =
            oer_hil_services_load::NetworkTurns::window(&turns.start, &network_turns(net.source));
    });
}

/// The last closed window of the network core's turns; none before one
/// closed.
#[cfg(target_arch = "riscv32")]
pub(crate) fn network_window(net: &Network) -> oer_hil_services_load::NetworkTurns {
    net.turns(|turns| turns.window)
}

/// What the network core's turns did since boot, as the load service names
/// it: the chip's observation, which the host's tests of this facade lack.
#[cfg(target_arch = "riscv32")]
fn network_turns(source: &Source) -> oer_hil_services_load::NetworkTurns {
    let turns = oer::observe::network_turns();
    oer_hil_services_load::NetworkTurns {
        available: true,
        turns: turns.turns,
        reserves: turns.reserves,
        releases: turns.releases,
        sends: turns.sends,
        closes: turns.closes,
        budget_spent: turns.budget_spent,
        budget_turns: turns.budget_turns,
        intake_full: turns.intake_full,
        accepted: turns.accepted,
        accepted_by_turn: turns.accepted_by_turn,
        publish_chose: turns.publish_chose,
        publish_refused: turns.publish_refused,
        publish_budget: turns.publish_budget,
        publish_deferred: turns.publish_deferred,
        publish_fault: turns.publish_fault,
        blocks_out: turns.blocks_out,
        credits_out: turns.credits_out,
        yields: turns.yields,
        yields_with_work: turns.yields_with_work,
        waits_pending: turns.waits_pending,
        waits_ready: turns.waits_ready,
        reserve_blocks: turns.reserve_blocks,
        reserve_by_size: turns.reserve_by_size,
        local_offered: turns.local_offered,
        local_refused: turns.local_refused,
        local_held: turns.local_held,
        local_declined: turns.local_declined,
        local_withdrawn: turns.local_withdrawn,
        local_waits_offers: turns.local_waits_offers,
        local_waits_changes: turns.local_waits_changes,
        producer_wakes: source.wakes(),
        producer_unknown: source.unknown(),
    }
}

/// A run's socket and the producer's source, the network's again when it
/// ends.
pub(crate) struct Run<'a> {
    socket: RefMut<'a, Socket>,
    source: &'static Source,
    end: &'static LocalEnd,
    delivered: &'a Cell<u32>,
    parked: &'a Parked,
}

impl Run<'_> {
    pub(crate) fn split(&mut self) -> (Receiving<'_>, Sending<'_>) {
        let receiving = &mut *self.socket;
        // What the last run could not give back: the lane has room by now.
        // One pass: a block refused again goes back behind the rest.
        let waiting = self.parked.try_borrow().map_or(0, |parked| parked.len());
        for _ in 0..waiting {
            let block = self
                .parked
                .try_borrow_mut()
                .ok()
                .and_then(|mut parked| parked.pop());
            if let Some(block) = block {
                home(receiving, self.parked, block);
            }
        }
        (
            Receiving {
                socket: receiving,
                delivered: self.delivered,
                parked: self.parked,
                next: None,
                returning: None,
                closed: false,
            },
            Sending {
                source: self.source,
                end: self.end,
            },
        )
    }
}

/// The receiving side of a run.
pub(crate) struct Receiving<'a> {
    socket: &'a mut Socket,
    delivered: &'a Cell<u32>,
    parked: &'a Parked,
    /// A datagram [`Self::ready`] found.
    next: Option<Received>,
    /// A block that found the socket's lane full.
    returning: Option<NetworkBlock>,
    closed: bool,
}

impl Drop for Receiving<'_> {
    fn drop(&mut self) {
        if let Some(datagram) = self.next.take() {
            home(self.socket, self.parked, datagram.block);
        }
        if let Some(block) = self.returning.take() {
            home(self.socket, self.parked, block);
        }
    }
}

impl<'a> Receiving<'a> {
    /// Wait for a datagram. A closed socket receives nothing more. A
    /// cancelled wait loses nothing: the socket's ends keep what they hold.
    pub(crate) async fn ready(&mut self) {
        if self.closed {
            core::future::pending::<()>().await;
        }
        if self.returning.is_some() {
            if self.socket.room().await.is_err() {
                self.closed = true;
                return;
            }
            self.release();
        }
        match self.socket.receive().await {
            Ok(datagram) => self.next = Some(datagram),
            Err(_) => self.closed = true,
        }
    }

    /// Give the block back, or keep it for [`Self::ready`].
    fn release(&mut self) {
        if let Some(block) = self.returning.take()
            && let Err(Refused { reason, block }) = self.socket.release(block)
        {
            // After a stop the pool's holder has what is left.
            self.closed = reason == Refusal::Closed;
            self.returning = Some(block);
        }
    }

    /// The next datagram here. One block at a time waits for the lane, so a
    /// waiting one holds the rest back until [`Self::ready`] gave it.
    pub(crate) fn take(&mut self) -> Option<Packet<'_, 'a>> {
        if self.returning.is_some() {
            self.release();
        }
        if self.returning.is_some() {
            return None;
        }
        let datagram = self.next.take().or_else(|| self.socket.try_receive())?;
        self.delivered.set(self.delivered.get().wrapping_add(1));
        Some(Packet {
            receiving: self,
            datagram: Some(datagram),
        })
    }
}

/// A received datagram; its block goes back when this ends.
pub(crate) struct Packet<'r, 'a> {
    receiving: &'r mut Receiving<'a>,
    /// `None` only once it went.
    datagram: Option<Received>,
}

impl Drop for Packet<'_, '_> {
    fn drop(&mut self) {
        if let Some(datagram) = self.datagram.take() {
            self.receiving.returning = Some(datagram.block);
            self.receiving.release();
        }
    }
}

impl Packet<'_, '_> {
    pub(crate) fn payload(&self) -> &[u8] {
        self.datagram
            .as_ref()
            .map_or(&[], |datagram| datagram.block.octets())
    }

    pub(crate) fn payload_mut(&mut self) -> &mut [u8] {
        let Some(datagram) = self.datagram.as_mut() else {
            return &mut [];
        };
        let length = datagram.block.octets().len();
        datagram.block.space().get_mut(..length).unwrap_or_default()
    }

    /// Send the payload back where it came from; whether the socket took it.
    pub(crate) fn reply(mut self) -> bool {
        let Some(datagram) = self.datagram.take() else {
            return false;
        };
        match self.receiving.socket.send(datagram.remote, datagram.block) {
            Ok(_result_given_up) => true,
            Err(Refused { block, .. }) => {
                self.receiving.returning = Some(block);
                false
            }
        }
    }
}

/// The sending side of a run: it hands each datagram's header to the
/// producer, which writes the datagram where it leaves.
pub(crate) struct Sending<'a> {
    source: &'a Source,
    end: &'a LocalEnd,
}

impl Sending<'_> {
    /// Offer `size` octets of the datagram `header` begins, without waiting.
    pub(crate) fn try_send(&mut self, size: usize, peer: Peer, header: Header) -> Attempt {
        // What no frame can carry, or no header fits, is lost here, as a
        // block too short for it is on the channel.
        if self.end.is_ended() || !(header::LENGTH..=wifi::BLOCK).contains(&size) {
            return Attempt::Lost;
        }
        let pending = Pending {
            header,
            size,
            peer,
            offered: false,
        };
        if self.source.push(pending) {
            Attempt::Taken
        } else {
            Attempt::NoRoom
        }
    }

    /// As [`Self::try_send`], once the producer has room; whether it took it.
    /// The network's end ends the wait too: the end flag wakes it.
    pub(crate) async fn send(&mut self, size: usize, peer: Peer, header: Header) -> bool {
        let end = self.end;
        let room = poll_fn(|cx| {
            self.source.room.register(cx.waker());
            match self.try_send(size, peer, header) {
                Attempt::Taken => Poll::Ready(true),
                Attempt::Lost => Poll::Ready(false),
                Attempt::NoRoom => Poll::Pending,
            }
        });
        match select(room, end.ended()).await {
            Either::First(taken) => taken,
            Either::Second(()) => false,
        }
    }
}
