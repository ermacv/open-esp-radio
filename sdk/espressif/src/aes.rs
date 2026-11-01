//! The shared AES service: one owner of the hardware engine and the
//! clients of one execution context, as many as its seats' storage holds
//! ([`AesSeats`], 1 to 32).
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
//!   service finished it;
//! - a wait that ended without its response (it ran outside `X`, or it was
//!   leaked) leaves its request or response in the seat, under its request
//!   number: the client's next request, of another number, withdraws or
//!   erases it first, and waits for one that runs. A response reaches only
//!   the request of its own number.
//!
//! Seats are claimed atomically, so a client can be issued before `X`'s
//! executor runs (an image's boot hands one to the Wi-Fi station). Requests
//! and responses live in one [`ContextCell`] of context `X` per seat, the
//! service's round in another: every access checks that it runs in `X` and
//! returns [`AesError::Foreign`] otherwise.
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
enum SeatState<const N: usize> {
    /// No request.
    Idle,
    /// The request numbered `ticket` waits for the service.
    Pending {
        request: AesRequest<N>,
        waker: Option<Waker>,
        ticket: u32,
    },
    /// The service runs the request numbered `ticket`.
    Running { waker: Option<Waker>, ticket: u32 },
    /// The service runs a request whose wait was dropped: its response is
    /// erased, then the seat is idle again.
    Discarding { waker: Option<Waker> },
    /// The service runs a request whose client was dropped: the seat's
    /// claim ends once it finished.
    Released,
    /// The response of the request numbered `ticket` waits for it.
    Done {
        response: AesResponse<N>,
        ticket: u32,
    },
}

/// One client seat of an AES service of context `X`: its request state,
/// lent only in `X`.
pub struct AesSeat<X: ExecutionContext, const N: usize>(ContextCell<SeatState<N>, Entered<X>>);

/// The client seats of one AES service: `C` of them, 1 to 32. A static of
/// this type is the storage [`AesService::new`] serves; its size is
/// `C` seats, each holding one request or response of capacity `N`.
pub struct AesSeats<X: ExecutionContext, const N: usize, const C: usize>([AesSeat<X, N>; C]);

impl<X: ExecutionContext, const N: usize, const C: usize> AesSeats<X, N, C> {
    pub const fn new() -> Self {
        const {
            if C == 0 || C > 32 {
                panic!("an AES service has 1 to 32 client seats");
            }
        };
        Self([const { AesSeat(ContextCell::new(SeatState::Idle)) }; C])
    }
}

impl<X: ExecutionContext, const N: usize, const C: usize> Default for AesSeats<X, N, C> {
    fn default() -> Self {
        Self::new()
    }
}

/// The service's own state: where its round continues and its waker.
struct Shared {
    /// The seat after the one served last.
    next: usize,
    server: Option<Waker>,
}

/// The shared AES service of context `X`: the client seats of one
/// [`AesSeats`], requests with key data of capacity `N`. The seat count is
/// the storage's, not the service's type: clients of services with
/// different counts have one type.
pub struct AesService<X: ExecutionContext + 'static, const N: usize> {
    /// One bit per seat a client holds.
    claimed: AtomicU32,
    seats: &'static [AesSeat<X, N>],
    state: ContextCell<Shared, Entered<X>>,
}

impl<X: ExecutionContext, const N: usize> AesService<X, N> {
    /// The service of `seats`.
    pub const fn new<const C: usize>(seats: &'static AesSeats<X, N, C>) -> Self {
        Self {
            claimed: AtomicU32::new(0),
            seats: &seats.0,
            state: ContextCell::new(Shared {
                next: 0,
                server: None,
            }),
        }
    }

    /// The seats' claim bits: one per seat, at most 32.
    fn all(&self) -> u32 {
        u32::MAX
            .checked_shr(32_u32.saturating_sub(self.seats.len() as u32))
            .unwrap_or(0)
    }

    /// Borrow seat `index` in `X`; `None` for a seat the storage has not.
    fn with_seat<R>(
        &self,
        entered: &Entered<X>,
        index: usize,
        f: impl FnOnce(&mut SeatState<N>) -> R,
    ) -> Result<Option<R>, AesError> {
        match self.seats.get(index) {
            Some(seat) => seat.0.with(entered, f).map(Some).ok_or(AesError::Foreign),
            None => Ok(None),
        }
    }

    /// A client on the first free seat; any context may take one.
    ///
    /// # Errors
    /// Every seat has a client.
    pub fn client(&self) -> Result<AesClient<'_, X, N>, Exhausted> {
        let all = self.all();
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
            ticket: 0,
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
        match self.next_pending(cx) {
            Ok(Some(taken)) => Poll::Ready(Ok(taken)),
            Ok(None) => Poll::Pending,
            Err(error) => Poll::Ready(Err(error)),
        }
    }

    fn next_pending(&self, cx: &Context<'_>) -> Result<Option<(usize, AesRequest<N>)>, AesError> {
        let entered = Entered::<X>::enter().ok_or(AesError::Foreign)?;
        let count = self.seats.len();
        let next = self
            .state
            .with(&entered, |state| state.next)
            .ok_or(AesError::Foreign)?;
        for offset in 0..count {
            let Some(index) = next.wrapping_add(offset).checked_rem(count) else {
                break;
            };
            let taken = self.with_seat(&entered, index, |seat| {
                if !matches!(seat, SeatState::Pending { .. }) {
                    return None;
                }
                let SeatState::Pending {
                    request,
                    waker,
                    ticket,
                } = core::mem::replace(seat, SeatState::Idle)
                else {
                    return None;
                };
                *seat = SeatState::Running { waker, ticket };
                Some(request)
            })?;
            if let Some(Some(request)) = taken {
                self.state
                    .with(&entered, |state| {
                        state.next = index.wrapping_add(1).checked_rem(count).unwrap_or(0);
                    })
                    .ok_or(AesError::Foreign)?;
                return Ok(Some((index, request)));
            }
        }
        self.state
            .with(&entered, |state| state.server = Some(cx.waker().clone()))
            .ok_or(AesError::Foreign)?;
        Ok(None)
    }

    /// Hand `response` to its seat's client, or erase it when nobody waits.
    fn finish(&self, index: usize, response: AesResponse<N>) -> Result<(), AesError> {
        let entered = Entered::<X>::enter().ok_or(AesError::Foreign)?;
        let mut response = Some(response);
        let handed = self.with_seat(&entered, index, |seat| {
            match core::mem::replace(seat, SeatState::Idle) {
                SeatState::Running { waker, ticket } => {
                    if let Some(response) = response.take() {
                        *seat = SeatState::Done { response, ticket };
                    }
                    (waker, false)
                }
                SeatState::Discarding { waker } => (waker, false),
                SeatState::Released => (None, true),
                // Only the service leaves `Running`: no other state holds a
                // request it ran. The response is erased and the seat kept.
                other => {
                    *seat = other;
                    (None, false)
                }
            }
        })?;
        let (waker, released) = handed.unwrap_or((None, false));
        if released {
            self.unclaim(index);
        }
        drop(response);
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }

    /// Move `request`, numbered `ticket`, into `seat`, or take its
    /// response. What the seat holds for another number, a wait that ended
    /// without its response left: a pending request is withdrawn, a
    /// response erased, a running request discarded and waited for.
    fn poll_seat(
        &self,
        seat: usize,
        ticket: u32,
        request: &mut Option<AesRequest<N>>,
        cx: &Context<'_>,
    ) -> Poll<Result<AesResponse<N>, AesError>> {
        let Some(entered) = Entered::<X>::enter() else {
            return Poll::Ready(Err(AesError::Foreign));
        };
        let polled = self.with_seat(&entered, seat, |slot| {
            let waker = Some(cx.waker().clone());
            let stale = match slot {
                SeatState::Pending { ticket: held, .. } | SeatState::Done { ticket: held, .. }
                    if *held != ticket =>
                {
                    match core::mem::replace(slot, SeatState::Idle) {
                        SeatState::Pending { request, .. } => Some(Dropped::Request(request)),
                        SeatState::Done { response, .. } => Some(Dropped::Response(response)),
                        _ => None,
                    }
                }
                SeatState::Running { ticket: held, .. } if *held != ticket => {
                    *slot = SeatState::Discarding { waker: None };
                    None
                }
                _ => None,
            };
            let (poll, submitted) = match slot {
                SeatState::Idle => match request.take() {
                    Some(request) => {
                        *slot = SeatState::Pending {
                            request,
                            waker,
                            ticket,
                        };
                        (Poll::Pending, true)
                    }
                    // The request moved in and its response was taken.
                    None => (Poll::Ready(Err(AesError::Foreign)), false),
                },
                SeatState::Pending { waker: stored, .. }
                | SeatState::Running { waker: stored, .. }
                | SeatState::Discarding { waker: stored } => {
                    *stored = waker;
                    (Poll::Pending, false)
                }
                SeatState::Done { .. } => match core::mem::replace(slot, SeatState::Idle) {
                    SeatState::Done { response, .. } => (Poll::Ready(Ok(response)), false),
                    other => {
                        *slot = other;
                        (Poll::Pending, false)
                    }
                },
                SeatState::Released => (Poll::Ready(Err(AesError::Foreign)), false),
            };
            (poll, submitted, stale)
        });
        let (poll, submitted, stale) = match polled {
            Ok(Some(polled)) => polled,
            Ok(None) | Err(_) => return Poll::Ready(Err(AesError::Foreign)),
        };
        drop(stale);
        // A new request wakes the service, outside the seat's borrow.
        let server = if submitted {
            match self.state.with(&entered, |state| state.server.take()) {
                Some(server) => server,
                None => return Poll::Ready(Err(AesError::Foreign)),
            }
        } else {
            None
        };
        if let Some(server) = server {
            server.wake();
        }
        poll
    }

    /// The wait for request `ticket` on `seat` ended before its response:
    /// withdraw it pending, let it be erased running, erase its untaken
    /// response. A seat that holds another request is left as it is.
    fn withdraw(&self, seat: usize, ticket: u32) {
        let Some(entered) = Entered::<X>::enter() else {
            return;
        };
        let dropped = self.with_seat(&entered, seat, |slot| {
            match slot {
                SeatState::Pending { ticket: held, .. }
                | SeatState::Running { ticket: held, .. }
                | SeatState::Done { ticket: held, .. }
                    if *held == ticket => {}
                _ => return None,
            }
            match core::mem::replace(slot, SeatState::Idle) {
                SeatState::Pending { request, .. } => Some(Dropped::Request(request)),
                SeatState::Running { .. } => {
                    *slot = SeatState::Discarding { waker: None };
                    None
                }
                SeatState::Done { response, .. } => Some(Dropped::Response(response)),
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
        let Some(entered) = Entered::<X>::enter() else {
            return;
        };
        let released = self.with_seat(&entered, seat, |slot| {
            match core::mem::replace(slot, SeatState::Idle) {
                SeatState::Running { .. } | SeatState::Discarding { .. } | SeatState::Released => {
                    *slot = SeatState::Released;
                    (false, None)
                }
                SeatState::Pending { request, .. } => (true, Some(Dropped::Request(request))),
                SeatState::Done { response, .. } => (true, Some(Dropped::Response(response))),
                SeatState::Idle => (true, None),
            }
        });
        match released {
            Ok(Some((free, dropped))) => {
                drop(dropped);
                if free {
                    self.unclaim(seat);
                }
            }
            // A seat the storage has not was never claimed by a client.
            Ok(None) => self.unclaim(seat),
            Err(_) => {}
        }
    }
}

/// What a withdrawal erases, outside the cell's borrow.
enum Dropped<const N: usize> {
    Request(#[allow(dead_code, reason = "held to erase on drop")] AesRequest<N>),
    Response(#[allow(dead_code, reason = "held to erase on drop")] AesResponse<N>),
}

/// One client of an [`AesService`]: its seat.
pub struct AesClient<'s, X: ExecutionContext + 'static, const N: usize> {
    service: &'s AesService<X, N>,
    seat: usize,
    /// The number of the client's last request.
    ticket: u32,
}

impl<'s, X: ExecutionContext, const N: usize> AesClient<'s, X, N> {
    /// Compute `request`; the future ends with its response.
    pub fn execute(&mut self, request: AesRequest<N>) -> Execute<'_, 's, X, N> {
        let ticket = self.next_ticket();
        Execute {
            client: self,
            request: Some(request),
            ticket,
            ended: false,
        }
    }

    /// A number no other live request of this client holds: a seat holds
    /// one request at a time, and the counter wraps only after 2^32 more.
    fn next_ticket(&mut self) -> u32 {
        self.ticket = self.ticket.wrapping_add(1);
        self.ticket
    }
}

impl<X: ExecutionContext, const N: usize> Drop for AesClient<'_, X, N> {
    fn drop(&mut self) {
        self.service.release(self.seat);
    }
}

/// The wait for one request's response.
#[must_use = "the request moves into the service only when the wait is polled"]
pub struct Execute<'c, 's, X: ExecutionContext + 'static, const N: usize> {
    client: &'c mut AesClient<'s, X, N>,
    request: Option<AesRequest<N>>,
    ticket: u32,
    ended: bool,
}

impl<X: ExecutionContext, const N: usize> Future for Execute<'_, '_, X, N> {
    type Output = Result<AesResponse<N>, AesError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let poll =
            this.client
                .service
                .poll_seat(this.client.seat, this.ticket, &mut this.request, cx);
        this.ended = poll.is_ready();
        poll
    }
}

impl<X: ExecutionContext, const N: usize> Drop for Execute<'_, '_, X, N> {
    fn drop(&mut self) {
        if !self.ended && self.request.is_none() {
            self.client.service.withdraw(self.client.seat, self.ticket);
        }
    }
}

/// A client is a key-unwrap engine: it moves into its operation and comes
/// back with the response, which is how the Wi-Fi station takes it.
impl<X: ExecutionContext, const N: usize> KeyUnwrap<N> for AesClient<'static, X, N> {
    type Operation = Unwrap<X, N>;

    fn unwrap_key(mut self, request: KeyUnwrapRequest<N>) -> Self::Operation {
        let ticket = self.next_ticket();
        Unwrap {
            client: Some(self),
            request: Some(AesRequest::KeyUnwrap(request)),
            ticket,
        }
    }
}

/// One key unwrap of a client that owns itself for the operation.
pub struct Unwrap<X: ExecutionContext + 'static, const N: usize> {
    client: Option<AesClient<'static, X, N>>,
    request: Option<AesRequest<N>>,
    ticket: u32,
}

impl<X: ExecutionContext, const N: usize> Future for Unwrap<X, N> {
    type Output = KeyUnwrapDone<AesClient<'static, X, N>, N>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(client) = this.client.as_ref() else {
            return Poll::Pending;
        };
        let response =
            match client
                .service
                .poll_seat(client.seat, this.ticket, &mut this.request, cx)
            {
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

impl<X: ExecutionContext, const N: usize> Drop for Unwrap<X, N> {
    fn drop(&mut self) {
        if let Some(client) = &self.client
            && self.request.is_none()
        {
            client.service.withdraw(client.seat, self.ticket);
        }
    }
}

#[cfg(test)]
mod tests;
