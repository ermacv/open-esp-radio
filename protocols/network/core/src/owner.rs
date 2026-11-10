//! The one owner of a core, its pool and its port.

use core::task::Context;

use oer_portable_events::{EventSink, report};
use oer_portable_time::Instant;
use oer_protocols_network_port::{FramePort, FrameSource, Keep, Taken, Unpublished};
use oer_protocols_network_wire::ip::Dscp;

use crate::block::{Block, CoreBrand, Group, Pool};
use crate::diagnostics::{Count, counted};
use crate::interface::Interface;
use crate::loans::{Loan, NotLent};
use crate::machine::{
    CommandId, CommandState, Configuration, Core, Datagram, Declined, Demand, Ends, Eviction,
    FillFailed, Grant, NetEvent, OpenRefused, Outcome, Queued, Remote, RetiredResults, SendRefusal,
    SendRefused, SocketId, StaleSocket, WorkId,
};
use crate::payload::Payload;

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
    T = (),
> {
    /// Its results, whenever their observers come.
    pub results: RetiredResults<B, SOCKETS, CREDITS, T>,
    pub pool: Pool<'m, B, SIZE, BLOCKS>,
    pub brand: CoreBrand<B>,
    pub port: P,
}

/// A frame the port took: its ticket, and the declared work it ended, with
/// its producer tag, for that work's producer.
#[derive(Debug, Eq, PartialEq)]
pub struct Transmitted<K, T> {
    pub ticket: K,
    /// The declared work that ended `Accepted` with this frame.
    pub ended: Option<(WorkId, T)>,
}

/// What a look for a received frame found.
#[derive(Debug)]
pub enum Intake<B, H> {
    /// No frame waited.
    Empty,
    /// A frame was read, and its buffer stayed the port's.
    Read,
    /// A datagram for a lending socket, in its buffer, with its loan.
    Lent(LentDatagram<B, H>),
}

impl<B, H> Intake<B, H> {
    /// Whether a frame was there.
    pub const fn found(&self) -> bool {
        !matches!(self, Self::Empty)
    }
}

/// A datagram a lending socket receives in the port's buffer. The buffer goes
/// back to the port when `frame` drops; `loan` goes back to the pool's
/// [`Loans`](crate::Loans) only then, so a credit frees once its memory has.
#[derive(Debug)]
pub struct LentDatagram<B, H> {
    /// The socket's flow, as [`SocketId::flow`] names it.
    pub flow: u8,
    pub remote: Remote,
    /// Where the datagram's data lies in the frame's Ethernet payload.
    pub data: core::ops::Range<usize>,
    pub frame: H,
    pub loan: Loan<B>,
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
    /// The frame did not fit the room the port gave, and the port took the
    /// room back: the same frame would meet the same, so its work ended, and a
    /// datagram's outcome is [`Outcome::Rejected`](crate::Outcome::Rejected).
    SlotTooShort,
    /// The producer declined to fill a declared work, and the port took the
    /// room back: the work ended
    /// [`Outcome::Declined`](crate::Outcome::Declined).
    Declined,
    /// The fill failed, and the port answered with a refusal of its own rather
    /// than the fill's error, which breaks [`FramePort::publish`]. The port
    /// says it took nothing, and the frame was never whole, so the work ended
    /// by the fill's cause: [`Outcome::Rejected`](crate::Outcome::Rejected)
    /// for a short slot, [`Outcome::Declined`](crate::Outcome::Declined) for a
    /// producer's decline. From then on every `transmit` answers
    /// [`Self::PortContract`].
    FillMisreported(R),
    /// The port said `Ok` though the frame was not written, which breaks
    /// [`FramePort::publish`], and wins over any error of the fill. What
    /// reached it is not known, so the work ended and is not repeated: a
    /// datagram's outcome is [`Outcome::Unknown`](crate::Outcome::Unknown).
    /// Every later `transmit` answers this too, for ARP as well, until the
    /// owner retires the core and takes the port back.
    PortContract,
}

/// What the owner saw of a fill: its own record, which decides what a port's
/// answer means.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Filled {
    /// The port never called it.
    Never,
    Wrote,
    Failed(FillFailed),
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
    T = (),
> {
    core: Core<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, T>,
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
    T: Copy,
> NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P, T>
{
    pub fn new(
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
    /// the port's answer is committed before this returns. A declared work's
    /// data is `produce`'s to write, in the slot itself, once its headers are
    /// there; no other work calls it. What the fill did is this owner's own
    /// record, and a port's answer that contradicts it is the port's fault,
    /// whatever the fill's error. `count` hears the frame once it was
    /// written whole in the port's slot ([`Count::materialised`]), whatever
    /// the port answers. A declared work the port took ends `Accepted` in
    /// this call and comes back in [`Transmitted::ended`] with its tag: its
    /// producer's one outcome, which no later end tells again.
    ///
    /// # Errors
    /// [`Unsent`].
    pub fn transmit<C: Count + ?Sized>(
        &mut self,
        now: Instant,
        grant: Grant<B>,
        produce: impl FnOnce(WorkId, T, &mut Payload<'_>) -> Result<(), Declined>,
        count: &C,
    ) -> Result<Transmitted<P::Ticket, T>, Unsent<P::Refusal>> {
        if self.broken {
            return Err(Unsent::PortContract);
        }
        // Matched in place: the frame is built where it is written from,
        // not moved into a result first.
        let Some(frame) = self.core.frame(now, &grant) else {
            return Err(Unsent::Stale);
        };
        let mut filled = Filled::Never;
        let published = self.port.publish(frame.length(), |slot| {
            let result = frame.write(slot, produce);
            filled = match result {
                Ok(()) => {
                    counted(count, Count::materialised);
                    Filled::Wrote
                }
                Err(cause) => Filled::Failed(cause),
            };
            result
        });
        match published {
            Ok(ticket) => {
                let written = filled == Filled::Wrote;
                let ended = self.core.commit(now, grant, written, &mut self.pool);
                if written {
                    Ok(Transmitted { ticket, ended })
                } else {
                    self.broken = true;
                    Err(Unsent::PortContract)
                }
            }
            // Generic in the fill's error, the port has no fill error but the
            // one the fill returned.
            Err(Unpublished::Fill(FillFailed::SlotTooShort)) => {
                self.core.reject(now, &grant, &mut self.pool);
                Err(Unsent::SlotTooShort)
            }
            Err(Unpublished::Fill(FillFailed::Declined(_))) => {
                self.core.decline(&grant, &mut self.pool);
                Err(Unsent::Declined)
            }
            Err(Unpublished::Port(refusal)) if matches!(filled, Filled::Failed(_)) => {
                if filled == Filled::Failed(FillFailed::SlotTooShort) {
                    self.core.reject(now, &grant, &mut self.pool);
                } else {
                    self.core.decline(&grant, &mut self.pool);
                }
                self.broken = true;
                Err(Unsent::FillMisreported(refusal))
            }
            Err(Unpublished::Port(refusal)) if P::is_final(&refusal) => {
                self.core.reject(now, &grant, &mut self.pool);
                Err(Unsent::Rejected(refusal))
            }
            Err(Unpublished::Port(refusal)) => Err(Unsent::Port(refusal)),
        }
    }

    /// Advance to `now`; see [`Core::tick`].
    pub fn tick(&mut self, now: Instant, events: &mut impl EventSink<NetEvent>) {
        self.core.tick(now, &mut self.pool, events);
    }

    /// Tell the core the port's link at `now`; see [`Core::link`].
    pub fn observe_link(&mut self, now: Instant)
    where
        P: FrameSource,
    {
        self.core.link(now, self.port.link());
    }

    /// See [`Core::configuration`].
    pub fn configuration(&self) -> Configuration {
        self.core.configuration()
    }

    /// See [`Core::configuration_changes`].
    pub const fn configuration_changes(&self) -> u32 {
        self.core.configuration_changes()
    }

    /// See [`Core::address_version`].
    pub const fn address_version(&self) -> u32 {
        self.core.address_version()
    }

    /// See [`Core::poll_deadline`].
    pub fn poll_deadline(&self) -> Option<Instant> {
        self.core.poll_deadline()
    }

    /// See [`Core::idle`].
    pub fn idle(&self) -> bool {
        self.core.idle()
    }

    /// See [`Core::demands`].
    pub fn demands<'s, C: Count + ?Sized>(
        &'s self,
        now: Instant,
        count: &'s C,
    ) -> impl Iterator<Item = Demand<B>> + Clone + 's {
        self.core.demands(now, count)
    }

    /// See [`Core::poll_demand`].
    pub fn poll_demand(&self, now: Instant) -> Option<Demand<B>> {
        self.core.poll_demand(now)
    }

    /// Take one received frame; see [`Core::input`].
    pub fn input(&mut self, now: Instant, frame: &[u8], events: &mut impl EventSink<NetEvent>) {
        self.core.input(now, frame, &mut self.pool, events);
    }

    /// Take one frame from the port, if one waits. A datagram for a lending
    /// socket keeps its buffer only with a loan of that socket's credit,
    /// taken before the port hands the buffer over; without one it is
    /// dropped with [`NotLent`] counted, and its buffer stays the port's.
    /// `count` hears a refusal for want of credit with the loans then out
    /// ([`Count::no_credit`]).
    pub fn take_frame<C: Count + ?Sized>(
        &mut self,
        now: Instant,
        events: &mut impl EventSink<NetEvent>,
        count: &C,
    ) -> Intake<B, P::Held>
    where
        P: FrameSource,
    {
        let core = &mut self.core;
        let pool = &mut self.pool;
        let mut lent = None;
        let taken = self.port.take_frame(|frame, holding| {
            let Some(to) = core.input_view(now, frame, holding, pool, events) else {
                return Keep::Drop;
            };
            match pool.loans().lend(usize::from(to.flow)) {
                Ok(loan) => {
                    lent = Some((to, loan));
                    Keep::Keep
                }
                Err(refused) => {
                    if refused == NotLent::NoCredit {
                        counted(count, |count| count.no_credit(pool.loans().outstanding()));
                    }
                    report(events, || NetEvent::NotLent(refused));
                    Keep::Drop
                }
            }
        });
        match (taken, lent) {
            (Taken::Empty, None) => Intake::Empty,
            (Taken::Kept(frame), Some((to, loan))) => Intake::Lent(LentDatagram {
                flow: to.flow,
                remote: to.remote,
                data: to.data,
                frame,
                loan,
            }),
            // A port that kept what it was not asked to: the buffer goes back
            // with the drop.
            (Taken::Kept(_frame), None) => Intake::Read,
            // A port that kept nothing it was asked to: the loan ends here.
            (Taken::Empty | Taken::Read, Some((_, loan))) => {
                pool.loans().give_back(loan);
                Intake::Read
            }
            (Taken::Read, None) => Intake::Read,
        }
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

    /// Make `cx` the one the port wakes, and say whether the port has
    /// something already ([`FrameSource::register`]).
    pub fn register_port(&mut self, cx: &mut Context<'_>) -> bool
    where
        P: FrameSource,
    {
        self.port.register(cx)
    }

    /// See [`Core::open`].
    ///
    /// # Errors
    /// [`OpenRefused`].
    pub fn open(&mut self, port: u16, dscp: Dscp) -> Result<SocketId<B>, OpenRefused> {
        self.core.open(port, dscp)
    }

    /// See [`Core::open_local`].
    ///
    /// # Errors
    /// [`OpenRefused`].
    pub fn open_local(&mut self, port: u16, dscp: Dscp) -> Result<SocketId<B>, OpenRefused> {
        self.core.open_local(port, dscp)
    }

    /// See [`Core::declare`].
    ///
    /// # Errors
    /// [`SendRefusal`].
    pub fn declare(
        &mut self,
        now: Instant,
        socket: &SocketId<B>,
        remote: Remote,
        length: usize,
        tag: T,
    ) -> Result<WorkId, SendRefusal> {
        self.core.declare(now, socket, remote, length, tag)
    }

    /// See [`Core::withdraw_declared`].
    pub fn withdraw_declared(
        &mut self,
        socket: &SocketId<B>,
        keep: impl FnMut(&T) -> bool,
    ) -> usize {
        self.core.withdraw_declared(socket, keep)
    }

    /// See [`Core::next_declared_end`].
    pub fn next_declared_end<C: Count + ?Sized>(
        &mut self,
        ends: &Ends<B>,
        count: &C,
    ) -> Option<(WorkId, T, Outcome)> {
        self.core.next_declared_end(ends, count)
    }

    /// See [`Core::neighbors_changed`].
    pub const fn neighbors_changed(&self) -> u32 {
        self.core.neighbors_changed()
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

    /// See [`Core::open_lending`]: the credit is granted from this owner's
    /// pool.
    ///
    /// # Errors
    /// [`OpenRefused`].
    pub fn open_lending(
        &mut self,
        port: u16,
        dscp: Dscp,
        group: Group<B>,
        credit: usize,
    ) -> Result<SocketId<B>, OpenRefused> {
        self.core
            .open_lending(port, dscp, group, credit, &mut self.pool)
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

    /// See [`Core::next_end`].
    pub fn next_end<C: Count + ?Sized>(&mut self, ends: &Ends<B>, count: &C) -> Option<WorkId> {
        self.core.next_end(ends, count)
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
    pub fn retire(self) -> Retired<'m, B, SIZE, SOCKETS, CREDITS, BLOCKS, P, T> {
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
