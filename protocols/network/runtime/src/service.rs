//! The task that owns the core.

use core::future::poll_fn;
use core::pin::pin;
use core::sync::atomic::Ordering;
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

use crate::socket::{Answer, Request, Sent, ServiceEnd, tag_bit};

/// Commands one turn takes from a lane.
const TURN_COMMANDS: usize = 4;
/// Frames one turn takes in, and publishes.
const TURN_FRAMES: usize = 8;

/// The lane that stops a service: whoever holds its sender asks once.
pub type StopLane = WakingSpscQueue<(), 1>;

/// A service that did not start, with its owner: pool, port and brand are not
/// lost with it. Sockets opened before the refusal stay open in the owner.
pub struct NotStarted<
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
    const BLOCKS: usize,
    P,
> {
    pub reason: OpenRefused,
    pub owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>,
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
    bound: [Bound<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS>; SOCKETS],
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
    /// Own `owner` and open a socket for each end.
    ///
    /// # Errors
    /// [`NotStarted`]: the core refused a socket.
    pub fn new(
        mut owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>,
        ends: [(u16, Dscp, ServiceEnd<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS>); SOCKETS],
        stop: WakingSpscReceiver<'a, (), 1>,
    ) -> Result<Self, NotStarted<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>>
    {
        let mut refused = None;
        let bound = ends.map(|(port, dscp, end)| {
            let socket = match owner.open(port, dscp) {
                Ok(socket) => Some(socket),
                Err(error) => {
                    refused = Some(error);
                    None
                }
            };
            Bound {
                end,
                socket,
                commands: [const { None }; CREDITS],
                closing: None,
                reserving: false,
                blocked: false,
            }
        });
        match refused {
            Some(reason) => Err(NotStarted { reason, owner }),
            None => Ok(Self { owner, bound, stop }),
        }
    }

    /// Serve until a stop is asked, and return what the core hands on.
    pub async fn run<T: Timer>(
        mut self,
        clock: &T,
        events: &mut impl EventSink<NetEvent>,
    ) -> Retired<'m, B, SIZE, SOCKETS, CREDITS, BLOCKS, P> {
        loop {
            if self.stop.try_receive().is_some() {
                return self.retire();
            }
            if self.turn(clock.now(), events) {
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
        for bound in &mut self.bound {
            moved |= bound.hand_back(&mut self.owner);
        }
        moved
    }

    fn commands(&mut self, now: Instant, index: usize) -> bool {
        let Some(bound) = self.bound.get_mut(index) else {
            return false;
        };
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
                        bound.ended(usize::from(tag), Sent::Ended(Outcome::SocketClosed));
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
                            bound.ended(usize::from(tag), Sent::Refused(refused.reason));
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
            for bound in bound.iter_mut() {
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

    /// Stop: nothing more is published, every known result goes to its
    /// socket, and so does the request lane, with what the service never saw.
    fn retire(self) -> Retired<'m, B, SIZE, SOCKETS, CREDITS, BLOCKS, P> {
        let mut retired = self.owner.retire();
        for bound in self.bound {
            bound.finish(&mut retired);
        }
        retired
    }
}

impl<'a, 'm, B, const SIZE: usize, const DEPTH: usize, const LANE: usize, const CREDITS: usize>
    Bound<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS>
{
    /// Tell the socket. `lanes` proved the lane's depth for all it can owe
    /// unread, and a tag or a reservation is reused only once it was read.
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

    /// The command of `tag` ended: its observer hears, or, gone, its tag frees.
    fn ended(&mut self, tag: usize, sent: Sent) {
        let tag = u8::try_from(tag).unwrap_or(u8::MAX);
        let bit = tag_bit(tag);
        if self.end.dropped.fetch_and(!bit, Ordering::AcqRel) & bit == 0 {
            self.answer(Answer::Ended { tag, sent });
        } else {
            self.answer(Answer::Forgotten { tag });
        }
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
            match owner.take_result(command) {
                CommandState::Running(command) => {
                    if let Some(place) = self.commands.get_mut(tag) {
                        *place = Some(command);
                    }
                }
                CommandState::Finished(outcome) => {
                    moved = true;
                    self.ended(tag, Sent::Ended(outcome));
                }
                CommandState::Gone => {
                    moved = true;
                    self.ended(tag, Sent::Ended(Outcome::Unknown));
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
        if self.blocked {
            ready |= pin!(self.end.datagrams.ready()).poll(cx).is_ready();
        }
        ready
    }

    /// The core retired: tell the socket what it decided, and hand it the
    /// lane the service stops reading.
    fn finish<const SOCKETS: usize, const BLOCKS: usize, P>(
        mut self,
        retired: &mut Retired<'m, B, SIZE, SOCKETS, CREDITS, BLOCKS, P>,
    ) {
        for tag in 0..CREDITS {
            if let Some(command) = self.commands.get_mut(tag).and_then(Option::take) {
                let sent = match retired.results.take(command) {
                    CommandState::Finished(outcome) => Sent::Ended(outcome),
                    CommandState::Running(_) | CommandState::Gone => Sent::Ended(Outcome::Unknown),
                };
                self.ended(tag, sent);
            }
        }
        let ServiceEnd {
            requests, answers, ..
        } = self.end;
        let _room_is_proven = answers.try_send(Answer::Stopped(requests));
    }
}
