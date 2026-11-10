//! A unit's outbox: one unit of the lifecycle's work takes the lifecycle's
//! one right to send before it moves its first owner, puts every message it
//! owes into an outbox whose places its type counts, and sends the outbox
//! whole. A refused message stays in the outbox with every message after
//! it, in order, and the outbox keeps the right: no unit follows, and the
//! retired lifecycle returns that remainder whole.

use super::*;

/// The most messages one unit of the lifecycle's work sends: a reply's
/// place, then its action's keyed frame (the lent key, the frame and the
/// key's recall). The outbox has that many places by type.
pub const UNSENT: usize = 4;

/// A message the actor's queues refused, or one after it in its unit: kept
/// whole, never sent.
#[allow(
    clippy::large_enum_variant,
    reason = "a unit's messages stay in place in its outbox until each is sent"
)]
pub enum Unsent<'id, D: DatapathPort<'id>> {
    /// For the actor's message queue.
    ToActor(ToActor<'id, D>),
    /// For the parcel queue.
    Parcel(crate::datapath::Parcel<'id, D>),
    /// For the link mailbox.
    Link(ActorLink<'id, D>),
}

/// An action's messages, in their order: at most three, by type.
pub(super) enum Messages<'id, D: DatapathPort<'id>> {
    /// None.
    None,
    /// One.
    One(Unsent<'id, D>),
    /// A message, then the one that announces it or follows it.
    Two(Unsent<'id, D>, ToActor<'id, D>),
    /// A keyed frame: the lent key, the frame and the key's recall.
    Three(ToActor<'id, D>, ToActor<'id, D>, ToActor<'id, D>),
}

impl<'id, D: DatapathPort<'id>> Messages<'id, D> {
    /// One message to the actor, or none.
    pub(super) fn to_actor(message: Option<ToActor<'id, D>>) -> Self {
        match message {
            Some(message) => Self::One(Unsent::ToActor(message)),
            None => Self::None,
        }
    }
}

/// The lifecycle's one right to send a unit's messages. Only
/// [`Sending::Open`] holds it between units; a unit takes it before it moves
/// its first owner and gives it back with its outbox.
pub(super) struct UnitRight(());

/// Where the lifecycle's right to send is.
pub(super) enum Sending<'id, D: DatapathPort<'id>> {
    /// No unit runs and none was refused.
    Open(UnitRight),
    /// A unit holds the right.
    Held,
    /// A unit's message was refused: its remainder, whole and in order.
    /// The right went with it, so no unit follows.
    Refused(Outbox<'id, D>),
}

impl<'id, D: DatapathPort<'id>> Sending<'id, D> {
    /// The right of a lifecycle that sent nothing yet.
    pub(super) const fn new() -> Self {
        Self::Open(UnitRight(()))
    }
    /// Take the right for one unit, when no unit holds it and none was
    /// refused.
    pub(super) fn take(&mut self) -> Option<UnitRight> {
        match core::mem::replace(self, Self::Held) {
            Self::Open(right) => Some(right),
            other => {
                *self = other;
                None
            }
        }
    }
    /// A refused unit's remainder, which takes the unit's right with it.
    pub(super) fn refused(right: UnitRight, outbox: Outbox<'id, D>) -> Self {
        let UnitRight(()) = right;
        Self::Refused(outbox)
    }
    /// The refused unit's remainder, when one was refused.
    pub(super) fn into_unsent(self) -> Option<Outbox<'id, D>> {
        match self {
            Self::Refused(outbox) => Some(outbox),
            Self::Open(_) | Self::Held => None,
        }
    }
}

/// One unit's messages, kept from the unit until each is sent; after a
/// refusal, what the unit did not send, whole and in order.
pub struct Outbox<'id, D: DatapathPort<'id>> {
    places: [Option<Unsent<'id, D>>; UNSENT],
}

impl<'id, D: DatapathPort<'id>> Outbox<'id, D> {
    /// The outbox of a unit: an optional `place` message, then its action's
    /// `messages`.
    pub(super) fn of(place: Option<ToActor<'id, D>>, messages: Messages<'id, D>) -> Self {
        let place = place.map(Unsent::ToActor);
        let places = match messages {
            Messages::None => [place, None, None, None],
            Messages::One(first) => [place, Some(first), None, None],
            Messages::Two(first, second) => {
                [place, Some(first), Some(Unsent::ToActor(second)), None]
            }
            Messages::Three(first, second, third) => [
                place,
                Some(Unsent::ToActor(first)),
                Some(Unsent::ToActor(second)),
                Some(Unsent::ToActor(third)),
            ],
        };
        Self { places }
    }
    /// The next message kept, in order: the refused one first.
    pub fn pop_front(&mut self) -> Option<Unsent<'id, D>> {
        self.places.iter_mut().find_map(Option::take)
    }
    /// Whether it keeps no message.
    pub fn is_empty(&self) -> bool {
        self.places.iter().all(Option::is_none)
    }
    /// Send each message in order through `actor`'s queues: the right back
    /// once all went, or the refusal's cause with the remainder kept here.
    pub(super) fn send<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
    ) -> Result<(), LifecycleViolation> {
        for place in &mut self.places {
            let Some(message) = place.take() else {
                continue;
            };
            let refused = match message {
                Unsent::ToActor(message) => actor
                    .to_actor
                    .try_send(message)
                    .map_err(|refused| refusal(refused, Unsent::ToActor)),
                Unsent::Parcel(parcel) => actor
                    .parcels
                    .try_send(parcel)
                    .map_err(|refused| refusal(refused, Unsent::Parcel)),
                Unsent::Link(link) => actor
                    .link
                    .try_send(link)
                    .map_err(|refused| refusal(refused, Unsent::Link)),
            };
            if let Err((violation, message)) = refused {
                *place = Some(message);
                return Err(violation);
            }
        }
        Ok(())
    }
}

/// A refused send's cause, a closed reception never named full, and its
/// message whole.
fn refusal<'id, D: DatapathPort<'id>, M>(
    refused: ClosingSpscSendError<M>,
    wrap: fn(M) -> Unsent<'id, D>,
) -> (LifecycleViolation, Unsent<'id, D>) {
    let violation = match refused {
        ClosingSpscSendError::Full(_) => LifecycleViolation::QueueFull,
        ClosingSpscSendError::Closed(_) => LifecycleViolation::ActorClosed,
    };
    (violation, wrap(refused.into_inner()))
}
