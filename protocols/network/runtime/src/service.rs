//! The task that owns the core.

use core::convert::Infallible;
use core::future::poll_fn;
use core::pin::pin;
use core::task::{Context, Poll};

use embassy_futures::select::select;
use embassy_futures::yield_now;
use oer_portable_events::{EventSink, report};
use oer_portable_memory::{WakingSpscQueue, WakingSpscReceiver};
use oer_portable_time::{Duration, Instant, Timer};
use oer_protocols_network_core::{
    CommandId, CommandState, Eviction, Group, GroupsFull, NetEvent, NetworkOwner, OpenRefused,
    Outcome, Pool, Retired, SocketId, Unsent,
};
use oer_protocols_network_port::{Candidate, Egress, FramePort, FrameSource};
use oer_protocols_network_wire::ip::Dscp;

use crate::local::{LocalEnd, LocalRefused, Produce, Producer, Producers};
use crate::observation::{PublishEnd, ServiceEvent, TurnEnd, seen};
use crate::returns::ReturnEnd;
use crate::socket::{
    Answer, Inbound, LateSocket, Request, Sent, ServiceEnd, Socket, SocketLanes, drain_requests,
    lanes,
};

/// Commands one turn takes from a lane.
const TURN_COMMANDS: usize = 4;
/// Frames one turn takes in, and publishes; offers a producer is asked for in
/// one turn, and its declared works open at once.
pub(crate) const TURN_FRAMES: usize = 8;

/// The egress with no policy: the first ready work goes, no account is kept and
/// a completion says nothing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Direct;

impl<Ticket, Notice> Egress<Ticket, Notice> for Direct {
    type Reservation = ();
    type Fault = Infallible;

    fn choose<C: Candidate>(
        &mut self,
        mut ready: impl Iterator<Item = C> + Clone,
    ) -> Result<Option<(C, ())>, Infallible> {
        Ok(ready.next().map(|work| (work, ())))
    }

    fn accepted(&mut self, (): (), _ticket: Ticket) -> Result<(), Infallible> {
        Ok(())
    }

    fn refused(&mut self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn unknown(&mut self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn notice(&mut self, _notice: Notice) -> Result<(), Infallible> {
        Ok(())
    }
}

/// The lane that stops a service: whoever holds its sender asks once.
pub type StopLane = WakingSpscQueue<(), 1>;

/// What a service does when memory runs out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryPolicy {
    /// With a socket waiting for a block and none free, a datagram queued this
    /// long may be evicted: the oldest of the group holding the most, and more
    /// than the waiter's. `None` never evicts. This names pressure and no
    /// deadline: a datagram is only evicted while others wait.
    pub evict_after: Option<Duration>,
}

impl MemoryPolicy {
    pub const KEEP: Self = Self { evict_after: None };
}

/// Why a service ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndCause {
    /// A stop was asked, or the service's owner retired it.
    Stopped,
    /// The port's answer contradicted what its fill did, so what it does is
    /// not known. It said it took a frame it never filled, and the command
    /// that met this ends `Outcome::Unknown`; or it refused a frame whose fill
    /// failed, and that command ends by the fill's cause: `Outcome::Rejected`
    /// for a short slot, `Outcome::Declined` for a producer's decline.
    /// The others end `Outcome::Retired`, and the port goes back to its owner
    /// in [`Ended::retired`].
    PortContract,
    /// The egress policy cannot go on: [`Ended::fault`] says why, and its
    /// account is in [`Ended::egress`].
    Egress,
}

/// What a service's end hands on. Dropping it strands what is still on its way
/// home.
#[must_use = "it holds the pool, the port and the way home of every block"]
pub struct Ended<
    'a,
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const BLOCKS: usize,
    const DEPTH: usize,
    const LANE: usize,
    P,
    E,
    F,
    R = (),
> {
    pub cause: EndCause,
    pub retired: Retired<'m, B, SIZE, SOCKETS, CREDITS, BLOCKS, P>,
    /// The policy and its account, in-flight work included.
    pub egress: E,
    /// What ended the policy, with [`EndCause::Egress`].
    pub fault: Option<F>,
    /// One per socket bound, for what a socket dropped later holds.
    pub sockets: [Option<LateSocket<'a, 'm, B, SIZE, DEPTH, LANE>>; SOCKETS],
    /// Those of earlier services, adopted here, whose sockets have not gone.
    pub adopted: [Option<LateSocket<'a, 'm, B, SIZE, DEPTH, LANE>>; SOCKETS],
    /// The receiving end of the lane, for the next service. The lane closes
    /// only when this ends.
    pub returns: Option<ReturnEnd<'a, 'm, B, SIZE, BLOCKS>>,
    /// The producer, which heard every outcome of its works: `()` for a
    /// service without one.
    pub producers: R,
}

impl<
    'a,
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const BLOCKS: usize,
    const DEPTH: usize,
    const LANE: usize,
    P,
    E,
    F,
    R,
> Ended<'a, 'm, B, SIZE, SOCKETS, CREDITS, BLOCKS, DEPTH, LANE, P, E, F, R>
{
    /// With no service to come, close the way home for good: what it accepted
    /// comes to the pool, and a later `give` is refused.
    pub fn close_returns(&mut self) {
        if let Some(returns) = self.returns.take() {
            returns.close(&mut self.retired.pool);
        }
    }

    /// With no service to come, bring home what has arrived.
    pub fn collect(&mut self) {
        let pool = &mut self.retired.pool;
        if let Some(returns) = &self.returns {
            let _any = returns.take_home(pool);
        }
        for late in self.sockets.iter_mut().chain(&mut self.adopted) {
            if late.as_mut().is_some_and(|late| late.collect(pool)) {
                *late = None;
            }
        }
    }
}

/// A socket the core refused, with its storage untouched for another try.
#[derive(Debug)]
pub struct BindRefused<'a, T> {
    pub reason: OpenRefused,
    pub storage: &'a mut T,
}

/// A socket while it is open: only an open socket is owed blocks, so its
/// close ends what it was owed with it.
struct Open<B> {
    id: SocketId<B>,
    /// Blocks asked for and not yet granted.
    reserving: usize,
}

/// One socket as the service holds it.
struct Bound<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const LANE: usize,
    const CREDITS: usize,
> {
    end: ServiceEnd<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS>,
    open: Option<Open<B>>,
    commands: [Option<CommandId<B>>; CREDITS],
    closing: Option<CommandId<B>>,
    /// A received datagram waits for room in the lane.
    blocked: bool,
    /// What this socket's reserved blocks count toward.
    group: Group<B>,
    /// The socket was dropped and its lanes are drained.
    abandoned: bool,
}

/// The network's task. `L` is its local producer: `()` for none, at no
/// cost, or a [`Producer`] ([`Service::with_producer`]).
pub struct Service<
    'a,
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
    const BLOCKS: usize,
    const DEPTH: usize,
    const LANE: usize,
    P: FramePort + FrameSource,
    E: Egress<P::Ticket, P::Notice>,
    L = (),
> {
    owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>,
    egress: E,
    /// What ended the policy.
    fault: Option<E::Fault>,
    bound: [Option<Bound<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS>>; SOCKETS],
    stop: WakingSpscReceiver<'a, (), 1>,
    /// Why [`Self::run`] ended; it serves no more once this is set.
    ended: Option<EndCause>,
    /// The socket after the last one given a block: all wait in turn when
    /// blocks run out.
    next_block: usize,
    memory: MemoryPolicy,
    /// When the oldest possible victim may go, under pressure.
    pressure_at: Option<Instant>,
    /// Sockets of earlier services not yet dropped.
    adopted: [Option<LateSocket<'a, 'm, B, SIZE, DEPTH, LANE>>; SOCKETS],
    returns: Option<ReturnEnd<'a, 'm, B, SIZE, BLOCKS>>,
    local: L,
}

impl<
    'a,
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
    const BLOCKS: usize,
    const DEPTH: usize,
    const LANE: usize,
    P: FramePort + FrameSource,
    E: Egress<P::Ticket, P::Notice>,
> Service<'a, 'm, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, DEPTH, LANE, P, E>
{
    /// Own `owner`, with no local producer. A service keeps its owners until
    /// [`Self::retire`] hands them on, so a cancelled [`Self::run`] loses
    /// none. Dropping a service instead is the end of the whole network: its
    /// port, pool and brand go, blocks in its lanes stay there unreachable and
    /// their memory is not reused, and waiting sockets and observers end.
    pub const fn new(
        owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>,
        egress: E,
        memory: MemoryPolicy,
        stop: WakingSpscReceiver<'a, (), 1>,
    ) -> Self {
        Self::build(owner, egress, memory, stop, ())
    }
}

impl<
    'a,
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
    const BLOCKS: usize,
    const DEPTH: usize,
    const LANE: usize,
    P: FramePort + FrameSource,
    E: Egress<P::Ticket, P::Notice>,
    Q: Produce,
>
    Service<
        'a,
        'm,
        B,
        SIZE,
        SOCKETS,
        CREDITS,
        RECEIVED,
        NEIGHBORS,
        BLOCKS,
        DEPTH,
        LANE,
        P,
        E,
        Producer<'a, B, Q, CREDITS>,
    >
{
    /// As [`Self::new`], with `producer`, owned by value, which the service
    /// calls in its own turns once [`Self::bind_local`] gave it a socket.
    /// `end` is the image's flag the service's end sets: at its retirement,
    /// after every outcome its producer hears, or when it is dropped, with
    /// nothing called.
    pub const fn with_producer(
        owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>,
        egress: E,
        memory: MemoryPolicy,
        stop: WakingSpscReceiver<'a, (), 1>,
        producer: Q,
        end: &'a LocalEnd,
    ) -> Self {
        Self::build(owner, egress, memory, stop, Producer::new(producer, end))
    }

    /// Open the producer's socket on `port`, whose datagrams carry `dscp`. It
    /// only sends: a datagram for its port is dropped before any block. A
    /// refusal changes nothing.
    ///
    /// # Errors
    /// [`LocalRefused`].
    pub fn bind_local(&mut self, port: u16, dscp: Dscp) -> Result<(), LocalRefused> {
        if self.local.bound() {
            return Err(LocalRefused::Bound);
        }
        let socket = self
            .owner
            .open_local(port, dscp)
            .map_err(LocalRefused::Open)?;
        self.local.bind(socket);
        Ok(())
    }
}

impl<
    'a,
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
    const BLOCKS: usize,
    const DEPTH: usize,
    const LANE: usize,
    P: FramePort + FrameSource,
    E: Egress<P::Ticket, P::Notice>,
    L,
> Service<'a, 'm, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, DEPTH, LANE, P, E, L>
{
    const fn build(
        owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>,
        egress: E,
        memory: MemoryPolicy,
        stop: WakingSpscReceiver<'a, (), 1>,
        local: L,
    ) -> Self {
        Self {
            owner,
            egress,
            fault: None,
            bound: [const { None }; SOCKETS],
            stop,
            ended: None,
            next_block: 0,
            memory,
            pressure_at: None,
            adopted: [const { None }; SOCKETS],
            returns: None,
            local,
        }
    }
}

impl<
    'a,
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
    const BLOCKS: usize,
    const DEPTH: usize,
    const LANE: usize,
    P: FramePort + FrameSource,
    E: Egress<P::Ticket, P::Notice>,
    L: Producers<B, CREDITS>,
> Service<'a, 'm, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, DEPTH, LANE, P, E, L>
{
    /// Take blocks home through `returns`, for as long as this service holds
    /// the pool.
    ///
    /// # Errors
    /// A lane is attached already: replacing it would strand what it holds, so
    /// `returns` comes back.
    pub fn attach_returns(
        &mut self,
        returns: ReturnEnd<'a, 'm, B, SIZE, BLOCKS>,
    ) -> Result<(), ReturnEnd<'a, 'm, B, SIZE, BLOCKS>> {
        if self.returns.is_some() {
            return Err(returns);
        }
        self.returns = Some(returns);
        Ok(())
    }

    /// Hold an earlier service's `late`: when its socket is dropped, its blocks
    /// come to this pool, which it could not reach on its own while this runs.
    ///
    /// # Errors
    /// Every place is taken: `late` comes back to its holder.
    pub fn adopt(
        &mut self,
        late: LateSocket<'a, 'm, B, SIZE, DEPTH, LANE>,
    ) -> Result<(), LateSocket<'a, 'm, B, SIZE, DEPTH, LANE>> {
        match self.adopted.iter_mut().find(|place| place.is_none()) {
            Some(place) => {
                *place = Some(late);
                Ok(())
            }
            None => Err(late),
        }
    }

    /// Bring home what came by the return lane and from adopted sockets.
    fn take_home(&mut self) -> bool {
        let mut moved = false;
        let pool = self.owner.pool();
        if let Some(returns) = &self.returns {
            moved |= returns.take_home(pool);
        }
        for late in &mut self.adopted {
            if late.as_mut().is_some_and(|late| late.collect(pool)) {
                *late = None;
                moved = true;
            }
        }
        moved
    }

    /// A new group of at most `limit` blocks.
    ///
    /// # Errors
    /// [`GroupsFull`].
    pub fn group(&mut self, limit: usize) -> Result<Group<B>, GroupsFull> {
        self.owner.pool().group(limit)
    }

    /// Open a socket on `port`, and give the application its end: a socket
    /// exists only once its service holds the other. A refusal changes nothing.
    ///
    /// # Errors
    /// [`BindRefused`], with `storage`.
    pub fn bind(
        &mut self,
        port: u16,
        dscp: Dscp,
        group: Group<B>,
        storage: &'a mut SocketLanes<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS>,
    ) -> Result<
        Socket<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS>,
        BindRefused<'a, SocketLanes<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS>>,
    > {
        let Some(place) = self.bound.iter_mut().find(|place| place.is_none()) else {
            return Err(BindRefused {
                reason: OpenRefused::NoSocket,
                storage,
            });
        };
        let id = match self.owner.open_in(port, dscp, group) {
            Ok(id) => id,
            Err(reason) => return Err(BindRefused { reason, storage }),
        };
        let (socket, end) = lanes(storage);
        *place = Some(Bound {
            end,
            group,
            open: Some(Open { id, reserving: 0 }),
            commands: [const { None }; CREDITS],
            closing: None,
            blocked: false,
            abandoned: false,
        });
        Ok(socket)
    }

    /// Serve until a stop is asked or the port is at fault, and return why:
    /// [`Self::retire`] then ends the sockets and hands every owner on. This
    /// only drives the service, so cancelling it leaves all as a wait left it:
    /// `run` may start again, or the owner may retire. Once this ended, it
    /// serves no more. `events` hears the core's drops and the service's turns
    /// ([`ServiceEvent`]).
    pub async fn run<T: Timer>(
        &mut self,
        clock: &T,
        events: &mut (impl EventSink<NetEvent> + EventSink<ServiceEvent>),
    ) -> EndCause {
        loop {
            if let Some(cause) = self.ended {
                return cause;
            }
            if self.stop.try_receive().is_some() {
                self.ended = Some(EndCause::Stopped);
                continue;
            }
            let moved = self.turn(clock.now(), events);
            if self.owner.port_broken() {
                self.ended = Some(EndCause::PortContract);
            } else if self.fault.is_some() {
                self.ended = Some(EndCause::Egress);
            } else if moved {
                report(events, || ServiceEvent::Yielded);
                yield_now().await;
            } else {
                self.wait(clock, events).await;
            }
        }
    }

    /// One bounded pass over everything; whether anything moved. Every exit
    /// goes through its one epilogue, which hands every known result to its
    /// client, so at an await each open work truly was not published.
    fn turn(
        &mut self,
        now: Instant,
        events: &mut (impl EventSink<NetEvent> + EventSink<ServiceEvent>),
    ) -> bool {
        let mut moved = false;
        self.local.turn_begins();
        self.owner.tick(now);
        for index in 0..SOCKETS {
            moved |= self.commands(now, index, events);
        }
        moved |= self.take_home();
        moved |= self.give_blocks();
        let mut intake_full = true;
        for _ in 0..TURN_FRAMES {
            if !self.owner.take_frame(now, events) {
                intake_full = false;
                break;
            }
            moved = true;
        }
        seen(events, || intake_full.then_some(ServiceEvent::IntakeFull));
        let mut told_all = false;
        for _ in 0..TURN_FRAMES {
            let Some(notice) = self.owner.take_notice() else {
                told_all = true;
                break;
            };
            moved = true;
            let answer = self.egress.notice(notice);
            self.note(answer);
            if self.fault.is_some() {
                break;
            }
        }
        if self.fault.is_some() {
            // Nothing more is offered or published: the epilogue delivers.
            report(events, || ServiceEvent::Published(PublishEnd::Fault));
        } else if told_all {
            moved |= self.local.offers(now, &mut self.owner);
            moved |= self.publish(now, events);
        } else {
            // A policy chooses knowing all the port has said: a later notice
            // may free what the choice needs. So publishing waits for the
            // next turn.
            report(events, || ServiceEvent::Published(PublishEnd::Deferred));
            moved = true;
        }
        moved |= self.deliver();
        if self.fault.is_none() && !self.owner.port_broken() {
            moved |= self.relieve(now);
        }
        self.turn_ended(events);
        moved
    }

    /// The turn's epilogue: hand every result the core knows to its client,
    /// the channel sockets' places and the producer alike; whether any went.
    fn deliver(&mut self) -> bool {
        let mut moved = false;
        for bound in self.bound.iter_mut().flatten() {
            moved |= bound.hand_back(&mut self.owner);
        }
        moved |= self.local.deliver(&mut self.owner);
        moved
    }

    /// Tell `events` what was free when the turn ended.
    fn turn_ended(&mut self, events: &mut impl EventSink<ServiceEvent>) {
        seen(events, || {
            let blocks_free = self.owner.pool().free() != 0;
            let credits_free = self
                .bound
                .iter()
                .flatten()
                .filter(|bound| bound.open.is_some() && !bound.abandoned)
                .all(|bound| bound.end.credits.any_free());
            Some(ServiceEvent::TurnEnded(TurnEnd {
                blocks_free,
                credits_free,
            }))
        });
    }

    /// With none free and a socket waiting, evict the oldest queued datagram
    /// old enough, counted to the group holding the most, and more than the
    /// waiter's.
    fn relieve(&mut self, now: Instant) -> bool {
        self.pressure_at = None;
        let Some(after) = self.memory.evict_after else {
            return false;
        };
        let pool = self.owner.pool();
        if pool.free() != 0 {
            return false;
        }
        let pool = &*pool;
        // The pool's accounts, not the sockets': a group holds blocks with or
        // without a socket here.
        let Some((source, most)) = pool.heaviest() else {
            return false;
        };
        let waiter = self.bound.iter().flatten().any(|socket| {
            socket.waiting()
                && socket.group != source
                && pool.held(socket.group) < most
                && pool.may_take(socket.group)
        });
        if !waiter {
            return false;
        }
        let oldest = self
            .owner
            .queued()
            .filter(|queued| queued.group == source)
            .min_by_key(|queued| queued.since);
        let Some(oldest) = oldest else {
            return false;
        };
        let due = oldest.since.saturating_add(after);
        if due > now {
            self.pressure_at = Some(due);
            return false;
        }
        self.owner.evict(oldest, Eviction::MemoryPressure)
    }

    fn commands(
        &mut self,
        now: Instant,
        index: usize,
        events: &mut impl EventSink<ServiceEvent>,
    ) -> bool {
        let Some(bound) = self.bound.get_mut(index).and_then(Option::as_mut) else {
            return false;
        };
        if let Some(inbound) = bound.end.abandon.try_receive() {
            bound.abort(inbound, &mut self.owner);
            return true;
        }
        let mut moved = false;
        let mut spent = true;
        for _ in 0..TURN_COMMANDS {
            let Some(request) = bound.end.requests.try_receive() else {
                spent = false;
                break;
            };
            moved = true;
            report(events, || ServiceEvent::Took(request.kind()));
            match request {
                // Demand only: each block is granted on its own, and counts
                // toward the group when it is.
                Request::Reserve(batch) => {
                    if let Some(open) = &mut bound.open {
                        open.reserving = open.reserving.saturating_add(batch.blocks());
                    }
                }
                Request::Release(block) => self.owner.pool().give(block),
                Request::Send { tag, remote, block } => {
                    let Some(Open { id: socket, .. }) = &bound.open else {
                        self.owner.pool().give(block);
                        bound.unsent(tag, Sent::Ended(Outcome::Closed));
                        continue;
                    };
                    match self.owner.send(now, socket, remote, block) {
                        Ok(command) => {
                            if let Some(place) = bound.commands.get_mut(usize::from(tag)) {
                                *place = Some(command);
                            }
                        }
                        Err(refused) => {
                            self.owner.pool().give(refused.block);
                            bound.unsent(tag, Sent::Refused(refused.reason));
                        }
                    }
                }
                // What the socket was owed ends with it; what it was
                // granted stays its own.
                Request::Close => {
                    if let Some(Open { id, .. }) = bound.open.take() {
                        match self.owner.close(id) {
                            Ok(close) => bound.closing = Some(close),
                            Err(_) => bound.closed(),
                        }
                    }
                }
            }
        }
        seen(events, || {
            (spent && !bound.end.requests.is_empty()).then_some(ServiceEvent::BudgetSpent)
        });
        moved
    }

    /// Give waiting sockets a block each: the group holding least first, and
    /// in turn within a group, each block starting after the last one served.
    /// Each block is chosen anew, so how many a socket asked for gives it no
    /// place ahead of another. This comes after all commands, so a socket's
    /// release gives it no place ahead of another either.
    fn give_blocks(&mut self) -> bool {
        let mut gave = false;
        while self.owner.pool().free() != 0 {
            // Each block from the cursor the last grant moved: sockets of
            // one group take turns within a pass too, however much one asks.
            let first = self.next_block;
            let pool = self.owner.pool();
            let pool = &*pool;
            let bound = &self.bound;
            let next = (0..SOCKETS)
                .filter_map(|offset| {
                    let index = first.wrapping_add(offset).checked_rem(SOCKETS)?;
                    let socket = bound.get(index)?.as_ref()?;
                    (socket.waiting() && pool.may_take(socket.group)).then_some((
                        pool.held(socket.group),
                        offset,
                        index,
                    ))
                })
                .min();
            let Some((_, _, index)) = next else {
                break;
            };
            let Some(bound) = self.bound.get_mut(index).and_then(Option::as_mut) else {
                break;
            };
            gave |= bound.reserve(self.owner.pool());
            self.next_block = index.wrapping_add(1);
        }
        gave
    }

    /// Keep the first fault of the policy.
    fn note(&mut self, answer: Result<(), E::Fault>) {
        if let Err(fault) = answer {
            self.fault.get_or_insert(fault);
        }
    }

    /// Publish what the policy chooses, until it or the port has no room. No
    /// await falls between a choice and its answer, so no cancellation can
    /// leave a reservation unanswered.
    fn publish(&mut self, now: Instant, events: &mut impl EventSink<ServiceEvent>) -> bool {
        let mut moved = false;
        let mut end = PublishEnd::Budget;
        for _ in 0..TURN_FRAMES {
            // After a declining fill, the producer's flow sits out the turn.
            let filtered = self.local.filtered();
            let ready = self
                .owner
                .demands(now)
                .filter(move |demand| filtered.is_none_or(|flow| demand.ready.flow != Some(flow)));
            let chosen = match self.egress.choose(ready) {
                Ok(chosen) => chosen,
                Err(fault) => {
                    self.note(Err(fault));
                    end = PublishEnd::Fault;
                    break;
                }
            };
            let Some((demand, reservation)) = chosen else {
                end = PublishEnd::Chose;
                break;
            };
            let local = &mut self.local;
            let fill = |work, data: &mut [u8]| local.fill(now, work, data);
            match self.owner.transmit(now, demand.grant(), fill) {
                Ok(ticket) => {
                    moved = true;
                    report(events, || ServiceEvent::Accepted);
                    let answer = self.egress.accepted(reservation, ticket);
                    self.note(answer);
                }
                Err(Unsent::Stale | Unsent::Rejected(_) | Unsent::SlotTooShort) => {
                    moved = true;
                    let answer = self.egress.refused(reservation);
                    self.note(answer);
                }
                // One declining fill a turn: the producer's flow is out of
                // the rest of it, so its declines never stop another's work.
                Err(Unsent::Declined) => {
                    moved = true;
                    self.local.declined();
                    let answer = self.egress.refused(reservation);
                    self.note(answer);
                }
                // The port took nothing, by its word, and is at fault: the
                // work ended by its fill's cause, and the service ends.
                Err(Unsent::FillMisreported(_)) => {
                    moved = true;
                    let answer = self.egress.refused(reservation);
                    self.note(answer);
                    end = PublishEnd::Fault;
                    break;
                }
                Err(Unsent::Port(_)) => {
                    let answer = self.egress.refused(reservation);
                    self.note(answer);
                    end = PublishEnd::Refused;
                    break;
                }
                Err(Unsent::PortContract) => {
                    let answer = self.egress.unknown(reservation);
                    self.note(answer);
                    end = PublishEnd::Fault;
                    break;
                }
            }
            if self.fault.is_some() {
                end = PublishEnd::Fault;
                break;
            }
        }
        report(events, || ServiceEvent::Published(end));
        if moved {
            self.owner.flush();
        }
        moved
    }

    /// Wait for anything to do: the port, a lane, a stop or a time wakes this.
    async fn wait<T: Timer>(&mut self, clock: &T, events: &mut impl EventSink<ServiceEvent>) {
        let deadline = [
            self.owner.poll_deadline(),
            self.pressure_at,
            self.local.deadline(),
        ]
        .into_iter()
        .flatten()
        .min();
        let Self {
            owner,
            bound,
            stop,
            adopted,
            returns,
            local,
            ..
        } = self;
        let mut registered = false;
        let ready = poll_fn(|cx| {
            // Any wake ends the wait: the turn looks at everything anyway.
            if core::mem::replace(&mut registered, true) {
                return Poll::Ready(());
            }
            let mut ready = pin!(stop.ready()).poll(cx).is_ready();
            owner.register_port(cx);
            local.register(cx);
            for bound in bound.iter_mut().flatten() {
                ready |= bound.poll_lanes(cx);
            }
            for late in adopted.iter_mut().flatten() {
                ready |= pin!(late.abandon.ready()).poll(cx).is_ready();
            }
            if let Some(returns) = returns {
                ready |= returns.poll_waiting(cx);
            }
            report(events, || ServiceEvent::Waited { ready });
            if ready {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        });
        match deadline {
            Some(deadline) => {
                let _first = select(ready, clock.wait_until(deadline)).await;
            }
            None => ready.await,
        }
    }

    /// End: nothing more is published, every known result goes to its place,
    /// each socket gets the request lane, with what the service never saw, and
    /// every owner comes back. Its owner calls this after [`Self::run`] ended
    /// or was cancelled.
    pub fn retire(
        self,
    ) -> Ended<'a, 'm, B, SIZE, SOCKETS, CREDITS, BLOCKS, DEPTH, LANE, P, E, E::Fault, L::Returned>
    {
        let cause = self.ended.unwrap_or(EndCause::Stopped);
        let mut this = self;
        while this.take_home() {}
        let adopted = this.adopted;
        let returns = this.returns;
        let mut retired = this.owner.retire();
        let sockets = this
            .bound
            .map(|bound| bound.map(|bound| bound.finish(&mut retired)));
        let producers = this.local.retire(&mut retired.results);
        Ended {
            cause,
            retired,
            egress: this.egress,
            fault: this.fault,
            sockets,
            adopted,
            returns,
            producers,
        }
    }
}

impl<'a, 'm, B, const SIZE: usize, const DEPTH: usize, const LANE: usize, const CREDITS: usize>
    Bound<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS>
{
    /// Tell the socket. Its answer lane holds all it can be owed: the
    /// reservations it may ask ahead, a close, and a stop.
    fn answer(&mut self, answer: Answer<'a, 'm, B, SIZE, DEPTH>) {
        let _room_is_proven = self.end.answers.try_send(answer);
    }

    /// Whether the open socket is owed a block.
    fn waiting(&self) -> bool {
        self.open.as_ref().is_some_and(|open| open.reserving != 0)
    }

    /// Grant the open socket one block it is owed, if one is free: only now
    /// does the block count toward the group.
    fn reserve<const BLOCKS: usize>(&mut self, pool: &mut Pool<'m, B, SIZE, BLOCKS>) -> bool {
        let Some(open) = self.open.as_mut().filter(|open| open.reserving != 0) else {
            return false;
        };
        let Some(block) = pool.take_as(self.group) else {
            return false;
        };
        open.reserving = open.reserving.saturating_sub(1);
        self.answer(Answer::Reserved(block));
        true
    }

    /// The socket's close ended.
    ///
    /// Out of line, as [`Self::unsent`]: inlined into a turn, its wake and
    /// another ending's were merged into one call whose debug scope no
    /// longer names the waker method the image's stack proof needs.
    #[cold]
    #[inline(never)]
    fn closed(&mut self) {
        self.answer(Answer::Closed);
    }

    /// A datagram of `tag` the core never took ended with `sent`.
    ///
    /// Out of line, as [`Self::closed`].
    #[cold]
    #[inline(never)]
    fn unsent(&self, tag: u8, sent: Sent) {
        self.ended(tag, sent);
    }

    /// The command of `tag` ended.
    fn ended(&self, tag: u8, sent: Sent) {
        if let Some(place) = self.end.places.get(usize::from(tag)) {
            place.end(sent, self.end.credits, tag);
        }
    }

    /// The socket was dropped: its accepted commands end, and every block
    /// in its lanes goes back to the pool.
    fn abort<
        const SOCKETS: usize,
        const RECEIVED: usize,
        const NEIGHBORS: usize,
        const BLOCKS: usize,
        P: FramePort,
    >(
        &mut self,
        inbound: Inbound<'a, 'm, B, SIZE, DEPTH, LANE>,
        owner: &mut NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>,
    ) {
        let this = &*self;
        drain_requests(&this.end.requests, owner.pool(), |tag| {
            this.ended(tag, Sent::Ended(Outcome::Closed));
        });
        if let Some(Open { id, .. }) = self.open.take() {
            self.closing = owner.close(id).ok();
        }
        inbound.drain(owner.pool());
        self.blocked = false;
        self.abandoned = true;
    }

    /// Hand results and datagrams back; whether any went.
    fn hand_back<
        const SOCKETS: usize,
        const RECEIVED: usize,
        const NEIGHBORS: usize,
        const BLOCKS: usize,
        P: FramePort,
    >(
        &mut self,
        owner: &mut NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>,
    ) -> bool {
        let mut moved = false;
        for tag in 0..CREDITS {
            let Some(command) = self.commands.get_mut(tag).and_then(Option::take) else {
                continue;
            };
            let tag_byte = u8::try_from(tag).unwrap_or(u8::MAX);
            match owner.take_result(command) {
                CommandState::Running(command) => {
                    if let Some(place) = self.commands.get_mut(tag) {
                        *place = Some(command);
                    }
                }
                CommandState::Finished(outcome) => {
                    moved = true;
                    self.ended(tag_byte, Sent::Ended(outcome));
                }
                CommandState::Gone => {
                    moved = true;
                    self.ended(tag_byte, Sent::Ended(Outcome::Unknown));
                }
            }
        }
        if let Some(close) = self.closing.take() {
            match owner.take_result(close) {
                CommandState::Running(close) => self.closing = Some(close),
                CommandState::Finished(_) | CommandState::Gone => {
                    moved = true;
                    self.closed();
                }
            }
        }
        self.blocked = false;
        if let Some(Open { id: socket, .. }) = &self.open {
            while let Some(permit) = self.end.datagrams.try_reserve() {
                let Ok(Some(datagram)) = owner.receive(socket) else {
                    return moved;
                };
                permit.send(datagram);
                moved = true;
            }
            self.blocked = true;
        }
        moved
    }

    /// Whether a lane has something for the turn; `cx` hears the next.
    fn poll_lanes(&mut self, cx: &mut Context<'_>) -> bool {
        let mut ready = pin!(self.end.requests.ready()).poll(cx).is_ready();
        if !self.abandoned {
            ready |= pin!(self.end.abandon.ready()).poll(cx).is_ready();
        }
        if self.blocked {
            ready |= pin!(self.end.datagrams.ready()).poll(cx).is_ready();
        }
        ready
    }

    /// The core retired: write what it decided into the places, say so, and
    /// hand on the lanes the service stops reading.
    fn finish<const SOCKETS: usize, const BLOCKS: usize, P>(
        self,
        retired: &mut Retired<'m, B, SIZE, SOCKETS, CREDITS, BLOCKS, P>,
    ) -> LateSocket<'a, 'm, B, SIZE, DEPTH, LANE> {
        let Self {
            end,
            mut commands,
            abandoned,
            ..
        } = self;
        for (tag, place) in (0_u8..).zip(end.places.iter()) {
            let command = commands.get_mut(usize::from(tag)).and_then(Option::take);
            if let Some(command) = command {
                let sent = match retired.results.take(command) {
                    CommandState::Finished(outcome) => Sent::Ended(outcome),
                    CommandState::Running(_) | CommandState::Gone => Sent::Ended(Outcome::Unknown),
                };
                place.end(sent, end.credits, tag);
            }
        }
        let ServiceEnd {
            requests,
            answers,
            abandon,
            ending,
            ..
        } = end;
        if let Some(inbound) = abandon.try_receive() {
            drain_requests(&requests, &mut retired.pool, |_| {});
            inbound.drain(&mut retired.pool);
        } else if !abandoned {
            let _room_is_proven = answers.try_send(Answer::Stopped(requests));
        }
        drop(ending);
        LateSocket { abandon }
    }
}
