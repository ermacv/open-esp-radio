//! One request at a time between a client and a server, without a lock.
//!
//! A [`RequestSlot`] lives in permanent backing and moves through four
//! states:
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
//! before they check the state, so no wake is lost. The protocol is CAS-only
//! and stays correct when one side preempts the other.

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

/// The permanent backing of one client's requests and answers.
pub struct RequestSlot<Req, Ans> {
    claimed: AtomicBool,
    state: AtomicU8,
    request: UnsafeCell<MaybeUninit<Req>>,
    answer: UnsafeCell<MaybeUninit<Ans>>,
    client: PriorityWaker,
    doorbell: &'static Doorbell,
    bit: u32,
}

#[allow(
    unsafe_code,
    reason = "the slot's state protocol gives each payload place one accessor at a time"
)]
// SAFETY: `request` and `answer` are accessed only through `Poster::post`,
// `take_request`, `Responder::answer` and `Awaiting::poll_answer`, each of
// which first owns the state that admits it (see the module docs); payloads
// move between execution contexts, so they must be `Send`.
unsafe impl<Req: Send, Ans: Send> Sync for RequestSlot<Req, Ans> {}

impl<Req, Ans> RequestSlot<Req, Ans> {
    /// A slot that rings `bit` of `doorbell`.
    pub const fn new(doorbell: &'static Doorbell, bit: u32) -> Self {
        Self {
            claimed: AtomicBool::new(false),
            state: AtomicU8::new(IDLE),
            request: UnsafeCell::new(MaybeUninit::uninit()),
            answer: UnsafeCell::new(MaybeUninit::uninit()),
            client: PriorityWaker::new(),
            doorbell,
            bit,
        }
    }

    /// The doorbell bit of this slot.
    pub const fn bit(&self) -> u32 {
        self.bit
    }

    /// The slot's one [`Poster`]; `None` after the first call.
    pub fn claim(&'static self) -> Option<Poster<Req, Ans>> {
        if self.claimed.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some(Poster { slot: self })
    }

    /// Write `request` and ring the doorbell. Only a [`Poster`] calls this,
    /// and a poster exists only while the slot is `IDLE`.
    fn post(&self, request: Req) {
        debug_assert_eq!(self.state.load(Ordering::Relaxed), IDLE);
        #[allow(
            unsafe_code,
            reason = "the IDLE slot's token holder owns the request place"
        )]
        // SAFETY: the slot is IDLE and the caller holds its unique poster: the
        // server touches `request` only after its CAS from POSTED, which has
        // not happened, and the previous request was moved out before the
        // slot returned to IDLE.
        unsafe {
            (*self.request.get()).write(request);
        }
        self.state.store(POSTED, Ordering::Release);
        self.doorbell.ring(self.bit);
    }

    /// Take a posted request and the duty to answer it.
    pub fn take_request(&self) -> Option<(Req, Responder<'_, Req, Ans>)> {
        self.state
            .compare_exchange(POSTED, WORKING, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;
        #[allow(
            unsafe_code,
            reason = "the WORKING slot's server owns the request place"
        )]
        // SAFETY: the CAS from POSTED to WORKING succeeded only here, so the
        // server is the request place's only accessor; the client's write
        // happened before its Release store of POSTED, which the Acquire CAS
        // observed.
        let request = unsafe { (*self.request.get()).assume_init_read() };
        Some((request, Responder { slot: self }))
    }

    /// The answer, once the server gave it; the slot is `IDLE` again.
    fn poll_answer(&self, cx: &mut Context<'_>) -> Poll<Ans> {
        self.client.register(cx.waker());
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
        Poll::Ready(unsafe { (*self.answer.get()).assume_init_read() })
    }
}

/// The client's right to post one request: the slot is `IDLE`.
#[must_use = "the poster is the client's only way to the server"]
pub struct Poster<Req: 'static, Ans: 'static> {
    slot: &'static RequestSlot<Req, Ans>,
}

impl<Req, Ans> Poster<Req, Ans> {
    /// Post `request` and ring the server; the answer comes back with the
    /// poster.
    pub fn post(self, request: Req) -> Awaiting<Req, Ans> {
        self.slot.post(request);
        Awaiting {
            slot: Some(self.slot),
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
/// Once it gave the answer it is spent and stays pending, so it never takes
/// the answer of a later request.
#[must_use = "the poster returns only with the answer"]
pub struct Awaiting<Req: 'static, Ans: 'static> {
    slot: Option<&'static RequestSlot<Req, Ans>>,
}

impl<Req, Ans> Awaiting<Req, Ans> {
    /// The answer and the returned poster, once the server answered.
    pub fn poll_answer(&mut self, cx: &mut Context<'_>) -> Poll<(Poster<Req, Ans>, Ans)> {
        let Some(slot) = self.slot else {
            return Poll::Pending;
        };
        let answer = core::task::ready!(slot.poll_answer(cx));
        self.slot = None;
        Poll::Ready((Poster { slot }, answer))
    }
}

/// The server's duty to answer one taken request.
///
/// Dropping it unanswered would leave its client waiting forever, so the drop
/// panics; [`Responder::answer`] consumes it without dropping.
#[must_use = "a taken request must be answered, or its client waits forever"]
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
        slot.client.wake();
    }
}

impl<Req, Ans> Drop for Responder<'_, Req, Ans> {
    fn drop(&mut self) {
        unreachable!("a taken request must be answered");
    }
}

#[cfg(test)]
mod tests;
