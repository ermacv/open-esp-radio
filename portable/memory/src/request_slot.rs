//! One request at a time between a client and a server, without a lock.
//!
//! A [`RequestSlot`] lives in permanent backing and moves through four
//! states:
//!
//! - `IDLE`: neither side touches the exchange; the client may post;
//! - `POSTED`: the client wrote the exchange;
//! - `WORKING`: the server took it by CAS and borrows it in place;
//! - `ANSWERED`: the server's borrow ended with its reply in the exchange;
//!   the client validates it, takes it and returns the slot to `IDLE`.
//!
//! The exchange is accessed only by the side that the state names, and every
//! hand-over is a Release store observed by an Acquire access. The slot holds
//! no waker: external signals carry only readiness. Abandonment leaves the
//! seat claimed; both idle endpoints must reunite before another pair can
//! claim it.

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};

const IDLE: u8 = 0;
const POSTED: u8 = 1;
const WORKING: u8 = 2;
const ANSWERED: u8 = 3;

/// The permanent backing of one client's requests and answers.
///
/// The request also carries its reply: the slot lends it to a server and
/// retains it until the client validates and takes the completed exchange.
/// Dropping either endpoint retains the seat permanently; only reuniting both
/// idle endpoints permits a new claim.
pub struct RequestSlot<Req> {
    claimed: AtomicBool,
    state: AtomicU8,
    request: UnsafeCell<MaybeUninit<Req>>,
}

#[allow(
    unsafe_code,
    reason = "the slot's state protocol gives each payload place one accessor at a time"
)]
// SAFETY: `request` is accessed only through the endpoints, each of which
// first owns the state that admits it. The server's borrow ends before
// ANSWERED, and the client validates and takes only in ANSWERED. A forgotten
// endpoint or interrupted service retains the seat. The payload crosses
// contexts and must be Send.
unsafe impl<Req: Send> Sync for RequestSlot<Req> {}

impl<Req> RequestSlot<Req> {
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

    // Only the client in ANSWERED may consume the initialized request.
    fn take_request_payload(&self) -> Req {
        #[allow(
            unsafe_code,
            reason = "the state protocol grants exclusive access to the initialized request"
        )]
        // SAFETY: the unique client observed ANSWERED with Acquire and ended
        // its validation borrow. The server cannot access the place in this
        // phase; the current request is initialized and read once.
        unsafe {
            (*self.request.get()).assume_init_read()
        }
    }

    /// Permanent storage whose notifications are supplied by its endpoints.
    pub const fn new_in_place() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            state: AtomicU8::new(IDLE),
            request: UnsafeCell::new(MaybeUninit::uninit()),
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
    slot: &'static RequestSlot<Req>,
}

/// The unique server of an in-place slot; it stores no waker.
#[must_use = "dropping the server retains the seat"]
pub struct InPlaceServer<Req: 'static> {
    slot: &'static RequestSlot<Req>,
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

#[cfg(test)]
mod tests;
