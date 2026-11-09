//! A local producer: work the network owner pulls from it, declares in the
//! core and lets it fill in the port's slot, all within the owner's turn.
//!
//! The service asks [`Produce::offer`] and makes each offer one
//! [`NetworkOwner::declare`](oer_protocols_network_core::NetworkOwner::declare),
//! the acceptance point: a refusal goes back through [`Produce::refused`],
//! and an accepted work has exactly one outcome, through [`Produce::ended`].
//! When the port gives the work a slot, [`Produce::fill`] writes its data
//! there, once, with no await between. The producer owns its data until
//! then; the core holds a place and no block.

use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll};

use oer_portable_events::{EventSink, report};
use oer_portable_memory::AtomicWaker;
use oer_portable_time::Instant;
use oer_protocols_network_core::{
    CommandId, CommandState, Declined, Ends, NetworkOwner, OpenRefused, Outcome, Remote,
    RetiredResults, SendRefusal, SocketId, WorkId,
};
use oer_protocols_network_port::FramePort;

use crate::observation::{LocalEvent, ServiceEvent};
use crate::service::TURN_FRAMES;

/// A source of datagrams the network owner calls in its own turn, on its own
/// hart: each call is synchronous and short.
///
/// The producer owns its data. Each work is named by a [`Self::Tag`] the
/// producer chooses, and a [`Work`] the service gives at acceptance. A
/// producer never reuses what an open tag names within one
/// [`Self::generation`]: it overwrites such data only in a new generation, or
/// keeps it until the work ends.
pub trait Produce {
    /// Names the data of one offer: typically its generation and its place.
    type Tag: Copy;

    /// The generation of the source. Any overwrite of what open tags name
    /// moves it; at its next turn the service then withdraws every declared
    /// work whose tag is not [`Self::current`].
    fn generation(&self) -> u32;

    /// Whether `tag`'s data is still the producer's to send. The service asks
    /// before every fill, after a generation change and about a held offer.
    fn current(&self, tag: Self::Tag) -> bool;

    /// The next datagram to declare, or when to ask again. An offer handed
    /// here is answered once: by [`Self::refused`], or, once declared, by one
    /// [`Self::ended`].
    ///
    /// # Errors
    /// [`Next`]: nothing now.
    fn offer(&mut self, now: Instant) -> Result<Offer<Self::Tag>, Next>;

    /// Write every octet of `work`'s data into `data`, which is exactly the
    /// declared length, in the port's slot. A fill consumes nothing: a
    /// refusal after it, by the device, means a later fill of the same work.
    ///
    /// # Errors
    /// [`Declined`]: the work ends [`Outcome::Declined`].
    fn fill(
        &mut self,
        now: Instant,
        work: Work<Self::Tag>,
        data: &mut [u8],
    ) -> Result<(), Declined>;

    /// The offer of `tag` was not declared, for `reason`. A final refusal
    /// ([`SendRefusal::TooLong`], [`SendRefusal::NoRoute`],
    /// [`SendRefusal::Stale`]) is the offer's one answer: the producer takes
    /// it out of what it offers until its own conditions change. A temporary
    /// one ([`SendRefusal::NoPlace`], [`SendRefusal::NeighborsFull`],
    /// [`SendRefusal::NoAddress`]) holds the producer's offers until what it
    /// lacks is released (a place, the neighbor counter, the address
    /// version), or until the
    /// held offer is stale: its generation moved or it is no longer
    /// [`Self::current`]. The producer keeps the refused data.
    fn refused(&mut self, tag: Self::Tag, reason: SendRefusal);

    /// `work` ended with `outcome`, its one answer. Its data leaves the
    /// producer here; an [`Outcome::Unknown`] is never sent again by the
    /// service.
    fn ended(&mut self, work: Work<Self::Tag>, outcome: Outcome);

    /// Make `cx` the one the source's next change wakes, as `interest`
    /// names: with [`Interest::Offers`] new data too; with
    /// [`Interest::Changes`] only a move of [`Self::generation`] or an open
    /// or held tag no longer [`Self::current`], never new data alone. Return
    /// whether data waits that [`Self::offer`] has not handed out, looked at
    /// after registering, so data that comes between the two is missed by
    /// neither. The service reads [`Self::generation`] and
    /// [`Self::current`] itself for the changes. A `true` is not also a wake.
    fn register(&mut self, cx: &mut Context<'_>, interest: Interest) -> bool;
}

/// What a waiting service can act on, which its producer's wake is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Interest {
    /// The service asks for offers at its next turn: new data is a reason
    /// to wake.
    Offers,
    /// The service cannot declare a next offer now: a refused offer holds,
    /// or its open works are at the turn's budget. What frees it is a place,
    /// the neighbor counter, the address version or a publication, each in a
    /// turn of its own; the
    /// producer wakes only when it makes an open or held work stale.
    Changes,
}

/// A datagram a producer has ready: where it goes, its length and its tag.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Offer<T> {
    pub remote: Remote,
    /// The UDP payload's octets.
    pub length: usize,
    pub tag: T,
}

/// When a producer with nothing to offer has more.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Next {
    /// At this time: the service's wait ends then.
    At(Instant),
    /// When the producer's own wake comes ([`Produce::register`]).
    Idle,
}

/// One accepted work: the identity the service gave it, and the producer's
/// tag of its data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Work<T> {
    pub id: WorkId,
    pub tag: T,
}

/// The end flag of a producer's service, in storage the image lends: set
/// once the service will call its producer no more, by its retirement or its
/// drop.
#[derive(Debug, Default)]
pub struct LocalEnd {
    ended: AtomicBool,
    waker: AtomicWaker,
}

impl LocalEnd {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            ended: AtomicBool::new(false),
            waker: AtomicWaker::new(),
        }
    }

    /// Whether the service ended: every outcome it knew came first.
    pub fn is_ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }

    /// Wait until the service ended. The flag has one waiter's cell: one
    /// task waits here at a time.
    pub async fn ended(&self) {
        poll_fn(|cx| {
            self.waker.register(cx.waker());
            if self.is_ended() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}

/// Sets the end flag when it goes: with the service's retirement, after
/// every outcome, or with its drop.
struct Ending<'a>(&'a LocalEnd);

impl Drop for Ending<'_> {
    fn drop(&mut self) {
        self.0.ended.store(true, Ordering::Release);
        self.0.waker.wake();
    }
}

impl core::fmt::Debug for Ending<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("Ending")
    }
}

/// Why a producer's socket did not open. A refusal changes nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalRefused {
    /// The core refused the socket.
    Open(OpenRefused),
    /// The producer has its socket already.
    Bound,
}

/// What a held offer waits for.
#[derive(Clone, Copy, Debug)]
enum Waits {
    /// One of the producer's places to free.
    Place,
    /// The core's neighbor change counter to move from this value.
    Neighbors(u32),
    /// The interface's address version to move from this value.
    Address(u32),
}

/// A temporary refusal's hold: it belongs to the refused offer.
#[derive(Clone, Copy, Debug)]
struct Hold<T> {
    tag: T,
    generation: u32,
    waits: Waits,
}

/// A service's producer, by value, with what the service keeps of it: its
/// socket, its open works and their tags, its hold, and the guard of its end
/// flag.
pub struct Producer<'a, B, P: Produce, const CREDITS: usize> {
    producer: P,
    socket: Option<SocketId<B>>,
    /// Where the core tells the socket's ends.
    ends: Option<Ends<B>>,
    /// Each open work by its core place.
    works: [Option<(CommandId<B>, P::Tag)>; CREDITS],
    /// The works open.
    open: usize,
    /// The generation the open works were last checked against.
    generation: u32,
    hold: Option<Hold<P::Tag>>,
    /// A fill declined this turn: the producer's flow is out of the rest of
    /// it.
    declined: bool,
    /// When the producer said it has more.
    at: Option<Instant>,
    /// The last offer step stopped before the producer said it had no more:
    /// at a hold or a budget, which no wake of the producer's names.
    stopped: bool,
    /// Last, so it goes after the producer.
    ending: Ending<'a>,
}

impl<'a, B, P: Produce, const CREDITS: usize> Producer<'a, B, P, CREDITS> {
    pub(crate) const fn new(producer: P, end: &'a LocalEnd) -> Self {
        Self {
            producer,
            socket: None,
            ends: None,
            works: [const { None }; CREDITS],
            open: 0,
            generation: 0,
            hold: None,
            declined: false,
            at: None,
            stopped: false,
            ending: Ending(end),
        }
    }

    pub(crate) const fn bound(&self) -> bool {
        self.socket.is_some()
    }

    pub(crate) fn bind(&mut self, socket: SocketId<B>) {
        self.ends = Some(socket.ends());
        self.socket = Some(socket);
    }
}

pub(crate) mod sealed {
    use super::{
        CommandId, CommandState, Context, Declined, Ends, EventSink, Instant, Remote,
        RetiredResults, SendRefusal, ServiceEvent, SocketId, WorkId,
    };

    /// What a producer's part of the service asks of the core.
    pub trait Owner<B> {
        fn declare(
            &mut self,
            now: Instant,
            socket: &SocketId<B>,
            remote: Remote,
            length: usize,
        ) -> Result<CommandId<B>, SendRefusal>;
        fn withdraw(&mut self, command: &CommandId<B>) -> bool;
        fn take_result(&mut self, command: CommandId<B>) -> CommandState<B>;
        fn next_end(&mut self, ends: &Ends<B>) -> Option<WorkId>;
        fn neighbors_changed(&self) -> u32;
        fn address_version(&self) -> u32;
    }

    /// A service's producers: none (`()`), or one [`super::Producer`].
    pub trait Local<B, const CREDITS: usize> {
        /// What retirement hands back.
        type Returned;

        /// A turn begins: no fill declined in it yet.
        fn turn_begins(&mut self);

        /// The offer step: withdraw what a new generation left stale,
        /// deliver what ended, then ask and declare within the hold rules
        /// and the turn's budget. Whether anything moved.
        fn offers(
            &mut self,
            now: Instant,
            owner: &mut impl Owner<B>,
            events: &mut impl EventSink<ServiceEvent>,
        ) -> bool;

        /// Fill `work`'s data in the port's slot, while its tag is current.
        ///
        /// # Errors
        /// [`Declined`].
        fn fill(&mut self, now: Instant, work: WorkId, data: &mut [u8]) -> Result<(), Declined>;

        /// A fill declined: the producer's flow is out of the rest of the
        /// turn.
        fn declined(&mut self);

        /// The flow the rest of the turn leaves out.
        fn filtered(&self) -> Option<u8>;

        /// Deliver every outcome the core knows; whether any came.
        fn deliver(
            &mut self,
            owner: &mut impl Owner<B>,
            events: &mut impl EventSink<ServiceEvent>,
        ) -> bool;

        /// Register the producer for what the service can act on now;
        /// whether some is here already.
        fn register(
            &mut self,
            cx: &mut Context<'_>,
            events: &mut impl EventSink<ServiceEvent>,
        ) -> bool;

        /// When the producer said it has more.
        fn deadline(&self) -> Option<Instant>;

        /// Whether the next turn would ask the producer for offers that the
        /// last turn stopped asking for: its hold or its budget freed since.
        /// No wake of the producer's names these.
        fn again(&self, owner: &impl Owner<B>) -> bool;

        /// The core retired: each open work's outcome, then the producer
        /// back, and only then the end flag.
        fn retire<const SOCKETS: usize>(
            self,
            results: &mut RetiredResults<B, SOCKETS, CREDITS>,
        ) -> Self::Returned;
    }
}

use sealed::{Local, Owner};

/// A service's producers: `()` for none, at no cost, or one [`Producer`].
pub trait Producers<B, const CREDITS: usize>: Local<B, CREDITS> {}

impl<B, const CREDITS: usize> Producers<B, CREDITS> for () {}

impl<B, P: Produce, const CREDITS: usize> Producers<B, CREDITS> for Producer<'_, B, P, CREDITS> {}

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
> Owner<B> for NetworkOwner<'m, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, BLOCKS, P>
{
    fn declare(
        &mut self,
        now: Instant,
        socket: &SocketId<B>,
        remote: Remote,
        length: usize,
    ) -> Result<CommandId<B>, SendRefusal> {
        Self::declare(self, now, socket, remote, length)
    }

    fn withdraw(&mut self, command: &CommandId<B>) -> bool {
        Self::withdraw(self, command)
    }

    fn take_result(&mut self, command: CommandId<B>) -> CommandState<B> {
        Self::take_result(self, command)
    }

    fn next_end(&mut self, ends: &Ends<B>) -> Option<WorkId> {
        Self::next_end(self, ends)
    }

    fn neighbors_changed(&self) -> u32 {
        Self::neighbors_changed(self)
    }

    fn address_version(&self) -> u32 {
        Self::address_version(self)
    }
}

impl<B, const CREDITS: usize> Local<B, CREDITS> for () {
    type Returned = ();

    #[inline(always)]
    fn turn_begins(&mut self) {}

    #[inline(always)]
    fn offers(
        &mut self,
        _now: Instant,
        _owner: &mut impl Owner<B>,
        _events: &mut impl EventSink<ServiceEvent>,
    ) -> bool {
        false
    }

    /// No work of a service without a producer is declared, so no frame
    /// asks for this.
    #[inline(always)]
    fn fill(&mut self, _now: Instant, _work: WorkId, _data: &mut [u8]) -> Result<(), Declined> {
        Err(Declined::Stale)
    }

    #[inline(always)]
    fn declined(&mut self) {}

    #[inline(always)]
    fn filtered(&self) -> Option<u8> {
        None
    }

    #[inline(always)]
    fn deliver(
        &mut self,
        _owner: &mut impl Owner<B>,
        _events: &mut impl EventSink<ServiceEvent>,
    ) -> bool {
        false
    }

    #[inline(always)]
    fn register(
        &mut self,
        _cx: &mut Context<'_>,
        _events: &mut impl EventSink<ServiceEvent>,
    ) -> bool {
        false
    }

    #[inline(always)]
    fn deadline(&self) -> Option<Instant> {
        None
    }

    #[inline(always)]
    fn again(&self, _owner: &impl Owner<B>) -> bool {
        false
    }

    #[inline(always)]
    fn retire<const SOCKETS: usize>(self, _results: &mut RetiredResults<B, SOCKETS, CREDITS>) {}
}

impl<B, P: Produce, const CREDITS: usize> Local<B, CREDITS> for Producer<'_, B, P, CREDITS> {
    type Returned = P;

    fn turn_begins(&mut self) {
        self.declined = false;
    }

    fn offers(
        &mut self,
        now: Instant,
        owner: &mut impl Owner<B>,
        events: &mut impl EventSink<ServiceEvent>,
    ) -> bool {
        self.at = None;
        self.stopped = false;
        if self.socket.is_none() {
            return false;
        }
        let mut moved = false;
        let generation = self.producer.generation();
        if generation != self.generation {
            self.generation = generation;
            let producer = &self.producer;
            for (command, tag) in self.works.iter().flatten() {
                if !producer.current(*tag) && owner.withdraw(command) {
                    report(events, || ServiceEvent::Local(LocalEvent::Withdrawn));
                    moved = true;
                }
            }
        }
        // What a withdrawal or a tick ended frees its place first.
        moved |= self.deliver(owner, events);
        if let Some(hold) = &self.hold {
            let released = match hold.waits {
                Waits::Place => self.open < CREDITS,
                Waits::Neighbors(seen) => owner.neighbors_changed() != seen,
                Waits::Address(seen) => owner.address_version() != seen,
            };
            // A hold belongs to its refused offer: a stale one gives way.
            if released || hold.generation != generation || !self.producer.current(hold.tag) {
                self.hold = None;
            } else {
                self.stopped = true;
                return moved;
            }
        }
        let Some(socket) = self.socket.as_ref() else {
            return moved;
        };
        for _ in 0..TURN_FRAMES {
            if self.open >= TURN_FRAMES {
                self.stopped = true;
                return moved;
            }
            let offer = match self.producer.offer(now) {
                Ok(offer) => offer,
                Err(Next::At(at)) => {
                    self.at = Some(at);
                    return moved;
                }
                Err(Next::Idle) => return moved,
            };
            // A refusal handled is progress too: the next pass yields.
            moved = true;
            report(events, || ServiceEvent::Local(LocalEvent::Offered));
            match owner.declare(now, socket, offer.remote, offer.length) {
                Ok(command) => {
                    // The core numbers this socket's places below `CREDITS`.
                    if let Some(place) = self.works.get_mut(command.work().place()) {
                        *place = Some((command, offer.tag));
                        self.open = self.open.saturating_add(1);
                    }
                }
                Err(reason) => {
                    self.producer.refused(offer.tag, reason);
                    let waits = match reason {
                        SendRefusal::NoPlace => Waits::Place,
                        SendRefusal::NeighborsFull => Waits::Neighbors(owner.neighbors_changed()),
                        SendRefusal::NoAddress => Waits::Address(owner.address_version()),
                        SendRefusal::TooLong | SendRefusal::NoRoute | SendRefusal::Stale => {
                            report(events, || ServiceEvent::Local(LocalEvent::Refused));
                            continue;
                        }
                    };
                    report(events, || ServiceEvent::Local(LocalEvent::Held));
                    self.hold = Some(Hold {
                        tag: offer.tag,
                        generation,
                        waits,
                    });
                    self.stopped = true;
                    return moved;
                }
            }
        }
        // The budget is spent with more to ask: the next turn asks again.
        self.stopped = true;
        true
    }

    fn fill(&mut self, now: Instant, work: WorkId, data: &mut [u8]) -> Result<(), Declined> {
        let tag = self
            .works
            .get(work.place())
            .and_then(Option::as_ref)
            .filter(|(command, _)| command.work() == work)
            .map(|(_, tag)| *tag)
            .ok_or(Declined::Stale)?;
        // The guarantee of identity: a source that moved on has no data of
        // this work's any more.
        if !self.producer.current(tag) {
            return Err(Declined::Stale);
        }
        self.producer.fill(now, Work { id: work, tag }, data)
    }

    fn declined(&mut self) {
        self.declined = true;
    }

    fn filtered(&self) -> Option<u8> {
        self.socket
            .as_ref()
            .filter(|_| self.declined)
            .map(SocketId::flow)
    }

    fn deliver(
        &mut self,
        owner: &mut impl Owner<B>,
        _events: &mut impl EventSink<ServiceEvent>,
    ) -> bool {
        let Some(ends) = self.ends else {
            return false;
        };
        let mut moved = false;
        // Only what the core told ended: open works are not looked at.
        while let Some(work) = owner.next_end(&ends) {
            // The generation decides: a told end that is not this work's
            // leaves the work where it is.
            let Some(slot) = self.works.get_mut(work.place()).filter(|slot| {
                slot.as_ref()
                    .is_some_and(|(command, _)| command.work() == work)
            }) else {
                continue;
            };
            let Some((command, tag)) = slot.take() else {
                continue;
            };
            let outcome = match owner.take_result(command) {
                // An end is told once it is a result, so this keeps the work.
                CommandState::Running(command) => {
                    *slot = Some((command, tag));
                    continue;
                }
                CommandState::Finished(outcome) => outcome,
                // Not this core's: nothing says what became of it.
                CommandState::Gone => Outcome::Unknown,
            };
            self.open = self.open.saturating_sub(1);
            self.producer.ended(Work { id: work, tag }, outcome);
            moved = true;
        }
        moved
    }

    fn register(
        &mut self,
        cx: &mut Context<'_>,
        events: &mut impl EventSink<ServiceEvent>,
    ) -> bool {
        if self.socket.is_none() {
            return false;
        }
        // Held, or at the budget of open works, the service asks nothing at
        // its next turn: what frees it comes in a turn of its own.
        let interest = if self.hold.is_some() || self.open >= TURN_FRAMES {
            Interest::Changes
        } else {
            Interest::Offers
        };
        report(events, || ServiceEvent::Local(LocalEvent::Waits(interest)));
        let data = self.producer.register(cx, interest);
        // What the next turn acts on, read after registering: new data while
        // it would ask, a new generation, or a hold gone stale.
        let generation = self.producer.generation();
        let stale = self
            .hold
            .as_ref()
            .is_some_and(|hold| hold.generation != generation || !self.producer.current(hold.tag));
        (interest == Interest::Offers && data) || generation != self.generation || stale
    }

    fn deadline(&self) -> Option<Instant> {
        self.at
    }

    fn again(&self, owner: &impl Owner<B>) -> bool {
        if !self.stopped || self.socket.is_none() || self.open >= TURN_FRAMES {
            return false;
        }
        self.hold.as_ref().is_none_or(|hold| {
            let released = match hold.waits {
                Waits::Place => self.open < CREDITS,
                Waits::Neighbors(seen) => owner.neighbors_changed() != seen,
                Waits::Address(seen) => owner.address_version() != seen,
            };
            released
                || hold.generation != self.producer.generation()
                || !self.producer.current(hold.tag)
        })
    }

    fn retire<const SOCKETS: usize>(self, results: &mut RetiredResults<B, SOCKETS, CREDITS>) -> P {
        let Self {
            mut producer,
            works,
            ending,
            ..
        } = self;
        for (command, tag) in works.into_iter().flatten() {
            let id = command.work();
            let outcome = match results.take(command) {
                CommandState::Finished(outcome) => outcome,
                CommandState::Running(_) | CommandState::Gone => Outcome::Unknown,
            };
            producer.ended(Work { id, tag }, outcome);
        }
        // After every outcome.
        drop(ending);
        producer
    }
}
