//! The task that owns the core.

use core::convert::Infallible;
use core::future::poll_fn;
use core::pin::pin;
use core::task::{Context, Poll};

use embassy_futures::select::select;
use embassy_futures::yield_now;
use oer_portable_events::EventSink;
use oer_portable_memory::{WakingSpscQueue, WakingSpscReceiver};
use oer_portable_time::{Duration, Instant, Timer};
use oer_protocols_network_core::{
    CommandId, CommandState, Eviction, Group, GroupsFull, NetEvent, NetworkOwner, OpenRefused,
    Outcome, Pool, Retired, SocketId, Unsent,
};
use oer_protocols_network_port::{Candidate, Egress, FramePort, FrameSource};
use oer_protocols_network_wire::ip::Dscp;

use crate::socket::{
    Answer, Inbound, LateSocket, Request, Sent, ServiceEnd, Socket, SocketLanes, drain_requests,
    lanes,
};

/// Commands one turn takes from a lane.
const TURN_COMMANDS: usize = 4;
/// Frames one turn takes in, and publishes.
const TURN_FRAMES: usize = 8;

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
    /// The port said it took a frame it never filled, so what it does is not
    /// known. The command that met this ends `Outcome::Unknown`, the others
    /// `Outcome::Retired`, and the port goes back to its owner in
    /// [`Ended::retired`].
    PortContract,
    /// The egress policy cannot go on: [`Ended::fault`] says why, and its
    /// account is in [`Ended::egress`].
    Egress,
}

/// What a service's end hands on.
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
> {
    pub cause: EndCause,
    pub retired: Retired<'m, B, SIZE, SOCKETS, CREDITS, BLOCKS, P>,
    /// The policy and its account, in-flight work included.
    pub egress: E,
    /// What ended the policy, with [`EndCause::Egress`].
    pub fault: Option<F>,
    /// One per socket bound, for what a socket dropped later holds.
    pub sockets: [Option<LateSocket<'a, 'm, B, SIZE, DEPTH, LANE>>; SOCKETS],
}

/// A socket the core refused, with its storage untouched for another try.
#[derive(Debug)]
pub struct BindRefused<'a, T> {
    pub reason: OpenRefused,
    pub storage: &'a mut T,
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
    socket: Option<SocketId<B>>,
    commands: [Option<CommandId<B>>; CREDITS],
    closing: Option<CommandId<B>>,
    /// A reservation waits for a free block.
    reserving: bool,
    /// A received datagram waits for room in the lane.
    blocked: bool,
    /// What this socket's reserved blocks count toward.
    group: Group<B>,
    /// The socket was dropped and its lanes are drained.
    abandoned: bool,
}

/// The network's task.
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
    /// Own `owner`. A service keeps its owners until [`Self::retire`] hands
    /// them on, so a cancelled [`Self::run`] loses none. Dropping a service
    /// instead is the end of the whole network: its port, pool and brand go,
    /// blocks in its lanes stay there unreachable and their memory is not
    /// reused, and waiting sockets and observers end.
    pub const fn new(
        owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>,
        egress: E,
        memory: MemoryPolicy,
        stop: WakingSpscReceiver<'a, (), 1>,
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
        }
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
    #[expect(clippy::type_complexity, reason = "the types of one socket's lanes")]
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
            socket: Some(id),
            commands: [const { None }; CREDITS],
            closing: None,
            reserving: false,
            blocked: false,
            abandoned: false,
        });
        Ok(socket)
    }

    /// Serve until a stop is asked or the port is at fault, and return why:
    /// [`Self::retire`] then ends the sockets and hands every owner on. This
    /// only drives the service, so cancelling it leaves all as a wait left it:
    /// `run` may start again, or the owner may retire. Once this ended, it
    /// serves no more.
    pub async fn run<T: Timer>(
        &mut self,
        clock: &T,
        events: &mut impl EventSink<NetEvent>,
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
                yield_now().await;
            } else {
                self.wait(clock).await;
            }
        }
    }

    /// One bounded pass over everything; whether anything moved.
    fn turn(&mut self, now: Instant, events: &mut impl EventSink<NetEvent>) -> bool {
        let mut moved = false;
        self.owner.tick(now);
        for index in 0..SOCKETS {
            moved |= self.commands(now, index);
        }
        moved |= self.give_blocks();
        for _ in 0..TURN_FRAMES {
            if !self.owner.take_frame(now, events) {
                break;
            }
            moved = true;
        }
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
                return true;
            }
        }
        // A policy chooses knowing all the port has said: a later notice may
        // free what the choice needs. So publishing waits for the next turn.
        if told_all {
            moved |= self.publish(now);
        } else {
            moved = true;
        }
        for bound in self.bound.iter_mut().flatten() {
            moved |= bound.hand_back(&mut self.owner);
        }
        moved |= self.relieve(now);
        moved
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
        let bound = self.bound.iter().flatten();
        let source = bound
            .clone()
            .map(|socket| (pool.held(socket.group), socket.group))
            .max_by_key(|(held, _)| *held);
        let Some((most, source)) = source else {
            return false;
        };
        let waiter = bound.clone().any(|socket| {
            socket.reserving
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

    fn commands(&mut self, now: Instant, index: usize) -> bool {
        let Some(bound) = self.bound.get_mut(index).and_then(Option::as_mut) else {
            return false;
        };
        if let Some(inbound) = bound.end.abandon.try_receive() {
            bound.abort(inbound, &mut self.owner);
            return true;
        }
        let mut moved = false;
        for _ in 0..TURN_COMMANDS {
            let Some(request) = bound.end.requests.try_receive() else {
                break;
            };
            moved = true;
            match request {
                Request::Reserve => bound.reserving = true,
                Request::Release(block) => self.owner.pool().give(block),
                Request::Send { tag, remote, block } => {
                    let Some(socket) = &bound.socket else {
                        self.owner.pool().give(block);
                        bound.ended(tag, Sent::Ended(Outcome::Closed));
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
                            bound.ended(tag, Sent::Refused(refused.reason));
                        }
                    }
                }
                Request::Close => {
                    if let Some(socket) = bound.socket.take() {
                        match self.owner.close(socket) {
                            Ok(close) => bound.closing = Some(close),
                            Err(_) => bound.answer(Answer::Closed),
                        }
                    }
                }
            }
        }
        moved
    }

    /// Give waiting sockets a block each: the group holding least first, and
    /// in turn within a group, the next pass starting after the last one served.
    /// This comes after all commands, so a socket's release gives it no place
    /// ahead of another.
    fn give_blocks(&mut self) -> bool {
        let first = self.next_block;
        let mut gave = false;
        while self.owner.pool().free() != 0 {
            let pool = self.owner.pool();
            let pool = &*pool;
            let bound = &self.bound;
            let next = (0..SOCKETS)
                .filter_map(|offset| {
                    let index = first.wrapping_add(offset).checked_rem(SOCKETS)?;
                    let socket = bound.get(index)?.as_ref()?;
                    (socket.reserving && pool.may_take(socket.group)).then_some((
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
    fn publish(&mut self, now: Instant) -> bool {
        let mut moved = false;
        for _ in 0..TURN_FRAMES {
            let chosen = match self.egress.choose(self.owner.demands(now)) {
                Ok(chosen) => chosen,
                Err(fault) => {
                    self.note(Err(fault));
                    break;
                }
            };
            let Some((demand, reservation)) = chosen else {
                break;
            };
            match self.owner.transmit(now, demand.grant()) {
                Ok(ticket) => {
                    moved = true;
                    let answer = self.egress.accepted(reservation, ticket);
                    self.note(answer);
                }
                Err(Unsent::Stale) => {
                    moved = true;
                    let answer = self.egress.refused(reservation);
                    self.note(answer);
                }
                Err(Unsent::Port(_)) => {
                    let answer = self.egress.refused(reservation);
                    self.note(answer);
                    break;
                }
                Err(Unsent::PortContract) => {
                    let answer = self.egress.unknown(reservation);
                    self.note(answer);
                    break;
                }
            }
            if self.fault.is_some() {
                break;
            }
        }
        moved
    }

    /// Wait for anything to do: the port, a lane, a stop or a time wakes this.
    async fn wait<T: Timer>(&mut self, clock: &T) {
        let deadline = match (self.owner.poll_deadline(), self.pressure_at) {
            (Some(core), Some(pressure)) => Some(core.min(pressure)),
            (core, pressure) => core.or(pressure),
        };
        let Self {
            owner, bound, stop, ..
        } = self;
        let mut registered = false;
        let ready = poll_fn(|cx| {
            // Any wake ends the wait: the turn looks at everything anyway.
            if core::mem::replace(&mut registered, true) {
                return Poll::Ready(());
            }
            let mut ready = pin!(stop.ready()).poll(cx).is_ready();
            owner.register_port(cx);
            for bound in bound.iter_mut().flatten() {
                ready |= bound.poll_lanes(cx);
            }
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
    ) -> Ended<'a, 'm, B, SIZE, SOCKETS, CREDITS, BLOCKS, DEPTH, LANE, P, E, E::Fault> {
        let cause = self.ended.unwrap_or(EndCause::Stopped);
        let mut retired = self.owner.retire();
        let sockets = self
            .bound
            .map(|bound| bound.map(|bound| bound.finish(&mut retired)));
        Ended {
            cause,
            retired,
            egress: self.egress,
            fault: self.fault,
            sockets,
        }
    }
}

impl<'a, 'm, B, const SIZE: usize, const DEPTH: usize, const LANE: usize, const CREDITS: usize>
    Bound<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS>
{
    /// Tell the socket. Its answer lane holds all it can be owed: one
    /// reservation at a time, a close, and a stop.
    fn answer(&mut self, answer: Answer<'a, 'm, B, SIZE, DEPTH>) {
        let _room_is_proven = self.end.answers.try_send(answer);
    }

    /// Answer a reservation, if a block is free.
    fn reserve<const BLOCKS: usize>(&mut self, pool: &mut Pool<'m, B, SIZE, BLOCKS>) -> bool {
        let Some(block) = pool.take_as(self.group) else {
            return false;
        };
        self.reserving = false;
        self.answer(Answer::Reserved(block));
        true
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
        if let Some(socket) = self.socket.take() {
            self.closing = owner.close(socket).ok();
        }
        inbound.drain(owner.pool());
        self.reserving = false;
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
                    self.answer(Answer::Closed);
                }
            }
        }
        self.blocked = false;
        if let Some(socket) = &self.socket {
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
