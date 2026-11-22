//! One request in work at a time between an asking task and an answering
//! task, and its one answer back.
//!
//! An [`Exchange`] is two places of depth one, built of [`WakingSpscQueue`]
//! alone: requests one way, answers the other. It splits once into the
//! asking end ([`Asking`]) and the answering end ([`Answering`]), each held
//! by its one task. The answering end gives each request with its
//! [`Reply`], the permit of the answer's place: answering consumes it, and it
//! borrows the answering end, so no next request is taken before it is
//! answered or dropped. The asker hands a request while none is in work
//! and takes its answer before it hands the next, so the answer's place is
//! free whenever a reply exists: answering neither waits nor fails.
//!
//! A [`Reply`] has no `Drop` and holds no slot. The answering end records
//! what it owes from the moment it takes a request; a reply dropped without
//! an answer (by its task, or in a future cancelled at an await) leaves the
//! request owed, and the next [`Answering::next`] pays it first as
//! [`Answered::Unanswered`]. The asker knows which request it asked: the
//! exchange carries no identity.
//!
//! The ends wake through their notifies: `N` the answering end's (a request
//! arrived, the answer's place was freed), `M` the asking end's (an answer
//! arrived, the request's place was freed). [`AtomicWaker`] is the default;
//! an end woken from a level below its own names a
//! [`PriorityWaker`](oer_portable_sync::PriorityWaker) instead.

use oer_portable_waker::AtomicWaker;

use crate::{
    QueueNotify, WakingSpscQueue, WakingSpscReceiver, WakingSpscSendPermit, WakingSpscSender,
};

/// What came back for a request.
#[derive(Debug, Eq, PartialEq)]
pub enum Answered<A> {
    Given(A),
    /// Its reply was dropped without an answer.
    Unanswered,
}

/// The two places, for requests `Q` and answers `A`. The composition places
/// it once and splits it once.
pub struct Exchange<Q, A, N = AtomicWaker, M = AtomicWaker> {
    requests: WakingSpscQueue<Q, 1, N, M>,
    answers: WakingSpscQueue<Answered<A>, 1, M, N>,
}

impl<Q, A> Exchange<Q, A> {
    /// An exchange whose two ends wake through ordinary wakers.
    pub const fn new() -> Self {
        Self::with_notifies(
            AtomicWaker::new(),
            AtomicWaker::new(),
            AtomicWaker::new(),
            AtomicWaker::new(),
        )
    }
}

impl<Q, A> Default for Exchange<Q, A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Q, A, N: QueueNotify, M: QueueNotify> Exchange<Q, A, N, M> {
    /// An exchange whose answering end wakes through `requests_arrive` and
    /// `answer_room`, and whose asking end through `answers_arrive` and
    /// `request_room`.
    pub const fn with_notifies(
        requests_arrive: N,
        answer_room: N,
        answers_arrive: M,
        request_room: M,
    ) -> Self {
        Self {
            requests: WakingSpscQueue::across_harts(requests_arrive, request_room),
            answers: WakingSpscQueue::across_harts(answers_arrive, answer_room),
        }
    }

    /// The asking end and the answering end.
    pub fn split(&mut self) -> Ends<'_, Q, A, N, M> {
        let (requests, taken) = self.requests.split();
        let (answers, back) = self.answers.split();
        (
            Asking {
                requests,
                answers: back,
            },
            Answering {
                requests: taken,
                answers,
                debt: Debt::Paid,
            },
        )
    }
}

/// The two ends of one exchange, as [`Exchange::split`] gives them.
pub type Ends<'a, Q, A, N, M> = (Asking<'a, Q, A, N, M>, Answering<'a, Q, A, N, M>);

/// The asking end: it hands requests and takes their answers.
pub struct Asking<'a, Q, A, N = AtomicWaker, M = AtomicWaker> {
    requests: WakingSpscSender<'a, Q, 1, N, M>,
    answers: WakingSpscReceiver<'a, Answered<A>, 1, M, N>,
}

impl<Q, A, N: QueueNotify, M: QueueNotify> Asking<'_, Q, A, N, M> {
    /// Hands `request` to the answering end without waiting.
    ///
    /// # Errors
    /// The request back while the previous one still lies in its place: the
    /// asker hands one while none is in work.
    pub fn hand(&self, request: Q) -> Result<(), Q> {
        self.requests.try_send(request).map_err(|refused| refused.0)
    }

    /// The answer of the request in work, once it comes back.
    pub async fn answered(&mut self) -> Answered<A> {
        loop {
            self.answers.ready().await;
            if let Some(answered) = self.answers.try_receive() {
                return answered;
            }
        }
    }
}

/// What the answering end owes the asker.
#[derive(Clone, Copy)]
enum Debt {
    Paid,
    /// A request was taken and its reply not answered.
    Owed,
}

/// The answering end: each request with the permit of its answer.
pub struct Answering<'a, Q, A, N = AtomicWaker, M = AtomicWaker> {
    requests: WakingSpscReceiver<'a, Q, 1, N, M>,
    answers: WakingSpscSender<'a, Answered<A>, 1, M, N>,
    debt: Debt,
}

impl<Q, A, N: QueueNotify, M: QueueNotify> Answering<'_, Q, A, N, M> {
    /// The next request and its [`Reply`], once the asker hands one. The
    /// reply borrows this end: no next request before it is consumed.
    ///
    /// A reply dropped without an answer leaves its request owed: this call
    /// pays it first, as [`Answered::Unanswered`]. A task that drops a reply
    /// and never asks again leaves the asker waiting.
    ///
    /// ```compile_fail,E0499
    /// use oer_portable_memory::Exchange;
    /// async fn two_in_work() {
    ///     let mut exchange = Exchange::<u8, u8>::new();
    ///     let (_asking, mut answering) = exchange.split();
    ///     let (_, first) = answering.next().await;
    ///     let (_, second) = answering.next().await;
    ///     drop((first, second));
    /// }
    /// ```
    ///
    /// A reply comes only from here: an answer without a request does not
    /// compile.
    ///
    /// ```compile_fail,E0451
    /// use oer_portable_memory::Reply;
    /// fn forged() -> Reply<'static, u8> {
    ///     Reply { place: todo!(), debt: todo!() }
    /// }
    /// ```
    pub async fn next(&mut self) -> (Q, Reply<'_, A, M>) {
        if let Debt::Owed = self.debt {
            // The asker waits for this answer before it hands another
            // request, so the place is free.
            self.answers.ready().await.send(Answered::Unanswered);
            self.debt = Debt::Paid;
        }
        let request = loop {
            self.requests.ready().await;
            if let Some(request) = self.requests.try_receive() {
                break request;
            }
        };
        // Owed from the moment the request is taken: a call dropped while it
        // waits for the place leaves the debt for the next call to pay.
        self.debt = Debt::Owed;
        let place = self.answers.ready().await;
        (
            request,
            Reply {
                place,
                debt: &mut self.debt,
            },
        )
    }
}

/// The permit of one request's answer: consumed by answering. Dropped
/// without an answer, it leaves the request owed, and the next request pays
/// it as unanswered ([`Answering::next`]).
#[must_use = "answer the request; a dropped reply is paid as unanswered at the next request"]
pub struct Reply<'a, A, M = AtomicWaker> {
    /// The answer's place, reserved; `M` wakes the asker.
    place: WakingSpscSendPermit<'a, Answered<A>, 1, M>,
    /// The answering end's record of what it owes, paid by the answer.
    debt: &'a mut Debt,
}

impl<A, M: QueueNotify> Reply<'_, A, M> {
    /// Answers the request.
    pub fn answer(self, answer: A) {
        let Self { place, debt } = self;
        place.send(Answered::Given(answer));
        *debt = Debt::Paid;
    }
}

#[cfg(test)]
mod tests;
