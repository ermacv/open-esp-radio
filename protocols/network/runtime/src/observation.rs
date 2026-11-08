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

/// What was free when a turn ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurnEnd {
    /// The pool had a free block.
    pub blocks_free: bool,
    /// Every open socket had a free credit.
    pub credits_free: bool,
}

/// What a service did, for a sink that counts. A turn reports its requests
/// as it takes them, then [`Self::BudgetSpent`] for each socket it left
/// requests in, then [`Self::IntakeFull`] when it took in its whole budget of
/// received frames, one [`Self::Accepted`] for each frame the port took, one
/// [`Self::Published`] and last [`Self::TurnEnded`]. Between turns,
/// [`Self::Yielded`] or [`Self::Waited`] says how the pass after the turn
/// went on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceEvent {
    /// The turn took this request from a socket's lane.
    Took(RequestKind),
    /// A socket's lane still held requests once the turn had taken its budget
    /// of them: the next turn finds them.
    BudgetSpent,
    /// The turn took in its whole budget of received frames: more may wait.
    IntakeFull,
    /// The port accepted a frame.
    Accepted,
    /// How the turn's publishing ended.
    Published(PublishEnd),
    /// The turn ended.
    TurnEnded(TurnEnd),
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
