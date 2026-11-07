//! The one owner of a core, its pool and its port.

use core::task::Context;

use oer_portable_events::EventSink;
use oer_portable_time::Instant;
use oer_protocols_network_port::{FramePort, FrameSource};
use oer_protocols_network_wire::ip::Dscp;

use crate::block::{Block, CoreBrand, Group, Pool};
use crate::interface::Interface;
use crate::machine::{
    CommandId, CommandState, Core, Datagram, Demand, Eviction, Grant, NetEvent, OpenRefused,
    Queued, Remote, RetiredResults, SendRefused, SocketId, StaleSocket,
};

/// What a retired core hands on, each to its own owner.
#[derive(Debug)]
pub struct Retired<
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const BLOCKS: usize,
    P,
> {
    /// Its results, whenever their observers come.
    pub results: RetiredResults<B, SOCKETS, CREDITS>,
    pub pool: Pool<'m, B, SIZE, BLOCKS>,
    pub brand: CoreBrand<B>,
    pub port: P,
}

/// Why a grant sent nothing the core knows of.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unsent<R> {
    /// The granted work ended, or is no longer that work, its peer's address
    /// among it: a final refusal of this grant, not of the work, which a
    /// new demand shows again.
    Stale,
    /// The port refused; the work stays ready.
    Port(R),
    /// The port refused this frame for good ([`FramePort::is_final`]), so its
    /// work ended: a datagram's outcome is
    /// [`Outcome::Rejected`](crate::Outcome::Rejected).
    Rejected(R),
    /// The port said `Ok` though the frame was not written, which breaks
    /// [`FramePort::publish`]. What reached it is not known, so the work
    /// ended and is not repeated: a datagram's outcome is
    /// [`Outcome::Unknown`](crate::Outcome::Unknown). Every later `transmit`
    /// answers this too, for ARP as well, until the owner retires the core
    /// and takes the port back.
    PortContract,
}

/// The sole owner of a core, its pool and its port: the core takes and gives
/// values, and this one method joins a frame, the port's answer and the
/// core's commitment, so no frame leaves unknown to the core.
#[derive(Debug)]
pub struct NetworkOwner<
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
    core: Core<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS>,
    pool: Pool<'m, B, SIZE, BLOCKS>,
    port: P,
    /// The port contradicted itself: nothing is published until it is
    /// replaced, by a retirement.
    broken: bool,
}

impl<
    'm,
    B,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
    const BLOCKS: usize,
    P: FramePort,
> NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>
{
    pub const fn new(
        brand: CoreBrand<B>,
        interface: Interface,
        pool: Pool<'m, B, SIZE, BLOCKS>,
        port: P,
    ) -> Self {
        Self {
            core: Core::new(brand, interface),
            pool,
            port,
            broken: false,
        }
    }

    /// Send the granted work: the frame is written in the port's room, and
    /// the port's answer is committed before this returns.
    ///
    /// # Errors
    /// [`Unsent`].
    pub fn transmit(
        &mut self,
        now: Instant,
        grant: Grant<B>,
    ) -> Result<P::Ticket, Unsent<P::Refusal>> {
        if self.broken {
            return Err(Unsent::PortContract);
        }
        let frame = self.core.frame(now, &grant).ok_or(Unsent::Stale)?;
        let mut written = false;
        let published = self.port.publish(frame.length(), |slot| {
            let result = frame.write(slot);
            written = result.is_ok();
            result
        });
        let ticket = match published {
            Ok(ticket) => ticket,
            Err(refusal) if P::is_final(&refusal) => {
                self.core.reject(&grant, &mut self.pool);
                return Err(Unsent::Rejected(refusal));
            }
            Err(refusal) => return Err(Unsent::Port(refusal)),
        };
        self.core.commit(now, grant, written, &mut self.pool);
        if written {
            Ok(ticket)
        } else {
            self.broken = true;
            Err(Unsent::PortContract)
        }
    }

    /// Advance to `now`; see [`Core::tick`].
    pub fn tick(&mut self, now: Instant) {
        self.core.tick(now, &mut self.pool);
    }

    /// See [`Core::poll_deadline`].
    pub fn poll_deadline(&self) -> Option<Instant> {
        self.core.poll_deadline()
    }

    /// See [`Core::demands`].
    pub fn demands(&self, now: Instant) -> impl Iterator<Item = Demand<B>> + Clone + '_ {
        self.core.demands(now)
    }

    /// See [`Core::poll_demand`].
    pub fn poll_demand(&self, now: Instant) -> Option<Demand<B>> {
        self.core.poll_demand(now)
    }

    /// Take one received frame; see [`Core::input`].
    pub fn input(&mut self, now: Instant, frame: &[u8], events: &mut impl EventSink<NetEvent>) {
        self.core.input(now, frame, &mut self.pool, events);
    }

    /// Take one frame from the port, if one waits; whether one did.
    pub fn take_frame(&mut self, now: Instant, events: &mut impl EventSink<NetEvent>) -> bool
    where
        P: FrameSource,
    {
        let core = &mut self.core;
        let pool = &mut self.pool;
        self.port
            .take_frame(|frame| core.input_view(now, frame, pool, events))
    }

    /// End a burst of publications.
    pub fn flush(&mut self) {
        self.port.flush();
    }

    /// The port's next notice, for the egress policy.
    pub fn take_notice(&mut self) -> Option<P::Notice>
    where
        P: FrameSource,
    {
        self.port.take_notice()
    }

    /// Make `cx` the one the port wakes.
    pub fn register_port(&mut self, cx: &mut Context<'_>)
    where
        P: FrameSource,
    {
        self.port.register(cx);
    }

    /// See [`Core::open`].
    ///
    /// # Errors
    /// [`OpenRefused`].
    pub fn open(&mut self, port: u16, dscp: Dscp) -> Result<SocketId<B>, OpenRefused> {
        self.core.open(port, dscp)
    }

    /// See [`Core::send`].
    ///
    /// # Errors
    /// [`SendRefused`], with the block.
    pub fn send(
        &mut self,
        now: Instant,
        socket: &SocketId<B>,
        remote: Remote,
        block: Block<'m, B, SIZE>,
    ) -> Result<CommandId<B>, SendRefused<'m, B, SIZE>> {
        self.core.send(now, socket, remote, block)
    }

    /// See [`Core::receive`].
    ///
    /// # Errors
    /// [`StaleSocket`].
    pub fn receive(
        &mut self,
        socket: &SocketId<B>,
    ) -> Result<Option<Datagram<'m, B, SIZE>>, StaleSocket> {
        self.core.receive(socket)
    }

    /// See [`Core::close`].
    ///
    /// # Errors
    /// [`StaleSocket`].
    pub fn close(&mut self, socket: SocketId<B>) -> Result<CommandId<B>, StaleSocket> {
        self.core.close(socket, &mut self.pool)
    }

    /// See [`Core::open_in`].
    ///
    /// # Errors
    /// [`OpenRefused`].
    pub fn open_in(
        &mut self,
        port: u16,
        dscp: Dscp,
        group: Group<B>,
    ) -> Result<SocketId<B>, OpenRefused> {
        self.core.open_in(port, dscp, group)
    }

    /// See [`Core::queued`].
    pub fn queued(&self) -> impl Iterator<Item = Queued<B>> + '_ {
        self.core.queued()
    }

    /// See [`Core::evict`].
    pub fn evict(&mut self, queued: Queued<B>, eviction: Eviction) -> bool {
        self.core.evict(queued, eviction, &mut self.pool)
    }

    /// See [`Core::take_result`].
    pub fn take_result(&mut self, command: CommandId<B>) -> CommandState<B> {
        self.core.take_result(command)
    }

    /// See [`Core::drop_observer`].
    pub fn drop_observer(&mut self, command: CommandId<B>) {
        self.core.drop_observer(command);
    }

    /// The pool, for blocks to fill and blocks to give back.
    pub const fn pool(&mut self) -> &mut Pool<'m, B, SIZE, BLOCKS> {
        &mut self.pool
    }

    /// The port, to read.
    pub const fn port(&self) -> &P {
        &self.port
    }

    /// Whether the port contradicted itself, which stops publishing.
    pub const fn port_broken(&self) -> bool {
        self.broken
    }

    /// End the core: results, pool, the next core's brand and the port all
    /// come back. See [`Core::retire`].
    pub fn retire(self) -> Retired<'m, B, SIZE, SOCKETS, CREDITS, BLOCKS, P> {
        let Self {
            core,
            mut pool,
            port,
            ..
        } = self;
        let (results, brand) = core.retire(&mut pool);
        Retired {
            results,
            pool,
            brand,
            port,
        }
    }

    #[cfg(test)]
    pub(crate) const fn port_mut(&mut self) -> &mut P {
        &mut self.port
    }
}
