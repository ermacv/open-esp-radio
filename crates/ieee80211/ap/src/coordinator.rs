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

use oer_ieee80211_mac::channel::{Band, Channel};
use oer_ieee80211_mac::channel_switch::ChannelSwitchMode;
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
    /// where a lost upstream was found).
    pub announce_count: u8,
}

impl ApFollowPolicy {
    /// 2.4 GHz only; the access point stops where it may not follow; five
    /// beacons announce a move of its own.
    pub const DEFAULT: Self = Self {
        bands: ApBands::GHZ2_4,
        unservable: ApUnservable::StopAccessPoint,
        announce_count: 5,
    };

    /// Whether the access point may serve `channel`.
    pub const fn serves(&self, channel: Channel) -> bool {
        self.bands.contains(channel.band()) && !requires_radar_detection(channel)
    }
}

/// How the station searches for an upstream lost without an announcement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApSearchPolicy<'a> {
    /// The channels searched, in turn; the access point's own channel,
    /// which the station hears between absences, is skipped. Empty: the
    /// station waits on the access point's channel alone.
    pub channels: &'a [Channel],
    /// How long one absence listens on another channel.
    pub dwell: Duration,
    /// How long after the access point's TBTT an absence starts, the
    /// access point's beacon sent.
    pub guard: Duration,
    /// How long the search takes one absence per beacon interval.
    pub dense_period: Duration,
    /// The spacing of absences after the dense period.
    pub sparse_interval: Duration,
}

impl<'a> ApSearchPolicy<'a> {
    /// Absences of 20 ms, 2 ms after the TBTT, each beacon interval for
    /// 30 s, then about one a second, over `channels`.
    pub const fn new(channels: &'a [Channel]) -> Self {
        Self {
            channels,
            dwell: Duration::from_millis(20),
            guard: Duration::from_millis(2),
            dense_period: Duration::from_secs(30),
            sparse_interval: Duration::from_secs(1),
        }
    }
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
    pub const DEFAULT: Self = Self {
        after: Duration::from_secs(30),
        action: UpstreamLossAction::RescanAndRestart,
    };
}

/// What the port's owner does for the coordinator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinatorAction {
    /// The access point announces a move of its BSS to `target` in its next
    /// `count` beacons.
    AnnounceSwitch {
        target: Channel,
        mode: ChannelSwitchMode,
        count: u8,
    },
    /// Move the port to `channel` at `at`, for every interface, tell each
    /// client its switch is done and report it
    /// ([`ChannelCoordinator::port_moved`]).
    Retune { channel: Channel, at: Instant },
    /// Release the access point's peers and close its BSS.
    StopAccessPoint,
    /// Start the access point on `channel`.
    StartAccessPoint { channel: Channel },
    /// The station leaves the upstream.
    LeaveUpstream,
    /// The station joins the upstream again on `channel`.
    JoinUpstream { channel: Channel },
    /// Leave the port's channel from `start` until `until` to listen for the
    /// upstream on `channel`, the access point's air reserved meanwhile.
    Absence {
        channel: Channel,
        start: Instant,
        until: Instant,
    },
}

/// The actions one fact asks for, in order: at most two.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CoordinatorActions([Option<CoordinatorAction>; 2]);

impl CoordinatorActions {
    pub const NONE: Self = Self([None, None]);

    const fn one(action: CoordinatorAction) -> Self {
        Self([Some(action), None])
    }

    const fn two(first: CoordinatorAction, second: CoordinatorAction) -> Self {
        Self([Some(first), Some(second)])
    }

    pub fn is_empty(&self) -> bool {
        self.0[0].is_none()
    }

    pub fn iter(&self) -> impl Iterator<Item = CoordinatorAction> + '_ {
        self.0.iter().flatten().copied()
    }
}

impl IntoIterator for CoordinatorActions {
    type Item = CoordinatorAction;
    type IntoIter = core::iter::Flatten<core::array::IntoIter<Option<CoordinatorAction>, 2>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter().flatten()
    }
}

/// The access point's schedule, which counts and absences follow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApSchedule {
    pub channel: Channel,
    /// Its next TBTT.
    pub next_tbtt: Instant,
    pub beacon_interval: Duration,
}

impl ApSchedule {
    /// The access point's TBTTs before `instant`, from the next one on.
    fn beacons_before(self, instant: Instant) -> u64 {
        let interval = self.beacon_interval.as_micros();
        if interval == 0 || instant <= self.next_tbtt {
            return 0;
        }
        instant
            .saturating_duration_since(self.next_tbtt)
            .as_micros()
            .div_ceil(interval)
    }

    /// The first TBTT at or after `instant`.
    fn tbtt_from(self, instant: Instant) -> Instant {
        let interval = self.beacon_interval.as_micros();
        if interval == 0 || instant <= self.next_tbtt {
            return self.next_tbtt;
        }
        let late = instant
            .saturating_duration_since(self.next_tbtt)
            .as_micros();
        let steps = late.div_ceil(interval);
        self.next_tbtt
            .checked_add(Duration::from_micros(steps * interval))
            .unwrap_or(instant)
    }
}

/// Where the upstream is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Upstream {
    /// The station is not connected.
    None,
    Connected(Channel),
    /// Lost without an announcement, searched for since `since`.
    Searching {
        since: Instant,
        next_absence: Option<Instant>,
        cursor: usize,
    },
    /// Found, but not joined yet; the access point may still be announcing
    /// the move to this channel.
    Joining {
        channel: Channel,
        since: Instant,
        waiting_for_move: bool,
    },
    /// Scanned for by the station alone on the port, the access point
    /// stopped by the [`UpstreamLossPolicy`]; it starts again on `fallback`
    /// when the scan finds none.
    Rescanning {
        fallback: Channel,
    },
}

/// Where the access point is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccessPoint {
    /// Not started, or stopped by the policy.
    Stopped,
    /// Asked to start and not running yet.
    Starting(Channel),
    Running(ApSchedule),
}

/// Decides the shared port's channel; see the module documentation.
#[derive(Clone, Copy, Debug)]
pub struct ChannelCoordinator<'a> {
    policy: ApFollowPolicy,
    search: ApSearchPolicy<'a>,
    upstream: Upstream,
    access_point: AccessPoint,
    /// The access point should run: the composition wants it, the policy
    /// may have stopped it.
    wanted: bool,
}

impl<'a> ChannelCoordinator<'a> {
    pub const fn new(policy: ApFollowPolicy, search: ApSearchPolicy<'a>) -> Self {
        Self {
            policy,
            search,
            upstream: Upstream::None,
            access_point: AccessPoint::Stopped,
            wanted: false,
        }
    }

    pub const fn policy(&self) -> ApFollowPolicy {
        self.policy
    }

    /// The configured search timing and channel list.
    pub const fn search_policy(&self) -> ApSearchPolicy<'a> {
        self.search
    }

    /// Whether the owner should observe the upstream between absences.
    pub const fn searching(&self) -> bool {
        matches!(self.upstream, Upstream::Searching { .. })
    }

    /// The composition wants the access point to run: on the upstream's
    /// channel once the station is connected and the policy lets it.
    pub fn want_access_point(&mut self) -> CoordinatorActions {
        self.wanted = true;
        self.start_if_servable()
    }

    /// The station connected to the upstream on `channel`.
    pub fn station_connected(&mut self, channel: Channel) -> CoordinatorActions {
        self.upstream = Upstream::Connected(channel);
        self.start_if_servable()
    }

    /// The port moved to `channel`, as a [`CoordinatorAction::Retune`]
    /// asked: a stopped access point may start there, or a newly discovered
    /// upstream can now be joined after the access point's CSA.
    pub fn port_moved(&mut self, channel: Channel) -> CoordinatorActions {
        if let AccessPoint::Running(schedule) = &mut self.access_point {
            schedule.channel = channel;
        }
        if let Upstream::Connected(_) = self.upstream {
            self.upstream = Upstream::Connected(channel);
        }
        if let Upstream::Joining {
            channel: found,
            since,
            waiting_for_move: true,
        } = self.upstream
            && found == channel
        {
            self.upstream = Upstream::Joining {
                channel,
                since,
                waiting_for_move: false,
            };
            return CoordinatorActions::one(CoordinatorAction::JoinUpstream { channel });
        }
        self.start_if_servable()
    }

    /// The access point runs on `schedule`'s channel; its next TBTT is
    /// `schedule.next_tbtt`. Repeated at each TBTT, it keeps counts and
    /// absences on the access point's schedule.
    pub fn access_point_running(&mut self, schedule: ApSchedule) {
        self.access_point = AccessPoint::Running(schedule);
    }

    /// The access point stopped.
    pub fn access_point_stopped(&mut self) {
        self.access_point = AccessPoint::Stopped;
    }

    /// The upstream announced a move to `target` at `at`, in `mode`.
    pub fn upstream_switch_announced(
        &mut self,
        target: Channel,
        mode: ChannelSwitchMode,
        at: Instant,
    ) -> CoordinatorActions {
        let retune = CoordinatorAction::Retune {
            channel: target,
            at,
        };
        let AccessPoint::Running(schedule) = self.access_point else {
            // The station is the port's only user: the port moves with the
            // upstream.
            return CoordinatorActions::one(retune);
        };
        if !self.policy.serves(target) {
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
    pub fn access_point_switch_due(&mut self, target: Channel, now: Instant) -> CoordinatorActions {
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
            AccessPoint::Running(_) => Upstream::Searching {
                since: now,
                next_absence: None,
                cursor: 0,
            },
            // Alone on the port, the station reconnects its own way.
            AccessPoint::Stopped | AccessPoint::Starting(_) => Upstream::None,
        };
    }

    /// The station heard the upstream on `channel` while it searched.
    pub fn upstream_found(&mut self, channel: Channel) -> CoordinatorActions {
        let Upstream::Searching { since, .. } = self.upstream else {
            return CoordinatorActions::NONE;
        };
        self.upstream = Upstream::Joining {
            channel,
            since,
            waiting_for_move: false,
        };
        let join = CoordinatorAction::JoinUpstream { channel };
        let AccessPoint::Running(schedule) = self.access_point else {
            return CoordinatorActions::one(join);
        };
        if channel == schedule.channel {
            return CoordinatorActions::one(join);
        }
        if !self.policy.serves(channel) {
            return match self.unservable() {
                stop @ CoordinatorAction::StopAccessPoint => CoordinatorActions::two(stop, join),
                leave => CoordinatorActions::one(leave),
            };
        }
        // The access point moves first; the station joins once the port
        // has ([`Self::port_moved`] then [`Self::station_connected`]).
        self.upstream = Upstream::Joining {
            channel,
            since,
            waiting_for_move: true,
        };
        CoordinatorActions::one(CoordinatorAction::AnnounceSwitch {
            target: channel,
            mode: ChannelSwitchMode::Continue,
            count: self.policy.announce_count.max(1),
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
    pub fn upstream_rescan(&mut self) -> CoordinatorActions {
        let (Upstream::Searching { .. }, AccessPoint::Running(schedule)) =
            (self.upstream, self.access_point)
        else {
            return CoordinatorActions::NONE;
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
    pub fn upstream_rescan_failed(&mut self, now: Instant) -> CoordinatorActions {
        let Upstream::Rescanning { fallback } = self.upstream else {
            return CoordinatorActions::NONE;
        };
        self.upstream = Upstream::Searching {
            since: now,
            next_absence: None,
            cursor: 0,
        };
        self.access_point = AccessPoint::Starting(fallback);
        CoordinatorActions::two(
            CoordinatorAction::Retune {
                channel: fallback,
                at: now,
            },
            CoordinatorAction::StartAccessPoint { channel: fallback },
        )
    }

    /// A discovered upstream refused the join. Resume searching without
    /// restarting the dense period that began at the original loss.
    pub fn upstream_join_failed(&mut self) {
        if let Upstream::Joining { since, .. } = self.upstream {
            self.upstream = Upstream::Searching {
                since,
                next_absence: None,
                cursor: 0,
            };
        }
    }

    /// The next absence of a search that is due at `now`.
    pub fn poll(&mut self, now: Instant) -> Option<CoordinatorAction> {
        let AccessPoint::Running(schedule) = self.access_point else {
            return None;
        };
        let Upstream::Searching {
            since,
            next_absence,
            cursor,
        } = self.upstream
        else {
            return None;
        };
        let candidates = self
            .search
            .channels
            .iter()
            .filter(|channel| !same_primary(**channel, schedule.channel));
        let count = candidates.clone().count();
        if count == 0 {
            return None;
        }
        let mut start = match next_absence {
            Some(start) => start,
            None => self.absence_after(schedule, now)?,
        };
        // A completed exchange can make the owner late. Never replay a
        // window whose end has already passed, nor accumulate a backlog
        // of missed dense windows.
        if self.search.dwell != Duration::ZERO && now >= start.checked_add(self.search.dwell)? {
            start = self.absence_after(schedule, now)?;
        }
        if now < start {
            self.upstream = Upstream::Searching {
                since,
                next_absence: Some(start),
                cursor,
            };
            return None;
        }
        let channel = *candidates.clone().nth(cursor % count)?;
        let until = start.checked_add(self.search.dwell)?;
        let dense_until = since.checked_add(self.search.dense_period)?;
        let spacing = if start < dense_until {
            schedule.beacon_interval
        } else {
            self.search.sparse_interval
        };
        let next = self.absence_after(schedule, start.checked_add(spacing)?)?;
        self.upstream = Upstream::Searching {
            since,
            next_absence: Some(next),
            cursor: (cursor + 1) % count,
        };
        Some(CoordinatorAction::Absence {
            channel,
            start,
            until,
        })
    }

    /// The earliest instant the coordinator needs its owner without input.
    pub fn next_deadline(&self) -> Option<Instant> {
        if let AccessPoint::Running(schedule) = self.access_point
            && !self
                .search
                .channels
                .iter()
                .any(|channel| !same_primary(*channel, schedule.channel))
        {
            return None;
        }
        match (self.upstream, self.access_point) {
            (
                Upstream::Searching {
                    next_absence: Some(next),
                    ..
                },
                AccessPoint::Running(_),
            ) => Some(next),
            (Upstream::Searching { .. }, AccessPoint::Running(schedule)) => {
                Some(schedule.next_tbtt)
            }
            _ => None,
        }
    }

    /// The access point's TBTTs before `at`, as a count it can announce.
    fn beacons_before(&self, schedule: ApSchedule, at: Instant) -> u8 {
        u8::try_from(schedule.beacons_before(at)).unwrap_or(u8::MAX)
    }

    /// The first absence start at or after `instant`: the guard after a
    /// TBTT.
    fn absence_after(&self, schedule: ApSchedule, instant: Instant) -> Option<Instant> {
        let tbtt = schedule.tbtt_from(instant.checked_sub(self.search.guard)?);
        tbtt.checked_add(self.search.guard)
    }

    /// Start the wanted access point on the upstream's channel, when the
    /// policy lets it serve that channel.
    fn start_if_servable(&mut self) -> CoordinatorActions {
        let Upstream::Connected(channel) = self.upstream else {
            return CoordinatorActions::NONE;
        };
        if !self.wanted
            || !matches!(self.access_point, AccessPoint::Stopped)
            || !self.policy.serves(channel)
        {
            return CoordinatorActions::NONE;
        }
        self.access_point = AccessPoint::Starting(channel);
        CoordinatorActions::one(CoordinatorAction::StartAccessPoint { channel })
    }

    /// The upstream went where the access point may not follow.
    fn unservable(&mut self) -> CoordinatorAction {
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

fn same_primary(first: Channel, second: Channel) -> bool {
    first.band() == second.band() && first.number() == second.number()
}

#[cfg(test)]
mod tests;
