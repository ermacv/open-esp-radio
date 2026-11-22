//! The one exchange between the console's task and the image's worker: a
//! request goes to the worker, and its one answer comes back.
//!
//! It is `portable/memory`'s [`Exchange`] with the request's identity and the
//! image's request as its request and HIL's outcome as its answer. The
//! worker's end gives each request with its [`Reply`], the permit of that
//! request's identity: answering consumes it, and it borrows the worker's
//! end, so the worker takes no next request before it answered or dropped
//! the reply. A reply dropped without an answer leaves the request owed, and
//! the worker's next request pays it first as unanswered. The console hands
//! a request only while none is in work and keeps its identity until the
//! answer comes back.

use core::convert::Infallible;

use oer_portable_memory::{Answering, Asking, Exchange};
use oer_protocols_hil_protocol::{Message, Outbound, RequestIdentity};

use crate::base::Requests;

/// A trap on an illegal instruction after an answer's write: what only a
/// service set that declares it (the fault service) may ask for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Trap;

/// What a service set may ask after an answer's write: [`Trap`], or
/// `Infallible` for a set that never traps. Two sets together trap when
/// either does.
pub trait Traps: Sized {
    type Or<B: Traps>: Traps;
}

impl Traps for Infallible {
    type Or<B: Traps> = B;
}

impl Traps for Trap {
    type Or<B: Traps> = Trap;
}

/// What the console does once it wrote an answer whole.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum After<T> {
    Nothing,
    /// End the console's run with this trap.
    Trap(T),
}

/// The outcome of one request, as the worker's reply gave it.
#[expect(
    clippy::large_enum_variant,
    reason = "the answer's place holds one encoded message by value; the other outcome carries nothing"
)]
pub(crate) enum Outcome<T> {
    /// The encoded answer, numbered and framed by the console.
    Encoded(Outbound, After<T>),
    /// The response could not be encoded.
    Unencodable,
}

type Handed<R> = (RequestIdentity, R);
type Back<R> = Outcome<<R as Requests>::Trap>;

/// The exchange between the console's task and the worker, for the image's
/// service set `R`. The image places it once and splits it once.
pub struct Mailbox<R: Requests>(Exchange<Handed<R>, Back<R>>);

impl<R: Requests> Default for Mailbox<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Requests> Mailbox<R> {
    pub const fn new() -> Self {
        Self(Exchange::new())
    }

    /// The console's end and the worker's end.
    pub fn split(&mut self) -> (Console<'_, R>, Inbox<'_, R>) {
        let (asking, answering) = self.0.split();
        (Console(asking), Inbox(answering))
    }
}

/// The console's end: it hands requests over and takes their answers.
pub struct Console<'a, R: Requests>(pub(crate) Asking<'a, Handed<R>, Back<R>>);

/// The worker's end: each request with the permit of its answer.
pub struct Inbox<'a, R: Requests>(Answering<'a, Handed<R>, Back<R>>);

impl<R: Requests> Inbox<'_, R> {
    /// The next request and its [`Reply`], once the console hands one over.
    /// The reply borrows this end: no next request before it is consumed.
    ///
    /// A reply dropped without an answer, by the worker or with a handler's
    /// future cancelled at an await, leaves its request owed: this call
    /// pays it first, as unanswered, which the console refuses to the host.
    /// A worker that drops a reply and never asks again leaves the host to
    /// its timeout; in every image a worker that stops asking has ended,
    /// and `serve` ends with it.
    pub async fn next(&mut self) -> (R, Reply<'_, R::Trap>) {
        let ((identity, request), inner) = self.0.next().await;
        (request, Reply { identity, inner })
    }
}

/// The permit of one request's answer: consumed by answering. Dropped
/// without an answer, it leaves the request owed, and the worker's next
/// request pays it as unanswered ([`Inbox::next`]).
#[must_use = "answer the request; a dropped reply is refused to the host at the next request"]
pub struct Reply<'a, T> {
    identity: RequestIdentity,
    inner: oer_portable_memory::Reply<'a, Outcome<T>>,
}

impl<T> Reply<'_, T> {
    /// The request this reply answers.
    pub const fn identity(&self) -> RequestIdentity {
        self.identity
    }

    /// Answers the request with `body`.
    pub fn answer<M: Message>(self, body: &M) {
        self.answer_then(body, After::Nothing);
    }

    /// Answers the request with `body`; once the console wrote it whole, it
    /// does `after`.
    pub fn answer_then<M: Message>(self, body: &M, after: After<T>) {
        let outcome =
            match Outbound::new(0, self.identity.session_id, self.identity.request_id, body) {
                Ok(message) => Outcome::Encoded(message, after),
                Err(_) => Outcome::Unencodable,
            };
        // The encoded answer is a payload: copied into its place here and out
        // of it by the console.
        self.inner.answer(outcome);
    }
}

#[cfg(test)]
mod tests;
