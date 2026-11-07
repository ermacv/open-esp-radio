//! The one owner of a core, its pool and its port.

use oer_portable_events::EventSink;
use oer_portable_time::Instant;
use oer_protocols_network_wire::ip::Dscp;

use crate::block::{Block, CoreBrand, Pool};
use crate::interface::Interface;
use crate::machine::{
    CommandId, CommandState, Core, Datagram, Demand, Grant, NetEvent, OpenRefused, Remote,
    SendRefused, SlotTooShort, SocketId, StaleSocket,
};

/// A frame's way out.
pub trait FramePort {
    /// Why the port took nothing, which names what it waits for.
    type Refusal;

    /// Take room for `length` octets, let `fill` write the frame there and
    /// accept it. `Ok` says the frame is the port's.
    ///
    /// # Errors
    /// The port accepted nothing: it did not have the room, or `fill` failed.
    fn publish(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), SlotTooShort>,
    ) -> Result<(), Self::Refusal>;
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
    /// The port said `Ok` though the frame was not written, which breaks
    /// [`FramePort::publish`]. What reached it is not known, so the work
    /// ended and is not repeated: a datagram's outcome is
    /// [`Outcome::Unknown`](crate::Outcome::Unknown).
    PortContract,
}

/// The sole owner of a core, its pool and its port: the core takes and gives
/// values, and this one method joins a frame, the port's answer and the
/// core's commitment, so no frame leaves unknown to the core.
#[derive(Debug)]
pub struct NetworkOwner<
    'id,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
    const BLOCKS: usize,
    P,
> {
    core: Core<'id, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS>,
    pool: Pool<'id, SIZE, BLOCKS>,
    port: P,
}

impl<
    'id,
    const SIZE: usize,
    const SOCKETS: usize,
    const CREDITS: usize,
    const RECEIVED: usize,
    const NEIGHBORS: usize,
    const BLOCKS: usize,
    P: FramePort,
> NetworkOwner<'id, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>
{
    pub const fn new(
        brand: CoreBrand<'id>,
        interface: Interface,
        pool: Pool<'id, SIZE, BLOCKS>,
        port: P,
    ) -> Self {
        Self {
            core: Core::new(brand, interface),
            pool,
            port,
        }
    }

    /// Send the granted work: the frame is written in the port's room, and
    /// the port's answer is committed before this returns.
    ///
    /// # Errors
    /// [`Unsent`].
    pub fn transmit(&mut self, now: Instant, grant: Grant<'id>) -> Result<(), Unsent<P::Refusal>> {
        let frame = self.core.frame(now, &grant).ok_or(Unsent::Stale)?;
        let mut written = false;
        self.port
            .publish(frame.length(), |slot| {
                let result = frame.write(slot);
                written = result.is_ok();
                result
            })
            .map_err(Unsent::Port)?;
        self.core.commit(now, grant, written, &mut self.pool);
        if written {
            Ok(())
        } else {
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
    pub fn demands(&self, now: Instant) -> impl Iterator<Item = Demand<'id>> + '_ {
        self.core.demands(now)
    }

    /// See [`Core::poll_demand`].
    pub fn poll_demand(&mut self, now: Instant) -> Option<Demand<'id>> {
        self.core.poll_demand(now)
    }

    /// Take one received frame; see [`Core::input`].
    pub fn input(&mut self, now: Instant, frame: &[u8], events: &mut impl EventSink<NetEvent>) {
        self.core.input(now, frame, &mut self.pool, events);
    }

    /// See [`Core::open`].
    ///
    /// # Errors
    /// [`OpenRefused`].
    pub fn open(&mut self, port: u16, dscp: Dscp) -> Result<SocketId<'id>, OpenRefused> {
        self.core.open(port, dscp)
    }

    /// See [`Core::send`].
    ///
    /// # Errors
    /// [`SendRefused`], with the block.
    pub fn send(
        &mut self,
        now: Instant,
        socket: &SocketId<'id>,
        remote: Remote,
        block: Block<'id, SIZE>,
    ) -> Result<CommandId<'id>, SendRefused<'id, SIZE>> {
        self.core.send(now, socket, remote, block)
    }

    /// See [`Core::receive`].
    ///
    /// # Errors
    /// [`StaleSocket`].
    pub fn receive(
        &mut self,
        socket: &SocketId<'id>,
    ) -> Result<Option<Datagram<'id, SIZE>>, StaleSocket> {
        self.core.receive(socket)
    }

    /// See [`Core::close`].
    ///
    /// # Errors
    /// [`StaleSocket`].
    pub fn close(&mut self, socket: SocketId<'id>) -> Result<CommandId<'id>, StaleSocket> {
        self.core.close(socket, &mut self.pool)
    }

    /// See [`Core::take_result`].
    pub fn take_result(&mut self, command: CommandId<'id>) -> CommandState<'id> {
        self.core.take_result(command)
    }

    /// See [`Core::drop_observer`].
    pub fn drop_observer(&mut self, command: CommandId<'id>) {
        self.core.drop_observer(command);
    }

    /// The pool, for blocks to fill and blocks to give back.
    pub const fn pool(&mut self) -> &mut Pool<'id, SIZE, BLOCKS> {
        &mut self.pool
    }

    /// The port, to read.
    pub const fn port(&self) -> &P {
        &self.port
    }

    /// End the core. Its blocks return to the pool, which comes back with the
    /// next core's brand and the port. See [`Core::retire`].
    pub fn retire(self) -> (Pool<'id, SIZE, BLOCKS>, CoreBrand<'id>, P) {
        let Self {
            core,
            mut pool,
            port,
        } = self;
        let brand = core.retire(&mut pool);
        (pool, brand, port)
    }

    #[cfg(test)]
    pub(crate) const fn port_mut(&mut self) -> &mut P {
        &mut self.port
    }
}
