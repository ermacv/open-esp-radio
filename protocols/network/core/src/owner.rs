//! The one owner of a core, its pool and its port.

use core::task::Context;

use oer_portable_events::{EventSink, report};
use oer_portable_time::Instant;
use oer_protocols_network_port::{
    BatchEnd, EthernetFrame, FramePort, FrameSource, Holding, Incoming, PortReady, ReceivedBatch,
    Unpublished,
};
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

/// What one intake batch did ([`NetworkOwner::take_frames`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Intake {
    /// The port's batch: the frames it handed and how it ended. A broken
    /// queue ([`BatchEnd::Fault`]) is recorded too ([`Broken::Receive`]).
    pub batch: ReceivedBatch,
    /// Of those frames, the datagrams handed on in their buffers with a loan.
    pub lent: usize,
    /// Of those frames, the datagrams for a lending socket dropped with their
    /// buffer: no credit, or no lane took them.
    pub unlent: usize,
}

impl Intake {
    /// The frames taken that were not datagrams for a lending socket: each
    /// may have left the core work (a reply, a copy for a socket, a change
    /// of address).
    pub const fn read(&self) -> usize {
        self.batch
            .taken
            .saturating_sub(self.lent)
            .saturating_sub(self.unlent)
    }
}

/// Why the holder of a lending socket's lane did not take a lent datagram:
/// its buffer is back with the port, and a loan it names goes back to the
/// account.
#[derive(Debug)]
pub enum Unlent<B> {
    /// No open lending socket holds the datagram's flow.
    NoSocket(Loan<B>),
    /// The socket's lane had no room; `None` when the dropped datagram
    /// counts its own loan back, as a lent one does.
    LaneFull(Option<Loan<B>>),
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
    /// The port failed after the fill ([`FramePort::is_fault`]): the frame
    /// did not go, so the work ended [`Outcome::Rejected`](crate::Outcome::Rejected),
    /// and it is not tried again. Every later `transmit` answers
    /// [`Self::PortContract`] until the owner retires the core and takes the
    /// port back with what it knows of its loss.
    PortFault(R),
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
    /// The port contradicted itself or failed: nothing is published until it
    /// is replaced, by a retirement.
    broken: Option<Broken>,
}

/// How the port broke.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Broken {
    /// Its answer contradicted its fill.
    Contract,
    /// It failed after a fill ([`FramePort::is_fault`]).
    Fault,
    /// Its receive queue broke.
    Receive(oer_protocols_network_port::ReceiveFault),
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
            broken: None,
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
        if self.broken.is_some() {
            return Err(Unsent::PortContract);
        }
        let frame = self.core.frame(now, &grant).ok_or(Unsent::Stale)?;
        let mut filled = Filled::Never;
        let published = self.port.publish(frame.meta(), |slot| {
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
                    self.broken = Some(Broken::Contract);
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
                self.broken = Some(Broken::Contract);
                Err(Unsent::FillMisreported(refusal))
            }
            // The frame was whole and did not go; the port is not to be
            // trusted again, and the work is not repeated.
            Err(Unpublished::Port(refusal)) if P::is_fault(&refusal) => {
                self.core.reject(now, &grant, &mut self.pool);
                self.broken = Some(Broken::Fault);
                Err(Unsent::PortFault(refusal))
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

    /// How often a socket's datagrams were prepared.
    #[cfg(test)]
    pub(crate) const fn prepares(&self) -> usize {
        self.core.prepares()
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

    /// The ready work as the chain of iterators it was, for the test that
    /// compares the walk with it.
    #[cfg(test)]
    pub(crate) fn demands_chain<'s, C: Count + ?Sized>(
        &'s self,
        now: Instant,
        count: &'s C,
    ) -> impl Iterator<Item = Demand<B>> + Clone + 's {
        self.core.demands_chain(now, count)
    }

    /// See [`Core::poll_demand`].
    pub fn poll_demand(&self, now: Instant) -> Option<Demand<B>> {
        self.core.poll_demand(now)
    }

    /// Take one received frame; see [`Core::input`].
    pub fn input(&mut self, now: Instant, frame: &[u8], events: &mut impl EventSink<NetEvent>) {
        self.core.input(now, frame, &mut self.pool, events);
    }

    /// Take the port's ready frames, at most `limit`, in one batch whose
    /// queue places the port frees once. Each frame goes through the core in
    /// its position. A datagram for a lending socket keeps its buffer only
    /// with a loan of that socket's credit, taken before the buffer moves on:
    /// with one, `lend` gets the datagram, its socket named by the core's own
    /// lookup ([`LentDatagram::flow`]), and says why it could not hand one
    /// on ([`Unlent`]), its buffer dropped first. Without credit the
    /// datagram is dropped with [`NotLent`] counted, its buffer back with the
    /// port, and the batch goes on: one socket's credit stops no other.
    /// `count` hears a refusal for want of credit with the loans then out
    /// ([`Count::no_credit`]).
    pub fn take_frames<C: Count + ?Sized>(
        &mut self,
        now: Instant,
        limit: usize,
        events: &mut impl EventSink<NetEvent>,
        count: &C,
        mut lend: impl FnMut(LentDatagram<B, P::Held>) -> Result<(), Unlent<B>>,
    ) -> Intake
    where
        P: FrameSource,
    {
        let core = &mut self.core;
        let pool = &mut self.pool;
        let mut lent = 0_usize;
        let mut unlent = 0_usize;
        let batch = self.port.take_frames(limit, |incoming| match incoming {
            Incoming::Copied {
                frame: Ok(view),
                reason,
            } => {
                // A copy is never lent: the core copies what it keeps.
                let _never = core.input_view(now, view, Holding::Copied(reason), pool, events);
            }
            Incoming::Copied { frame: Err(_), .. } => report(events, || NetEvent::Malformed),
            Incoming::Lent(mut frame) => {
                let to = match frame.view() {
                    Ok(view) => core.input_view(now, view, Holding::Lendable, pool, events),
                    Err(_) => {
                        report(events, || NetEvent::Malformed);
                        None
                    }
                };
                // Anything but a lent datagram leaves its buffer here.
                let Some(to) = to else {
                    return;
                };
                match pool.loans().lend(usize::from(to.flow)) {
                    Ok(loan) => {
                        let datagram = LentDatagram {
                            flow: to.flow,
                            remote: to.remote,
                            data: to.data,
                            frame,
                            loan,
                        };
                        let lent_to = lend(datagram);
                        if lent_to.is_ok() {
                            lent = lent.saturating_add(1);
                        } else {
                            unlent = unlent.saturating_add(1);
                        }
                        match lent_to {
                            Ok(()) => {}
                            Err(Unlent::NoSocket(loan)) => {
                                pool.loans().give_back(loan);
                                report(events, || NetEvent::NoSocket);
                            }
                            Err(Unlent::LaneFull(loan)) => {
                                if let Some(loan) = loan {
                                    pool.loans().give_back(loan);
                                }
                                report(events, || NetEvent::LentLaneFull);
                            }
                        }
                    }
                    Err(refused) => {
                        unlent = unlent.saturating_add(1);
                        if refused == NotLent::NoCredit {
                            counted(count, |count| count.no_credit(pool.loans().outstanding()));
                        }
                        report(events, || NetEvent::NotLent(refused));
                    }
                }
            }
        });
        if let BatchEnd::Fault(fault) = batch.end {
            self.broken = Some(Broken::Receive(fault));
        }
        Intake {
            batch,
            lent,
            unlent,
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

    /// Make `cx` the one the port wakes, and say what the port has
    /// already ([`FrameSource::register`]).
    pub fn register_port(&mut self, cx: &mut Context<'_>) -> PortReady
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

    /// Whether the port contradicted itself or failed, which stops
    /// publishing.
    pub const fn port_broken(&self) -> bool {
        self.broken.is_some()
    }

    /// How the port broke, if it did.
    pub const fn broken(&self) -> Option<Broken> {
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
