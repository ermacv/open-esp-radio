//! One request at a time between a client and a server, without a lock.
//!
//! A [`RequestSlot`] lives in permanent backing. Its default [`ByValue`]
//! mode moves through four states:
//!
//! - `IDLE`: the client holds the slot's [`Poster`]; neither side touches
//!   the payload;
//! - `POSTED`: the client wrote the request and rang the server's
//!   [`Doorbell`];
//! - `WORKING`: the server took the request by CAS and owns the answer
//!   place;
//! - `ANSWERED`: the server wrote the answer and woke the client, which takes
//!   it by CAS and returns the slot to `IDLE`.
//!
//! Each payload place is accessed only by the side that the state names, and
//! every hand-over is a Release store observed by an Acquire CAS. A client
//! posts only with the slot's one [`Poster`], which [`RequestSlot::claim`]
//! gives once, [`Poster::post`] consumes and [`Awaiting::poll_answer`]
//! returns with the answer, so at most one request is in a slot. Waits register their waker
//! before they check the state, so no wake is lost. The by-value handoff is
//! CAS-only and stays correct when one side preempts the other.
//!
//! [`InPlace`] uses the same handoffs without either waker or a doorbell:
//! its unique server borrows the whole exchange, including the reply, in
//! `WORKING`. Its unique client validates in `ANSWERED` before taking the
//! exchange and returning to `IDLE`. External signals carry only readiness.
//! Abandonment leaves the seat claimed; both idle endpoints must reunite
//! before another pair can claim it.

use core::{
    cell::UnsafeCell,
    mem::{ManuallyDrop, MaybeUninit},
    sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering},
    task::{Context, Poll},
};

use oer_sync::PriorityWaker;

const IDLE: u8 = 0;
const POSTED: u8 = 1;
const WORKING: u8 = 2;
const ANSWERED: u8 = 3;
const QUARANTINED: u8 = 4;

/// The server abandoned its response; the seat remains permanently claimed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    Unanswered {
        /// Caller-assigned request kind or site, stable within the image.
        kind: u32,
        /// Index of the slot's doorbell bit.
        slot: u32,
        /// Caller's epoch, when the client has one; otherwise zero.
        epoch: u32,
        /// Wrapping per-slot post number, starting at one.
        sequence: u32,
    },
}

impl RequestError {
    /// Retained observation layout: request kind/site, client slot, epoch,
    /// and per-slot sequence. Keep this conversion shared by every image.
    pub const fn observations(self) -> [u32; 4] {
        match self {
            Self::Unanswered {
                kind,
                slot,
                epoch,
                sequence,
            } => [kind, slot, epoch, sequence],
        }
    }

    /// Attach the epoch held by the client to the original request identity.
    pub const fn with_epoch(self, epoch: u32) -> Self {
        match self {
            Self::Unanswered {
                kind,
                slot,
                sequence,
                ..
            } => Self::Unanswered {
                kind,
                slot,
                epoch,
                sequence,
            },
        }
    }
}

/// Owners retained after a request lost its mandatory response. Dropping or
/// forgetting this diagnostic never releases those owners for reuse.
pub struct RequestQuarantine<T> {
    pub reason: RequestError,
    _owners: ManuallyDrop<T>,
}

impl<T> RequestQuarantine<T> {
    pub fn new(reason: RequestError, owners: T) -> Self {
        Self {
            reason,
            _owners: ManuallyDrop::new(owners),
        }
    }
}

// Extracting the diagnostic consumes only the wrapper. Its ManuallyDrop
// payload remains quarantined, including when reset takes the error by value.
impl<T> From<RequestQuarantine<T>> for RequestError {
    fn from(quarantine: RequestQuarantine<T>) -> Self {
        quarantine.reason
    }
}

/// The server's wake source: one bit per slot that has a request posted.
pub struct Doorbell {
    pending: AtomicU32,
    waker: PriorityWaker,
}

impl Doorbell {
    pub const fn new() -> Self {
        Self {
            pending: AtomicU32::new(0),
            waker: PriorityWaker::new(),
        }
    }

    fn ring(&self, bit: u32) {
        self.pending.fetch_or(bit, Ordering::Release);
        self.waker.wake();
    }

    /// The slots that rang since the last call, once any did.
    pub fn poll_rung(&self, cx: &mut Context<'_>) -> Poll<u32> {
        self.waker.register(cx.waker());
        match self.pending.swap(0, Ordering::Acquire) {
            0 => Poll::Pending,
            rung => Poll::Ready(rung),
        }
    }
}

impl Default for Doorbell {
    fn default() -> Self {
        Self::new()
    }
}

/// By-value requests and answers, with task notifications in both directions.
pub struct ByValue {
    client: PriorityWaker,
    doorbell: &'static Doorbell,
    bit: u32,
    kind: u32,
}

/// In-place processing with external notifications and no stored wakers.
///
/// The request also carries its reply. Dropping either endpoint retains the
/// seat permanently; only reuniting both idle endpoints permits a new claim.
pub struct InPlace;

/// The permanent backing of one client's requests and answers.
///
/// The default [`ByValue`] mode moves requests and answers between tasks.
/// [`InPlace`] instead lends the request to a server and retains it until
/// the client validates and takes the completed exchange.
pub struct RequestSlot<Req, Ans, Mode = ByValue> {
    claimed: AtomicBool,
    state: AtomicU8,
    request: UnsafeCell<MaybeUninit<Req>>,
    answer: UnsafeCell<MaybeUninit<Ans>>,
    mode: Mode,
}

#[allow(
    unsafe_code,
    reason = "the slot's state protocol gives each payload place one accessor at a time"
)]
// SAFETY: `request` and `answer` are accessed only through `Poster::post`,
// `take_request`, `Responder::answer`, `Awaiting::poll_answer`, and the
// in-place endpoints, each of which first owns the state that admits it.
// In-place server borrows end before ANSWERED, and the client validates and
// takes only in ANSWERED. A forgotten endpoint or interrupted service retains
// the seat. Payloads cross contexts and must be Send; mode data must be Sync.
unsafe impl<Req: Send, Ans: Send, Mode: Sync> Sync for RequestSlot<Req, Ans, Mode> {}

impl<Req, Ans> RequestSlot<Req, Ans> {
    /// A slot that rings `bit` of `doorbell`. `kind` identifies the request
    /// type or posting site in retained abandonment diagnostics.
    pub const fn new(doorbell: &'static Doorbell, bit: u32, kind: u32) -> Self {
        Self {
            claimed: AtomicBool::new(false),
            state: AtomicU8::new(IDLE),
            request: UnsafeCell::new(MaybeUninit::uninit()),
            answer: UnsafeCell::new(MaybeUninit::uninit()),
            mode: ByValue {
                client: PriorityWaker::new(),
                doorbell,
                bit,
                kind,
            },
        }
    }

    /// The doorbell bit of this slot.
    pub const fn bit(&self) -> u32 {
        self.mode.bit
    }

    /// The slot's one [`Poster`]; `None` after the first call.
    pub fn claim(&'static self) -> Option<Poster<Req, Ans>> {
        if self.claimed.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some(Poster {
            slot: self,
            sequence: 0,
        })
    }

    /// Take a posted request and the duty to answer it.
    pub fn take_request(&self) -> Option<(Req, Responder<'_, Req, Ans>)> {
        self.state
            .compare_exchange(POSTED, WORKING, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;
        let request = self.take_request_payload();
        Some((request, Responder { slot: self }))
    }

    /// The answer, once the server gave it; the slot is `IDLE` again.
    fn poll_answer(&self, cx: &mut Context<'_>) -> Poll<Result<Ans, ()>> {
        self.mode.client.register(cx.waker());
        if self.state.load(Ordering::Acquire) == QUARANTINED {
            return Poll::Ready(Err(()));
        }
        if self
            .state
            .compare_exchange(ANSWERED, IDLE, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Poll::Pending;
        }
        #[allow(
            unsafe_code,
            reason = "the slot's client owns the answer place after ANSWERED"
        )]
        // SAFETY: the CAS from ANSWERED to IDLE succeeded only here, so this
        // client is the answer place's only accessor; the server's write
        // happened before its Release store of ANSWERED, which the Acquire
        // CAS observed.
        Poll::Ready(Ok(unsafe { (*self.answer.get()).assume_init_read() }))
    }
}

impl<Req, Ans, Mode> RequestSlot<Req, Ans, Mode> {
    /// Write `request` and publish POSTED. The unique client calls
    /// this only while the slot is `IDLE`.
    fn post(&self, request: Req) {
        #[allow(
            unsafe_code,
            reason = "the IDLE slot's token holder owns the request place"
        )]
        // SAFETY: the slot is IDLE and the caller holds its unique client token: the
        // server touches `request` only after its CAS from POSTED, which has
        // not happened, and the previous request was moved out before the
        // slot returned to IDLE.
        unsafe {
            (*self.request.get()).write(request);
        }
        self.state.store(POSTED, Ordering::Release);
    }

    // Only the by-value server in WORKING or the in-place client in
    // ANSWERED may consume the initialized request.
    fn take_request_payload(&self) -> Req {
        #[allow(
            unsafe_code,
            reason = "the state protocol grants exclusive access to the initialized request"
        )]
        // SAFETY: the by-value server owns WORKING after its Acquire CAS, or
        // the unique in-place client observed ANSWERED with Acquire and ended
        // its validation borrow. The other endpoint cannot access the place
        // in either phase; the current request is initialized and read once.
        unsafe {
            (*self.request.get()).assume_init_read()
        }
    }
}

impl<Req> RequestSlot<Req, (), InPlace> {
    /// Permanent storage whose notifications are supplied by its endpoints.
    pub const fn new_in_place() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            state: AtomicU8::new(IDLE),
            request: UnsafeCell::new(MaybeUninit::uninit()),
            answer: UnsafeCell::new(MaybeUninit::uninit()),
            mode: InPlace,
        }
    }

    /// Claim both unique endpoints. An abandoned seat cannot be reclaimed.
    pub fn claim_in_place(&'static self) -> Option<(InPlaceClient<Req>, InPlaceServer<Req>)> {
        if self.claimed.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some((InPlaceClient { slot: self }, InPlaceServer { slot: self }))
    }
}

/// The unique client of an in-place slot; notification stays with the caller.
#[must_use = "dropping the client retains the seat"]
pub struct InPlaceClient<Req: 'static> {
    slot: &'static RequestSlot<Req, (), InPlace>,
}

/// The unique server of an in-place slot; it stores no waker.
#[must_use = "dropping the server retains the seat"]
pub struct InPlaceServer<Req: 'static> {
    slot: &'static RequestSlot<Req, (), InPlace>,
}

impl<Req> InPlaceClient<Req> {
    /// Post an exchange only into an idle slot. An occupied slot returns the
    /// new request untouched, retaining the previous exchange in place.
    pub fn post(&mut self, request: Req) -> Result<(), Req> {
        if !self.is_idle() {
            return Err(request);
        }
        self.slot.post(request);
        Ok(())
    }

    /// Whether there is no request or reply retained by this client.
    pub fn is_idle(&self) -> bool {
        self.slot.state.load(Ordering::Acquire) == IDLE
    }

    /// Validate the completed exchange before consuming it. A refusal leaves
    /// the exchange in place, so repeated validation cannot duplicate or drop
    /// its payload. No reference into the slot escapes the validation call.
    pub fn try_take_if<E>(
        &mut self,
        validate: impl FnOnce(&Req) -> Result<(), E>,
    ) -> Option<Result<Req, E>> {
        if self.slot.state.load(Ordering::Acquire) != ANSWERED {
            return None;
        }
        #[allow(
            unsafe_code,
            reason = "ANSWERED grants the unique client the completed exchange"
        )]
        // SAFETY: the server's Release store of ANSWERED ended its mutable
        // borrow. Only this client can read the exchange or return it to IDLE;
        // the validation borrow ends before the payload is moved below.
        let reply = unsafe { (&*self.slot.request.get()).assume_init_ref() };
        if let Err(error) = validate(reply) {
            return Some(Err(error));
        }
        let reply = self.slot.take_request_payload();
        self.slot.state.store(IDLE, Ordering::Release);
        Some(Ok(reply))
    }

    /// Release the seat only by consuming both matching idle endpoints.
    /// Refusal returns both owners and leaves the seat claimed.
    pub fn reunite(self, server: InPlaceServer<Req>) -> Result<(), (Self, InPlaceServer<Req>)> {
        if !core::ptr::eq(self.slot, server.slot) || !self.is_idle() {
            return Err((self, server));
        }
        self.slot.claimed.store(false, Ordering::Release);
        Ok(())
    }
}

impl<Req> InPlaceServer<Req> {
    /// Process at most one posted request by borrowing it in place. The
    /// callback writes its reply into the exchange; its borrow ends before
    /// Release publishes ANSWERED. Duplicate or nested service does nothing.
    /// The caller publishes its external reply notification on `true`.
    pub fn service(&mut self, process: impl FnOnce(&mut Req)) -> bool {
        if self
            .slot
            .state
            .compare_exchange(POSTED, WORKING, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return false;
        }
        #[allow(
            unsafe_code,
            reason = "WORKING grants the server exclusive in-place access"
        )]
        // SAFETY: the Acquire CAS observed the client's Release publication
        // of a fully initialized request and grants this server sole access.
        // P2 cannot read or replace it before ANSWERED, stored after the
        // callback's borrow ends. An interrupted callback retains WORKING.
        let request = unsafe { (&mut *self.slot.request.get()).assume_init_mut() };
        process(request);
        self.slot.state.store(ANSWERED, Ordering::Release);
        true
    }
}

/// The client's right to post one request: the slot is `IDLE`.
#[must_use = "the poster is the client's only way to the server"]
pub struct Poster<Req: 'static, Ans: 'static> {
    slot: &'static RequestSlot<Req, Ans>,
    sequence: u32,
}

impl<Req, Ans> Poster<Req, Ans> {
    /// Post `request` and ring the server; the answer comes back with the
    /// poster.
    pub fn post(self, request: Req) -> Awaiting<Req, Ans> {
        self.slot.post(request);
        self.slot.mode.doorbell.ring(self.slot.mode.bit);
        Awaiting {
            slot: Some(self.slot),
            sequence: self.sequence.wrapping_add(1),
        }
    }
}

impl<Req, Ans> core::fmt::Debug for Poster<Req, Ans> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Poster")
    }
}

/// A posted request whose answer has not been taken yet.
///
/// Dropping it while it waits loses nothing: the answer stays in the slot.
/// Once it gave the answer or reported quarantine it is spent and stays
/// pending, so it never takes the answer of a later request.
#[must_use = "the poster returns only with the answer"]
pub struct Awaiting<Req: 'static, Ans: 'static> {
    slot: Option<&'static RequestSlot<Req, Ans>>,
    sequence: u32,
}

impl<Req, Ans> Awaiting<Req, Ans> {
    /// The answer and the returned poster, once the server answered.
    pub fn poll_answer(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(Poster<Req, Ans>, Ans), RequestError>> {
        let Some(slot) = self.slot else {
            return Poll::Pending;
        };
        let answer = core::task::ready!(slot.poll_answer(cx));
        self.slot = None;
        Poll::Ready(
            answer
                .map(|answer| {
                    (
                        Poster {
                            slot,
                            sequence: self.sequence,
                        },
                        answer,
                    )
                })
                .map_err(|()| RequestError::Unanswered {
                    kind: slot.mode.kind,
                    slot: slot.mode.bit.trailing_zeros(),
                    epoch: 0,
                    sequence: self.sequence,
                }),
        )
    }
}

/// The server's duty to answer one taken request.
///
/// Dropping it unanswered quarantines the seat and wakes its client with
/// [`RequestError::Unanswered`]. No poster is returned and the seat is never
/// reused. [`Responder::answer`] consumes it without running this drop.
#[must_use = "answer the request or dropping the responder quarantines its seat"]
pub struct Responder<'s, Req, Ans> {
    slot: &'s RequestSlot<Req, Ans>,
}

impl<Req, Ans> Responder<'_, Req, Ans> {
    /// Answer the request, and wake the client.
    pub fn answer(self, answer: Ans) {
        let slot = ManuallyDrop::new(self).slot;
        #[allow(
            unsafe_code,
            reason = "the WORKING slot's server owns the answer place"
        )]
        // SAFETY: the slot is WORKING since this responder's `take_request`,
        // and only the server holds the responder; the client reads `answer`
        // only after it observes ANSWERED, stored below, and the previous
        // answer was moved out before the slot returned to IDLE.
        unsafe {
            (*slot.answer.get()).write(answer);
        }
        slot.state.store(ANSWERED, Ordering::Release);
        slot.mode.client.wake();
    }
}

impl<Req, Ans> Drop for Responder<'_, Req, Ans> {
    fn drop(&mut self) {
        self.slot.state.store(QUARANTINED, Ordering::Release);
        self.slot.mode.client.wake();
    }
}

#[cfg(test)]
mod tests;
