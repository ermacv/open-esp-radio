//! The channel of a port that a station and an access point share.
//!
//! One radio serves both interfaces, so the port has one channel. The
//! station's BSS (the upstream) decides it: the access point beside the
//! station follows the upstream wherever its [`ApFollowPolicy`] lets it.
//! The [`ChannelCoordinator`] decides each move and the port's owner carries
//! it out: the coordinator takes the station's and the access point's facts
//! as values and answers with [`CoordinatorAction`]s; it has no clock,
//! executor or port of its own.
//!
//! - **The upstream announces a move.** The access point announces the same
//!   move to its peers, counting the beacons it still sends before the
//!   upstream's switch instant, in the upstream's mode; when not one fits,
//!   it counts one and tells its peers to stop transmitting. The port moves
//!   once, at the access point's TBTT where its switch is due: the station
//!   follows the upstream at most one beacon interval late.
//! - **The upstream goes where the access point may not follow** (a band
//!   outside the policy's, or a channel that needs radar detection,
//!   [`requires_radar_detection`]): the policy's [`ApUnservable`] decides
//!   whether the access point stops or the station leaves the upstream.
//! - **The upstream is lost without an announcement.** The access point
//!   keeps its channel and the station searches for the upstream in short
//!   absences from it, one after each of the access point's TBTTs while the
//!   search is dense, one about every [`ApSearchPolicy::sparse_interval`]
//!   after that. Found on the access point's channel, the station joins it
//!   again; found elsewhere, the access point announces the move first.
//! - **The upstream stays lost.** An [`UpstreamLossPolicy`] bounds the
//!   search: [`UpstreamLossPolicy::after`] the loss, the access point stops,
//!   the station scans alone and joins the upstream, and the access point
//!   starts on the upstream's channel. A scan that finds none starts the
//!   access point again on its previous channel, and the next attempt
//!   follows `after` later ([`ChannelCoordinator::upstream_rescan`]).

use core::num::{NonZeroU8, NonZeroU64};

use oer_ieee80211_contracts_port::{Ieee80211Capabilities, SupportedChannel};
use oer_ieee80211_wire::channel::{Band, Channel};
use oer_ieee80211_wire::channel_switch::ChannelSwitchMode;
use oer_time::{Duration, Instant};

use crate::channel::requires_radar_detection;

/// The bands an access point beside a station may serve.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApBands {
    pub ghz2_4: bool,
    pub ghz5: bool,
}

impl ApBands {
    /// The 2.4 GHz band alone: the access point's peers that know no other
    /// band never lose it to a move.
    pub const GHZ2_4: Self = Self {
        ghz2_4: true,
        ghz5: false,
    };

    pub const fn contains(self, band: Band) -> bool {
        match band {
            Band::Ghz2_4 => self.ghz2_4,
            Band::Ghz5 => self.ghz5,
        }
    }
}

/// What happens when the upstream goes where the access point may not
/// follow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApUnservable {
    /// The access point releases its peers and closes its BSS; the station
    /// keeps its link, and the access point starts again once the upstream
    /// is on a channel it may serve.
    StopAccessPoint,
    /// The station leaves the upstream; the access point keeps its channel
    /// and its peers.
    LeaveUpstream,
}

/// How the access point follows the upstream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApFollowPolicy {
    pub bands: ApBands,
    pub unservable: ApUnservable,
    /// The beacons that announce a move the coordinator starts itself (to
    /// where a lost upstream was found, or where the station joins).
    /// Zero is rejected when the policy is built, before any move:
    /// ```compile_fail,E0308
    /// use oer_ieee80211_core_arbiter::coordinator::ApFollowPolicy;
    /// let policy = ApFollowPolicy {
    ///     announce_count: 0,
    ///     ..ApFollowPolicy::DEFAULT
    /// };
    /// ```
    /// A nonzero count is carried unchanged into the announcement:
    /// ```
    /// use core::num::{NonZeroU8, NonZeroU64};
    /// use oer_ieee80211_core_arbiter::coordinator::ApFollowPolicy;
    /// let policy = ApFollowPolicy {
    ///     announce_count: NonZeroU8::MIN,
    ///     ..ApFollowPolicy::DEFAULT
    /// };
    /// assert_eq!(policy.announce_count.get(), 1);
    /// ```
    pub announce_count: NonZeroU8,
    /// The shortest time between two moves of the access point to where
    /// the station joins, counted from their announcements: within it a
    /// join elsewhere is refused ([`JoinAway::TooSoon`]). It doubles after
    /// each failed join that followed a move, up to
    /// [`Self::max_follow_interval`], and comes back to this once the
    /// station connects.
    pub min_follow_interval: Duration,
    /// The ceiling of the doubled follow interval.
    pub max_follow_interval: Duration,
}

impl ApFollowPolicy {
    /// 2.4 GHz only; the access point stops where it may not follow; five
    /// beacons announce a move of its own; at most one move to where the
    /// station joins every 30 s, doubled after each failed join up to 480 s.
    ///
    /// Owner policy (2026-10-18 confirmation).
    pub const DEFAULT: Self = Self {
        bands: ApBands::GHZ2_4,
        unservable: ApUnservable::StopAccessPoint,
        announce_count: const { NonZeroU8::new(5).unwrap() },
        min_follow_interval: Duration::from_secs(30),
        max_follow_interval: Duration::from_secs(480),
    };

    /// Whether the access point may serve `channel`.
    pub const fn serves(&self, channel: Channel) -> bool {
        self.bands.contains(channel.band()) && !requires_radar_detection(channel)
    }
}

/// How the station searches for an upstream lost without an announcement.
///
/// Channel traversal belongs to the station, so this policy has no channel list:
/// ```compile_fail,E0599
/// use oer_ieee80211_core_arbiter::coordinator::ApSearchPolicy;
/// let policy = ApSearchPolicy::new(&[]);
/// ```
/// Timing is a value independent of the station's channel storage:
/// ```
/// use oer_ieee80211_core_arbiter::coordinator::ApSearchPolicy;
/// let policy = ApSearchPolicy::DEFAULT;
/// assert!(policy.sparse_interval > oer_time::Duration::ZERO);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApSearchPolicy {
    /// How long the search takes one absence per beacon interval.
    pub dense_period: Duration,
    /// The spacing of absences after the dense period.
    pub sparse_interval: Duration,
}

impl ApSearchPolicy {
    /// One absence per beacon interval for 30 s, then about one a second.
    pub const DEFAULT: Self = Self {
        dense_period: Duration::from_secs(30),
        sparse_interval: Duration::from_secs(1),
    };
}

/// What the pair does when its upstream stays lost.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpstreamLossAction {
    /// Stop the access point (its peers released, its BSS closed), let the
    /// station scan alone for the upstream and join it, then start the
    /// access point on the upstream's channel. A scan that finds no
    /// upstream starts the access point again on its previous channel; the
    /// next attempt follows [`UpstreamLossPolicy::after`] later.
    RescanAndRestart,
}

/// When and how the pair gives up waiting for a lost upstream on the
/// access point's channel. Both fields are mandatory: there is no implicit
/// wait without end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpstreamLossPolicy {
    /// How long after the loss, or after a scan that found no upstream, the
    /// action runs.
    pub after: Duration,
    pub action: UpstreamLossAction,
}

impl UpstreamLossPolicy {
    /// After 30 s, stop the access point, scan and restart it.
    ///
    /// Owner policy (2026-10-18 confirmation).
    pub const DEFAULT: Self = Self {
        after: Duration::from_secs(30),
        action: UpstreamLossAction::RescanAndRestart,
    };
}

/// What the port's owner does for the coordinator.
#[derive(Debug, Eq, PartialEq)]
pub enum CoordinatorAction<K> {
    /// The access point announces a move of its BSS to `target` in its next
    /// `count` beacons.
    AnnounceSwitch {
        target: SupportedChannel<K>,
        mode: ChannelSwitchMode,
        count: u8,
    },
    /// Move the port to `channel` at `at`, for every interface, tell each
    /// client its switch is done and report it
    /// ([`ChannelCoordinator::port_moved`]).
    Retune {
        channel: SupportedChannel<K>,
        at: Instant,
    },
    /// Release the access point's peers and close its BSS.
    StopAccessPoint,
    /// Start the access point on `channel`.
    StartAccessPoint { channel: SupportedChannel<K> },
    /// The station leaves the upstream.
    LeaveUpstream,
}

/// The actions one fact asks for, in order: at most two.
#[derive(Debug, Eq, PartialEq)]
pub struct CoordinatorActions<K>([Option<CoordinatorAction<K>>; 2]);

impl<K> CoordinatorActions<K> {
    const fn none() -> Self {
        Self([None, None])
    }

    const fn one(action: CoordinatorAction<K>) -> Self {
        Self([Some(action), None])
    }

    const fn two(first: CoordinatorAction<K>, second: CoordinatorAction<K>) -> Self {
        Self([Some(first), Some(second)])
    }
}

impl<K> IntoIterator for CoordinatorActions<K> {
    type Item = CoordinatorAction<K>;
    type IntoIter = core::iter::Flatten<core::array::IntoIter<Option<CoordinatorAction<K>>, 2>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter().flatten()
    }
}

/// How the access point meets a station that joins away from its channel
/// ([`ChannelCoordinator::station_joining_away`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JoinAway {
    /// The access point announces its move to the station's channel in its
    /// next `count` beacons; the station joins once the port moved. An
    /// announced move is never withdrawn: the access point stays there
    /// whether the station joins or not.
    Follow { count: u8 },
    /// The access point may not serve the channel and stops; the station
    /// joins alone.
    StopAccessPoint,
    /// The access point may not serve the channel and keeps it; the station
    /// may not join there.
    Refused,
    /// The access point moved to where the station joins less than
    /// [`ApFollowPolicy::min_follow_interval`] ago; the station may try
    /// again at `retry_at`.
    TooSoon { retry_at: Instant },
    /// The follow holdoff ends outside representable monotonic time. It
    /// cannot authorize another move or supply a shortened retry deadline.
    ScheduleExhausted,
}

/// A strictly positive AP beacon interval, in microseconds.
///
/// ```
/// use oer_ieee80211_core_arbiter::coordinator::BeaconInterval;
/// use oer_time::Duration;
/// assert!(BeaconInterval::new(Duration::ZERO).is_none());
/// assert_eq!(BeaconInterval::new(Duration::from_millis(100)).unwrap().as_micros(), 100_000);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeaconInterval(NonZeroU64);
impl BeaconInterval {
    pub const fn new(interval: Duration) -> Option<Self> {
        match NonZeroU64::new(interval.as_micros()) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
    pub const fn as_micros(self) -> u64 {
        self.0.get()
    }
    // Both operands are nonzero in the division; zero elapsed time needs no interval.
    pub(crate) const fn intervals_covering(self, micros: u64) -> u64 {
        match NonZeroU64::new(micros) {
            Some(micros) => micros.div_ceil(self.0).get(),
            None => 0,
        }
    }
    pub const fn duration(self) -> Duration {
        Duration::from_micros(self.as_micros())
    }
}

/// The access point's schedule, which counts and absences follow.
/// Raw durations cannot construct a schedule; zero must be rejected before
/// publishing an AP fact:
/// ```compile_fail,E0308
/// use oer_ieee80211_core_arbiter::coordinator::ApSchedule;
/// use oer_ieee80211_contracts_port::SupportedChannel;
/// fn raw_interval<K>(channel: SupportedChannel<K>) -> ApSchedule<K> {
///     ApSchedule { channel, next_tbtt: oer_time::Instant::from_micros(0),
///         beacon_interval: oer_time::Duration::ZERO }
/// }
/// ```
///
/// The channel must already be accepted by the backend:
/// ```compile_fail,E0308
/// use oer_ieee80211_core_arbiter::coordinator::ApSchedule;
/// use oer_ieee80211_wire::channel::Channel;
/// use oer_time::{Duration, Instant};
/// fn raw_schedule<K>(channel: Channel) -> ApSchedule<K> {
///     ApSchedule {
///         channel,
///         next_tbtt: Instant::from_micros(100),
///         beacon_interval: oer_ieee80211_core_arbiter::coordinator::BeaconInterval::new(Duration::from_millis(100)).unwrap(),
///     }
/// }
/// ```
/// A checked channel is carried unchanged into the schedule:
/// ```
/// use oer_ieee80211_core_arbiter::coordinator::ApSchedule;
/// use oer_ieee80211_contracts_port::SupportedChannel;
/// use oer_time::{Duration, Instant};
/// fn checked_schedule<K>(channel: SupportedChannel<K>) -> ApSchedule<K> {
///     ApSchedule {
///         channel,
///         next_tbtt: Instant::from_micros(100),
///         beacon_interval: oer_ieee80211_core_arbiter::coordinator::BeaconInterval::new(Duration::from_millis(100)).unwrap(),
///     }
/// }
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct ApSchedule<K> {
    pub channel: SupportedChannel<K>,
    /// Its next TBTT.
    pub next_tbtt: Instant,
    pub beacon_interval: BeaconInterval,
}

impl<K> ApSchedule<K> {
    /// The access point's TBTTs before `instant`, from the next one on.
    fn beacons_before(self, instant: Instant) -> u64 {
        if instant <= self.next_tbtt {
            return 0;
        }
        self.beacon_interval.intervals_covering(
            instant
                .saturating_duration_since(self.next_tbtt)
                .as_micros(),
        )
    }

    /// When a switch announced at `now` in the next `count` beacons is
    /// due: at the TBTT after the last of them. None means no representable
    /// future TBTT; callers must refuse the schedule rather than approximate it.
    pub fn switch_at(self, now: Instant, count: u8) -> Option<Instant> {
        let offset = self
            .beacon_interval
            .as_micros()
            .checked_mul(u64::from(count))?;
        crate::slot(self, now, Duration::ZERO)?.checked_add(Duration::from_micros(offset))
    }
}

/// Where the upstream is.
#[derive(Debug, Eq, PartialEq)]
enum Upstream<K> {
    /// The station is not connected.
    None,
    Connected(SupportedChannel<K>),
    /// Lost without an announcement, searched for since `since`.
    Searching {
        since: Instant,
    },
    /// Found, but not joined yet; the access point may still be announcing
    /// the move to this channel.
    Joining {
        since: Instant,
    },
    /// Scanned for by the station alone on the port, the access point
    /// stopped by the [`UpstreamLossPolicy`]; it starts again on `fallback`
    /// when the scan finds none.
    Rescanning {
        fallback: SupportedChannel<K>,
    },
    /// The failed scan owes a return before its access point may start.
    Returning {
        fallback: SupportedChannel<K>,
        since: Instant,
    },
}

/// Where the access point is.
#[derive(Debug, Eq, PartialEq)]
enum AccessPoint<K> {
    /// Not started, or stopped by the policy.
    Stopped,
    /// Asked to start and not running yet.
    Starting,
    Running(ApSchedule<K>),
}

/// Decides the shared port's channel; see the module documentation.
///
/// Facts cannot introduce an unchecked channel:
/// ```compile_fail,E0308
/// use oer_ieee80211_core_arbiter::coordinator::ChannelCoordinator;
/// use oer_ieee80211_wire::channel::Channel;
/// fn raw_fact<K: oer_ieee80211_contracts_port::Ieee80211Capabilities>(
///     coordinator: &mut ChannelCoordinator<K>, channel: Channel,
/// ) {
///     let _ = coordinator.station_connected(channel);
/// }
/// ```
///
/// The arbiter owns search timing; the coordinator has no independent poll loop:
/// ```compile_fail,E0599
/// use oer_ieee80211_core_arbiter::coordinator::ChannelCoordinator;
/// use oer_time::Instant;
/// fn separate_loop<K: oer_ieee80211_contracts_port::Ieee80211Capabilities>(
///     coordinator: &mut ChannelCoordinator<K>, now: Instant,
/// ) {
///     coordinator.poll(now);
/// }
/// ```
///
/// Its protocol state cannot be copied into a second owner:
/// ```compile_fail,E0382
/// use oer_ieee80211_core_arbiter::coordinator::ChannelCoordinator;
/// fn duplicate<K>(coordinator: ChannelCoordinator<K>) {
///     let first = coordinator;
///     let second = coordinator;
/// }
/// ```
/// One owner can consume facts and move its coordinator:
/// ```
/// use oer_ieee80211_core_arbiter::coordinator::{ApFollowPolicy, ChannelCoordinator};
/// use oer_ieee80211_contracts_port::{Ieee80211Capabilities, SupportedChannel};
/// fn accept<K: Ieee80211Capabilities>(channel: SupportedChannel<K>) {
///     let mut coordinator = ChannelCoordinator::new(ApFollowPolicy::DEFAULT);
///     let actions = coordinator.station_connected(channel);
///     for action in actions { /* the owner carries out each effect */ }
///     let owner = coordinator;
///     assert_eq!(owner.search_since(), None);
/// }
/// ```
#[derive(Debug)]
pub struct ChannelCoordinator<K> {
    policy: ApFollowPolicy,
    upstream: Upstream<K>,
    access_point: AccessPoint<K>,
    /// The access point should run: the composition wants it, the policy
    /// may have stopped it.
    wanted: bool,
    /// When the access point last announced a move to where the station
    /// joins.
    last_follow: Option<Instant>,
    /// The interval the next move to where the station joins waits for.
    follow_interval: Duration,
}

impl<K: Ieee80211Capabilities> ChannelCoordinator<K> {
    #[cfg(test)]
    pub(super) fn last_follow_for_test(&self) -> Option<Instant> {
        self.last_follow
    }

    pub const fn new(policy: ApFollowPolicy) -> Self {
        Self {
            policy,
            upstream: Upstream::None,
            access_point: AccessPoint::Stopped,
            wanted: false,
            last_follow: None,
            follow_interval: policy.min_follow_interval,
        }
    }

    /// A join the access point moved for failed: the next follow waits
    /// twice as long, up to the policy's ceiling.
    pub fn follow_failed(&mut self) {
        let doubled = Duration::from_micros(self.follow_interval.as_micros().saturating_mul(2));
        self.follow_interval = if doubled > self.policy.max_follow_interval {
            self.policy.max_follow_interval
        } else {
            doubled
        };
    }

    /// Since when the upstream is searched for, while it is.
    pub const fn search_since(&self) -> Option<Instant> {
        match self.upstream {
            Upstream::Searching { since, .. } => Some(since),
            _ => None,
        }
    }

    /// The station, beside the running access point, joins on `channel`,
    /// away from the access point's, at `now`: the policy decides whether
    /// the access point follows, stops or keeps its channel, and how soon
    /// it follows again. This inspection changes no state: the arbiter
    /// commits the announcement only after its complete schedule fits.
    pub fn station_joining_away(&self, channel: SupportedChannel<K>, now: Instant) -> JoinAway {
        if !self.policy.serves(channel.channel()) {
            return match self.policy.unservable {
                ApUnservable::StopAccessPoint => JoinAway::StopAccessPoint,
                ApUnservable::LeaveUpstream => JoinAway::Refused,
            };
        }
        if let Some(last) = self.last_follow {
            let Some(retry_at) = last.checked_add(self.follow_interval) else {
                return JoinAway::ScheduleExhausted;
            };
            if now < retry_at {
                return JoinAway::TooSoon { retry_at };
            }
        }
        JoinAway::Follow {
            count: self.policy.announce_count.get(),
        }
    }

    /// Commit the follow whose announcement and timeout the arbiter admitted.
    pub(super) fn follow_announced(&mut self, now: Instant) {
        self.last_follow = Some(now);
    }

    /// The composition wants the access point to run: on the upstream's
    /// channel once the station is connected and the policy lets it.
    pub fn want_access_point(&mut self) -> CoordinatorActions<K> {
        self.wanted = true;
        self.start_if_servable()
    }

    /// The station connected to the upstream on `channel`.
    pub fn station_connected(&mut self, channel: SupportedChannel<K>) -> CoordinatorActions<K> {
        self.follow_interval = self.policy.min_follow_interval;
        self.upstream = Upstream::Connected(channel);
        self.start_if_servable()
    }

    /// The port moved to `channel`, as a [`CoordinatorAction::Retune`]
    /// asked: a stopped access point may start there. The station owns its
    /// join lease and starts joining when that lease becomes ready.
    pub fn port_moved(&mut self, channel: SupportedChannel<K>) -> CoordinatorActions<K> {
        if let Upstream::Returning { fallback, since } = self.upstream
            && channel == fallback
        {
            self.upstream = Upstream::Searching { since };
            self.access_point = AccessPoint::Starting;
            return CoordinatorActions::one(CoordinatorAction::StartAccessPoint { channel });
        }
        if let AccessPoint::Running(schedule) = &mut self.access_point {
            schedule.channel = channel;
        }
        if let Upstream::Connected(_) = self.upstream {
            self.upstream = Upstream::Connected(channel);
        }
        self.start_if_servable()
    }

    /// The access point runs on `schedule`'s channel; its next TBTT is
    /// `schedule.next_tbtt`. Repeated at each TBTT, it keeps counts and
    /// absences on the access point's schedule.
    pub fn access_point_running(&mut self, schedule: ApSchedule<K>) {
        self.access_point = AccessPoint::Running(schedule);
    }

    /// The access point stopped.
    pub fn access_point_stopped(&mut self) {
        self.access_point = AccessPoint::Stopped;
    }

    /// The upstream announced a move to `target` at `at`, in `mode`.
    pub fn upstream_switch_announced(
        &mut self,
        target: SupportedChannel<K>,
        mode: ChannelSwitchMode,
        at: Instant,
    ) -> CoordinatorActions<K> {
        let retune = CoordinatorAction::Retune {
            channel: target,
            at,
        };
        let AccessPoint::Running(schedule) = self.access_point else {
            // The station is the port's only user: the port moves with the
            // upstream.
            return CoordinatorActions::one(retune);
        };
        if !self.policy.serves(target.channel()) {
            return match self.unservable() {
                // The station follows the upstream alone.
                stop @ CoordinatorAction::StopAccessPoint => CoordinatorActions::two(stop, retune),
                leave => CoordinatorActions::one(leave),
            };
        }
        let (count, mode) = match self.beacons_before(schedule, at) {
            0 => (1, ChannelSwitchMode::StopTransmitting),
            beacons => (beacons, mode),
        };
        CoordinatorActions::one(CoordinatorAction::AnnounceSwitch {
            target,
            mode,
            count,
        })
    }

    /// The access point's announced move to `target` is due at `now`: the
    /// port moves for both interfaces.
    pub fn access_point_switch_due(
        &mut self,
        target: SupportedChannel<K>,
        now: Instant,
    ) -> CoordinatorActions<K> {
        CoordinatorActions::one(CoordinatorAction::Retune {
            channel: target,
            at: now,
        })
    }

    /// The station lost the upstream without an announcement at `now`.
    pub fn upstream_lost(&mut self, now: Instant) {
        self.upstream = match self.access_point {
            // The access point holds its channel; the station searches
            // around it.
            AccessPoint::Running(_) => Upstream::Searching { since: now },
            // Alone on the port, the station reconnects its own way.
            AccessPoint::Stopped | AccessPoint::Starting => Upstream::None,
        };
    }

    /// The station heard the upstream on `channel` while it searched.
    pub fn upstream_found(&mut self, channel: SupportedChannel<K>) -> CoordinatorActions<K> {
        let Upstream::Searching { since, .. } = self.upstream else {
            return CoordinatorActions::none();
        };
        self.upstream = Upstream::Joining { since };
        let AccessPoint::Running(schedule) = self.access_point else {
            return CoordinatorActions::none();
        };
        if channel == schedule.channel {
            return CoordinatorActions::none();
        }
        if !self.policy.serves(channel.channel()) {
            return CoordinatorActions::one(self.unservable());
        }
        // The access point moves first. The station's lease, owned by the
        // arbiter, admits the join once that move finishes.
        CoordinatorActions::one(CoordinatorAction::AnnounceSwitch {
            target: channel,
            mode: ChannelSwitchMode::Continue,
            count: self.policy.announce_count.get(),
        })
    }

    /// When the [`UpstreamLossPolicy`] acts on the upstream lost while the
    /// access point runs: `policy.after` the loss, or after a scan that
    /// found none. `None` while the upstream is not searched for.
    pub fn upstream_loss_due(&self, policy: UpstreamLossPolicy) -> Option<Instant> {
        match (self.upstream, self.access_point) {
            (Upstream::Searching { since, .. }, AccessPoint::Running(_)) => {
                since.checked_add(policy.after)
            }
            _ => None,
        }
    }

    /// The [`UpstreamLossPolicy`]'s action is due: the access point stops,
    /// and the station, alone on the port, scans for the upstream. Report
    /// the outcome: [`Self::station_connected`] when it joined,
    /// [`Self::upstream_rescan_failed`] when it did not.
    pub fn upstream_rescan(&mut self) -> CoordinatorActions<K> {
        let (Upstream::Searching { .. }, AccessPoint::Running(schedule)) =
            (self.upstream, self.access_point)
        else {
            return CoordinatorActions::none();
        };
        self.upstream = Upstream::Rescanning {
            fallback: schedule.channel,
        };
        self.access_point = AccessPoint::Stopped;
        CoordinatorActions::one(CoordinatorAction::StopAccessPoint)
    }

    /// The station's scan found no upstream to join at `now`: the port goes
    /// back to the access point's previous channel, the access point starts
    /// there, and the loss counts again from `now`.
    pub fn upstream_rescan_failed(&mut self, now: Instant) -> CoordinatorActions<K> {
        let Upstream::Rescanning { fallback } = self.upstream else {
            return CoordinatorActions::none();
        };
        self.upstream = Upstream::Returning {
            fallback,
            since: now,
        };
        CoordinatorActions::one(CoordinatorAction::Retune {
            channel: fallback,
            at: now,
        })
    }

    /// A discovered upstream refused the join. Resume searching without
    /// restarting the dense period that began at the original loss.
    pub(super) fn join_failed(&mut self) {
        if let Upstream::Joining { since, .. } = self.upstream {
            self.upstream = Upstream::Searching { since };
        }
    }

    /// The access point's TBTTs before `at`, as a count it can announce.
    fn beacons_before(&self, schedule: ApSchedule<K>, at: Instant) -> u8 {
        u8::try_from(schedule.beacons_before(at)).unwrap_or(u8::MAX)
    }

    /// Start the wanted access point on the upstream's channel, when the
    /// policy lets it serve that channel.
    fn start_if_servable(&mut self) -> CoordinatorActions<K> {
        let Upstream::Connected(channel) = self.upstream else {
            return CoordinatorActions::none();
        };
        if !self.wanted
            || !matches!(self.access_point, AccessPoint::Stopped)
            || !self.policy.serves(channel.channel())
        {
            return CoordinatorActions::none();
        }
        self.access_point = AccessPoint::Starting;
        CoordinatorActions::one(CoordinatorAction::StartAccessPoint { channel })
    }

    /// The upstream went where the access point may not follow.
    fn unservable(&mut self) -> CoordinatorAction<K> {
        match self.policy.unservable {
            ApUnservable::StopAccessPoint => {
                self.access_point = AccessPoint::Stopped;
                CoordinatorAction::StopAccessPoint
            }
            ApUnservable::LeaveUpstream => {
                self.upstream = Upstream::None;
                CoordinatorAction::LeaveUpstream
            }
        }
    }
}

// Only value facts are copied. The backend marker carries no storage and
// need not implement Copy; ChannelCoordinator remains a single owner.
impl<K> Copy for CoordinatorAction<K> {}

impl<K> Clone for CoordinatorAction<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for CoordinatorActions<K> {}

impl<K> Clone for CoordinatorActions<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for ApSchedule<K> {}

impl<K> Clone for ApSchedule<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Upstream<K> {}

impl<K> Clone for Upstream<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for AccessPoint<K> {}

impl<K> Clone for AccessPoint<K> {
    fn clone(&self) -> Self {
        *self
    }
}

#[cfg(test)]
mod tests;
