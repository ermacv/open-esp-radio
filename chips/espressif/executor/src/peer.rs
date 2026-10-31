//! A request from a task of one executor to a task of an executor on another
//! hart, and its answer: one in-place request slot
//! ([`oer_memory::RequestSlot`] in [`InPlace`] mode, which stores no waker)
//! and the two executors' signals, which carry only readiness.
//!
//! The client posts a request into the idle slot ([`PeerClient::post`]): a
//! successful post creates the obligation of one answer. It then raises the
//! server executor's signal, which pends that executor through its software
//! line (`fence w, o`, then the raise). The server's task, woken in its own
//! executor's context, processes the request in place
//! ([`PeerServer::serve`]): the slot is `WORKING` only inside that call,
//! never visible to the client. It publishes `ANSWERED`, then raises the
//! client executor's signal. The client takes the answer
//! ([`PeerClient::answer`]) or, when its `deadline` passes first, a terminal
//! [`PeerError::Expired`]; the request then stays in the slot, which the
//! image's record and reset end. No waker crosses harts: each side wakes only
//! through the other executor's signal, which only that executor's handler
//! services.
//!
//! [`liveness`] builds the durable supervision of a peer hart on one such
//! slot: a periodic request with its own deadline.

use core::{
    convert::Infallible,
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

use oer_espressif_interrupt_table::signal::{ForeignContext, IrqSignal, SignalExecutor};
use oer_memory::{InPlace, InPlaceClient, InPlaceServer, RequestSlot};
use oer_time::{Instant, Timer};

/// The fact a raise of the server's signal carries: a request was posted.
pub const POSTED: u32 = 1;
/// The fact a raise of the client's signal carries: the request was answered.
pub const ANSWERED: u32 = 1;

/// Why a client got no answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerError {
    /// The deadline passed before the answer; the request stays posted.
    Expired,
    /// The client's signal was polled outside its executor's context.
    Foreign(ForeignContext),
}

/// The slot still holds an earlier exchange; the new request comes back
/// untouched.
#[derive(Debug, Eq, PartialEq)]
pub struct Occupied<Req>(pub Req);

/// The requesting side, on the client executor `C`'s hart.
#[must_use = "dropping the client retains the slot's seat"]
pub struct PeerClient<Req: 'static, S: SignalExecutor, C: SignalExecutor> {
    client: InPlaceClient<Req>,
    request: &'static IrqSignal<S>,
    reply: &'static IrqSignal<C>,
}

/// The answering side, on the server executor `S`'s hart.
#[must_use = "dropping the server retains the slot's seat"]
pub struct PeerServer<Req: 'static, S: SignalExecutor, C: SignalExecutor> {
    server: InPlaceServer<Req>,
    request: &'static IrqSignal<S>,
    reply: &'static IrqSignal<C>,
}

/// Claim `slot`'s two endpoints with the server executor's `request` signal
/// and the client executor's `reply` signal; `None` when the slot was
/// claimed before.
pub fn pair<Req, S: SignalExecutor, C: SignalExecutor>(
    slot: &'static RequestSlot<Req, (), InPlace>,
    request: &'static IrqSignal<S>,
    reply: &'static IrqSignal<C>,
) -> Option<(PeerClient<Req, S, C>, PeerServer<Req, S, C>)> {
    let (client, server) = slot.claim_in_place()?;
    Some((
        PeerClient {
            client,
            request,
            reply,
        },
        PeerServer {
            server,
            request,
            reply,
        },
    ))
}

impl<Req, S: SignalExecutor, C: SignalExecutor> PeerClient<Req, S, C> {
    /// Post `request` into the idle slot and pend the server: the obligation
    /// of one answer starts here.
    ///
    /// # Errors
    ///
    /// [`Occupied`] when an earlier exchange is still in the slot.
    pub fn post(&mut self, request: Req) -> Result<(), Occupied<Req>> {
        self.client.post(request).map_err(Occupied)?;
        let _pended = self.request.raise(POSTED);
        Ok(())
    }

    /// Wait for the answer of the posted request until `deadline` on
    /// `timer`; the slot is idle again when it returns the answer.
    ///
    /// # Errors
    ///
    /// [`PeerError::Expired`] when the deadline passes first: the request
    /// stays in the slot and no later answer is taken. [`PeerError::Foreign`]
    /// when polled outside the client executor's context.
    pub async fn answer<T: Timer>(
        &mut self,
        timer: &T,
        deadline: Instant,
    ) -> Result<Req, PeerError> {
        let mut expiry = pin!(timer.wait_until(deadline));
        poll_fn(|cx| {
            // Register before looking at the slot, so an answer published
            // after the look pends this task again.
            if let Poll::Ready(Err(error)) = self.reply.poll_take(cx) {
                return Poll::Ready(Err(PeerError::Foreign(error)));
            }
            if let Some(Ok(answer)) = self.client.try_take_if(|_| Ok::<(), Infallible>(())) {
                return Poll::Ready(Ok(answer));
            }
            if expiry.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err(PeerError::Expired));
            }
            Poll::Pending
        })
        .await
    }
}

impl<Req, S: SignalExecutor, C: SignalExecutor> PeerServer<Req, S, C> {
    /// Serve the client for good, in the server executor's context: wait for
    /// a post, answer it in place with `process` and pend the client.
    ///
    /// # Errors
    ///
    /// Returns only when polled outside its executor's context.
    pub async fn serve(&mut self, mut process: impl FnMut(&mut Req)) -> ForeignContext {
        loop {
            if let Err(error) = self.serve_one(&mut process).await {
                return error;
            }
        }
    }

    /// Serve one request, in the server executor's context: wait for a
    /// post, answer it in place with `process` and pend the client. Work
    /// that waits (a hardware operation under timers) runs between two
    /// calls, on what one answer handed over, never inside `process`.
    ///
    /// # Errors
    ///
    /// [`ForeignContext`] when polled outside its executor's context.
    pub async fn serve_one(
        &mut self,
        process: impl FnOnce(&mut Req),
    ) -> Result<(), ForeignContext> {
        let mut process = Some(process);
        loop {
            self.request.wait().await?;
            // A raise without a post (a fact coalesced into an earlier
            // round) leaves `process` for the next post.
            let answered = self.server.service(|request| {
                if let Some(process) = process.take() {
                    process(request);
                }
            });
            if answered {
                let _pended = self.reply.raise(ANSWERED);
                return Ok(());
            }
        }
    }
}

pub mod liveness;

#[cfg(test)]
mod tests;
