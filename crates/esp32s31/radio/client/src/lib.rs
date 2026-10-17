#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! The client contract of the ESP32-S31 radio system.
//!
//! One actor owns the shared RF front-end: the PHY domain, the shared radio
//! registers, the modem clocks and coexistence (`oer-esp32s31-radio-system`).
//! A protocol driver is one of its clients ([`Client`]) and reaches it only
//! through its [`MembershipCell`]: it posts `join`, `track` or `leave`
//! synchronously, never waiting and never refused at the post, and awaits the
//! answer in the same cell.
//!
//! - **Cells.** Each client has one cell per image, in permanent backing,
//!   claimed once with the actor's end by [`claim`], which returns `None` on a
//!   second call. Posting consumes the cell token and the answer returns it,
//!   so a cell holds at most one request. The protocol is lock-free: two
//!   atomic wakers and a CAS on the cell's state (`cell`). Dropping a
//!   [`Pending`] future loses nothing: the answer stays in the cell for the
//!   next poll of the same `Pending`.
//! - **Membership.** `join` answers [`Member<K, Untracked>`](Member) and the
//!   current [`RfEpoch`]; `track` is the only producer of
//!   [`Member<K, Tracked>`](Member), which alone permits RF use; `leave`
//!   takes either state and never refuses. A failure before any hardware work
//!   is [`Failure::Refused`] and hands everything back; a hardware failure
//!   poisons the domain ([`Failure::Poisoned`]), and every later answer
//!   returns what was posted.
//! - **Wi-Fi.** The actor brings the Wi-Fi MAC up inside `track`, after the
//!   initial tracking, the RX enable and the initial channel, in the order of
//!   the driver's former cold start: the driver lends its clocked Wi-Fi client
//!   for that one request and gets it back with the MAC up ([`wifi`]).
//! - **Bounds.** [`ACTOR_STEP_BOUND`] is the owner's provisional bound of one
//!   actor poll; the request bounds follow from measurements and are added
//!   then.

mod cell;
mod lp_clock;
mod member;
pub mod wifi;

use core::{
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};

pub use cell::{Doorbell, Responder, Slot};
pub use lp_clock::{LpClock, LpSource, RcSlow, Xtal32k};
pub use member::{Leaving, Member, MemberMint, RfEpoch, Tracked, Untracked};

/// The owner's provisional bound of one poll of the actor future
/// (docs/decisions.md, "Radio system").
pub const ACTOR_STEP_BOUND: oer_time::Duration = oer_time::Duration::from_micros(50);

mod sealed {
    pub trait Sealed {}
}

/// A client of the radio system and the payloads of its requests.
pub trait Client: sealed::Sealed + Sized + 'static {
    /// What `join` lends: the client's cold partition.
    type Join: Send;
    /// What a successful `join` gives back beside the membership.
    type Joined: Send;
    /// What a failed `join` gives back.
    type JoinReturned: Send;
    /// What `track` lends.
    type Track: Send;
    /// What a successful `track` gives back beside the membership.
    type Tracked: Send;
    /// What a failed `track` gives back beside the membership.
    type TrackReturned: Send;
    /// What `leave` lends.
    type Leave: Send;
    /// What `leave` gives back.
    type Left: Send;

    /// A `join` handed back unchanged.
    fn join_returned(join: Self::Join) -> Self::JoinReturned;
    /// A `track` handed back unchanged.
    fn track_returned(track: Self::Track) -> Self::TrackReturned;
}

/// A request in a client's cell.
pub enum Request<K: Client> {
    Join(K::Join),
    Track(Member<K, Untracked>, K::Track),
    Leave(Leaving<K>, K::Leave),
}

/// An answer in a client's cell.
pub enum Answer<K: Client> {
    Join(JoinAnswer<K>),
    Track(TrackAnswer<K>),
    Leave(Left<K>),
}

/// How `join` prepared the PHY domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPreparation {
    /// RF was already open for another member.
    AlreadyOpen,
    /// Closed RF woke with the retained registration.
    Woke { micros: u64 },
    /// The domain was registered.
    Registered {
        calibration_path: oer_esp32s31_phy::PhyCalibrationPath,
        micros: u64,
    },
}

/// Why a request did not succeed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    /// Refused before any hardware work; nothing changed.
    Refused,
    /// A hardware transaction failed, or had failed before: the PHY domain is
    /// poisoned until reset.
    Poisoned,
}

/// A failed request and what it hands back.
#[derive(Debug)]
pub struct Failed<T> {
    pub returned: T,
    pub cause: Failure,
}

/// A client joined the PHY domain.
pub struct Joined<K: Client> {
    pub member: Member<K, Untracked>,
    pub epoch: RfEpoch,
    pub payload: K::Joined,
}

/// The answer to `join`.
pub type JoinAnswer<K> = Result<Joined<K>, Failed<<K as Client>::JoinReturned>>;

/// A member's initial tracking ran.
pub struct TrackedMember<K: Client> {
    pub member: Member<K, Tracked>,
    pub payload: K::Tracked,
}

/// The answer to `track`.
pub type TrackAnswer<K> =
    Result<TrackedMember<K>, Failed<(Member<K, Untracked>, <K as Client>::TrackReturned)>>;

/// Whether the PHY domain survived.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomainHealth {
    Healthy,
    Poisoned,
}

/// A client left; `leave` never refuses.
pub struct Left<K: Client> {
    pub payload: K::Left,
    pub domain: DomainHealth,
}

type ClientSlot<K> = Slot<Request<K>, Answer<K>>;

/// A client's one cell to the actor.
#[must_use = "the cell is the client's only way to the actor"]
pub struct MembershipCell<K: Client> {
    slot: &'static ClientSlot<K>,
}

impl<K: Client> core::fmt::Debug for MembershipCell<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("MembershipCell")
    }
}

impl<K: Client> MembershipCell<K> {
    /// Join the PHY domain: register it, or wake closed RF, and enter `K`.
    pub fn join(self, join: K::Join) -> Pending<K, JoinAnswer<K>> {
        self.slot.post(Request::Join(join));
        Pending::new(self.slot, |answer| match answer {
            Answer::Join(answer) => answer,
            _ => unreachable!("the actor answers a join with a join answer"),
        })
    }

    /// Run the member's initial tracking.
    pub fn track(
        self,
        member: Member<K, Untracked>,
        track: K::Track,
    ) -> Pending<K, TrackAnswer<K>> {
        self.slot.post(Request::Track(member, track));
        Pending::new(self.slot, |answer| match answer {
            Answer::Track(answer) => answer,
            _ => unreachable!("the actor answers a track with a track answer"),
        })
    }

    /// Leave the PHY domain; the last member's leave closes RF.
    pub fn leave(self, member: Leaving<K>, leave: K::Leave) -> Pending<K, Left<K>> {
        self.slot.post(Request::Leave(member, leave));
        Pending::new(self.slot, |answer| match answer {
            Answer::Leave(answer) => answer,
            _ => unreachable!("the actor answers a leave with a leave answer"),
        })
    }
}

/// A posted request: a future of its answer and the returned cell.
///
/// Polling it through `&mut` and dropping that future loses nothing: the
/// answer stays in the cell. After it gave its answer it stays pending.
#[must_use = "the cell returns only with the answer"]
pub struct Pending<K: Client, O> {
    slot: Option<&'static ClientSlot<K>>,
    extract: fn(Answer<K>) -> O,
    output: PhantomData<fn() -> O>,
}

impl<K: Client, O> Pending<K, O> {
    fn new(slot: &'static ClientSlot<K>, extract: fn(Answer<K>) -> O) -> Self {
        Self {
            slot: Some(slot),
            extract,
            output: PhantomData,
        }
    }
}

impl<K: Client, O> Future for Pending<K, O> {
    type Output = (MembershipCell<K>, O);

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Some(slot) = self.slot else {
            return Poll::Pending;
        };
        match slot.poll_answer(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(answer) => {
                self.slot = None;
                Poll::Ready((MembershipCell { slot }, (self.extract)(answer)))
            }
        }
    }
}

impl<K: Client, O> Unpin for Pending<K, O> {}

/// A taken request and the duty to answer it.
pub type Taken<K> = (Request<K>, Responder<'static, Request<K>, Answer<K>>);

/// The actor's end of one client's cell.
pub struct ClientInbox<K: Client> {
    slot: &'static ClientSlot<K>,
}

impl<K: Client> ClientInbox<K> {
    /// The doorbell bit this client rings.
    pub const fn bit(&self) -> u32 {
        self.slot.bit()
    }

    /// The posted request and the duty to answer it, if one is posted.
    pub fn take(&self) -> Option<Taken<K>> {
        self.slot.take_request()
    }
}

/// The actor's end of every cell.
pub struct ActorEnd {
    pub doorbell: &'static Doorbell,
    pub wifi: ClientInbox<wifi::Wifi>,
    pub mint: MemberMint,
}

/// The clients' ends of every cell.
pub struct ClientCells {
    pub wifi: MembershipCell<wifi::Wifi>,
}

static DOORBELL: Doorbell = Doorbell::new();
static WIFI: ClientSlot<wifi::Wifi> = Slot::new(&DOORBELL, wifi::BIT);
static CLAIMED: AtomicBool = AtomicBool::new(false);

/// The actor's end and the clients' cells, once per image; `None` after the
/// first call.
pub fn claim() -> Option<(ActorEnd, ClientCells)> {
    if CLAIMED.swap(true, Ordering::AcqRel) {
        return None;
    }
    Some(ends(&DOORBELL, &WIFI))
}

fn ends(
    doorbell: &'static Doorbell,
    wifi: &'static ClientSlot<wifi::Wifi>,
) -> (ActorEnd, ClientCells) {
    (
        ActorEnd {
            doorbell,
            wifi: ClientInbox { slot: wifi },
            mint: MemberMint::new(),
        },
        ClientCells {
            wifi: MembershipCell { slot: wifi },
        },
    )
}

/// Fresh ends in leaked backing, for one host test; the static claim stays
/// with the image.
#[cfg(any(test, feature = "test-support"))]
pub fn claim_for_host_test() -> (ActorEnd, ClientCells) {
    extern crate std;
    let doorbell: &'static Doorbell = std::boxed::Box::leak(std::boxed::Box::new(Doorbell::new()));
    let wifi = std::boxed::Box::leak(std::boxed::Box::new(Slot::new(doorbell, wifi::BIT)));
    ends(doorbell, wifi)
}

#[cfg(test)]
mod tests;
