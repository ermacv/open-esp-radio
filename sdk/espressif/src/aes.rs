//! The shared AES service: one owner of the hardware engine and up to `C`
//! clients of one execution context.
//!
//! [`AesService::serve`] owns the engine and serves one request at a time,
//! the next pending client after the last one served. A client
//! ([`AesClient`]) holds one seat; [`AesClient::execute`] moves its request
//! into the seat, and its future ends with that request's response, which
//! moves back by value. The request's secrets travel inside it and erase
//! themselves on drop.
//!
//! Cancellation never loses the engine or another client's work:
//!
//! - a wait dropped before the service took the request withdraws it, and
//!   the request is erased;
//! - a wait dropped while the service runs the request leaves the seat
//!   discarding: the service finishes, erases the response, and the client's
//!   next request waits for that;
//! - a client dropped while its request runs releases its seat once the
//!   service finished it.
//!
//! Seats are claimed atomically, so a client can be issued before `X`'s
//! executor runs (an image's boot hands one to the Wi-Fi station). Requests
//! and responses live in one [`ContextCell`] of context `X`: every access
//! checks that it runs in `X` and returns [`AesError::Foreign`] otherwise.
//! Wakers are woken after the cell's borrow ends, never inside it, and never
//! leave `X`'s executor.

use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll, Waker},
};

use oer_crypto::{
    KeyUnwrap, KeyUnwrapDone, KeyUnwrapRequest, KeyUnwrapResponse, block::BlockError,
};
use oer_memory::{ContextCell, Entered, ExecutionContext};

/// One request of a client: what the service computes.
pub enum AesRequest<const N: usize> {
    /// RFC 3394 key unwrap under an AES-128 key-encryption key.
    KeyUnwrap(KeyUnwrapRequest<N>),
}

/// The response of one [`AesRequest`], of the same kind.
pub enum AesResponse<const N: usize> {
    KeyUnwrap(KeyUnwrapResponse<N>),
}

/// Why the service or a client refused to act.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AesError {
    /// The call ran outside the service's execution context, or inside a
    /// borrow of its state.
    Foreign,
}

/// Every seat has a client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Exhausted;

/// One seat's request state; whether a client holds it is the service's
/// claim mask.
enum Seat<const N: usize> {
    /// No request.
    Idle,
    /// The request waits for the service.
    Pending {
        request: AesRequest<N>,
        waker: Option<Waker>,
    },
    /// The service runs the request.
    Running { waker: Option<Waker> },
    /// The service runs a request whose wait was dropped: its response is
    /// erased, then the seat is idle again.
    Discarding { waker: Option<Waker> },
    /// The service runs a request whose client was dropped: the seat's
    /// claim ends once it finished.
    Released,
    /// The response waits for its client.
    Done { response: AesResponse<N> },
}

struct State<const N: usize, const C: usize> {
    seats: [Seat<N>; C],
    /// The seat after the one served last.
    next: usize,
    server: Option<Waker>,
}

/// The shared AES service of context `X`: `C` client seats, requests with
/// key data of capacity `N`.
pub struct AesService<X: ExecutionContext, const N: usize, const C: usize> {
    /// One bit per seat a client holds.
    claimed: AtomicU32,
    state: ContextCell<State<N, C>, Entered<X>>,
}

impl<X: ExecutionContext, const N: usize, const C: usize> AesService<X, N, C> {
    pub const fn new() -> Self {
        const {
            if C == 0 || C > 32 {
                panic!("an AES service has 1 to 32 client seats");
            }
        };
        Self {
            claimed: AtomicU32::new(0),
            state: ContextCell::new(State {
                seats: [const { Seat::Idle }; C],
                next: 0,
                server: None,
            }),
        }
    }

    /// Borrow the state in `X`.
    fn with<R>(&self, f: impl FnOnce(&mut State<N, C>) -> R) -> Result<R, AesError> {
        let entered = Entered::<X>::enter().ok_or(AesError::Foreign)?;
        self.state.with(&entered, f).ok_or(AesError::Foreign)
    }

    /// A client on the first free seat; any context may take one.
    ///
    /// # Errors
    /// Every seat has a client.
    pub fn client(&self) -> Result<AesClient<'_, X, N, C>, Exhausted> {
        let all = u32::MAX >> (32 - C);
        let previous = self
            .claimed
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |claimed| {
                let free = !claimed & all;
                (free != 0).then(|| claimed | (free & free.wrapping_neg()))
            })
            .map_err(|_| Exhausted)?;
        let free = !previous & all;
        Ok(AesClient {
            service: self,
            seat: free.trailing_zeros() as usize,
        })
    }

    /// The seat's claim ends.
    fn unclaim(&self, seat: usize) {
        if let Some(bit) = 1u32.checked_shl(seat as u32) {
            self.claimed.fetch_and(!bit, Ordering::AcqRel);
        }
    }

    /// Serve the clients' requests with `engine`, one at a time, for good.
    /// It returns only when it runs outside `X`.
    pub async fn serve<E: KeyUnwrap<N>>(&self, mut engine: E) -> AesError {
        loop {
            let (seat, request) = match core::future::poll_fn(|cx| self.take_pending(cx)).await {
                Ok(taken) => taken,
                Err(error) => return error,
            };
            let response = match request {
                AesRequest::KeyUnwrap(request) => {
                    let done = engine.unwrap_key(request).await;
                    engine = done.engine;
                    AesResponse::KeyUnwrap(done.response)
                }
            };
            if let Err(error) = self.finish(seat, response) {
                return error;
            }
        }
    }

    /// Take the next pending request, after the seat served last.
    fn take_pending(&self, cx: &Context<'_>) -> Poll<Result<(usize, AesRequest<N>), AesError>> {
        let taken = self.with(|state| {
            for offset in 0..C {
                let index = (state.next + offset) % C;
                let Some(seat) = state.seats.get_mut(index) else {
                    continue;
                };
                if !matches!(seat, Seat::Pending { .. }) {
                    continue;
                }
                let Seat::Pending { request, waker } =
                    core::mem::replace(seat, Seat::Running { waker: None })
                else {
                    continue;
                };
                *seat = Seat::Running { waker };
                state.next = (index + 1) % C;
                return Some((index, request));
            }
            state.server = Some(cx.waker().clone());
            None
        });
        match taken {
            Ok(Some(taken)) => Poll::Ready(Ok(taken)),
            Ok(None) => Poll::Pending,
            Err(error) => Poll::Ready(Err(error)),
        }
    }

    /// Hand `response` to its seat's client, or erase it when nobody waits.
    fn finish(&self, index: usize, response: AesResponse<N>) -> Result<(), AesError> {
        let (waker, erased, released) = self.with(|state| {
            let Some(seat) = state.seats.get_mut(index) else {
                return (None, Some(response), false);
            };
            match core::mem::replace(seat, Seat::Idle) {
                Seat::Running { waker } => {
                    *seat = Seat::Done { response };
                    (waker, None, false)
                }
                Seat::Discarding { waker } => (waker, Some(response), false),
                Seat::Released => (None, Some(response), true),
                // Only the service leaves `Running`: no other state holds a
                // request it ran. The response is erased and the seat kept.
                other => {
                    *seat = other;
                    (None, Some(response), false)
                }
            }
        })?;
        if released {
            self.unclaim(index);
        }
        drop(erased);
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }

    /// Move `request` into `seat` or take its response.
    fn poll_seat(
        &self,
        seat: usize,
        request: &mut Option<AesRequest<N>>,
        cx: &Context<'_>,
    ) -> Poll<Result<AesResponse<N>, AesError>> {
        let polled = self.with(|state| {
            let Some(slot) = state.seats.get_mut(seat) else {
                return (Poll::Ready(Err(AesError::Foreign)), None);
            };
            let waker = Some(cx.waker().clone());
            match slot {
                Seat::Idle => match request.take() {
                    Some(request) => {
                        *slot = Seat::Pending { request, waker };
                        (Poll::Pending, state.server.take())
                    }
                    // The request moved in and its response was taken.
                    None => (Poll::Ready(Err(AesError::Foreign)), None),
                },
                Seat::Pending { waker: stored, .. }
                | Seat::Running { waker: stored }
                | Seat::Discarding { waker: stored } => {
                    *stored = waker;
                    (Poll::Pending, None)
                }
                Seat::Done { .. } => match core::mem::replace(slot, Seat::Idle) {
                    Seat::Done { response } => (Poll::Ready(Ok(response)), None),
                    other => {
                        *slot = other;
                        (Poll::Pending, None)
                    }
                },
                Seat::Released => (Poll::Ready(Err(AesError::Foreign)), None),
            }
        });
        match polled {
            Ok((poll, server)) => {
                if let Some(server) = server {
                    server.wake();
                }
                poll
            }
            Err(error) => Poll::Ready(Err(error)),
        }
    }

    /// A wait on `seat` ended before its response: withdraw a pending
    /// request, let a running one be erased, erase an untaken response.
    fn withdraw(&self, seat: usize) {
        let dropped = self.with(|state| {
            let slot = state.seats.get_mut(seat)?;
            match core::mem::replace(slot, Seat::Idle) {
                Seat::Pending { request, .. } => Some(Dropped::Request(request)),
                Seat::Running { .. } | Seat::Discarding { .. } => {
                    *slot = Seat::Discarding { waker: None };
                    None
                }
                Seat::Done { response } => Some(Dropped::Response(response)),
                other => {
                    *slot = other;
                    None
                }
            }
        });
        drop(dropped);
    }

    /// The client of `seat` is gone: its claim ends now, or once the
    /// service finished its running request. Outside `X` the seat cannot
    /// be inspected and stays claimed.
    fn release(&self, seat: usize) {
        let released = self.with(|state| {
            let Some(slot) = state.seats.get_mut(seat) else {
                return (true, None);
            };
            match core::mem::replace(slot, Seat::Idle) {
                Seat::Running { .. } | Seat::Discarding { .. } | Seat::Released => {
                    *slot = Seat::Released;
                    (false, None)
                }
                Seat::Pending { request, .. } => (true, Some(Dropped::Request(request))),
                Seat::Done { response } => (true, Some(Dropped::Response(response))),
                Seat::Idle => (true, None),
            }
        });
        if let Ok((free, dropped)) = released {
            drop(dropped);
            if free {
                self.unclaim(seat);
            }
        }
    }
}

impl<X: ExecutionContext, const N: usize, const C: usize> Default for AesService<X, N, C> {
    fn default() -> Self {
        Self::new()
    }
}

/// What a withdrawal erases, outside the cell's borrow.
enum Dropped<const N: usize> {
    Request(#[allow(dead_code, reason = "held to erase on drop")] AesRequest<N>),
    Response(#[allow(dead_code, reason = "held to erase on drop")] AesResponse<N>),
}

/// One client of an [`AesService`]: its seat.
pub struct AesClient<'s, X: ExecutionContext, const N: usize, const C: usize> {
    service: &'s AesService<X, N, C>,
    seat: usize,
}

impl<'s, X: ExecutionContext, const N: usize, const C: usize> AesClient<'s, X, N, C> {
    /// Compute `request`; the future ends with its response.
    pub fn execute(&mut self, request: AesRequest<N>) -> Execute<'_, 's, X, N, C> {
        Execute {
            client: self,
            request: Some(request),
            ended: false,
        }
    }
}

impl<X: ExecutionContext, const N: usize, const C: usize> Drop for AesClient<'_, X, N, C> {
    fn drop(&mut self) {
        self.service.release(self.seat);
    }
}

/// The wait for one request's response.
#[must_use = "the request moves into the service only when the wait is polled"]
pub struct Execute<'c, 's, X: ExecutionContext, const N: usize, const C: usize> {
    client: &'c mut AesClient<'s, X, N, C>,
    request: Option<AesRequest<N>>,
    ended: bool,
}

impl<X: ExecutionContext, const N: usize, const C: usize> Future for Execute<'_, '_, X, N, C> {
    type Output = Result<AesResponse<N>, AesError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let poll = this
            .client
            .service
            .poll_seat(this.client.seat, &mut this.request, cx);
        this.ended = poll.is_ready();
        poll
    }
}

impl<X: ExecutionContext, const N: usize, const C: usize> Drop for Execute<'_, '_, X, N, C> {
    fn drop(&mut self) {
        if !self.ended && self.request.is_none() {
            self.client.service.withdraw(self.client.seat);
        }
    }
}

/// A client is a key-unwrap engine: it moves into its operation and comes
/// back with the response, which is how the Wi-Fi station takes it.
impl<X: ExecutionContext, const N: usize, const C: usize> KeyUnwrap<N>
    for AesClient<'static, X, N, C>
{
    type Operation = Unwrap<X, N, C>;

    fn unwrap_key(self, request: KeyUnwrapRequest<N>) -> Self::Operation {
        Unwrap {
            client: Some(self),
            request: Some(AesRequest::KeyUnwrap(request)),
        }
    }
}

/// One key unwrap of a client that owns itself for the operation.
pub struct Unwrap<X: ExecutionContext + 'static, const N: usize, const C: usize> {
    client: Option<AesClient<'static, X, N, C>>,
    request: Option<AesRequest<N>>,
}

impl<X: ExecutionContext, const N: usize, const C: usize> Future for Unwrap<X, N, C> {
    type Output = KeyUnwrapDone<AesClient<'static, X, N, C>, N>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(client) = this.client.as_ref() else {
            return Poll::Pending;
        };
        let response = match client.service.poll_seat(client.seat, &mut this.request, cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Ok(AesResponse::KeyUnwrap(response))) => response,
            // Outside the service's context nothing ran: the engine is
            // unavailable to this operation.
            Poll::Ready(Err(AesError::Foreign)) => {
                KeyUnwrapResponse::engine_failed(BlockError::Unavailable)
            }
        };
        match this.client.take() {
            Some(engine) => Poll::Ready(KeyUnwrapDone { engine, response }),
            None => Poll::Pending,
        }
    }
}

impl<X: ExecutionContext, const N: usize, const C: usize> Drop for Unwrap<X, N, C> {
    fn drop(&mut self) {
        if let Some(client) = &self.client
            && self.request.is_none()
        {
            client.service.withdraw(client.seat);
        }
    }
}

#[cfg(test)]
mod tests;
