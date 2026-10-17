//! One request at a time between a client and the actor, without a lock.
//!
//! A [`Slot`] lives in permanent backing and moves through four states:
//!
//! - `IDLE`: the client holds the slot's token; neither side touches the
//!   payload;
//! - `POSTED`: the client wrote the request and rang the actor's
//!   [`Doorbell`];
//! - `WORKING`: the actor took the request by CAS and owns the answer place;
//! - `ANSWERED`: the actor wrote the answer and woke the client, which takes
//!   it by CAS and returns the slot to `IDLE`.
//!
//! Each payload place is accessed only by the side that the state names, and
//! every hand-over is a Release store observed by an Acquire CAS. A client
//! posts only with the slot's token, which the post consumes and the answer
//! returns, so at most one request is in a slot. Waits register their waker
//! before they check the state, so no wake is lost. The protocol is CAS-only
//! and stays correct when one side preempts the other.

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, AtomicU32, Ordering},
    task::{Context, Poll},
};

use atomic_waker::AtomicWaker;

const IDLE: u8 = 0;
const POSTED: u8 = 1;
const WORKING: u8 = 2;
const ANSWERED: u8 = 3;

/// The actor's wake source: one bit per slot that has a request posted.
pub struct Doorbell {
    pending: AtomicU32,
    waker: AtomicWaker,
}

impl Doorbell {
    pub const fn new() -> Self {
        Self {
            pending: AtomicU32::new(0),
            waker: AtomicWaker::new(),
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
pub struct Slot<Req, Ans> {
    state: AtomicU8,
    request: UnsafeCell<MaybeUninit<Req>>,
    answer: UnsafeCell<MaybeUninit<Ans>>,
    client: AtomicWaker,
    doorbell: &'static Doorbell,
    bit: u32,
}

#[allow(
    unsafe_code,
    reason = "the slot's state protocol gives each payload place one accessor at a time"
)]
// SAFETY: `request` and `answer` are accessed only through `post`,
// `take_request`, `Responder::answer` and `poll_answer`, each of which first
// owns the state that admits it (see the module docs); payloads move between
// execution contexts, so they must be `Send`.
unsafe impl<Req: Send, Ans: Send> Sync for Slot<Req, Ans> {}

impl<Req, Ans> Slot<Req, Ans> {
    /// A slot that rings `bit` of `doorbell`.
    pub const fn new(doorbell: &'static Doorbell, bit: u32) -> Self {
        Self {
            state: AtomicU8::new(IDLE),
            request: UnsafeCell::new(MaybeUninit::uninit()),
            answer: UnsafeCell::new(MaybeUninit::uninit()),
            client: AtomicWaker::new(),
            doorbell,
            bit,
        }
    }

    /// The doorbell bit of this slot.
    pub const fn bit(&self) -> u32 {
        self.bit
    }

    /// Post `request`. Only the holder of the slot's token calls this, which
    /// puts the slot in `IDLE`.
    pub(crate) fn post(&self, request: Req) {
        debug_assert_eq!(self.state.load(Ordering::Relaxed), IDLE);
        #[allow(
            unsafe_code,
            reason = "the IDLE slot's token holder owns the request place"
        )]
        // SAFETY: the slot is IDLE and the caller holds its unique token: the
        // actor touches `request` only after its CAS from POSTED, which has
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
            reason = "the WORKING slot's actor owns the request place"
        )]
        // SAFETY: the CAS from POSTED to WORKING succeeded only here, so the
        // actor is the request place's only accessor; the client's write
        // happened before its Release store of POSTED, which the Acquire CAS
        // observed.
        let request = unsafe { (*self.request.get()).assume_init_read() };
        Some((request, Responder { slot: self }))
    }

    /// The answer, once the actor gave it; the slot is `IDLE` again.
    pub(crate) fn poll_answer(&self, cx: &mut Context<'_>) -> Poll<Ans> {
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
        // client is the answer place's only accessor; the actor's write
        // happened before its Release store of ANSWERED, which the Acquire
        // CAS observed.
        Poll::Ready(unsafe { (*self.answer.get()).assume_init_read() })
    }
}

/// The actor's duty to answer one taken request.
#[must_use = "a taken request must be answered, or its client waits forever"]
pub struct Responder<'s, Req, Ans> {
    slot: &'s Slot<Req, Ans>,
}

impl<Req, Ans> Responder<'_, Req, Ans> {
    /// Answer the request, and wake the client.
    pub fn answer(self, answer: Ans) {
        #[allow(unsafe_code, reason = "the WORKING slot's actor owns the answer place")]
        // SAFETY: the slot is WORKING since this responder's `take_request`,
        // and only the actor holds the responder; the client reads `answer`
        // only after it observes ANSWERED, stored below, and the previous
        // answer was moved out before the slot returned to IDLE.
        unsafe {
            (*self.slot.answer.get()).write(answer);
        }
        self.slot.state.store(ANSWERED, Ordering::Release);
        self.slot.client.wake();
    }
}

#[cfg(test)]
mod tests;
