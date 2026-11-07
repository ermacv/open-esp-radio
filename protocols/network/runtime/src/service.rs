//! The task that owns the core.

use core::future::poll_fn;
use core::pin::pin;
use core::task::{Context, Poll};

use embassy_futures::select::select;
use embassy_futures::yield_now;
use oer_portable_events::EventSink;
use oer_portable_memory::{WakingSpscQueue, WakingSpscReceiver};
use oer_portable_time::{Instant, Timer};
use oer_protocols_network_core::{
    CommandId, CommandState, FramePort, FrameSource, NetEvent, NetworkOwner, OpenRefused, Outcome,
    Pool, Retired, SocketId, Unsent,
};
use oer_protocols_network_wire::ip::Dscp;

use crate::socket::{
    Answer, Inbound, LateSocket, Request, Sent, ServiceEnd, Socket, SocketLanes, drain_requests,
    lanes,
};

/// Commands one turn takes from a lane.
const TURN_COMMANDS: usize = 4;
/// Frames one turn takes in, and publishes.
const TURN_FRAMES: usize = 8;

/// The lane that stops a service: whoever holds its sender asks once.
pub type StopLane = WakingSpscQueue<(), 1>;

/// Why a service ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndCause {
    /// A stop was asked.
    Stopped,
    /// The port said it took a frame it never filled, so what it does is not
    /// known. The command that met this ends `Outcome::Unknown`, the others
    /// `Outcome::Retired`, and the port goes back to its owner in
    /// [`Ended::retired`].
    PortContract,
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
> {
    pub cause: EndCause,
    pub retired: Retired<'m, B, SIZE, SOCKETS, CREDITS, BLOCKS, P>,
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
    P,
> {
    owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>,
    bound: [Option<Bound<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS>>; SOCKETS],
    stop: WakingSpscReceiver<'a, (), 1>,
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
> Service<'a, 'm, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, DEPTH, LANE, P>
{
    /// Own `owner`. Dropping a service drops its pool, so no block has an
    /// owner left; its sockets end and their observers read `Retired`.
    pub const fn new(
        owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>,
        stop: WakingSpscReceiver<'a, (), 1>,
    ) -> Self {
        Self {
            owner,
            bound: [const { None }; SOCKETS],
            stop,
        }
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
        let id = match self.owner.open(port, dscp) {
            Ok(id) => id,
            Err(reason) => return Err(BindRefused { reason, storage }),
        };
        let (socket, end) = lanes(storage);
        *place = Some(Bound {
            end,
            socket: Some(id),
            commands: [const { None }; CREDITS],
            closing: None,
            reserving: false,
            blocked: false,
            abandoned: false,
        });
        Ok(socket)
    }

    /// Serve until a stop is asked or the port is at fault, and return why,
    /// with what the core hands on.
    pub async fn run<T: Timer>(
        mut self,
        clock: &T,
        events: &mut impl EventSink<NetEvent>,
    ) -> Ended<'a, 'm, B, SIZE, SOCKETS, CREDITS, BLOCKS, DEPTH, LANE, P> {
        loop {
            if self.stop.try_receive().is_some() {
                return self.retire(EndCause::Stopped);
            }
            let moved = self.turn(clock.now(), events);
            if self.owner.port_broken() {
                return self.retire(EndCause::PortContract);
            }
            if moved {
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
        for _ in 0..TURN_FRAMES {
            if !self.owner.take_frame(now, events) {
                break;
            }
            moved = true;
        }
        moved |= self.publish(now);
        for bound in self.bound.iter_mut().flatten() {
            moved |= bound.hand_back(&mut self.owner);
        }
        moved
    }

    fn commands(&mut self, now: Instant, index: usize) -> bool {
        let Some(bound) = self.bound.get_mut(index).and_then(Option::as_mut) else {
            return false;
        };
        if let Some(inbound) = bound.end.abandon.try_receive() {
            bound.abort(inbound, &mut self.owner);
            return true;
        }
        let mut moved = bound.reserving && bound.reserve(self.owner.pool());
        for _ in 0..TURN_COMMANDS {
            let Some(request) = bound.end.requests.try_receive() else {
                break;
            };
            moved = true;
            match request {
                Request::Reserve => {
                    bound.reserving = true;
                    let _found = bound.reserve(self.owner.pool());
                }
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

    /// Publish what is ready, until the port has no room.
    fn publish(&mut self, now: Instant) -> bool {
        let mut moved = false;
        for _ in 0..TURN_FRAMES {
            let Some(demand) = self.owner.poll_demand(now) else {
                break;
            };
            match self.owner.transmit(now, demand.grant()) {
                Ok(()) | Err(Unsent::Stale) => moved = true,
                Err(Unsent::Port(_) | Unsent::PortContract) => break,
            }
        }
        moved
    }

    /// Wait for anything to do: the port, a lane, a stop or a time wakes this.
    async fn wait<T: Timer>(&mut self, clock: &T) {
        let deadline = self.owner.poll_deadline();
        let Self { owner, bound, stop } = self;
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
    /// and each socket gets the request lane, with what the service never saw.
    fn retire(
        self,
        cause: EndCause,
    ) -> Ended<'a, 'm, B, SIZE, SOCKETS, CREDITS, BLOCKS, DEPTH, LANE, P> {
        let mut retired = self.owner.retire();
        let sockets = self
            .bound
            .map(|bound| bound.map(|bound| bound.finish(&mut retired)));
        Ended {
            cause,
            retired,
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
        let Some(block) = pool.take() else {
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
