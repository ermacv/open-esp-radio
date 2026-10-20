#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
#![cfg_attr(
    test,
    allow(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::disallowed_macros,
    )
)]

//! The arbiter of the channel one radio's station and access point share:
//! the third sans-IO machine of their loop.
//!
//! Only the arbiter changes the port's channel: it closes the transmit
//! gate, retunes the port and opens the gate again, one gate's cycle at a
//! time. The station asks it for a channel and holds the [`Lease`] it
//! answers with; the access point tells it its schedule and follows its
//! commands. A station alone has an arbiter of the same shape, which
//! leases at once.
//!
//! - **The answer is a return value.** [`Arbiter::request`] answers in the
//!   same step, with a lease or a command to leave, so no request goes
//!   unanswered. Only a lease's start is later: its moment `from` is the
//!   arbiter's own deadline.
//! - **One lease at a time, by type.** A request takes the station's
//!   [`LeasePlace`], which comes back with the lease's end or cancellation,
//!   so a station never holds two leases. [`StationNotice`]s name the
//!   lease, so a notice of an earlier lease is never taken for the
//!   current one, and every lease ends in exactly one of
//!   [`StationNotice::LeaseEnded`] or [`StationNotice::LeaseCancelled`].
//! - **A connected station's absence** takes its own
//!   token, the [`AbsencePlace`], and names the running primary lease
//!   ([`Arbiter::request_absence`]): it starts a preparation window later,
//!   in which the station tells its access point it leaves (a Null with
//!   PM=1), and only once the station said [`Arbiter::ready`]; one not
//!   ready at its start is cancelled ([`CancelReason::NotReady`]). It
//!   returns to the primary lease's channel, and it ends before the
//!   primary lease does. When the upstream moves the port, the absence is
//!   cancelled ([`CancelReason::Switch`]), and on the [`StationNotice::Moved`]
//!   notice the station exchanges its primary lease for one on the new
//!   channel ([`Arbiter::exchange`]): the old lease goes by value, so two
//!   primary leases never exist at once.
//! - **Order.** The gate closes before every move of the port, so every
//!   terminal of the station's attempts comes before the lease's start and
//!   before its end.
//! - **Time.** The gate starts closing at the lease's `from`, and the lease
//!   starts once it closed and the port moved. Its `until` is the moment
//!   the port is back on the access point's channel: the return starts a
//!   retune earlier, and the station listens only until then
//!   ([`Lease::listen_until`]). A lease whose gate closes too late for two
//!   retunes and the policy's shortest listening before `until` is
//!   cancelled ([`CancelReason::Late`]).
//! - **Beside an access point**, the station leaves the access point's
//!   channel only in absences after the access point's TBTTs. If the
//!   access point stops during an absence, the lease is cancelled
//!   ([`CancelReason::AccessPointStopped`]) and the port returns.
//! - **A refused move** onto a lease's channel cancels that lease
//!   ([`CancelReason::Refused`]) and the port returns; a failed return, or
//!   a poisoned port, stops the station and the access point
//!   ([`LeaveReason::PortFailed`]).
//! - **The air reservation** is the arbiter's type parameter
//!   ([`ReservationMode`]): an arbiter that reserves the access point's air
//!   for an absence closes the gate with one last frame, a CTS-to-self the
//!   driver sends with the Duration left, and only a phase that sends it
//!   ([`AirReservationPhase`])
//!   closes its gates ([`CloseGateWith`]).
//! - **The upstream** (the access point the station is connected to) is
//!   the [`coordinator`]'s: the station reports its switch announcements,
//!   its loss and where it found it again ([`UpstreamFact`]); the arbiter
//!   moves the port, announces the access point's own switch, stops and
//!   starts the access point, and tells the station where the port went
//!   ([`StationNotice::Moved`]) or that it must leave.
//! - **A join elsewhere** beside a running access point follows the
//!   [`ApFollowPolicy`]: the access point announces its
//!   move to the station's channel and the join's lease starts with the
//!   move; one it may not serve stops it, or refuses the join
//!   ([`Refusal::Unservable`]). The announced move is never withdrawn:
//!   after a failed join the access point stays there. Within the follow
//!   interval of its last such move, a join elsewhere is refused with the
//!   moment to retry ([`Refusal::TooSoon`]); the interval doubles after
//!   each failed join that followed a move, up to
//!   [`ApFollowPolicy::max_follow_interval`], and starts again from
//!   [`ApFollowPolicy::min_follow_interval`] once the station connects. A
//!   lease names the access point's channel ([`Lease::access_point`]), so
//!   the station can prefer a candidate there.
//! - **The outbox** holds one cell for each kind of notice and command,
//!   whose capacity the tokens prove: a lease's end carries the one place,
//!   a start needs a lease of it, and a move or a start happens only in a
//!   gate's cycle, before which the owner takes every cell.
//!
//! # Compile-fail tests
//!
//! A phase without the air reservation closes no gate of a reserving
//! arbiter:
//!
//! ```compile_fail,E0277
//! # use oer_ieee80211_arbiter::{AirReservation, CloseGateWith};
//! # use oer_ieee80211_port::{GateOpen, Ieee80211Instant, LowerMacPhase, PhaseTypes};
//! fn close<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(phase: &mut P, gate: GateOpen<'id, <P::Types as PhaseTypes<'id>>::GatePath>) {
//!     CloseGateWith::<'p, 'id, AirReservation>::close_gate_with(
//!         phase,
//!         gate,
//!         None,
//!         |_| -> Ieee80211Instant { unreachable!() },
//!     );
//! }
//! ```
//!
//! Its compiling twins: such a phase closes the gates of an arbiter without
//! the reservation, and a phase with it those of a reserving one.
//!
//! ```
//! # use oer_ieee80211_arbiter::{AirReservation, CloseGateWith, NoReservation};
//! # use oer_ieee80211_port::{AirReservationPhase, GateClosed, GateOpen, Ieee80211Instant, LowerMacPhase, PhaseTypes};
//! fn plain<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(phase: &mut P, gate: GateOpen<'id, <P::Types as PhaseTypes<'id>>::GatePath>) {
//!     CloseGateWith::<'p, 'id, NoReservation>::close_gate_with(
//!         phase,
//!         gate,
//!         (),
//!         |_| -> Ieee80211Instant { unreachable!() },
//!     );
//! }
//! fn reserving<'p, 'id: 'p, P: AirReservationPhase<'p, 'id>>(phase: &mut P, gate: GateOpen<'id, <P::Types as PhaseTypes<'id>>::GatePath>) {
//!     CloseGateWith::<'p, 'id, AirReservation>::close_gate_with(
//!         phase,
//!         gate,
//!         None,
//!         |_| -> Ieee80211Instant { unreachable!() },
//!     );
//! }
//! ```
//!
//! No caller makes a lease place, so no station holds two leases:
//!
//! ```compile_fail,E0451
//! let _ = oer_ieee80211_arbiter::LeasePlace::<oer_radio_port::Root> {
//!     private: core::marker::PhantomData,
//! };
//! ```

#[cfg(test)]
extern crate std;

pub mod channel;
pub mod coordinator;

use core::{fmt::Debug, marker::PhantomData};

use oer_ieee80211_port::{
    AirReservationPhase, GateClosed, GateOpen, Ieee80211Capabilities, Ieee80211Instant,
    LowerMacPhase, PhaseTypes, SupportedChannel,
};
use oer_ieee80211_wire::channel_switch::ChannelSwitchMode;
use oer_radio_port::{PoolPath, Root};
use oer_time::{Duration, Instant};

use coordinator::{
    ApFollowPolicy, ApSchedule, ApSearchPolicy, BeaconInterval, ChannelCoordinator,
    CoordinatorAction, CoordinatorActions, JoinAway, UpstreamLossPolicy,
};

/// The longest air reservation: the Duration field's limit, in
/// microseconds.
pub const MAX_RESERVATION: Duration = Duration::from_micros(32_767);

type Brand<'id, Path> = PhantomData<fn(&'id (), Path) -> (&'id (), Path)>;

/// One gate and its arbiter's unique construction authority.
///
/// The installation's mint builds this component once. Returning a gate
/// after a cycle does not mint another arbiter or another pair of lease places.
pub struct ArbiterPool;

impl oer_radio_port::PoolSet for ArbiterPool {
    type Pools<'id, Path: PoolPath> = (
        oer_ieee80211_port::GateOpen<'id, Path>,
        oer_ieee80211_port::GateIssuer<'id, Path>,
        ArbiterAuthority<'id, Path>,
    );

    fn build<'id, Path: PoolPath>(
        builder: oer_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        let (open, issuer) = builder.into_pools::<oer_ieee80211_port::GatePool>();
        (open, issuer, ArbiterAuthority { brand: PhantomData })
    }
}

/// The sole right to construct the arbiter of this installation's gate.
#[derive(Debug)]
#[must_use = "construct the arbiter exactly once with its initial gate"]
pub struct ArbiterAuthority<'id, Path: PoolPath = Root> {
    brand: Brand<'id, Path>,
}

/// The name of one lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseId<'id, Path: PoolPath = Root>(u64, Brand<'id, Path>);

/// The station's right to hold one lease: a request takes it, and the
/// lease's end or cancellation gives it back. Only the arbiter makes one.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "retain the sole place for the station's next channel request"]
pub struct LeasePlace<'id, Path: PoolPath = Root> {
    private: Brand<'id, Path>,
}

/// The connected station's right to one absence from its primary lease's
/// channel: [`Arbiter::request_absence`] takes it, and the absence's end or
/// cancellation gives it back. Only the arbiter makes one.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "retain the sole place for the station's next absence request"]
pub struct AbsencePlace<'id, Path: PoolPath = Root> {
    private: Brand<'id, Path>,
}

/// Complete station admission bank returned by a stopped arbiter. Both places
/// are actual returned owners; a station restart cannot accept a partial bank.
#[must_use = "move both places into the restarted station"]
pub struct StationPlaces<'id, Path: PoolPath = Root> {
    lease: LeasePlace<'id, Path>,
    absence: AbsencePlace<'id, Path>,
}

impl<'id, Path: PoolPath> StationPlaces<'id, Path> {
    pub fn into_parts(self) -> (LeasePlace<'id, Path>, AbsencePlace<'id, Path>) {
        (self.lease, self.absence)
    }
}

/// Places already idle when the station stopped, followed by the places of
/// leases cancelled by arbiter retirement. This collection has no restart
/// authority. Only the stopped arbiter consumes it into `StationPlaces`.
#[must_use = "return the collection to the stopped arbiter or retire it"]
pub struct StationPlaceReturns<'id, Path: PoolPath = Root> {
    lease: Option<LeasePlace<'id, Path>>,
    absence: Option<AbsencePlace<'id, Path>>,
}

impl<'id, Path: PoolPath> StationPlaceReturns<'id, Path> {
    pub fn new(
        lease: Option<LeasePlace<'id, Path>>,
        absence: Option<AbsencePlace<'id, Path>>,
    ) -> Self {
        Self { lease, absence }
    }

    /// The installation has exactly one place of each kind. These notices
    /// return owners which had left the station, so they cannot duplicate an
    /// idle place retained by the same branded collection.
    pub fn returned<K>(&mut self, notice: StationNotice<'id, K, Path>) {
        match notice {
            StationNotice::LeaseEnded(place) | StationNotice::LeaseCancelled(_, place) => {
                self.lease = Some(place);
            }
            StationNotice::AbsenceEnded(place) | StationNotice::AbsenceCancelled(_, place) => {
                self.absence = Some(place);
            }
            StationNotice::LeaseStarted(_)
            | StationNotice::AbsenceStarted(_)
            | StationNotice::Moved(_)
            | StationNotice::Leave(_) => {}
        }
    }
}

/// The port's channel, leased to the station from `from` until `until`
/// (`None`: until the station gives it back). Only the arbiter makes one,
/// and the station gives it back by value.
#[derive(Debug)]
#[must_use = "a lease goes back through `Arbiter::release`, or ends by its notice"]
pub struct Lease<'id, K, Path: PoolPath = Root> {
    id: LeaseId<'id, Path>,
    channel: SupportedChannel<K>,
    from: Instant,
    until: Option<Instant>,
    listen_until: Option<Instant>,
    access_point: Option<SupportedChannel<K>>,
}

impl<'id, K, Path: PoolPath> PartialEq for Lease<'id, K, Path> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.channel == other.channel
            && self.from == other.from
            && self.until == other.until
            && self.listen_until == other.listen_until
            && self.access_point == other.access_point
    }
}

impl<'id, K, Path: PoolPath> Eq for Lease<'id, K, Path> {}

impl<'id, K, Path: PoolPath> Lease<'id, K, Path> {
    /// Its name.
    pub const fn id(&self) -> LeaseId<'id, Path> {
        self.id
    }

    /// The channel it leases.
    pub const fn channel(&self) -> SupportedChannel<K> {
        self.channel
    }

    /// When it starts, at the earliest.
    pub const fn from(&self) -> Instant {
        self.from
    }

    /// The channel of the access point beside the station when the lease
    /// was granted, if one runs: a join there needs no move of it.
    pub const fn access_point(&self) -> Option<SupportedChannel<K>> {
        self.access_point
    }

    /// When it ends: the port is back on the access point's channel then;
    /// `None` until it is given back.
    pub const fn until(&self) -> Option<Instant> {
        self.until
    }

    /// Until when the station may listen on the lease's channel: its end,
    /// less the return's retune.
    pub const fn listen_until(&self) -> Option<Instant> {
        self.listen_until
    }
}

/// What the station wants a channel for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Purpose {
    /// Listen for access points for `dwell`.
    Scan {
        /// How long to listen.
        dwell: Duration,
    },
    /// Join an access point on the port's channel, and stay.
    Connect,
    /// Look for the lost upstream for `dwell`.
    Search {
        /// How long to listen.
        dwell: Duration,
    },
}

/// A station's request for a channel.
#[derive(Debug)]
pub struct ChannelRequest<K> {
    /// The channel.
    pub channel: SupportedChannel<K>,
    /// What for.
    pub purpose: Purpose,
}

/// The arbiter's answer to a connected station's absence.
#[derive(Debug)]
#[must_use = "deliver the lease or retain the returned absence place"]
pub enum AbsenceAnswer<'id, K, Path: PoolPath = Root> {
    /// The absence, leased: it starts a preparation window later, once the
    /// station said [`Arbiter::ready`], and returns to the primary lease's
    /// channel.
    Lease(Lease<'id, K, Path>),
    /// No absence now; the place comes back.
    Refused(AbsenceRefusal, AbsencePlace<'id, Path>),
}

/// Why the arbiter refused an absence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbsenceRefusal {
    /// This installation used every lease identity; its places still return.
    IdentityExhausted,
    /// No future beacon/absence slot fits the clock's representable range.
    ScheduleExhausted,
    /// The primary lease named is not the running one.
    NotConnected,
    /// The primary lease ends before this absence can start.
    PrimaryEnds { at: Instant },
}

/// The arbiter's answer to a request.
#[derive(Debug)]
#[must_use = "deliver the lease or retain the returned admission place"]
pub enum Answer<'id, K, Path: PoolPath = Root> {
    /// The channel, leased; the arbiter keeps the place until the lease
    /// ends. A join away from the running access point's channel, which
    /// the access point follows, starts once the access point's announced
    /// move took the port there: its `from` is the expected switch. The
    /// move is never withdrawn, whether the station joins or not.
    Lease(Lease<'id, K, Path>),
    /// The station may not have the channel; the place comes back.
    Refused(Refusal, LeasePlace<'id, Path>),
}

/// Why the arbiter refused a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// This installation used every lease identity; its place still returns.
    IdentityExhausted,
    /// No future beacon/absence slot fits the clock's representable range.
    ScheduleExhausted,
    /// The station joins where the access point beside it may not follow,
    /// and the policy keeps the access point.
    Unservable,
    /// A join away from the running access point's channel too soon after
    /// its last move to where the station joins: the station may ask again
    /// at `retry_at` ([`ApFollowPolicy`]'s interval, doubled after each
    /// failed join that followed a move).
    TooSoon {
        /// When the access point may follow again.
        retry_at: Instant,
    },
}

/// Why the station must leave its network.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaveReason {
    /// The application stopped the service normally.
    Stopped,
    /// The upstream moved where the access point may not follow, and the
    /// policy keeps the access point.
    Unservable,
    /// The port could not return to the access point's channel, or it is
    /// poisoned: the station and the access point stop.
    PortFailed,
}

/// Why a lease ended without its own end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelReason {
    /// The application stopped the service normally.
    Stopped,
    /// The station was not ready at the lease's start.
    NotReady,
    /// The station gave it back before its start.
    Released,
    /// The upstream announced a switch.
    Switch,
    /// The upstream was lost.
    UpstreamLost,
    /// The access point stopped; the port returned to its channel.
    AccessPointStopped,
    /// The port refused the move onto the lease's channel; it returns to
    /// the access point's.
    Refused,
    /// The gate closed too late for the lease.
    Late,
    /// The access point did not complete its announced move within one
    /// beacon interval of the promised switch instant.
    SwitchTimeout,
    /// The port failed.
    PortFailed,
}

/// What the arbiter tells the station.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "deliver the notice and return any admission place to the station"]
pub enum StationNotice<'id, K, Path: PoolPath = Root> {
    /// The lease started: the port runs on its channel.
    LeaseStarted(LeaseId<'id, Path>),
    /// The lease ended: the port left its channel; the place comes back.
    LeaseEnded(LeasePlace<'id, Path>),
    /// The lease ended without its own end; the place comes back.
    LeaseCancelled(CancelReason, LeasePlace<'id, Path>),
    /// The absence started: the port runs on its channel.
    AbsenceStarted(LeaseId<'id, Path>),
    /// The absence ended: the port is back on the primary lease's channel;
    /// the place comes back.
    AbsenceEnded(AbsencePlace<'id, Path>),
    /// The absence ended without its own end; the place comes back.
    AbsenceCancelled(CancelReason, AbsencePlace<'id, Path>),
    /// The port moved without the station's request. A station that holds
    /// its primary lease exchanges it for one on the new channel
    /// ([`Arbiter::exchange`]).
    Moved(SupportedChannel<K>),
    /// The station must leave its network.
    Leave(LeaveReason),
}

/// What the arbiter tells the access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointCommand<K> {
    /// Announce a switch to `target` in the next `count` beacons.
    AnnounceSwitch {
        /// The channel.
        target: SupportedChannel<K>,
        /// What its peers do until the switch.
        mode: ChannelSwitchMode,
        /// The beacons left.
        count: u8,
    },
    /// Start on the channel.
    Start(SupportedChannel<K>),
    /// Stop: release the peers and close the BSS.
    Stop,
}

/// The access point's facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointFact<K> {
    /// It runs on `channel`, its next TBTT at `next_tbtt`, one every
    /// `beacon_interval`; repeated at each TBTT.
    Running {
        /// Its channel.
        channel: SupportedChannel<K>,
        /// Its next TBTT.
        next_tbtt: Instant,
        /// Its beacon interval.
        beacon_interval: BeaconInterval,
    },
    /// It stopped.
    Stopped,
    /// Its announced switch to the channel is due.
    SwitchDue(SupportedChannel<K>),
}

/// What the station reports of its upstream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpstreamFact<K> {
    /// It joined its upstream on the channel.
    Connected(SupportedChannel<K>),
    /// Its upstream announced a switch to `target` at `at`.
    SwitchAnnounced {
        /// The channel.
        target: SupportedChannel<K>,
        /// What the station does until the switch.
        mode: ChannelSwitchMode,
        /// When the upstream switches.
        at: Instant,
    },
    /// It lost its upstream without an announcement.
    Lost,
    /// It heard its lost upstream on the channel.
    Found(SupportedChannel<K>),
    /// Joining the rediscovered upstream failed. Resume the original
    /// search and loss deadline; the join did not start a new loss period.
    JoinFailed,
    /// Its scan, alone on the port, found no upstream.
    NotFound,
}

/// Whether an arbiter reserves the access point's air for an absence: its
/// type parameter, so only a phase that sends the reservation closes its
/// gates ([`CloseGateWith`]).
pub trait ReservationMode {
    /// Maximum absence representable by this mode's air reservation, if any.
    const MAX_ABSENCE: Option<Duration>;
    /// What a gate's closing carries: the end of the reservation, or
    /// nothing.
    type Reserve: Copy + Debug + Eq;

    /// What a gate's closing carries for an absence that ends at `until`,
    /// `length` long.
    fn reserve(until: Instant, length: Duration) -> Self::Reserve;

    /// What a gate's closing that leaves for no absence carries.
    fn none() -> Self::Reserve;
}

/// An arbiter that reserves no air.
pub enum NoReservation {}

impl ReservationMode for NoReservation {
    const MAX_ABSENCE: Option<Duration> = None;
    type Reserve = ();

    fn reserve(_: Instant, _: Duration) {}

    fn none() {}
}

/// An arbiter that reserves the access point's air for each absence no
/// longer than [`MAX_RESERVATION`].
pub enum AirReservation {}

impl ReservationMode for AirReservation {
    const MAX_ABSENCE: Option<Duration> = Some(MAX_RESERVATION);
    type Reserve = Option<Instant>;

    fn reserve(until: Instant, length: Duration) -> Option<Instant> {
        (length <= MAX_RESERVATION).then_some(until)
    }

    fn none() -> Option<Instant> {
        None
    }
}

/// A phase that closes the gates of an arbiter of the reservation mode
/// `R`: every phase for [`NoReservation`], only one that sends the
/// reservation for [`AirReservation`].
pub trait CloseGateWith<'p, 'id: 'p, R: ReservationMode>: LowerMacPhase<'p, 'id> {
    /// Close the gate as the arbiter's action asks; `radio` turns the
    /// reservation's end into the port's radio time.
    fn close_gate_with(
        &mut self,
        gate: GateOpen<'id, <Self::Types as PhaseTypes<'id>>::GatePath>,
        reserve: R::Reserve,
        radio: impl FnOnce(Instant) -> Ieee80211Instant,
    );
}

impl<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>> CloseGateWith<'p, 'id, NoReservation> for P {
    fn close_gate_with(
        &mut self,
        gate: GateOpen<'id, <Self::Types as PhaseTypes<'id>>::GatePath>,
        (): (),
        _: impl FnOnce(Instant) -> Ieee80211Instant,
    ) {
        self.close_gate(gate);
    }
}

impl<'p, 'id: 'p, P: AirReservationPhase<'p, 'id>> CloseGateWith<'p, 'id, AirReservation> for P {
    fn close_gate_with(
        &mut self,
        gate: GateOpen<'id, <Self::Types as PhaseTypes<'id>>::GatePath>,
        reserve: Option<Instant>,
        radio: impl FnOnce(Instant) -> Ieee80211Instant,
    ) {
        match reserve {
            Some(until) => self.close_gate_reserving(gate, radio(until)),
            None => self.close_gate(gate),
        }
    }
}

/// Proof of the one retune the arbiter requested.
/// Discarding a produced token is diagnosed by `unused_must_use`:
/// ```compile_fail
/// #![deny(unused_must_use)]
/// use oer_ieee80211_arbiter::PendingRetune;
/// fn ignore<'id, K, Path: oer_radio_port::PoolPath>(next: impl FnOnce() -> PendingRetune<'id, K, Path>) {
///     next();
/// }
/// ```
/// Its consumer retains it across the requested operation:
/// ```
/// #![deny(unused_must_use)]
/// use oer_ieee80211_arbiter::PendingRetune;
/// fn forward<'id, K, Path: oer_radio_port::PoolPath>(next: impl FnOnce() -> PendingRetune<'id, K, Path>) -> PendingRetune<'id, K, Path> {
///     next()
/// }
/// ```
///
/// Only [`Arbiter::poll_action`] creates this token. The service keeps it
/// through disable, tuning and enable, then consumes it in either
/// [`Arbiter::retuned`] or [`Arbiter::retune_refused`]. The destination and
/// cycle purpose remain attached to the answer; no independent channel or
/// unsolicited answer can be supplied.
///
/// ```compile_fail
/// use oer_ieee80211_arbiter::PendingRetune;
/// let pending = PendingRetune { to: (), purpose: () };
/// ```
#[derive(Debug)]
#[must_use = "finish or refuse the retune with this token"]
pub struct PendingRetune<'id, K, Path: PoolPath = Root> {
    brand: Brand<'id, Path>,
    to: SupportedChannel<K>,
    purpose: CyclePurpose<K>,
}

impl<'id, K, Path: PoolPath> PendingRetune<'id, K, Path> {
    /// The checked channel this action asks the service to tune.
    pub const fn channel(&self) -> SupportedChannel<K> {
        self.to
    }
}

/// One action of the arbiter.
#[derive(Debug)]
#[must_use = "perform the action and retain any gate or retune token"]
pub enum ArbiterAction<'id, K, R: ReservationMode, Path: PoolPath = Root> {
    /// Tell the station.
    Station(StationNotice<'id, K, Path>),
    /// Tell the access point.
    AccessPoint(AccessPointCommand<K>),
    /// Close the port's transmit gate ([`Arbiter::gate_closed`] follows),
    /// with the reservation of an absence the mode carries.
    CloseGate {
        /// The open gate.
        gate: GateOpen<'id, Path>,
        /// The air reservation's end, of an arbiter that reserves it.
        reserve: R::Reserve,
    },
    /// Move the port to the channel: disable, set it, enable
    /// ([`Arbiter::retuned`] or [`Arbiter::retune_refused`] follows).
    Retune(PendingRetune<'id, K, Path>),
    /// Open the port's transmit gate ([`Arbiter::gate_opened`] takes the
    /// open gate back).
    OpenGate(GateClosed<'id, Path>),
}

/// The arbiter's policy.
#[derive(Clone, Copy, Debug)]
pub struct ArbiterPolicy {
    /// The window a station that announces its absence has to tell its
    /// access point.
    pub preparation: Duration,
    /// How long after the access point's TBTT an absence starts.
    pub guard: Duration,
    /// The longest absence from the access point's channel, its two
    /// retunes included.
    pub absence: Duration,
    /// The longest retune of the port.
    pub retune: Duration,
    /// The shortest listening worth an absence.
    pub min_listen: Duration,
    /// How the access point follows its upstream.
    pub follow: ApFollowPolicy,
    /// How the search for a lost upstream spaces its absences.
    pub search: ApSearchPolicy,
    /// When the pair gives up waiting for a lost upstream.
    pub loss: UpstreamLossPolicy,
}

impl ArbiterPolicy {
    /// Check the complete absence budget against the backend's publication
    /// bound before moving any gate or arbiter construction authority.
    pub fn checked<R: ReservationMode>(
        self,
        publication_bound: Duration,
    ) -> Result<CheckedArbiterPolicy<R>, ArbiterPolicyError> {
        if self.follow.min_follow_interval > self.follow.max_follow_interval {
            return Err(ArbiterPolicyError::FollowIntervalOrder);
        }
        if R::MAX_ABSENCE.is_some_and(|limit| self.absence > limit) {
            return Err(ArbiterPolicyError::ReservationTooLong);
        }
        let needed = publication_bound
            .as_micros()
            .checked_add(self.retune.as_micros())
            .and_then(|one| one.checked_mul(2))
            .and_then(|moves| moves.checked_add(self.min_listen.as_micros()))
            .ok_or(ArbiterPolicyError::AbsenceBudgetOverflow)?;
        if needed > self.absence.as_micros() {
            return Err(ArbiterPolicyError::AbsenceTooShort {
                needed: Duration::from_micros(needed),
            });
        }
        Ok(CheckedArbiterPolicy {
            policy: self,
            publication_bound,
            mode: PhantomData,
        })
    }

    /// A 10 ms preparation, absences 2 ms after the TBTT, at most 20 ms
    /// with their two retunes of at most 2 ms each, at least 5 ms of
    /// listening, and the coordinator's default policies.
    pub const DEFAULT: Self = Self {
        preparation: Duration::from_millis(10),
        guard: Duration::from_millis(2),
        absence: Duration::from_millis(20),
        retune: Duration::from_millis(2),
        min_listen: Duration::from_millis(5),
        follow: ApFollowPolicy::DEFAULT,
        search: ApSearchPolicy::DEFAULT,
        loss: UpstreamLossPolicy::DEFAULT,
    };
}

/// An arbiter policy checked for one reservation mode and publication bound.
pub struct CheckedArbiterPolicy<R: ReservationMode> {
    policy: ArbiterPolicy,
    publication_bound: Duration,
    mode: PhantomData<R>,
}

/// A policy rejected before constructing any arbiter or lease places.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArbiterPolicyError {
    /// The minimum follow interval exceeds its ceiling.
    FollowIntervalOrder,
    /// The absence exceeds the reservation Duration field.
    ReservationTooLong,
    /// Two publication/retune bounds and minimum listening do not fit time.
    AbsenceBudgetOverflow,
    /// The configured absence cannot contain both moves and minimum listening.
    AbsenceTooShort { needed: Duration },
}

/// `a + b`, at most the longest duration.
const fn sum(a: Duration, b: Duration) -> Duration {
    Duration::from_micros(a.as_micros().saturating_add(b.as_micros()))
}

/// The port's transmit gate and who holds its token.
enum Gate<'id, Path: PoolPath> {
    /// Open: the arbiter holds the open gate.
    Open(GateOpen<'id, Path>),
    /// Closing: the port holds the gate.
    Closing,
    /// Closed: the arbiter holds the closed gate.
    Closed(GateClosed<'id, Path>),
    /// Opening: the port gives the open gate back.
    Opening,
}

/// The port's channel.
enum Port<K> {
    /// On the channel.
    On(SupportedChannel<K>),
    /// Moving to `to`.
    Retuning,
}

/// How a lease ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaseEnd {
    /// At its `until`, or never.
    Own,
    /// As soon as it can, with its own end.
    Released,
    /// As soon as it can, cancelled for the reason.
    Cancelled(CancelReason),
}

/// Which of the station's leases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Which {
    Primary,
    Absence,
}

/// A lease the arbiter granted, by its steps.
struct Granted<'id, K, Path: PoolPath> {
    id: LeaseId<'id, Path>,
    channel: SupportedChannel<K>,
    from: Instant,
    until: Option<Instant>,
    /// The port returns to the home channel at its end.
    returns: bool,
    end: LeaseEnd,
    step: LeaseStep,
}

/// Only an absence terminal may carry a deferred leave command.
struct AbsenceTerminal<'id, Path: PoolPath> {
    place: AbsencePlace<'id, Path>,
    then_leave: Option<LeaveReason>,
}

impl<'id, K, Path: PoolPath> Granted<'id, K, Path> {
    /// Leave enough time to close the gate and restore the return channel.
    fn listen_end(&self, retune: Duration) -> Option<Instant> {
        self.until.map(|until| {
            if self.returns {
                until.checked_sub(retune).unwrap_or(until)
            } else {
                until
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaseStep {
    /// Before its start.
    Waiting { ready: bool },
    /// A join's lease that starts with the access point's announced move
    /// onto its channel.
    AwaitingMove { deadline: Instant },
    /// The outgoing gate is closing; the late admission check is still due.
    Closing,
    /// The outgoing retune was admitted. Its time is no longer charged again.
    Moving,
    /// Running on its channel.
    Running,
    /// The gate cycles back.
    Ending,
}

/// The station and the access point that share the port: whether the
/// access point runs, whether the port moves without a lease, whether the
/// upstream is lost.
enum Pair<K> {
    /// The station alone on the port.
    Alone,
    /// The access point runs on its schedule.
    Serving { schedule: ApSchedule<K> },
    /// The port moves to `target` at `at`, for both interfaces; the access
    /// point's schedule while it runs.
    Moving {
        target: SupportedChannel<K>,
        at: Instant,
        access_point: Option<ApSchedule<K>>,
    },
    /// The upstream is lost since `since` while the access point runs; the
    /// search's last absence started at `last_search`.
    UpstreamLost {
        since: Instant,
        last_search: Option<Instant>,
        schedule: ApSchedule<K>,
    },
}

/// One cycle of the gate: closing, the port's move when it must, opening,
/// then what the cycle was for.
struct Cycle<K, Reserve> {
    to: SupportedChannel<K>,
    reserve: Reserve,
    purpose: CyclePurpose<K>,
}

// The channel, the reservation and the purpose are values: copies of
// them name no token, whatever the capabilities marker.
impl<K> Clone for Port<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Port<K> {}

impl<K> Clone for Pair<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Pair<K> {}

impl<K, Reserve: Copy> Clone for Cycle<K, Reserve> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K, Reserve: Copy> Copy for Cycle<K, Reserve> {}

#[derive(Debug, Eq, PartialEq)]
enum CyclePurpose<K> {
    LeaseStart(Which),
    LeaseEnd(Which),
    /// A move no lease asked for; the station hears of it.
    Move {
        /// A coordinator move received while this cycle owns the gate.
        next: Option<(SupportedChannel<K>, Instant)>,
    },
    /// The return after a refused move or a late lease; the station heard
    /// of its lease's end already.
    Return {
        to: SupportedChannel<K>,
    },
}

impl<K> Copy for CyclePurpose<K> {}

impl<K> Clone for CyclePurpose<K> {
    fn clone(&self) -> Self {
        *self
    }
}

/// What the arbiter owes the station and the access point, until its
/// owner takes it: one cell for each kind, whose capacity the tokens prove.
///
/// - A lease's end carries its place, of which there is one: a second end
///   waits for a lease, which waits for the place, which comes with the
///   first end.
/// - A lease's start needs a lease of the place; its end comes after it,
///   and the next start needs the end's place.
/// - The cells are taken whole before the gate's next cycle, and a move or
///   a lease starts only in a cycle: one of each per cycle.
/// - A command to leave keeps the port's failure over any other reason.
/// - The access point's stop withdraws its start and its announcement.
struct Outbox<'id, K, Path: PoolPath> {
    primary_started: Option<LeaseId<'id, Path>>,
    absence_started: Option<LeaseId<'id, Path>>,
    primary_end: Option<(Option<CancelReason>, LeasePlace<'id, Path>)>,
    absence_end: Option<(Option<CancelReason>, AbsencePlace<'id, Path>)>,
    moved: Option<SupportedChannel<K>>,
    leave: Option<LeaveReason>,
    stop: bool,
    start: Option<SupportedChannel<K>>,
    announce: Option<AccessPointCommand<K>>,
}

impl<'id, K, Path: PoolPath> Outbox<'id, K, Path> {
    const fn new() -> Self {
        Self {
            primary_started: None,
            absence_started: None,
            primary_end: None,
            absence_end: None,
            moved: None,
            leave: None,
            stop: false,
            start: None,
            announce: None,
        }
    }

    fn leave(&mut self, reason: LeaveReason) {
        if self.leave != Some(LeaveReason::PortFailed) {
            self.leave = Some(reason);
        }
    }

    fn stop(&mut self) {
        self.stop = true;
        self.start = None;
        self.announce = None;
    }

    /// The next thing owed: the access point's commands, then the
    /// station's notices, starts before ends and absences before the
    /// primary lease.
    fn take(&mut self) -> Option<ArbiterNotice<'id, K, Path>> {
        if core::mem::take(&mut self.stop) {
            return Some(ArbiterNotice::AccessPoint(AccessPointCommand::Stop));
        }
        if let Some(channel) = self.start.take() {
            return Some(ArbiterNotice::AccessPoint(AccessPointCommand::Start(
                channel,
            )));
        }
        if let Some(command) = self.announce.take() {
            return Some(ArbiterNotice::AccessPoint(command));
        }
        let notice = if let Some(id) = self.primary_started.take() {
            StationNotice::LeaseStarted(id)
        } else if let Some(id) = self.absence_started.take() {
            StationNotice::AbsenceStarted(id)
        } else if let Some((reason, place)) = self.absence_end.take() {
            match reason {
                Some(reason) => StationNotice::AbsenceCancelled(reason, place),
                None => StationNotice::AbsenceEnded(place),
            }
        } else if let Some((reason, place)) = self.primary_end.take() {
            match reason {
                Some(reason) => StationNotice::LeaseCancelled(reason, place),
                None => StationNotice::LeaseEnded(place),
            }
        } else if let Some(channel) = self.moved.take() {
            StationNotice::Moved(channel)
        } else {
            StationNotice::Leave(self.leave.take()?)
        };
        Some(ArbiterNotice::Station(notice))
    }
}

/// A notice deliverable without owning an enabled port phase.
#[derive(Debug)]
#[must_use = "deliver the notice and retain any admission place"]
pub enum ArbiterNotice<'id, K, Path: PoolPath = Root> {
    Station(StationNotice<'id, K, Path>),
    AccessPoint(AccessPointCommand<K>),
}

/// Why the working arbiter is being consumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArbiterEnd {
    Stopped,
    PortFailed,
}

/// Resource returns shared by both terminal outcomes; no planning authority.
struct ArbiterReturns<'id, K, Path: PoolPath> {
    gate: Gate<'id, Path>,
    outbox: Outbox<'id, K, Path>,
}

/// Terminal failure: only owed notices and late owners can drain.
/// There is no coordinator, admission or restart authority.
#[must_use = "drain terminal notices and retain returned resources"]
pub struct FailedArbiter<'id, K, Path: PoolPath = Root> {
    returns: ArbiterReturns<'id, K, Path>,
}

impl<'id, K, Path: PoolPath> FailedArbiter<'id, K, Path> {
    pub fn poll_notice(&mut self) -> Option<ArbiterNotice<'id, K, Path>> {
        self.returns.outbox.take()
    }

    pub fn return_closed_gate(&mut self, closed: GateClosed<'id, Path>) {
        self.returns.gate = Gate::Closed(closed);
    }

    pub fn retune_ended(&mut self, pending: PendingRetune<'id, K, Path>) {
        end_retune(pending);
    }
}

/// A normal stop retains policy, its gate and the unspent identity sequence.
/// It contains no old coordinator, lease schedule or protocol transition.
#[must_use = "retain the stopped arbiter bank for restart or retirement"]
pub struct StoppedArbiter<'id, K, R: ReservationMode = NoReservation, Path: PoolPath = Root> {
    returns: ArbiterReturns<'id, K, Path>,
    policy: CheckedArbiterPolicy<R>,
    next_id: Option<u64>,
}

/// A backend did not return the gate operation before declaring itself
/// disabled. The bank stays owned here; no replacement authority is minted.
#[must_use = "retain the incomplete stopped bank"]
pub struct IncompleteArbiter<'id, K, R: ReservationMode, Path: PoolPath> {
    pub bank: StoppedArbiter<'id, K, R, Path>,
    pub places: StationPlaceReturns<'id, Path>,
    pub missing: MissingArbiterReturn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MissingArbiterReturn {
    GateTerminal,
    StationPlaces,
}

/// The restarted coordinator and the complete station admission bank.
#[must_use = "return the admission places to the station"]
pub struct ArbiterRestart<'id, K: Ieee80211Capabilities, R: ReservationMode, Path: PoolPath> {
    arbiter: Arbiter<'id, K, R, Path>,
    places: StationPlaces<'id, Path>,
}

impl<'id, K: Ieee80211Capabilities, R: ReservationMode, Path: PoolPath>
    ArbiterRestart<'id, K, R, Path>
{
    pub fn into_parts(self) -> (Arbiter<'id, K, R, Path>, StationPlaces<'id, Path>) {
        (self.arbiter, self.places)
    }
}

impl<'id, K: Ieee80211Capabilities, R: ReservationMode, Path: PoolPath>
    StoppedArbiter<'id, K, R, Path>
{
    /// Build a fresh coordinator on the channel selected by the disabled
    /// backend. Preserve the gate, any undrained resource returns, policy and
    /// monotonic lease identities from this installation.
    #[expect(
        clippy::result_large_err,
        reason = "a missing gate terminal returns the complete affine bank without allocation"
    )]
    pub fn restart(
        self,
        channel: SupportedChannel<K>,
        places: StationPlaceReturns<'id, Path>,
    ) -> Result<ArbiterRestart<'id, K, R, Path>, IncompleteArbiter<'id, K, R, Path>> {
        if matches!(self.returns.gate, Gate::Closing | Gate::Opening) {
            return Err(IncompleteArbiter {
                bank: self,
                places,
                missing: MissingArbiterReturn::GateTerminal,
            });
        }
        let places = match places {
            StationPlaceReturns {
                lease: Some(lease),
                absence: Some(absence),
            } => StationPlaces { lease, absence },
            places => {
                return Err(IncompleteArbiter {
                    bank: self,
                    places,
                    missing: MissingArbiterReturn::StationPlaces,
                });
            }
        };
        let cycle = matches!(self.returns.gate, Gate::Closed(_)).then(|| Cycle {
            to: channel,
            reserve: R::none(),
            purpose: CyclePurpose::Return { to: channel },
        });
        let CheckedArbiterPolicy {
            policy,
            publication_bound,
            mode: _,
        } = self.policy;
        Ok(ArbiterRestart {
            arbiter: Arbiter {
                policy,
                publication_bound,
                coordinator: ChannelCoordinator::new(policy.follow),
                gate: self.returns.gate,
                port: Port::On(channel),
                pair: Pair::Alone,
                home: channel,
                primary: None,
                absence: None,
                following: false,
                cycle,
                next_id: self.next_id,
                outbox: self.returns.outbox,
            },
            places,
        })
    }
}

/// The terminal bank distinguishes reusable Stop from terminal failure.
#[must_use = "drain notices and retain the returned bank"]
pub enum RetiredArbiter<'id, K, R: ReservationMode = NoReservation, Path: PoolPath = Root> {
    Stopped(StoppedArbiter<'id, K, R, Path>),
    PortFailed(FailedArbiter<'id, K, Path>),
}

impl<'id, K, R: ReservationMode, Path: PoolPath> RetiredArbiter<'id, K, R, Path> {
    fn returns(&mut self) -> &mut ArbiterReturns<'id, K, Path> {
        match self {
            Self::Stopped(stopped) => &mut stopped.returns,
            Self::PortFailed(failed) => &mut failed.returns,
        }
    }

    pub fn poll_notice(&mut self) -> Option<ArbiterNotice<'id, K, Path>> {
        self.returns().outbox.take()
    }

    pub fn return_closed_gate(&mut self, closed: GateClosed<'id, Path>) {
        self.returns().gate = Gate::Closed(closed);
    }

    pub fn retune_ended(&mut self, pending: PendingRetune<'id, K, Path>) {
        end_retune(pending);
    }
}

fn end_retune<K, Path: PoolPath>(pending: PendingRetune<'_, K, Path>) {
    let PendingRetune {
        brand: _,
        to: _,
        purpose: _,
    } = pending;
}

/// The owned continuation of a refused retune. Only an outbound lease move
/// can return home; a failed return or shared move ends the working arbiter.
#[must_use = "complete the return move or drain the terminal arbiter"]
#[expect(
    clippy::large_enum_variant,
    reason = "the owned transition retains the arbiter inline without requiring an allocator"
)]
pub enum RetuneRefused<'id, K: Ieee80211Capabilities, R: ReservationMode, Path: PoolPath = Root> {
    Return {
        arbiter: Arbiter<'id, K, R, Path>,
        pending: PendingRetune<'id, K, Path>,
    },
    Failed(FailedArbiter<'id, K, Path>),
}

/// The arbiter.
pub struct Arbiter<
    'id,
    K: Ieee80211Capabilities,
    R: ReservationMode = NoReservation,
    Path: PoolPath = Root,
> {
    policy: ArbiterPolicy,
    publication_bound: Duration,
    coordinator: ChannelCoordinator<K>,
    gate: Gate<'id, Path>,
    port: Port<K>,
    pair: Pair<K>,
    /// The channel the port returns to: the access point's, the primary
    /// lease's, or the port's alone.
    home: SupportedChannel<K>,
    /// The station's primary lease and its place.
    primary: Option<(Granted<'id, K, Path>, LeasePlace<'id, Path>)>,
    /// The connected station's absence and its place.
    absence: Option<(Granted<'id, K, Path>, AbsenceTerminal<'id, Path>)>,
    /// The primary lease is a join the access point moved for, which has
    /// not connected yet.
    following: bool,
    cycle: Option<Cycle<K, R::Reserve>>,
    next_id: Option<u64>,
    outbox: Outbox<'id, K, Path>,
}

impl<'id, K: Ieee80211Capabilities, R: ReservationMode, Path: PoolPath> Arbiter<'id, K, R, Path> {
    /// The arbiter of a port on `channel`, holding its open gate, and the
    /// station's places for its primary lease and its absence.
    /// The checked policy uses the backend's `PUBLICATION_BOUND`, not its
    /// retry-plan length. Each move reserves this drain plus its retune.
    pub fn new(
        policy: CheckedArbiterPolicy<R>,
        channel: SupportedChannel<K>,
        authority: ArbiterAuthority<'id, Path>,
        gate: GateOpen<'id, Path>,
    ) -> (Self, LeasePlace<'id, Path>, AbsencePlace<'id, Path>) {
        let CheckedArbiterPolicy {
            policy,
            publication_bound,
            mode: _,
        } = policy;
        let ArbiterAuthority { brand: _ } = authority;
        (
            Self {
                policy,
                publication_bound,
                coordinator: ChannelCoordinator::new(policy.follow),
                gate: Gate::Open(gate),
                port: Port::On(channel),
                pair: Pair::Alone,
                home: channel,
                primary: None,
                absence: None,
                following: false,
                cycle: None,
                next_id: Some(0),
                outbox: Outbox::new(),
            },
            LeasePlace {
                private: PhantomData,
            },
            AbsencePlace {
                private: PhantomData,
            },
        )
    }

    /// The channel the port runs on, when it is not moving.
    pub fn channel(&self) -> Option<SupportedChannel<K>> {
        match self.port {
            Port::On(channel) => Some(channel),
            _ => None,
        }
    }

    /// The access point's schedule while it runs.
    fn schedule(&self) -> Option<ApSchedule<K>> {
        match self.pair {
            Pair::Alone => None,
            Pair::Serving { schedule } | Pair::UpstreamLost { schedule, .. } => Some(schedule),
            Pair::Moving { access_point, .. } => access_point,
        }
    }

    /// The pair once no move runs: alone, serving, or searching for the
    /// upstream the coordinator searches for.
    fn settle(&mut self, schedule: Option<ApSchedule<K>>) {
        self.pair = match (schedule, self.coordinator.search_since()) {
            (None, _) => Pair::Alone,
            (Some(schedule), Some(since)) => Pair::UpstreamLost {
                since,
                last_search: None,
                schedule,
            },
            (Some(schedule), None) => Pair::Serving { schedule },
        };
    }

    /// The access point stopped, or is stopping.
    fn access_point_gone(&mut self) {
        match &mut self.pair {
            Pair::Moving { access_point, .. } => *access_point = None,
            pair => *pair = Pair::Alone,
        }
    }

    /// Whether the port's move is due at `now`.
    fn move_due(&self, now: Instant) -> bool {
        matches!(self.pair, Pair::Moving { at, .. } if now >= at)
    }

    fn available_id(&self) -> Option<LeaseId<'id, Path>> {
        self.next_id.map(|id| LeaseId(id, PhantomData))
    }

    /// The absence slot and end for `dwell` from `earliest`: beside an
    /// access point after one of its TBTTs, alone at once; the move there,
    /// the listening and the move back, at most the policy's absence.
    fn absence_window(
        &self,
        earliest: Instant,
        dwell: Option<Duration>,
    ) -> Option<(Instant, Instant)> {
        let retune = sum(self.publication_bound, self.policy.retune);
        let from = match self.schedule() {
            Some(schedule) => slot(schedule, earliest, self.policy.guard)?,
            None => earliest,
        };
        let wanted = dwell.map_or(self.policy.absence, |dwell| sum(sum(dwell, retune), retune));
        let length = if wanted < self.policy.absence {
            wanted
        } else {
            self.policy.absence
        };
        Some((from, from.checked_add(length)?))
    }

    fn lease_value(&self, granted: &Granted<'id, K, Path>) -> Lease<'id, K, Path> {
        let retune = sum(self.publication_bound, self.policy.retune);
        Lease {
            access_point: self.schedule().map(|_| self.home),
            id: granted.id,
            channel: granted.channel,
            from: granted.from,
            until: granted.until,
            listen_until: granted.listen_end(retune),
        }
    }

    /// Answer the station's request for its primary lease at `now`, in
    /// this step; the request takes the station's place.
    pub fn request(
        &mut self,
        now: Instant,
        request: ChannelRequest<K>,
        place: LeasePlace<'id, Path>,
    ) -> Answer<'id, K, Path> {
        let Some(id) = self.available_id() else {
            return Answer::Refused(Refusal::IdentityExhausted, place);
        };
        let away = request.channel != self.home;
        let mut follows = None;
        if away
            && request.purpose == Purpose::Connect
            && let Some(schedule) = self.schedule()
        {
            match self.coordinator.station_joining_away(request.channel, now) {
                JoinAway::Follow { count } => {
                    let Some((at, deadline)) = schedule.switch_at(now, count).and_then(|at| {
                        at.checked_add(schedule.beacon_interval.duration())
                            .map(|deadline| (at, deadline))
                    }) else {
                        return Answer::Refused(Refusal::ScheduleExhausted, place);
                    };
                    self.coordinator.follow_announced(now);
                    self.outbox.announce = Some(AccessPointCommand::AnnounceSwitch {
                        target: request.channel,
                        mode: ChannelSwitchMode::Continue,
                        count,
                    });
                    follows = Some((at, deadline));
                    self.following = true;
                }
                JoinAway::StopAccessPoint => {
                    self.coordinator.access_point_stopped();
                    self.access_point_gone();
                    self.outbox.stop();
                }
                JoinAway::Refused => return Answer::Refused(Refusal::Unservable, place),
                JoinAway::TooSoon { retry_at } => {
                    return Answer::Refused(Refusal::TooSoon { retry_at }, place);
                }
                JoinAway::ScheduleExhausted => {
                    return Answer::Refused(Refusal::ScheduleExhausted, place);
                }
            }
        }
        if let Some((switch, deadline)) = follows {
            let granted = Granted {
                id,
                channel: request.channel,
                from: switch,
                until: None,
                returns: false,
                end: LeaseEnd::Own,
                step: LeaseStep::AwaitingMove { deadline },
            };
            let lease = self.lease_value(&granted);
            self.next_id = id.0.checked_add(1);
            self.primary = Some((granted, place));
            return Answer::Lease(lease);
        }
        let schedule = self.schedule();
        let mut earliest = now;
        let dwell = match request.purpose {
            Purpose::Scan { dwell } => Some(dwell),
            Purpose::Search { dwell } => {
                // A lost upstream's search spaces its absences: one per
                // beacon interval while dense, sparser after.
                if let Pair::UpstreamLost {
                    since,
                    last_search: Some(last),
                    schedule,
                } = self.pair
                {
                    let dense = since.saturating_add(self.policy.search.dense_period);
                    let spacing = if last < dense {
                        schedule.beacon_interval.duration()
                    } else {
                        self.policy.search.sparse_interval
                    };
                    let next = last.saturating_add(spacing);
                    if next > earliest {
                        earliest = next;
                    }
                }
                Some(dwell)
            }
            Purpose::Connect => None,
        };
        let (from, until, returns) = if away && schedule.is_some() {
            // Beside an access point, away from its channel only in an
            // absence after its TBTT.
            let Some((from, until)) = self.absence_window(earliest, dwell) else {
                return Answer::Refused(Refusal::ScheduleExhausted, place);
            };
            (from, Some(until), true)
        } else {
            // Alone, or on the access point's channel, the port stays.
            (
                earliest,
                dwell.map(|dwell| {
                    earliest
                        .saturating_add(sum(self.publication_bound, self.policy.retune))
                        .saturating_add(dwell)
                }),
                false,
            )
        };
        if matches!(request.purpose, Purpose::Search { .. })
            && let Pair::UpstreamLost { last_search, .. } = &mut self.pair
        {
            *last_search = Some(from);
        }
        let granted = Granted {
            id,
            channel: request.channel,
            from,
            until,
            returns,
            end: LeaseEnd::Own,
            step: LeaseStep::Waiting { ready: true },
        };
        let lease = self.lease_value(&granted);
        self.next_id = id.0.checked_add(1);
        self.primary = Some((granted, place));
        Answer::Lease(lease)
    }

    /// Answer the connected station's request for an absence from its
    /// running primary lease `home`, at `now`, in this step.
    pub fn request_absence(
        &mut self,
        now: Instant,
        request: ChannelRequest<K>,
        home: &Lease<'id, K, Path>,
        place: AbsencePlace<'id, Path>,
    ) -> AbsenceAnswer<'id, K, Path> {
        let connected = self.primary.as_ref().is_some_and(|(granted, _)| {
            granted.id == home.id && granted.step == LeaseStep::Running
        });
        if !connected {
            return AbsenceAnswer::Refused(AbsenceRefusal::NotConnected, place);
        }
        let Some(id) = self.available_id() else {
            return AbsenceAnswer::Refused(AbsenceRefusal::IdentityExhausted, place);
        };
        let dwell = match request.purpose {
            Purpose::Scan { dwell } | Purpose::Search { dwell } => Some(dwell),
            Purpose::Connect => None,
        };
        let Some((from, until)) = now
            .checked_add(self.policy.preparation)
            .and_then(|earliest| self.absence_window(earliest, dwell))
        else {
            return AbsenceAnswer::Refused(AbsenceRefusal::ScheduleExhausted, place);
        };
        let until = match self.primary.as_ref().and_then(|(granted, _)| granted.until) {
            Some(end) if from >= end => {
                return AbsenceAnswer::Refused(AbsenceRefusal::PrimaryEnds { at: end }, place);
            }
            Some(end) => until.min(end),
            None => until,
        };
        let granted = Granted {
            id,
            channel: request.channel,
            from,
            until: Some(until),
            returns: true,
            end: LeaseEnd::Own,
            step: LeaseStep::Waiting { ready: false },
        };
        let lease = self.lease_value(&granted);
        self.next_id = id.0.checked_add(1);
        self.absence = Some((
            granted,
            AbsenceTerminal {
                place,
                then_leave: None,
            },
        ));
        AbsenceAnswer::Lease(lease)
    }

    /// Exchange the primary lease `old`, after a [`StationNotice::Moved`],
    /// for the lease of the channel the port moved to, under a new name:
    /// the old lease goes by value, so the station never holds two. A
    /// A lease that is not the moved primary one, or for which no fresh
    /// identity remains, comes back unchanged in `Err`.
    pub fn exchange(
        &mut self,
        old: Lease<'id, K, Path>,
    ) -> Result<Lease<'id, K, Path>, Lease<'id, K, Path>> {
        let Some(id) = self.available_id() else {
            return Err(old);
        };
        let Some((granted, _)) = self.primary.as_mut() else {
            return Err(old);
        };
        if granted.id != old.id || granted.channel == old.channel {
            return Err(old);
        }
        self.next_id = id.0.checked_add(1);
        granted.id = id;
        match &self.primary {
            Some((granted, _)) => Ok(self.lease_value(granted)),
            None => Err(old),
        }
    }

    /// The station is ready for its absence `lease`: it told its access
    /// point it leaves.
    pub fn ready(&mut self, lease: LeaseId<'id, Path>) {
        if let Some((granted, _)) = &mut self.absence
            && granted.id == lease
            && let LeaseStep::Waiting { ready } = &mut granted.step
        {
            *ready = true;
        }
    }

    /// The station gives `lease` back, its primary lease or its absence: it
    /// ends with its cancellation before its start, with its end after,
    /// once the gate's cycle that runs ended. A primary lease ends after
    /// the absence.
    pub fn release(&mut self, lease: Lease<'id, K, Path>) {
        let which = if self
            .primary
            .as_ref()
            .is_some_and(|(granted, _)| granted.id == lease.id)
        {
            Which::Primary
        } else if self
            .absence
            .as_ref()
            .is_some_and(|(granted, _)| granted.id == lease.id)
        {
            Which::Absence
        } else {
            return;
        };
        if which == Which::Primary {
            self.end_lease(Which::Absence, LeaseEnd::Cancelled(CancelReason::Released));
        }
        self.end_lease(which, LeaseEnd::Released);
    }

    fn granted(&mut self, which: Which) -> Option<&mut Granted<'id, K, Path>> {
        match which {
            Which::Primary => self.primary.as_mut().map(|(granted, _)| granted),
            Which::Absence => self.absence.as_mut().map(|(granted, _)| granted),
        }
    }

    /// End the lease: a waiting one is cancelled at once; one that started
    /// ends as `end` once the gate's cycle that runs ended.
    fn end_lease(&mut self, which: Which, end: LeaseEnd) {
        let Some(granted) = self.granted(which) else {
            return;
        };
        match granted.step {
            LeaseStep::Waiting { .. } | LeaseStep::AwaitingMove { .. } => {
                let reason = match end {
                    LeaseEnd::Cancelled(reason) => reason,
                    LeaseEnd::Own | LeaseEnd::Released => CancelReason::Released,
                };
                self.cancel(which, reason);
            }
            LeaseStep::Closing | LeaseStep::Moving | LeaseStep::Running => granted.end = end,
            LeaseStep::Ending => {}
        }
    }

    /// A primary lease ended: a join the access point moved for and that
    /// never connected failed, which doubles the follow interval.
    fn follow_ended(&mut self) {
        if core::mem::take(&mut self.following) {
            self.coordinator.follow_failed();
        }
    }

    /// Tell the station the lease ended without its own end.
    fn cancel(&mut self, which: Which, reason: CancelReason) {
        match which {
            Which::Primary => {
                if let Some((_, place)) = self.primary.take() {
                    self.follow_ended();
                    self.outbox.primary_end = Some((Some(reason), place));
                }
            }
            Which::Absence => {
                self.finish_absence(Some(reason));
            }
        }
    }

    /// Tell the station the lease ended as it was to end.
    fn ended(&mut self, which: Which) {
        let end = match which {
            Which::Primary => self.primary.as_ref().map(|(granted, _)| granted.end),
            Which::Absence => self.absence.as_ref().map(|(granted, _)| granted.end),
        };
        match end {
            Some(LeaseEnd::Cancelled(reason)) => self.cancel(which, reason),
            Some(LeaseEnd::Own | LeaseEnd::Released) => match which {
                Which::Primary => {
                    if let Some((_, place)) = self.primary.take() {
                        self.follow_ended();
                        self.outbox.primary_end = Some((None, place));
                    }
                }
                Which::Absence => {
                    self.finish_absence(None);
                }
            },
            None => {}
        }
    }

    /// Every absence terminal returns its place and releases its pending leave.
    fn finish_absence(&mut self, reason: Option<CancelReason>) {
        if let Some((_, terminal)) = self.absence.take() {
            self.outbox.absence_end = Some((reason, terminal.place));
            if let Some(leave) = terminal.then_leave {
                self.outbox.leave(leave);
            }
        }
    }

    /// The access point's fact at `now`.
    pub fn access_point(&mut self, now: Instant, fact: AccessPointFact<K>) {
        match fact {
            AccessPointFact::Running {
                channel,
                next_tbtt,
                beacon_interval,
            } => {
                let schedule = ApSchedule {
                    channel,
                    next_tbtt,
                    beacon_interval,
                };
                match &mut self.pair {
                    Pair::Serving { schedule: running }
                    | Pair::UpstreamLost {
                        schedule: running, ..
                    } => *running = schedule,
                    Pair::Moving { access_point, .. } => *access_point = Some(schedule),
                    Pair::Alone => {}
                }
                let moving = matches!(
                    self.cycle,
                    Some(Cycle {
                        purpose: CyclePurpose::Move { .. },
                        ..
                    })
                );
                if !moving {
                    self.home = channel;
                }
                self.coordinator.access_point_running(schedule);
                if !moving && matches!(self.pair, Pair::Alone) {
                    self.settle(Some(schedule));
                }
            }
            AccessPointFact::Stopped => {
                self.access_point_gone();
                self.coordinator.access_point_stopped();
                // A join that waited for the access point's move goes
                // alone.
                if let Some((granted, _)) = &mut self.primary
                    && matches!(granted.step, LeaseStep::AwaitingMove { .. })
                {
                    granted.step = LeaseStep::Waiting { ready: true };
                }
                // A lease away from its channel is cancelled and returns.
                let reason = LeaseEnd::Cancelled(CancelReason::AccessPointStopped);
                self.end_lease(Which::Absence, reason);
                if self
                    .primary
                    .as_ref()
                    .is_some_and(|(granted, _)| granted.returns)
                {
                    self.end_lease(Which::Primary, reason);
                }
            }
            AccessPointFact::SwitchDue(target) => {
                let actions = self.coordinator.access_point_switch_due(target, now);
                self.apply(actions);
            }
        }
    }

    /// The station's fact about its upstream at `now`.
    pub fn upstream(&mut self, now: Instant, fact: UpstreamFact<K>) {
        match fact {
            UpstreamFact::Connected(channel) => {
                self.following = false;
                if let Pair::UpstreamLost { schedule, .. } = self.pair {
                    self.pair = Pair::Serving { schedule };
                }
                let actions = self.coordinator.station_connected(channel);
                self.apply(actions);
            }
            UpstreamFact::SwitchAnnounced { target, mode, at } => {
                self.end_lease(Which::Absence, LeaseEnd::Cancelled(CancelReason::Switch));
                if self
                    .primary
                    .as_ref()
                    .is_some_and(|(granted, _)| matches!(granted.step, LeaseStep::Waiting { .. }))
                {
                    self.cancel(Which::Primary, CancelReason::Switch);
                }
                let actions = self.coordinator.upstream_switch_announced(target, mode, at);
                self.apply(actions);
            }
            UpstreamFact::Lost => {
                let reason = LeaseEnd::Cancelled(CancelReason::UpstreamLost);
                self.end_lease(Which::Absence, reason);
                self.coordinator.upstream_lost(now);
                if !matches!(self.pair, Pair::Moving { .. }) {
                    self.settle(self.schedule());
                }
            }
            UpstreamFact::Found(channel) => {
                let actions = self.coordinator.upstream_found(channel);
                self.apply(actions);
            }
            UpstreamFact::JoinFailed => {
                self.coordinator.join_failed();
                if !matches!(self.pair, Pair::Moving { .. }) {
                    self.settle(self.schedule());
                }
            }
            UpstreamFact::NotFound => {
                let actions = self.coordinator.upstream_rescan_failed(now);
                self.apply(actions);
            }
        }
    }

    /// The composition wants the access point to run.
    pub fn want_access_point(&mut self) {
        let actions = self.coordinator.want_access_point();
        self.apply(actions);
    }

    /// The station must leave: its absence ends first, then it hears it.
    fn command_leave(&mut self, reason: LeaveReason) {
        if let Some((_, terminal)) = &mut self.absence {
            terminal.then_leave = Some(reason);
            self.end_lease(Which::Absence, LeaseEnd::Cancelled(CancelReason::Switch));
        } else {
            self.outbox.leave(reason);
        }
    }

    /// Carry the coordinator's actions out.
    fn apply(&mut self, actions: CoordinatorActions<K>) {
        for action in actions {
            match action {
                CoordinatorAction::AnnounceSwitch {
                    target,
                    mode,
                    count,
                } => {
                    self.outbox.announce = Some(AccessPointCommand::AnnounceSwitch {
                        target,
                        mode,
                        count,
                    });
                }
                CoordinatorAction::Retune { channel, at } => {
                    if let Some(Cycle {
                        purpose: CyclePurpose::Move { next, .. },
                        ..
                    }) = &mut self.cycle
                    {
                        *next = Some((channel, at));
                    } else {
                        self.pair = Pair::Moving {
                            target: channel,
                            at,
                            access_point: self.schedule(),
                        };
                    }
                }
                CoordinatorAction::StopAccessPoint => {
                    self.access_point_gone();
                    self.outbox.stop();
                }
                CoordinatorAction::StartAccessPoint { channel } => {
                    self.outbox.start = Some(channel);
                }
                CoordinatorAction::LeaveUpstream => self.command_leave(LeaveReason::Unservable),
            }
        }
    }

    /// The port's gate closed.
    pub fn gate_closed(&mut self, closed: GateClosed<'id, Path>) {
        self.gate = Gate::Closed(closed);
    }

    /// The port's gate opened, which gives the open gate back and ends the
    /// gate's cycle.
    pub fn gate_opened(&mut self, open: GateOpen<'id, Path>) {
        self.gate = Gate::Open(open);
        if let Some(cycle) = self.cycle.take() {
            self.finish_cycle(cycle);
        }
    }

    /// What a cycle was for, once the gate opened again.
    fn finish_cycle(&mut self, cycle: Cycle<K, R::Reserve>) {
        // The return/move is complete and the gate is open. Only this
        // boundary can release an AP restart retained by the coordinator.
        let actions = self.coordinator.port_moved(cycle.to);
        self.apply(actions);
        match cycle.purpose {
            CyclePurpose::LeaseStart(which) => {
                if which == Which::Primary
                    && self.schedule().is_none()
                    && let Some((granted, _)) = &self.primary
                    && granted.until.is_none()
                {
                    // A connected station alone: its channel is home.
                    self.home = granted.channel;
                }
                if let Some(granted) = self.granted(which) {
                    granted.step = LeaseStep::Running;
                    let id = Some(granted.id);
                    match which {
                        Which::Primary => self.outbox.primary_started = id,
                        Which::Absence => self.outbox.absence_started = id,
                    }
                }
            }
            CyclePurpose::LeaseEnd(which) => {
                self.ended(which);
            }
            CyclePurpose::Move { next } => {
                let access_point = self.schedule().map(|schedule| ApSchedule {
                    channel: cycle.to,
                    ..schedule
                });
                match next {
                    Some((target, at)) => {
                        self.pair = Pair::Moving {
                            target,
                            at,
                            access_point,
                        }
                    }
                    None => self.settle(access_point),
                }
                match self.primary.as_mut() {
                    // A join's lease starts with the move onto its channel.
                    Some((granted, _))
                        if matches!(granted.step, LeaseStep::AwaitingMove { .. })
                            && granted.channel == cycle.to =>
                    {
                        granted.step = LeaseStep::Running;
                        self.outbox.primary_started = Some(granted.id);
                    }
                    // The primary lease moves with the port; the station
                    // exchanges its value for the new one.
                    Some((granted, _)) if granted.step == LeaseStep::Running => {
                        granted.channel = cycle.to;
                        self.outbox.moved = Some(cycle.to);
                    }
                    _ => self.outbox.moved = Some(cycle.to),
                }
            }
            CyclePurpose::Return { to } => self.home = to,
        }
    }

    /// The requested retune completed and the port runs on its channel.
    pub fn retuned(&mut self, pending: PendingRetune<'id, K, Path>) {
        self.port = Port::On(pending.to);
    }

    /// The requested retune was refused: a move onto a lease's channel
    /// cancels that lease and returns home; a refused return or shared move
    /// stops the station and the access point.
    pub fn retune_refused(
        mut self,
        pending: PendingRetune<'id, K, Path>,
    ) -> RetuneRefused<'id, K, R, Path> {
        match pending.purpose {
            CyclePurpose::LeaseStart(which) => {
                self.cancel(which, CancelReason::Refused);
                self.port = Port::Retuning;
                self.cycle = Some(Cycle {
                    to: self.home,
                    reserve: R::none(),
                    purpose: CyclePurpose::Return { to: self.home },
                });
                let pending = PendingRetune {
                    brand: PhantomData,
                    to: self.home,
                    purpose: CyclePurpose::Return { to: self.home },
                };
                RetuneRefused::Return {
                    arbiter: self,
                    pending,
                }
            }
            CyclePurpose::LeaseEnd(_) | CyclePurpose::Move { .. } | CyclePurpose::Return { .. } => {
                RetuneRefused::Failed(self.finish_failed())
            }
        }
    }

    /// Consume all planning authority and return a terminal resource bank.
    /// Stop and failure preserve distinct causes for every accepted lease.
    pub fn finish(mut self, end: ArbiterEnd) -> RetiredArbiter<'id, K, R, Path> {
        self.terminate(end);
        let returns = ArbiterReturns {
            gate: self.gate,
            outbox: self.outbox,
        };
        match end {
            ArbiterEnd::Stopped => RetiredArbiter::Stopped(StoppedArbiter {
                returns,
                policy: CheckedArbiterPolicy {
                    policy: self.policy,
                    publication_bound: self.publication_bound,
                    mode: PhantomData,
                },
                next_id: self.next_id,
            }),
            ArbiterEnd::PortFailed => RetiredArbiter::PortFailed(FailedArbiter { returns }),
        }
    }

    fn terminate(&mut self, end: ArbiterEnd) {
        let (cancel, leave) = match end {
            ArbiterEnd::Stopped => (CancelReason::Stopped, LeaveReason::Stopped),
            ArbiterEnd::PortFailed => (CancelReason::PortFailed, LeaveReason::PortFailed),
        };
        self.cancel(Which::Absence, cancel);
        self.cancel(Which::Primary, cancel);
        self.outbox.leave(leave);
        self.outbox.stop();
    }

    fn finish_failed(mut self) -> FailedArbiter<'id, K, Path> {
        self.terminate(ArbiterEnd::PortFailed);
        FailedArbiter {
            returns: ArbiterReturns {
                gate: self.gate,
                outbox: self.outbox,
            },
        }
    }

    /// The cycle that starts or ends the lease at `now`, if it is due.
    fn lease_cycle(&mut self, which: Which, now: Instant) -> Option<Cycle<K, R::Reserve>> {
        let home = self.home;
        let retune = sum(self.publication_bound, self.policy.retune);
        let pending_due = self.move_due(now);
        let granted = self.granted(which)?;
        match granted.step {
            LeaseStep::AwaitingMove { deadline } if now >= deadline && !pending_due => {
                self.cancel(which, CancelReason::SwitchTimeout);
                None
            }
            LeaseStep::Waiting { ready } if now >= granted.from => {
                if !ready {
                    self.cancel(which, CancelReason::NotReady);
                    return None;
                }
                granted.step = LeaseStep::Closing;
                let reserve = match (granted.returns, granted.until) {
                    (true, Some(until)) => match until.checked_duration_since(now) {
                        Some(length) => R::reserve(until, length),
                        None => R::none(),
                    },
                    _ => R::none(),
                };
                Some(Cycle {
                    to: granted.channel,
                    reserve,
                    purpose: CyclePurpose::LeaseStart(which),
                })
            }
            LeaseStep::Running => {
                let ends = granted.listen_end(retune);
                // An absence ends before the port moves with the upstream.
                let moving = which == Which::Absence && pending_due;
                if granted.end != LeaseEnd::Own || moving || ends.is_some_and(|ends| now >= ends) {
                    if moving && granted.end == LeaseEnd::Own {
                        granted.end = LeaseEnd::Cancelled(CancelReason::Switch);
                    }
                    granted.step = LeaseStep::Ending;
                    Some(Cycle {
                        to: if granted.returns {
                            home
                        } else {
                            granted.channel
                        },
                        reserve: R::none(),
                        purpose: CyclePurpose::LeaseEnd(which),
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Start the next gate's cycle due at `now`, if none runs.
    fn plan(&mut self, now: Instant) {
        if self.cycle.is_some() {
            return;
        }
        // A lost upstream past the loss policy: the access point stops and
        // the station scans alone.
        if let Some(due) = self.coordinator.upstream_loss_due(self.policy.loss)
            && now >= due
        {
            let actions = self.coordinator.upstream_rescan();
            self.apply(actions);
        }
        // The absence first: it ends before the primary lease does and
        // before the port moves.
        if self.move_due(now)
            && self
                .absence
                .as_ref()
                .is_some_and(|(granted, _)| matches!(granted.step, LeaseStep::Waiting { .. }))
        {
            self.cancel(Which::Absence, CancelReason::Switch);
        }
        if let Some(cycle) = self.lease_cycle(Which::Absence, now) {
            self.cycle = Some(cycle);
            return;
        }
        if self.absence.is_none()
            && let Some(cycle) = self.lease_cycle(Which::Primary, now)
        {
            self.cycle = Some(cycle);
            return;
        }
        if self.absence.is_none()
            && self.move_due(now)
            && let Pair::Moving {
                target,
                access_point,
                ..
            } = self.pair
        {
            self.home = target;
            // The cycle owns the scheduled move from here on. Its next
            // input is a gate/retune terminal, never the consumed deadline.
            self.settle(access_point);
            self.cycle = Some(Cycle {
                to: target,
                reserve: R::none(),
                purpose: CyclePurpose::Move { next: None },
            });
        }
    }

    /// The next action at `now`, if any. The service takes every action
    /// before it waits again.
    #[must_use = "perform the action before waiting"]
    pub fn poll_action(&mut self, now: Instant) -> Option<ArbiterAction<'id, K, R, Path>> {
        if let Some(notice) = self.poll_notice(now) {
            return Some(match notice {
                ArbiterNotice::Station(notice) => ArbiterAction::Station(notice),
                ArbiterNotice::AccessPoint(command) => ArbiterAction::AccessPoint(command),
            });
        }
        let cycle = self.cycle?;
        match core::mem::replace(&mut self.gate, Gate::Closing) {
            Gate::Open(open) => Some(ArbiterAction::CloseGate {
                gate: open,
                reserve: cycle.reserve,
            }),
            Gate::Closing => None,
            Gate::Opening => {
                self.gate = Gate::Opening;
                None
            }
            Gate::Closed(closed) => {
                let cancellation = if let CyclePurpose::LeaseStart(which) = cycle.purpose {
                    match self
                        .granted(which)
                        .map(|granted| (granted.step, granted.end))
                    {
                        Some((LeaseStep::Closing, LeaseEnd::Cancelled(reason))) => {
                            Some((which, reason))
                        }
                        Some((LeaseStep::Closing, LeaseEnd::Released)) => {
                            Some((which, CancelReason::Released))
                        }
                        _ if self.too_late(which, now) => Some((which, CancelReason::Late)),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some((which, reason)) = cancellation {
                    // A cancellation while Closing precedes the timing check;
                    // preserve its cause and never start the cancelled move.
                    self.cancel(which, reason);
                    self.cycle = Some(Cycle {
                        to: self.home,
                        purpose: CyclePurpose::Return { to: self.home },
                        ..cycle
                    });
                    self.gate = Gate::Closed(closed);
                    return self.poll_action(now);
                }
                match self.port {
                    Port::On(channel) if channel == cycle.to => {
                        self.gate = Gate::Opening;
                        Some(ArbiterAction::OpenGate(closed))
                    }
                    Port::On(_) => {
                        if let CyclePurpose::LeaseStart(which) = cycle.purpose
                            && let Some(granted) = self.granted(which)
                        {
                            granted.step = LeaseStep::Moving;
                        }
                        self.gate = Gate::Closed(closed);
                        self.port = Port::Retuning;
                        Some(ArbiterAction::Retune(PendingRetune {
                            brand: PhantomData,
                            to: cycle.to,
                            purpose: cycle.purpose,
                        }))
                    }
                    Port::Retuning => {
                        self.gate = Gate::Closed(closed);
                        None
                    }
                }
            }
        }
    }

    /// Notices available without a running port phase. Planning may update
    /// deadlines, but this API never takes a gate or issues a retune token.
    /// Access-point notices precede station notices, as in `poll_action`.
    #[must_use = "deliver the notice before waiting"]
    pub fn poll_notice(&mut self, now: Instant) -> Option<ArbiterNotice<'id, K, Path>> {
        if let Some(notice) = self.outbox.take() {
            return Some(notice);
        }
        self.plan(now);
        self.outbox.take()
    }

    /// Whether the starting lease no longer fits two retunes and the
    /// shortest listening before its end.
    fn too_late(&mut self, which: Which, now: Instant) -> bool {
        let policy = self.policy;
        let publication_bound = self.publication_bound;
        let Some(granted) = self.granted(which) else {
            return false;
        };
        if granted.step != LeaseStep::Closing {
            return false;
        }
        let Some(until) = granted.until else {
            return false;
        };
        let retunes = if granted.returns { 2 } else { 1 };
        // The outbound gate is already closed at this check. Only the
        // return move still has a current publication to drain.
        let listening = if granted.returns {
            sum(policy.min_listen, publication_bound)
        } else {
            policy.min_listen
        };
        let needed = (0..retunes).fold(listening, |needed, _| sum(needed, policy.retune));
        now.saturating_add(needed) > until
    }

    /// When the arbiter next needs [`Self::poll_action`], even without an
    /// input.
    #[must_use = "arm the timer for the returned deadline"]
    pub fn poll_deadline(&self) -> Option<Instant> {
        if self.cycle.is_some() {
            return None;
        }
        let retune = sum(self.publication_bound, self.policy.retune);
        let lease = |granted: &Granted<'id, K, Path>| match granted.step {
            LeaseStep::Waiting { .. } => Some(granted.from),
            LeaseStep::AwaitingMove { deadline } => Some(deadline),
            LeaseStep::Running => granted.listen_end(retune),
            _ => None,
        };
        let primary = self
            .primary
            .as_ref()
            .and_then(|(granted, _)| lease(granted));
        let absence = self
            .absence
            .as_ref()
            .and_then(|(granted, _)| lease(granted));
        let pending = match self.pair {
            Pair::Moving { at, .. } => Some(at),
            _ => None,
        };
        let loss = self.coordinator.upstream_loss_due(self.policy.loss);
        [primary, absence, pending, loss]
            .into_iter()
            .flatten()
            .min()
    }
}

/// The first absence slot that starts at or after `earliest`: the guard
/// after one of the schedule's TBTTs.
fn slot<K>(schedule: ApSchedule<K>, earliest: Instant, guard: Duration) -> Option<Instant> {
    let first = schedule.next_tbtt.checked_add(guard)?;
    if earliest <= first {
        return Some(first);
    }
    let late = earliest.checked_duration_since(first)?.as_micros();
    let interval = schedule.beacon_interval.as_micros();
    let steps = late.div_ceil(interval);
    let offset = steps.checked_mul(interval)?;
    first.checked_add(Duration::from_micros(offset))
}

#[cfg(test)]
mod tests;
