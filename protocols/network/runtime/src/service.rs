//! The task that owns the core.

use core::future::poll_fn;
use core::pin::pin;
use core::task::{Context, Poll};

use embassy_futures::select::select;
use embassy_futures::yield_now;
use oer_portable_events::{EventSink, report};
use oer_portable_memory::{WakingSpscQueue, WakingSpscReceiver};
use oer_portable_time::{Duration, Instant, Timer};
use oer_protocols_network_core::{
    Broken, CommandId, CommandState, Count, Ends, Eviction, Group, GroupsFull, Intake,
    LentDatagram, LoansRefused, NetEvent, NetworkOwner, OpenRefused, Outcome, Payload, Pool,
    Retired, SocketId, Unsent,
};
use oer_protocols_network_port::{FramePort, FrameSource};
use oer_protocols_network_wire::ip::Dscp;

use crate::diagnostics::{Diagnostics, Stage, Tally};
use crate::local::{LocalEnd, LocalRefused, Produce, Producer, Producers};
use crate::observation::{LocalEvent, PublishEnd, ServiceEvent, TurnEnd, seen};
use crate::returns::ReturnEnd;
use crate::socket::{
    Answer, Inbound, LateSocket, LendingLanes, LendingSocket, LentEnd, Request, Sent, ServiceEnd,
    Socket, SocketLanes, drain_requests, lanes, lending_lanes,
};
use crate::watch::{ConfigurationWatch, WatchRefused, Watched};

/// Commands one turn takes from a lane.
const TURN_COMMANDS: usize = 4;
/// Frames one turn takes in, and publishes; offers a producer is asked for in
/// one turn, and its declared works open at once.
pub(crate) const TURN_FRAMES: usize = 8;
/// Notices one turn hands the policy before it publishes. A turn that finds
/// no more within them publishes, so a Block Ack's ends, fewer than this
/// (the S31 station's aggregates hold 32 MPDUs), settle together and the same
/// turn publishes again. A notice is one settlement of one frame, so this
/// bounds the turn's work as [`TURN_FRAMES`] does.
pub(crate) const TURN_NOTICES: usize = 64;

/// What a frame the port did not take left to its publishing.
#[derive(Clone, Copy, Debug)]
struct AfterUnsent {
    /// Its work moved: it ended, or the producer's flow sat out.
    moved: bool,
    /// Publishing ends here.
    end: Option<PublishEnd>,
}

impl AfterUnsent {
    /// The work ended or is other work: the next work goes.
    const ENDED: Self = Self {
        moved: true,
        end: None,
    };
}

/// What a turn did.
#[derive(Clone, Copy, Debug)]
struct Turned {
    /// It left work no wake names: a budget spent, notices untold, a block
    /// evicted, a producer's hold or budget freed, a refused publication of
    /// the address.
    again: bool,
    /// Only frames are left: a budget of intake or publication spent, or
    /// a flow left out after a declining fill. The next turn may take only
    /// the frames ([`Turn::Packets`]).
    packets: bool,
}

/// Which steps a turn takes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Turn {
    /// Every step: time, commands, returns, blocks, frames, eviction and
    /// configuration.
    Full,
    /// The frames only, over the same state: intake, notices, offers and
    /// publication, then delivery. Taken only when no control work is
    /// pending ([`Service::control_pending`]).
    Packets,
}

/// What one batch of returns did.
#[derive(Clone, Copy, Debug)]
struct Returned {
    /// Something came back.
    any: bool,
    /// A block came home while a socket waits for one.
    work: bool,
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
    /// The port failed after a fill (`Unsent::PortFault`): the frame did not
    /// go and its command ends `Outcome::Rejected`, not tried again. The
    /// others end `Outcome::Retired`, and the port goes back to its owner in
    /// [`Ended::retired`] with what it knows of its loss.
    PortFault,
    /// The port's receive queue broke: it gave no frame where it held one.
    /// No frame is read after it; the commands end `Outcome::Retired`, and
    /// the port goes back to its owner in [`Ended::retired`].
    ReceiveFault(oer_protocols_network_port::ReceiveFault),
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
    R = (),
    T = (),
> {
    pub cause: EndCause,
    pub retired: Retired<'m, B, SIZE, SOCKETS, CREDITS, BLOCKS, P, T>,
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
    R,
    T,
> Ended<'a, 'm, B, SIZE, SOCKETS, CREDITS, BLOCKS, DEPTH, LANE, P, R, T>
{
    /// With no service to come, close the way home for good: what it accepted
    /// comes to the pool, and a later `give` is refused.
    pub fn close_returns(&mut self) {
        if let Some(returns) = self.returns.take() {
            returns.close(&mut self.retired.pool);
        }
    }

    /// With no service to come, take back each lent buffer's loan as it
    /// returns, until none is out; see [`serve_late_loans`].
    pub async fn serve_loans(&mut self) {
        serve_late_loans(
            &mut self.retired.pool,
            [&mut self.sockets[..], &mut self.adopted[..]],
        )
        .await;
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

/// With no service to come, bring home into `pool` what the `late` sockets
/// give back, and wait for the rest, until no receive loan of the pool is
/// out: a late return closes the pool's accounting. A loan whose buffer is
/// held for ever keeps this waiting, and nothing expires. A cancelled wait
/// loses nothing: what came back is in `pool`, and the rest still counts.
pub async fn serve_late_loans<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const BLOCKS: usize,
    const GROUPS: usize,
>(
    pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    mut late: [&mut [Option<LateSocket<'a, 'm, B, SIZE, DEPTH, RECEIVED>>]; GROUPS],
) {
    poll_fn(|cx| {
        // Each hears its next return first; a return before that is taken
        // below.
        for socket in late.iter_mut().flat_map(|group| group.iter_mut()).flatten() {
            let _ready = socket.poll(cx);
        }
        for place in late.iter_mut().flat_map(|group| group.iter_mut()) {
            if place.as_mut().is_some_and(|socket| socket.collect(pool)) {
                *place = None;
            }
        }
        if pool.loans().released() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
}

/// A service's watch, which its end, retirement or drop, ends: a cancelled
/// `run` drops no service, so it ends nothing.
struct Watching<'a> {
    watch: &'a ConfigurationWatch,
    /// The core's count of changes at the last publication.
    seen: u32,
}

impl Drop for Watching<'_> {
    fn drop(&mut self) {
        self.watch.end();
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
    H,
> {
    end: ServiceEnd<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS>,
    /// A lending socket's loans.
    lent: Option<LentEnd<'a, B, H, LANE>>,
    open: Option<Open<B>>,
    /// Where the core tells this socket's ends, which outlives its close.
    ends: Ends<B>,
    /// The right to each accepted command's result and the socket's tag of
    /// it, at the command's place in the core: an end the core tells is
    /// matched here by place and generation.
    commands: [Option<(CommandId<B>, u8)>; CREDITS],
    closing: Option<CommandId<B>>,
    /// A received datagram waits for room in the lane.
    blocked: bool,
    /// What this socket's reserved blocks count toward.
    group: Group<B>,
    /// The socket was dropped and its lanes are drained.
    abandoned: bool,
}

/// The network's task. `L` is its local producer: `()` for none, at no
/// cost, or a [`Producer`] ([`Service::with_producer`]). `D` are its
/// switchable diagnostics: `()` for none, at no cost, or an image's
/// ([`Service::with_diagnostics`]).
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
    L: Producers<B, CREDITS> = (),
    D = (),
> {
    owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P, L::Tag>,
    bound: [Option<Bound<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS, P::Held>>; SOCKETS],
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
    /// Where the interface's address is published, and the core's count of
    /// changes at the last publication.
    watch: Option<Watching<'a>>,
    diagnostics: D,
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
    /// Own `owner`, with no local producer. A service keeps its owners until
    /// [`Self::retire`] hands them on, so a cancelled [`Self::run`] loses
    /// none. Dropping a service instead is the end of the whole network: its
    /// port, pool and brand go, blocks in its lanes stay there unreachable and
    /// their memory is not reused, and waiting sockets and observers end.
    pub const fn new(
        owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>,
        memory: MemoryPolicy,
        stop: WakingSpscReceiver<'a, (), 1>,
    ) -> Self {
        Self::build(owner, memory, stop, ())
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
        Producer<'a, B, Q, CREDITS>,
    >
{
    /// As [`Self::new`], with `producer`, owned by value, which the service
    /// calls in its own turns once [`Self::bind_local`] gave it a socket.
    /// `end` is the image's flag the service's end sets: at its retirement,
    /// after every outcome its producer hears, or when it is dropped, with
    /// nothing called.
    pub const fn with_producer(
        owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P, Q::Tag>,
        memory: MemoryPolicy,
        stop: WakingSpscReceiver<'a, (), 1>,
        producer: Q,
        end: &'a LocalEnd,
    ) -> Self {
        Self::build(owner, memory, stop, Producer::new(producer, end))
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
    Q: Produce,
    D,
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
        Producer<'a, B, Q, CREDITS>,
        D,
    >
{
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
    L: Producers<B, CREDITS>,
> Service<'a, 'm, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, DEPTH, LANE, P, L>
{
    const fn build(
        owner: NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P, L::Tag>,
        memory: MemoryPolicy,
        stop: WakingSpscReceiver<'a, (), 1>,
        local: L,
    ) -> Self {
        Self {
            owner,
            bound: [const { None }; SOCKETS],
            stop,
            ended: None,
            next_block: 0,
            memory,
            pressure_at: None,
            adopted: [const { None }; SOCKETS],
            returns: None,
            local,
            watch: None,
            diagnostics: (),
        }
    }

    /// As it is, counting into `diagnostics`, which it asks whether to at
    /// each turn and each pass of the wait that takes returns (see
    /// [`Diagnostics`]).
    #[inline(always)]
    pub fn with_diagnostics<D: Diagnostics>(
        self,
        diagnostics: D,
    ) -> Service<'a, 'm, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, DEPTH, LANE, P, L, D>
    {
        let Self {
            owner,
            bound,
            stop,
            ended,
            next_block,
            memory,
            pressure_at,
            adopted,
            returns,
            local,
            watch,
            diagnostics: (),
        } = self;
        Service {
            owner,
            bound,
            stop,
            ended,
            next_block,
            memory,
            pressure_at,
            adopted,
            returns,
            local,
            watch,
            diagnostics,
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
    L: Producers<B, CREDITS>,
    D: Diagnostics,
> Service<'a, 'm, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, DEPTH, LANE, P, L, D>
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

    /// Publish the interface's address to `watch` from now on: at once, then
    /// at the end of each turn that changed it, and at the service's end.
    ///
    /// # Errors
    /// [`WatchRefused::Taken`]: this service or another publishes already.
    pub fn watch(&mut self, watch: &'a ConfigurationWatch) -> Result<(), WatchRefused> {
        if self.watch.is_some() {
            return Err(WatchRefused::Taken);
        }
        let changes = self.owner.configuration_changes();
        watch.begin(Watched {
            configuration: Some(self.owner.configuration()),
        })?;
        self.watch = Some(Watching {
            watch,
            seen: changes,
        });
        Ok(())
    }

    /// Publish the address if the turn changed it; whether a refusal left
    /// it for the next turn.
    fn publish_configuration(&mut self) -> bool {
        let changes = self.owner.configuration_changes();
        if let Some(Watching { watch, seen }) = &mut self.watch
            && *seen != changes
        {
            // Only this service lends the cell, and readers lend it for a
            // copy: a refusal leaves the count, and the next turn tries again.
            if watch
                .publish(Watched {
                    configuration: Some(self.owner.configuration()),
                })
                .is_ok()
            {
                *seen = changes;
            } else {
                return true;
            }
        }
        false
    }

    /// Take the returns published to this service, as one batch.
    fn take_returns(&mut self, tally: &mut Tally<D>) -> Returned {
        Self::returns_of(
            &mut self.owner,
            &mut self.bound,
            self.returns.as_ref(),
            &mut self.adopted,
            tally,
        )
    }

    /// The service's return servicing: take what came back by the return
    /// lane, from adopted sockets and as lent buffers back with their port,
    /// and give it to the pool and the loans' account. It runs no command,
    /// reads no frame, chooses nothing and builds nothing. A block that came
    /// home while a socket waits for one is work, which a turn grants. While
    /// `tally` is on, a batch that took something counts, with the blocks it
    /// brought home and the loans it gave back.
    fn returns_of(
        owner: &mut NetworkOwner<
            'm,
            B,
            SIZE,
            SOCKETS,
            CREDITS,
            RECEIVED,
            NEIGHBORS,
            BLOCKS,
            P,
            L::Tag,
        >,
        bound: &mut [Option<Bound<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS, P::Held>>; SOCKETS],
        returns: Option<&ReturnEnd<'a, 'm, B, SIZE, BLOCKS>>,
        adopted: &mut [Option<LateSocket<'a, 'm, B, SIZE, DEPTH, LANE>>; SOCKETS],
        tally: &mut Tally<D>,
    ) -> Returned {
        let mut any = false;
        let pool = owner.pool();
        let free = pool.free();
        let loans = if tally.on() {
            pool.loans().outstanding()
        } else {
            0
        };
        if let Some(returns) = returns {
            any |= returns.take_home(pool);
        }
        for late in adopted.iter_mut() {
            if late.as_mut().is_some_and(|late| late.collect(pool)) {
                *late = None;
                any = true;
            }
        }
        for bound in bound.iter_mut().flatten() {
            if let Some(lent) = &mut bound.lent {
                any |= lent.collect(pool.loans());
            }
        }
        let came = pool.free() > free;
        if any || came {
            tally.note(|counts| {
                let blocks = pool.free().saturating_sub(free);
                let credits = loans.saturating_sub(pool.loans().outstanding());
                counts.return_batches = counts.return_batches.wrapping_add(1);
                counts.blocks_returned = counts
                    .blocks_returned
                    .wrapping_add(u32::try_from(blocks).unwrap_or(u32::MAX));
                counts.credits_restored = counts
                    .credits_restored
                    .wrapping_add(u32::try_from(credits).unwrap_or(u32::MAX));
            });
        }
        Returned {
            any: any || came,
            work: came && bound.iter().flatten().any(Bound::waiting),
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
            lent: None,
            group,
            ends: id.ends(),
            open: Some(Open { id, reserving: 0 }),
            commands: [const { None }; CREDITS],
            closing: None,
            blocked: false,
            abandoned: false,
        });
        Ok(socket)
    }

    /// Lend from `buffers` of the port's own, of which `reserve` stay the
    /// port's, for the frames on their way: [`Loans::pool`], whose account
    /// outlives this service with its pool.
    ///
    /// # Errors
    /// [`LoansRefused`]; what was set stays.
    ///
    /// [`Loans::pool`]: oer_protocols_network_core::Loans::pool
    pub fn lend_from(&mut self, buffers: usize, reserve: usize) -> Result<(), LoansRefused> {
        self.owner.pool().loans().pool(buffers, reserve)
    }

    /// Open a socket on `port` that receives in the port's own buffers, with
    /// a credit of `CREDIT` loans from the pool's account, and give the
    /// application its end. The lane holds every loan the credit allows, so
    /// `CREDIT` is checked against it when this compiles; the account checks
    /// it against the pool here. A refusal changes nothing.
    ///
    /// # Errors
    /// [`BindRefused`], with `storage`: [`OpenRefused::Loans`] when the
    /// account cannot hold the credit.
    pub fn bind_lending<const CREDIT: usize>(
        &mut self,
        port: u16,
        dscp: Dscp,
        group: Group<B>,
        storage: &'a mut LendingLanes<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS, P::Held>,
    ) -> Result<
        LendingSocket<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS, P::Held>,
        BindRefused<'a, LendingLanes<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS, P::Held>>,
    > {
        const {
            if CREDIT == 0 || CREDIT > LANE {
                panic!("a lending socket's credit is at least one, and its lane holds it");
            }
        };
        let Some(place) = self.bound.iter_mut().find(|place| place.is_none()) else {
            return Err(BindRefused {
                reason: OpenRefused::NoSocket,
                storage,
            });
        };
        let id = match self.owner.open_lending(port, dscp, group, CREDIT) {
            Ok(id) => id,
            Err(reason) => return Err(BindRefused { reason, storage }),
        };
        let (socket, end, lent) = lending_lanes(storage);
        *place = Some(Bound {
            end,
            lent: Some(lent),
            group,
            ends: id.ends(),
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
        // Whether the last turn left only frames.
        let mut only_frames = false;
        loop {
            if let Some(cause) = self.ended {
                return cause;
            }
            if self.stop.try_receive().is_some() {
                self.ended = Some(EndCause::Stopped);
                continue;
            }
            let now = clock.now();
            let kind = if only_frames && !self.control_pending(now) {
                Turn::Packets
            } else {
                Turn::Full
            };
            let turned = self.turn(now, kind, events);
            only_frames = turned.packets;
            if let Some(broken) = self.owner.broken() {
                self.ended = Some(match broken {
                    Broken::Contract => EndCause::PortContract,
                    Broken::Fault => EndCause::PortFault,
                    Broken::Receive(fault) => EndCause::ReceiveFault(fault),
                });
            } else if turned.again {
                // The turn left work no wake will name: the next turn
                // takes it, once the executor had its turn.
                report(events, || ServiceEvent::Yielded);
                yield_now().await;
            } else if self.wait(clock, events).await {
                // The wait was over at its first poll, by work or by a time
                // already due: no turn follows another within the same poll,
                // and a source that says it is ready for good cannot hold
                // the executor.
                report(events, || ServiceEvent::Yielded);
                yield_now().await;
            }
        }
    }

    /// One bounded pass over everything: whether it left work for the next turn that no wake names. Every exit goes
    /// through its one epilogue, which hands every known result to its
    /// client, so at an await each open work truly was not published.
    fn turn(
        &mut self,
        now: Instant,
        kind: Turn,
        events: &mut (impl EventSink<NetEvent> + EventSink<ServiceEvent>),
    ) -> Turned {
        // What only frames leave, and what more than frames leaves.
        let mut packets = false;
        let mut again = false;
        let mut tally = Tally::turn(&mut self.diagnostics);
        report(events, || ServiceEvent::TurnBegan);
        if kind == Turn::Packets {
            report(events, || ServiceEvent::PacketTurn);
        }
        self.local.turn_begins();
        // A link that changed ends what was prepared for the old one: every
        // turn sees it before a frame moves.
        self.owner.observe_link(now);
        if kind == Turn::Full {
            self.owner.tick(now, events);
            for index in 0..SOCKETS {
                self.commands(now, index, events);
            }
        }
        tally.stage(Stage::Intake, &mut self.diagnostics);
        // Lent memory coming back is the frames' own: every turn takes it.
        self.take_returns(&mut tally);
        if kind == Turn::Full {
            self.give_blocks();
        }
        tally.stage(Stage::Returns, &mut self.diagnostics);
        let mut intake_full = true;
        for _ in 0..TURN_FRAMES {
            let intake = self.owner.take_frame(now, events, tally.steps());
            // Read right after the refusal's frame, before anything else
            // moves the loans.
            let bound = &self.bound;
            tally.read_unreturned(|| Self::unreturned(bound));
            match intake {
                Intake::Empty => {
                    intake_full = false;
                    break;
                }
                Intake::Read => {}
                Intake::Lent(datagram) => self.lend(datagram, events),
                // The owner records the broken queue, which ends the run.
                Intake::Broken(_) => {
                    intake_full = false;
                    break;
                }
            }
            report(events, || ServiceEvent::FrameTaken);
        }
        seen(events, || intake_full.then_some(ServiceEvent::IntakeFull));
        packets |= intake_full;
        tally.stage(Stage::Receive, &mut self.diagnostics);
        let mut told_all = false;
        for _ in 0..TURN_NOTICES {
            let Some(notice) = self.owner.take_notice() else {
                told_all = true;
                break;
            };
            // A notice is the port's: taking it is what frees its place.
            let _settled = notice;
            report(events, || ServiceEvent::Noticed);
        }
        tally.stage(Stage::Notices, &mut self.diagnostics);
        // Notices the budget left are the next turn's.
        seen(events, || (!told_all).then_some(ServiceEvent::NoticesFull));
        again |= !told_all;
        self.local
            .offers(now, &mut self.owner, events, tally.steps());
        let end = self.publish(now, events, &mut tally);
        // More may be ready past the budget; a declining fill kept the
        // producer's flow out of the rest of this turn only.
        packets |= end == PublishEnd::Budget || self.local.filtered().is_some();
        tally.stage(Stage::Publish, &mut self.diagnostics);
        // Eviction ends commands too, so it comes before the delivery: the
        // epilogue's delivery is the turn's last step that can learn a result.
        if kind == Turn::Full && !self.owner.port_broken() {
            // An evicted block is granted by the next turn.
            again |= self.relieve(now);
        }
        self.deliver(events, &tally);
        // What the delivery freed may release the producer's hold or budget,
        // which no wake of its own names: more frames.
        packets |= self.local.again(&self.owner);
        if kind == Turn::Full {
            again |= self.publish_configuration();
        }
        self.turn_ended(events);
        tally.stage(Stage::Settle, &mut self.diagnostics);
        tally.end(&mut self.diagnostics);
        Turned {
            again: again || packets,
            packets: packets && !again,
        }
    }

    /// Whether control work is pending that a packet turn would leave, each
    /// a step only the full turn takes: a socket's request or abandon (its
    /// commands), a due time of the core, the producer or the pool's
    /// pressure (the tick, the offers' deadline, the eviction's age), a
    /// socket owed a block (its grant, or with none free its eviction) and
    /// a configuration not yet published. Read from each source's own head,
    /// once a turn.
    fn control_pending(&self, now: Instant) -> bool {
        let due = [
            self.owner.poll_deadline(),
            self.pressure_at,
            self.local.deadline(),
        ]
        .into_iter()
        .flatten()
        .any(|at| at <= now);
        let unpublished = self
            .watch
            .as_ref()
            .is_some_and(|watching| watching.seen != self.owner.configuration_changes());
        due || unpublished
            || self.bound.iter().flatten().any(|bound| {
                bound.waiting() || !bound.end.requests.is_empty() || !bound.end.abandon.is_empty()
            })
    }

    /// The lent buffers back with their port whose loans the service has not
    /// yet taken back, over every lending socket.
    fn unreturned(
        bound: &[Option<Bound<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS, P::Held>>; SOCKETS],
    ) -> u32 {
        bound
            .iter()
            .flatten()
            .filter_map(|bound| bound.lent.as_ref())
            .fold(0_u32, |sum, lent| sum.saturating_add(lent.unreturned()))
    }

    /// Hand a lent datagram to its socket, or say why not: its buffer then
    /// goes back to the port, and its loan to the account.
    fn lend(&mut self, datagram: LentDatagram<B, P::Held>, events: &mut impl EventSink<NetEvent>) {
        let flow = datagram.flow;
        let end = self
            .bound
            .iter_mut()
            .flatten()
            .filter(|bound| !bound.abandoned)
            .find(|bound| {
                bound
                    .open
                    .as_ref()
                    .is_some_and(|open| open.id.flow() == flow)
            })
            .and_then(|bound| bound.lent.as_mut());
        let account = self.owner.pool().loans();
        match end {
            Some(end) => {
                if end.lend(datagram, account).is_err() {
                    report(events, || NetEvent::LentLaneFull);
                }
            }
            // The core lends only to an open lending socket, which a lending
            // bind holds here.
            None => {
                let LentDatagram { frame, loan, .. } = datagram;
                drop(frame);
                account.give_back(loan);
                report(events, || NetEvent::NoSocket);
            }
        }
    }

    /// The turn's epilogue: hand every result the core knows to its client,
    /// the channel sockets' places and the producer alike; whether any went.
    fn deliver(&mut self, events: &mut impl EventSink<ServiceEvent>, tally: &Tally<D>) -> bool {
        let mut moved = false;
        for bound in self.bound.iter_mut().flatten() {
            moved |= bound.hand_back(&mut self.owner, tally.steps());
        }
        moved |= self.local.deliver(&mut self.owner, events, tally.steps());
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
            let loans = self.owner.pool().loans().outstanding();
            Some(ServiceEvent::TurnEnded(TurnEnd {
                blocks_free,
                credits_free,
                loans,
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
        self.relieve_full(now, after)
    }

    /// [`Self::relieve`] once the pool has no free block: rare, and apart
    /// from the turn's own path.
    #[cold]
    #[inline(never)]
    fn relieve_full(&mut self, now: Instant, after: Duration) -> bool {
        let pool = &*self.owner.pool();
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

    // Each socket's commands, rare beside the frames: apart from the turn's
    // own path.
    #[inline(never)]
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
                        // The core numbers this socket's places below
                        // `CREDITS`.
                        Ok(command) => {
                            if let Some(place) = bound.commands.get_mut(command.work().place()) {
                                *place = Some((command, tag));
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

    /// Publish the ready work, the first the core lists each time, until the
    /// port has no room or the turn's budget of frames is spent: control
    /// first, then the sockets in turn from the one whose turn it is. The air
    /// is the port's to choose: a frame it takes goes in its queue, and its
    /// MAC decides when and charges it. No await falls between a choice and
    /// its answer, so no cancellation can leave a frame unanswered.
    #[inline(never)]
    fn publish(
        &mut self,
        now: Instant,
        events: &mut impl EventSink<ServiceEvent>,
        tally: &mut Tally<D>,
    ) -> PublishEnd {
        let mut moved = false;
        let mut frames = 0_usize;
        let end = loop {
            if frames == TURN_FRAMES {
                break PublishEnd::Budget;
            }
            // Certainly no work: nothing is walked.
            if self.owner.idle() {
                break PublishEnd::Chose;
            }
            // After a declining fill, the producer's flow sits out the turn.
            let filtered = self.local.filtered();
            let demand = self
                .owner
                .demands(now, tally.steps())
                .find(|demand| filtered.is_none_or(|flow| demand.ready().flow != Some(flow)));
            let Some(demand) = demand else {
                break PublishEnd::Chose;
            };
            frames = frames.saturating_add(1);
            let local = &mut self.local;
            let fill = |work, tag, data: &mut Payload<'_>| local.fill(now, work, tag, data);
            match self
                .owner
                .transmit(now, demand.grant(), fill, tally.steps())
            {
                Ok(transmitted) => {
                    moved = true;
                    report(events, || ServiceEvent::Accepted);
                    // The datagram's one outcome is its producer's now.
                    if let Some((work, tag)) = transmitted.ended {
                        self.local.accepted(work, tag);
                    }
                }
                // Every other answer ends the work or the publishing: rare,
                // and apart from the frames' own path.
                Err(unsent) => {
                    let after = self.unsent(unsent, events);
                    moved |= after.moved;
                    if let Some(end) = after.end {
                        break end;
                    }
                }
            }
        };
        report(events, || ServiceEvent::Published(end));
        if moved {
            self.owner.flush();
        }
        end
    }

    /// What a frame the port did not take does to its work and the
    /// publishing. Kept out of the frames' own path,
    /// which only an accepted frame takes.
    #[cold]
    #[inline(never)]
    fn unsent(
        &mut self,
        unsent: Unsent<P::Refusal>,
        events: &mut impl EventSink<ServiceEvent>,
    ) -> AfterUnsent {
        match unsent {
            // The work ended or is other work: the next goes.
            Unsent::Stale | Unsent::Rejected(_) | Unsent::SlotTooShort => AfterUnsent::ENDED,
            // One declining fill a turn: the producer's flow is out of the
            // rest of it, so its declines never stop another's work.
            Unsent::Declined => {
                report(events, || ServiceEvent::Local(LocalEvent::Declined));
                self.local.declined();
                AfterUnsent::ENDED
            }
            // The port took nothing, by its word, and is at fault: the work
            // ended by its fill's cause, and the service ends.
            Unsent::FillMisreported(_) => AfterUnsent {
                moved: true,
                end: Some(PublishEnd::Fault),
            },
            Unsent::Port(_) => AfterUnsent {
                moved: false,
                end: Some(PublishEnd::Refused),
            },
            Unsent::PortContract => AfterUnsent {
                moved: false,
                end: Some(PublishEnd::Fault),
            },
            // The work ended; the port is not trusted again.
            Unsent::PortFault(_) => AfterUnsent {
                moved: true,
                end: Some(PublishEnd::Fault),
            },
        }
    }

    /// Wait for anything to do: the port, a lane, the producer, a stop or a
    /// time ends this. Returns do not: a pass that finds them takes them as
    /// one batch ([`Self::returns_of`]) and waits on, unless a block came
    /// home for a socket that waits for one. Each pass registers every source
    /// and reads its readiness from that registration ([`FrameSource::register`],
    /// [`Produce::register`](crate::Produce::register)), so what comes between
    /// is missed by neither, and ordinary work is known apart from returns. A
    /// pass with nothing ready waits on, whatever woke it. Whether the whole
    /// wait, its time included, was over at its first poll.
    async fn wait<T: Timer>(
        &mut self,
        clock: &T,
        events: &mut impl EventSink<ServiceEvent>,
    ) -> bool {
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
            diagnostics,
            ..
        } = self;
        let ready = poll_fn(|cx| {
            let mut ready = pin!(stop.ready()).poll(cx).is_ready();
            ready |= owner.register_port(cx);
            ready |= local.register(cx, events);
            let mut returned = false;
            for bound in bound.iter_mut().flatten() {
                ready |= bound.poll_lanes(cx);
                returned |= bound.poll_returns(cx);
            }
            for late in adopted.iter_mut().flatten() {
                returned |= late.poll(cx);
            }
            if let Some(returns) = returns.as_ref() {
                returned |= returns.poll_waiting(cx);
            }
            if returned {
                let mut tally = Tally::begin(diagnostics);
                let taken = Self::returns_of(owner, bound, returns.as_ref(), adopted, &mut tally);
                tally.stage(Stage::Waiting, diagnostics);
                tally.end(diagnostics);
                report(events, || ServiceEvent::Returned { work: taken.work });
                ready |= taken.work;
            }
            report(events, || ServiceEvent::Waited { ready });
            if ready {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        });
        let mut wait = pin!(async {
            match deadline {
                Some(deadline) => {
                    let _first = select(ready, clock.wait_until(deadline)).await;
                }
                None => ready.await,
            }
        });
        let mut polls = 0_u32;
        poll_fn(|cx| {
            polls = polls.saturating_add(1);
            wait.as_mut().poll(cx)
        })
        .await;
        polls == 1
    }

    /// End: nothing more is published, every known result goes to its place,
    /// each socket gets the request lane, with what the service never saw, and
    /// every owner comes back. Its owner calls this after [`Self::run`] ended
    /// or was cancelled.
    #[allow(
        clippy::type_complexity,
        reason = "the service's end hands on each owner it held"
    )]
    pub fn retire(
        self,
    ) -> Ended<'a, 'm, B, SIZE, SOCKETS, CREDITS, BLOCKS, DEPTH, LANE, P, L::Returned, L::Tag> {
        let cause = self.ended.unwrap_or(EndCause::Stopped);
        let mut this = self;
        // The lease leaves with the core: the watch ends here, as it would
        // at a drop.
        drop(this.watch.take());
        while this.take_returns(&mut Tally::off()).any {}
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
            sockets,
            adopted,
            returns,
            producers,
        }
    }
}

impl<'a, 'm, B, const SIZE: usize, const DEPTH: usize, const LANE: usize, const CREDITS: usize, H>
    Bound<'a, 'm, B, SIZE, DEPTH, LANE, CREDITS, H>
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
        T: Copy,
    >(
        &mut self,
        inbound: Inbound<'a, 'm, B, SIZE, DEPTH, LANE>,
        owner: &mut NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P, T>,
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
        T: Copy,
    >(
        &mut self,
        owner: &mut NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P, T>,
        count: &impl Count,
    ) -> bool {
        let mut moved = false;
        // Only what the core told ended: running commands are not looked at.
        while let Some(work) = owner.next_end(&self.ends, count) {
            if self
                .closing
                .as_ref()
                .is_some_and(|close| close.work() == work)
            {
                if let Some(close) = self.closing.take() {
                    let _socket_closed = owner.take_result(close);
                }
                moved = true;
                self.closed();
                continue;
            }
            // The generation decides: a told end that is not this right's
            // command leaves the right where it is.
            let Some(slot) = self.commands.get_mut(work.place()).filter(|slot| {
                slot.as_ref()
                    .is_some_and(|(command, _)| command.work() == work)
            }) else {
                continue;
            };
            let Some((command, tag)) = slot.take() else {
                continue;
            };
            let sent = match owner.take_result(command) {
                CommandState::Finished(outcome) => Sent::Ended(outcome),
                // An end is told once it is a result, so this keeps its right.
                CommandState::Running(command) => {
                    *slot = Some((command, tag));
                    continue;
                }
                CommandState::Gone => Sent::Ended(Outcome::Unknown),
            };
            moved = true;
            self.ended(tag, sent);
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

    /// Whether lent buffers came back; `cx` hears the next.
    fn poll_returns(&self, cx: &Context<'_>) -> bool {
        self.lent.as_ref().is_some_and(|lent| lent.poll(cx))
    }

    /// The core retired: write what it decided into the places, say so, and
    /// hand on the lanes the service stops reading.
    fn finish<const SOCKETS: usize, const BLOCKS: usize, P, T: Copy>(
        self,
        retired: &mut Retired<'m, B, SIZE, SOCKETS, CREDITS, BLOCKS, P, T>,
    ) -> LateSocket<'a, 'm, B, SIZE, DEPTH, LANE> {
        let Self {
            end,
            lent,
            mut commands,
            abandoned,
            ..
        } = self;
        for (command, tag) in commands.iter_mut().filter_map(Option::take) {
            let sent = match retired.results.take(command) {
                CommandState::Finished(outcome) => Sent::Ended(outcome),
                CommandState::Running(_) | CommandState::Gone => Sent::Ended(Outcome::Unknown),
            };
            if let Some(place) = end.places.get(usize::from(tag)) {
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
        let mut dropped = abandoned;
        if let Some(inbound) = abandon.try_receive() {
            drain_requests(&requests, &mut retired.pool, |_| {});
            inbound.drain(&mut retired.pool);
            dropped = true;
        } else if !abandoned {
            let _room_is_proven = answers.try_send(Answer::Stopped(requests));
        }
        drop(ending);
        let mut loans = lent.map(LentEnd::into_late);
        // What came back before the end is the retired pool's at once.
        if let Some(loans) = &mut loans {
            let _any = loans.collect(retired.pool.loans());
        }
        LateSocket {
            abandon,
            dropped,
            loans,
        }
    }
}
