//! The one exchange between the console's task and the image's worker: a
//! request goes to the worker, and its one answer comes back.
//!
//! Two places of depth one (`portable/memory`'s waking SPSC queue), one each
//! way. The worker's end gives each request with its [`Reply`], the permit
//! of that request's identity: answering consumes it, and it borrows the
//! worker's end, so the worker takes no next request before it answered or
//! dropped the reply. The console hands a request only while none is in
//! work and takes the answer before it hands the next, so the answer's
//! place is free whenever a reply exists: answering neither waits nor
//! fails.

use core::convert::Infallible;

use oer_portable_memory::{
    WakingSpscQueue, WakingSpscReceiver, WakingSpscSendPermit, WakingSpscSender,
};
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

/// The outcome of one request, as the worker's reply left it.
pub(crate) enum Outcome<T> {
    /// The encoded answer, numbered and framed by the console.
    Encoded(Outbound, After<T>),
    /// The response could not be encoded.
    Unencodable,
    /// The reply was dropped without an answer.
    Unanswered,
}

/// One request's outcome, back to the console.
pub(crate) struct Answered<T> {
    pub(crate) identity: RequestIdentity,
    pub(crate) outcome: Outcome<T>,
}

type Handed<R> = (RequestIdentity, R);
type Back<R> = Answered<<R as Requests>::Trap>;

/// The two places between the console's task and the worker, for the image's
/// service set `R`. The image places it once and splits it once.
pub struct Mailbox<R: Requests> {
    requests: WakingSpscQueue<Handed<R>, 1>,
    answers: WakingSpscQueue<Back<R>, 1>,
}

impl<R: Requests> Default for Mailbox<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Requests> Mailbox<R> {
    pub const fn new() -> Self {
        Self {
            requests: WakingSpscQueue::new(),
            answers: WakingSpscQueue::new(),
        }
    }

    /// The console's end and the worker's end.
    pub fn split(&mut self) -> (Console<'_, R>, Inbox<'_, R>) {
        let (requests, taken) = self.requests.split();
        let (answers, back) = self.answers.split();
        (
            Console {
                requests,
                answers: back,
            },
            Inbox {
                requests: taken,
                answers,
            },
        )
    }
}

/// The console's end: it hands requests over and takes their answers.
pub struct Console<'a, R: Requests> {
    pub(crate) requests: WakingSpscSender<'a, Handed<R>, 1>,
    pub(crate) answers: WakingSpscReceiver<'a, Back<R>, 1>,
}

/// The worker's end: each request with the permit of its answer.
pub struct Inbox<'a, R: Requests> {
    requests: WakingSpscReceiver<'a, Handed<R>, 1>,
    answers: WakingSpscSender<'a, Back<R>, 1>,
}

impl<R: Requests> Inbox<'_, R> {
    /// The next request and its [`Reply`], once the console hands one over.
    /// The reply borrows this end: no next request before it is consumed.
    ///
    /// ```compile_fail,E0499
    /// use oer_protocols_hil_agent::base::NoRequests;
    /// use oer_protocols_hil_agent::mailbox::Mailbox;
    /// async fn two_in_work() {
    ///     let mut mailbox = Mailbox::<NoRequests>::new();
    ///     let (_console, mut inbox) = mailbox.split();
    ///     let (_, first) = inbox.next().await;
    ///     let (_, second) = inbox.next().await;
    ///     drop((first, second));
    /// }
    /// ```
    ///
    /// A reply comes only from here: an answer without a request does not
    /// compile.
    ///
    /// ```compile_fail,E0451
    /// use core::convert::Infallible;
    /// use oer_protocols_hil_agent::mailbox::Reply;
    /// fn forged(identity: oer_protocols_hil_protocol::RequestIdentity) -> Reply<'static, Infallible> {
    ///     Reply { identity, place: None }
    /// }
    /// ```
    pub async fn next(&mut self) -> (R, Reply<'_, R::Trap>) {
        let (identity, request) = loop {
            self.requests.ready().await;
            if let Some(entry) = self.requests.try_receive() {
                break entry;
            }
        };
        // The console took the previous answer before it handed this
        // request, so the place is free and this wait ends at once.
        let place = self.answers.ready().await;
        (
            request,
            Reply {
                identity,
                place: Some(place),
            },
        )
    }
}

/// The permit of one request's answer: consumed by answering. Dropped
/// without an answer, it ends the request as unanswered, and the console
/// refuses it to the host.
#[must_use = "answer the request; a dropped reply is refused to the host"]
pub struct Reply<'a, T> {
    identity: RequestIdentity,
    /// The answer's place, held until the outcome is sent. Rust moves no
    /// field out of a type with `Drop` without `unsafe`: the place is taken
    /// once, by the answer or by the drop, and is never refilled.
    place: Option<WakingSpscSendPermit<'a, Answered<T>, 1>>,
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
    pub fn answer_then<M: Message>(mut self, body: &M, after: After<T>) {
        let outcome =
            match Outbound::new(0, self.identity.session_id, self.identity.request_id, body) {
                Ok(message) => Outcome::Encoded(message, after),
                Err(_) => Outcome::Unencodable,
            };
        self.end(outcome);
    }

    fn end(&mut self, outcome: Outcome<T>) {
        if let Some(place) = self.place.take() {
            // The encoded answer is a payload: copied into its place here and
            // out of it by the console.
            place.send(Answered {
                identity: self.identity,
                outcome,
            });
        }
    }
}

impl<T> Drop for Reply<'_, T> {
    fn drop(&mut self) {
        self.end(Outcome::Unanswered);
    }
}

#[cfg(test)]
mod tests;
