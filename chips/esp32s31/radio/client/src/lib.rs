#![no_std]
#![forbid(unsafe_code)]

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
//!   atomic wakers and a CAS on the cell's state (`oer_memory::RequestSlot`). Dropping a
//!   [`Pending`] future loses nothing: the answer stays in the cell for the
//!   next poll of the same `Pending`.
//! - **Membership.** `join` answers [`Member<K, Untracked>`](Member) and the
//!   current [`RfEpoch`]; `track` is the only producer of
//!   [`Member<K, Tracked>`](Member), which alone permits RF use; `leave`
//!   takes either state and never refuses. A failed request hands back what
//!   it lent; its [`Failure`] says what the failure touched (below), and
//!   after [`Failure::Poisoned`] every later answer returns what was posted.
//! - **Wi-Fi.** The actor brings the Wi-Fi MAC up inside `track`, after the
//!   initial tracking, the RX enable and the initial channel, in the order of
//!   the driver's cold start: the driver lends its clocked Wi-Fi client
//!   for that one request and gets it back with the MAC up ([`wifi`]).
//! - **Bounds.** [`ACTOR_STEP_BOUND`] is the provisional bound of one
//!   actor poll; the request bounds follow from measurements and are added
//!   then.

mod lp_clock;
mod member;
pub mod wifi;

use core::{
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

pub use lp_clock::{LpClock, LpSource, RcSlow, Xtal32k};
pub use member::{Leaving, Member, MemberMint, RfEpoch, Tracked, Untracked};
pub use oer_memory::{Awaiting, Doorbell, Poster, RequestSlot, Responder};

/// The provisional bound of one poll of the actor future
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
    /// Stopped-client channel transaction and its returned ownership.
    type Retune: Send;
    type Retuned: Send;
    type RetuneReturned: Send;
    /// What `leave` lends.
    type Leave: Send;
    /// What `leave` gives back.
    type Left: Send;

    /// A `join` handed back unchanged.
    fn join_returned(join: Self::Join) -> Self::JoinReturned;
    /// A `track` handed back unchanged.
    fn track_returned(track: Self::Track) -> Self::TrackReturned;
    fn retune_returned(retune: Self::Retune) -> Self::RetuneReturned;
}

/// A request in a client's cell.
pub enum Request<K: Client> {
    Join(K::Join),
    Track(Member<K, Untracked>, K::Track),
    Retune(Member<K, Tracked>, K::Retune),
    Leave(Leaving<K>, K::Leave),
}

/// An answer in a client's cell.
pub enum Answer<K: Client> {
    Join(JoinAnswer<K>),
    Track(TrackAnswer<K>),
    Retune(RetuneAnswer<K>),
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
        /// The registration's complete outcome, with its diagnostics.
        #[cfg(oer_trace)]
        outcome: oer_esp32s31_phy::PhyRegisterOutcome,
    },
}

/// Why a request did not succeed. The backend classifies every failure by
/// what it touched; in each case the answer hands back everything the
/// request lent.
///
/// | Cause | What changed |
/// | --- | --- |
/// | [`Refused`](Self::Refused) | Nothing: refused before any hardware write, a clock planner's refusal before its first write included. |
/// | [`ClientFailed`](Self::ClientFailed) | Only the client's own hardware (its MAC partition, its clocks, its coexistence participation), and the rollback restored it. The shared domain (PHY, shared radio registers, common power) was not touched, or not left in an intermediate state. |
/// | [`Poisoned`](Self::Poisoned) | Shared state was written before the failure, or the rollback failed: the domain is poisoned until reset. |
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    /// Refused before any hardware write; nothing changed.
    Refused,
    /// The client's own hardware work failed and was rolled back; the shared
    /// domain is healthy.
    ClientFailed,
    /// Shared hardware work failed, or had failed before: the PHY domain is
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

/// A stopped member after its channel transaction.
pub struct RetunedMember<K: Client> {
    pub member: Member<K, Tracked>,
    pub payload: K::Retuned,
}

pub type RetuneAnswer<K> =
    Result<RetunedMember<K>, Failed<(Member<K, Tracked>, <K as Client>::RetuneReturned)>>;

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

type ClientSlot<K> = RequestSlot<Request<K>, Answer<K>>;

/// A client's one cell to the actor.
#[must_use = "the cell is the client's only way to the actor"]
pub struct MembershipCell<K: Client> {
    poster: Poster<Request<K>, Answer<K>>,
}

impl<K: Client> core::fmt::Debug for MembershipCell<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("MembershipCell")
    }
}

impl<K: Client> MembershipCell<K> {
    /// Join the PHY domain: register it, or wake closed RF, and enter `K`.
    pub fn join(self, join: K::Join) -> Pending<K, JoinAnswer<K>> {
        Pending::new(
            self.poster.post(Request::Join(join)),
            |answer| match answer {
                Answer::Join(answer) => answer,
                _ => unreachable!("the actor answers a join with a join answer"),
            },
        )
    }

    /// Run the member's initial tracking.
    pub fn track(
        self,
        member: Member<K, Untracked>,
        track: K::Track,
    ) -> Pending<K, TrackAnswer<K>> {
        Pending::new(
            self.poster.post(Request::Track(member, track)),
            |answer| match answer {
                Answer::Track(answer) => answer,
                _ => unreachable!("the actor answers a track with a track answer"),
            },
        )
    }

    /// Retune a tracked client while its DMA and interrupt routes are stopped.
    pub fn retune(
        self,
        member: Member<K, Tracked>,
        retune: K::Retune,
    ) -> Pending<K, RetuneAnswer<K>> {
        Pending::new(
            self.poster.post(Request::Retune(member, retune)),
            |answer| match answer {
                Answer::Retune(answer) => answer,
                _ => unreachable!("the actor answers a retune with a retune answer"),
            },
        )
    }

    /// Leave the PHY domain; the last member's leave closes RF.
    pub fn leave(self, member: Leaving<K>, leave: K::Leave) -> Pending<K, Left<K>> {
        Pending::new(
            self.poster.post(Request::Leave(member, leave)),
            |answer| match answer {
                Answer::Leave(answer) => answer,
                _ => unreachable!("the actor answers a leave with a leave answer"),
            },
        )
    }
}

/// A posted request: a future of its answer and the returned cell.
///
/// Polling it through `&mut` and dropping that future loses nothing: the
/// answer stays in the cell. After it gave its answer it stays pending.
#[must_use = "the cell returns only with the answer"]
pub struct Pending<K: Client, O> {
    awaiting: Awaiting<Request<K>, Answer<K>>,
    extract: fn(Answer<K>) -> O,
    output: PhantomData<fn() -> O>,
}

impl<K: Client, O> Pending<K, O> {
    fn new(awaiting: Awaiting<Request<K>, Answer<K>>, extract: fn(Answer<K>) -> O) -> Self {
        Self {
            awaiting,
            extract,
            output: PhantomData,
        }
    }
}

impl<K: Client, O> Future for Pending<K, O> {
    type Output = (MembershipCell<K>, O);

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let (poster, answer) = core::task::ready!(self.awaiting.poll_answer(cx));
        Poll::Ready((MembershipCell { poster }, (self.extract)(answer)))
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
static WIFI: ClientSlot<wifi::Wifi> = RequestSlot::new(&DOORBELL, wifi::BIT);

/// The actor's end and the clients' cells, once per image; `None` after the
/// first call.
pub fn claim() -> Option<(ActorEnd, ClientCells)> {
    ends(&DOORBELL, &WIFI)
}

/// The ends over `wifi`'s slot, once: its one poster is claimed here.
fn ends(
    doorbell: &'static Doorbell,
    wifi: &'static ClientSlot<wifi::Wifi>,
) -> Option<(ActorEnd, ClientCells)> {
    let poster = wifi.claim()?;
    Some((
        ActorEnd {
            doorbell,
            wifi: ClientInbox { slot: wifi },
            mint: MemberMint::new(),
        },
        ClientCells {
            wifi: MembershipCell { poster },
        },
    ))
}

/// Fresh ends in leaked backing, for one host test; the static claim stays
/// with the image.
#[cfg(any(test, feature = "test-support"))]
pub fn claim_for_host_test() -> (ActorEnd, ClientCells) {
    extern crate std;
    let doorbell: &'static Doorbell = std::boxed::Box::leak(std::boxed::Box::new(Doorbell::new()));
    let wifi = std::boxed::Box::leak(std::boxed::Box::new(RequestSlot::new(doorbell, wifi::BIT)));
    ends(doorbell, wifi).expect("a fresh slot has its poster")
}

#[cfg(test)]
mod tests;
