//! What a service tells a sink that counts its turns: the requests a turn took,
//! the frames its port accepted, how the turn ended, and how each pass of
//! [`Service::run`](crate::Service::run) gave up its poll.
//!
//! The service reports through the sink [`Service::run`](crate::Service::run)
//! is lent, as the core reports its drops. A disabled sink, `()` among them,
//! hears nothing and the service builds no event: the report points leave no
//! code. No event reads a clock.

use oer_portable_events::EventSink;

/// A request a turn takes from a socket's lane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestKind {
    /// A reservation of `blocks`, one to
    /// [`RESERVE_AHEAD`](crate::RESERVE_AHEAD), granted one by one later.
    Reserve {
        blocks: u8,
    },
    Release,
    Send,
    Close,
}

/// How a turn's publishing ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishEnd {
    /// The policy chose nothing: no work was ready, or none may go now.
    Chose,
    /// The port refused a frame for room or link: the work stays ready.
    Refused,
    /// Publishing took its whole budget of frames: more may be ready.
    Budget,
    /// Notices were left untold, so this turn published nothing.
    Deferred,
    /// The port contradicted itself, or the policy cannot go on.
    Fault,
}

/// What a turn did with its local producer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalEvent {
    /// The producer offered a datagram.
    Offered,
    /// The core refused an offer for good.
    Refused,
    /// The core refused an offer for now: the producer's offers hold.
    Held,
    /// A fill declined.
    Declined,
    /// A declared work was withdrawn, its source having moved on.
    Withdrawn,
    /// A wait registered the producer's waker, for this.
    Waits(crate::Interest),
}

/// How a policy's choice ended ([`Choice`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Chosen {
    /// It chose a work, which the port is asked for next.
    Work,
    /// It chose nothing.
    Nothing,
    /// The policy cannot go on.
    Fault,
}

/// One choice of the policy, as the service saw it: the works the policy
/// pulled from the ready list (after the producer's filter), over every pass
/// it made, and the passes it began. Counts past the field's range read its
/// largest value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Choice {
    pub examined: u16,
    pub walks: u8,
    pub chosen: Chosen,
}

/// Why the port published nothing of a chosen work: the core's [`Unsent`],
/// without its port's reason.
///
/// [`Unsent`]: oer_protocols_network_core::Unsent
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsentKind {
    Stale,
    Port,
    Rejected,
    SlotTooShort,
    Declined,
    FillMisreported,
    PortContract,
}

impl UnsentKind {
    /// The kind of `unsent`.
    pub const fn of<R>(unsent: &oer_protocols_network_core::Unsent<R>) -> Self {
        use oer_protocols_network_core::Unsent;
        match unsent {
            Unsent::Stale => Self::Stale,
            Unsent::Port(_) => Self::Port,
            Unsent::Rejected(_) => Self::Rejected,
            Unsent::SlotTooShort => Self::SlotTooShort,
            Unsent::Declined => Self::Declined,
            Unsent::FillMisreported(_) => Self::FillMisreported,
            Unsent::PortContract => Self::PortContract,
        }
    }
}

/// What a turn brought home before its received frames, when anything came:
/// the blocks the return lane held, and the loans of lent buffers back with
/// their port that it gave back to the pool's account. Counts past the
/// field's range read its largest value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Home {
    pub blocks: u16,
    pub loans: u16,
}

/// The pool's receive loans when a received frame went unlent for want of
/// its socket's credit, read once the frame was taken in: the loans out of
/// the account, old installations' included, and the lent buffers already
/// back with their port whose loans the service has not yet given back.
/// `drops` frames of the one take went unlent so. Counts past the field's
/// range read its largest value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Credit {
    pub drops: u8,
    pub outstanding: u16,
    pub returned: u16,
}

/// What was free when a turn ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurnEnd {
    /// The pool had a free block.
    pub blocks_free: bool,
    /// Every open socket had a free credit.
    pub credits_free: bool,
    /// Receive loans out of the pool's account, old installations'
    /// included.
    pub loans: usize,
}

/// What a service did, for a sink that counts. A turn reports
/// [`Self::TurnBegan`] first, its requests as it takes them, then
/// [`Self::BudgetSpent`] for each socket it left requests in, [`Self::Home`]
/// when anything came home, one [`Self::FrameTaken`] for each received frame
/// it took in, with [`Self::NoCredit`] after a take that went unlent for want
/// of credit, then [`Self::IntakeFull`] when it took in its whole budget of
/// received frames. Its notices run from [`Self::NoticesBegan`] to
/// [`Self::NoticesEnded`]: one [`Self::Noticed`] for each notice of the port
/// it handed the policy, and [`Self::Settled`] once the policy took it in.
/// Each attempt to publish runs from [`Self::ChoiceBegan`] through the
/// policy's [`Self::Egress`] events to [`Self::Chose`]; a chosen work then
/// goes to the port, which ends with [`Self::Accepted`] or [`Self::Unsent`],
/// and the policy's answer with [`Self::Answered`]. Publishing ends with one
/// [`Self::Published`], and [`Self::Flushed`] when it moved anything; last
/// comes [`Self::TurnEnded`]. Between turns, [`Self::Local`] counts what a
/// turn did with its producer, and what a wait registered it for.
/// [`Self::Yielded`] or [`Self::Waited`] says how the pass after the turn
/// went on.
///
/// The pairs of events around a step let a sink that reads a clock at each
/// event time the step: the events of one step are reported at the same
/// points whatever the policy, so the steps of two policies have the same
/// bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceEvent {
    /// A turn begins.
    TurnBegan,
    /// The turn took this request from a socket's lane.
    Took(RequestKind),
    /// A socket's lane still held requests once the turn had taken its budget
    /// of them: the next turn finds them.
    BudgetSpent,
    /// What the turn brought home before its received frames.
    Home(Home),
    /// The turn took in one received frame from the port.
    FrameTaken,
    /// A take of a received frame left frames unlent for want of credit.
    NoCredit(Credit),
    /// The turn took in its whole budget of received frames: more may wait.
    IntakeFull,
    /// The turn begins handing the policy the port's notices.
    NoticesBegan,
    /// The turn handed the policy one notice of the port: one settlement of
    /// one transmitted frame.
    Noticed,
    /// The policy took the notice in.
    Settled,
    /// The turn handed the policy every notice it will this turn.
    NoticesEnded,
    /// An attempt to publish begins: the ready list, then the policy's
    /// choice.
    ChoiceBegan,
    /// What the policy reported of its choice.
    Egress(oer_protocols_network_port::EgressEvent),
    /// The policy's choice ended.
    Chose(Choice),
    /// The port accepted a frame.
    Accepted,
    /// The port published nothing of the chosen work.
    Unsent(UnsentKind),
    /// The policy answered the attempt.
    Answered,
    /// How the turn's publishing ended.
    Published(PublishEnd),
    /// Publishing moved something and flushed the port: one ring of what
    /// waits for its frames.
    Flushed,
    /// The turn ended.
    TurnEnded(TurnEnd),
    /// What the turn, or the wait after it, did with its producer.
    Local(LocalEvent),
    /// After a turn that moved something, the task gave its executor a turn:
    /// its poll ends here, the task woken again.
    Yielded,
    /// After a turn that moved nothing, the wait registered every wake. With
    /// `ready`, a lane, the port or a stop had something then, and the next
    /// turn runs within the same poll; without, the poll ends here unless
    /// the wait's deadline was already due.
    Waited { ready: bool },
}

/// Report what `event` builds to `sink`, if anything. The closure runs only
/// for an enabled sink, so a disabled one pays for neither the event nor
/// what decides it.
#[inline(always)]
pub(crate) fn seen<S: EventSink<ServiceEvent> + ?Sized>(
    sink: &mut S,
    event: impl FnOnce() -> Option<ServiceEvent>,
) {
    if S::ENABLED
        && let Some(event) = event()
    {
        sink.event(event);
    }
}

/// A count past `u16` reads its largest value.
pub(crate) fn short(count: usize) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

/// What a choice's passes over the ready list pulled: the works and the
/// passes begun, which [`Counted`] adds to.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Walked {
    examined: u32,
    walks: u32,
}

impl Walked {
    /// The choice that ended `chosen`, with these counts.
    pub(crate) fn choice(self, chosen: Chosen) -> Choice {
        Choice {
            examined: u16::try_from(self.examined).unwrap_or(u16::MAX),
            walks: u8::try_from(self.walks).unwrap_or(u8::MAX),
            chosen,
        }
    }
}

/// The ready list as a measuring service hands it to its policy: each pass
/// begun and each work pulled adds to `walked`. A clone is a pass of its own,
/// counted once it pulls.
pub(crate) struct Counted<'c, I> {
    ready: I,
    begun: bool,
    walked: &'c core::cell::Cell<Walked>,
}

impl<'c, I> Counted<'c, I> {
    pub(crate) const fn new(ready: I, walked: &'c core::cell::Cell<Walked>) -> Self {
        Self {
            ready,
            begun: false,
            walked,
        }
    }
}

impl<I: Clone> Clone for Counted<'_, I> {
    fn clone(&self) -> Self {
        Self {
            ready: self.ready.clone(),
            begun: false,
            walked: self.walked,
        }
    }
}

impl<I: Iterator> Iterator for Counted<'_, I> {
    type Item = I::Item;

    fn next(&mut self) -> Option<I::Item> {
        let mut walked = self.walked.get();
        if !self.begun {
            self.begun = true;
            walked.walks = walked.walks.saturating_add(1);
        }
        let work = self.ready.next();
        if work.is_some() {
            walked.examined = walked.examined.saturating_add(1);
        }
        self.walked.set(walked);
        work
    }
}

/// The policy's events, as the service's: each one goes to `sink` as
/// [`ServiceEvent::Egress`].
pub(crate) struct ToService<'s, S: ?Sized>(pub(crate) &'s mut S);

impl<S: EventSink<ServiceEvent> + ?Sized> EventSink<oer_protocols_network_port::EgressEvent>
    for ToService<'_, S>
{
    const ENABLED: bool = S::ENABLED;

    #[inline(always)]
    fn event(&mut self, event: oer_protocols_network_port::EgressEvent) {
        self.0.event(ServiceEvent::Egress(event));
    }
}

/// The core's drops on their way to `sink`, with the frames left unlent for
/// want of credit counted.
pub(crate) struct CreditSeen<'s, S: ?Sized> {
    sink: &'s mut S,
    drops: u8,
}

impl<'s, S: ?Sized> CreditSeen<'s, S> {
    pub(crate) const fn new(sink: &'s mut S) -> Self {
        Self { sink, drops: 0 }
    }

    pub(crate) const fn drops(&self) -> u8 {
        self.drops
    }
}

impl<S: EventSink<oer_protocols_network_core::NetEvent> + ?Sized>
    EventSink<oer_protocols_network_core::NetEvent> for CreditSeen<'_, S>
{
    const ENABLED: bool = S::ENABLED;

    #[inline(always)]
    fn event(&mut self, event: oer_protocols_network_core::NetEvent) {
        use oer_protocols_network_core::{NetEvent, NotLent};
        if event == NetEvent::NotLent(NotLent::NoCredit) {
            self.drops = self.drops.saturating_add(1);
        }
        self.sink.event(event);
    }
}
