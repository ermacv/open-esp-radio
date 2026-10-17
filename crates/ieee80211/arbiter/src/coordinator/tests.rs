use oer_ieee80211_mac::channel::ChannelWidth;
use std::vec::Vec;

use super::*;
use crate::tests::Caps;

const INTERVAL: Duration = Duration::from_micros(102_400);
const FIRST_TBTT: u64 = 1_000_000;

fn ghz2_4(number: u8) -> SupportedChannel<Caps> {
    SupportedChannel::new(Channel::ghz2_4(number, ChannelWidth::Mhz20).unwrap()).unwrap()
}

fn ghz5(number: u8) -> SupportedChannel<Caps> {
    SupportedChannel::new(Channel::ghz5(number, ChannelWidth::Mhz20).unwrap()).unwrap()
}

const fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

fn schedule(channel: SupportedChannel<Caps>) -> ApSchedule<Caps> {
    ApSchedule {
        channel,
        next_tbtt: at(FIRST_TBTT),
        beacon_interval: crate::coordinator::BeaconInterval::new(INTERVAL).unwrap(),
    }
}

fn actions(actions: CoordinatorActions<Caps>) -> Vec<CoordinatorAction<Caps>> {
    actions.into_iter().collect()
}

/// A coordinator whose access point runs beside a station on `channel`.
fn running(policy: ApFollowPolicy, channel: SupportedChannel<Caps>) -> ChannelCoordinator<Caps> {
    let mut coordinator = ChannelCoordinator::new(policy);
    // Wanted before the station connects, the access point waits for the
    // upstream's channel.
    assert!(coordinator.want_access_point().into_iter().next().is_none());
    assert_eq!(
        actions(coordinator.station_connected(channel)),
        [CoordinatorAction::StartAccessPoint { channel }]
    );
    coordinator.access_point_running(schedule(channel));
    coordinator
}

#[test]
fn the_access_point_announces_the_upstream_s_move_in_the_beacons_that_fit_before_it() {
    let mut coordinator = running(ApFollowPolicy::DEFAULT, ghz2_4(6));
    // The upstream moves two and a half of our intervals after our next
    // TBTT: three of our beacons go out before it.
    let switch_at = at(FIRST_TBTT + 2 * INTERVAL.as_micros() + INTERVAL.as_micros() / 2);
    assert_eq!(
        actions(coordinator.upstream_switch_announced(
            ghz2_4(11),
            ChannelSwitchMode::Continue,
            switch_at
        )),
        [CoordinatorAction::AnnounceSwitch {
            target: ghz2_4(11),
            mode: ChannelSwitchMode::Continue,
            count: 3,
        }]
    );
    // The port moves once, when the access point's switch is due.
    let due = at(FIRST_TBTT + 3 * INTERVAL.as_micros());
    assert_eq!(
        actions(coordinator.access_point_switch_due(ghz2_4(11), due)),
        [CoordinatorAction::Retune {
            channel: ghz2_4(11),
            at: due,
        }]
    );
    assert!(
        coordinator
            .port_moved(ghz2_4(11))
            .into_iter()
            .next()
            .is_none()
    );
}

#[test]
fn a_move_before_the_access_point_s_next_beacon_stops_its_peers_for_one_beacon() {
    let mut coordinator = running(ApFollowPolicy::DEFAULT, ghz2_4(6));
    assert_eq!(
        actions(coordinator.upstream_switch_announced(
            ghz2_4(1),
            ChannelSwitchMode::Continue,
            at(FIRST_TBTT - 1)
        )),
        [CoordinatorAction::AnnounceSwitch {
            target: ghz2_4(1),
            mode: ChannelSwitchMode::StopTransmitting,
            count: 1,
        }]
    );
}

#[test]
fn a_station_alone_on_the_port_moves_with_the_upstream() {
    let mut coordinator = ChannelCoordinator::new(ApFollowPolicy::DEFAULT);
    assert!(
        coordinator
            .station_connected(ghz2_4(6))
            .into_iter()
            .next()
            .is_none()
    );
    let switch_at = at(5_000_000);
    assert_eq!(
        actions(coordinator.upstream_switch_announced(
            ghz5(36),
            ChannelSwitchMode::Continue,
            switch_at
        )),
        [CoordinatorAction::Retune {
            channel: ghz5(36),
            at: switch_at,
        }]
    );
}

#[test]
fn the_default_policy_stops_the_access_point_where_it_may_not_follow_and_starts_it_again() {
    let mut coordinator = running(ApFollowPolicy::DEFAULT, ghz2_4(6));
    // 5 GHz is outside the default bands: the access point stops, the
    // station follows the upstream alone.
    let switch_at = at(3_000_000);
    assert_eq!(
        actions(coordinator.upstream_switch_announced(
            ghz5(36),
            ChannelSwitchMode::Continue,
            switch_at
        )),
        [
            CoordinatorAction::StopAccessPoint,
            CoordinatorAction::Retune {
                channel: ghz5(36),
                at: switch_at,
            },
        ]
    );
    coordinator.access_point_stopped();
    assert!(
        coordinator
            .port_moved(ghz5(36))
            .into_iter()
            .next()
            .is_none()
    );
    // The upstream comes back to 2.4 GHz: the access point starts there.
    let back_at = at(9_000_000);
    assert_eq!(
        actions(coordinator.upstream_switch_announced(
            ghz2_4(1),
            ChannelSwitchMode::Continue,
            back_at
        )),
        [CoordinatorAction::Retune {
            channel: ghz2_4(1),
            at: back_at,
        }]
    );
    assert_eq!(
        actions(coordinator.port_moved(ghz2_4(1))),
        [CoordinatorAction::StartAccessPoint { channel: ghz2_4(1) }]
    );
}

#[test]
fn a_channel_that_needs_radar_detection_is_never_served_even_in_an_allowed_band() {
    let both = ApFollowPolicy {
        bands: ApBands {
            ghz2_4: true,
            ghz5: true,
        },
        unservable: ApUnservable::LeaveUpstream,
        announce_count: NonZeroU8::new(5).unwrap(),
        min_follow_interval: Duration::from_secs(30),
        max_follow_interval: Duration::from_secs(480),
    };
    let mut coordinator = running(both, ghz5(36));
    assert_eq!(
        actions(coordinator.upstream_switch_announced(
            ghz5(149),
            ChannelSwitchMode::Continue,
            at(FIRST_TBTT + 5 * INTERVAL.as_micros())
        )),
        [CoordinatorAction::AnnounceSwitch {
            target: ghz5(149),
            mode: ChannelSwitchMode::Continue,
            count: 5,
        }]
    );
    // Channel 100 shares its air with radar: the policy keeps the access
    // point and the station leaves the upstream.
    assert_eq!(
        actions(coordinator.upstream_switch_announced(
            ghz5(100),
            ChannelSwitchMode::Continue,
            at(FIRST_TBTT + 5 * INTERVAL.as_micros())
        )),
        [CoordinatorAction::LeaveUpstream]
    );
}

#[test]
fn a_lost_upstream_found_elsewhere_moves_the_access_point_first() {
    let mut coordinator = running(ApFollowPolicy::DEFAULT, ghz2_4(6));
    coordinator.upstream_lost(at(FIRST_TBTT));
    assert_eq!(
        actions(coordinator.upstream_found(ghz2_4(11))),
        [CoordinatorAction::AnnounceSwitch {
            target: ghz2_4(11),
            mode: ChannelSwitchMode::Continue,
            count: 5,
        }]
    );
    // The search is over.
    assert_eq!(coordinator.search_since(), None);
    let due = at(FIRST_TBTT + 5 * INTERVAL.as_micros());
    assert_eq!(
        actions(coordinator.access_point_switch_due(ghz2_4(11), due)),
        [CoordinatorAction::Retune {
            channel: ghz2_4(11),
            at: due,
        }]
    );
    assert!(
        coordinator
            .port_moved(ghz2_4(1))
            .into_iter()
            .next()
            .is_none()
    );
    assert!(
        coordinator
            .port_moved(ghz2_4(11))
            .into_iter()
            .next()
            .is_none()
    );
    assert!(
        coordinator
            .port_moved(ghz2_4(11))
            .into_iter()
            .next()
            .is_none()
    );
    assert!(
        coordinator
            .station_connected(ghz2_4(11))
            .into_iter()
            .next()
            .is_none()
    );

    // Found on the access point's own channel, the station just joins.
    coordinator.upstream_lost(at(FIRST_TBTT));
    assert!(
        coordinator
            .upstream_found(ghz2_4(11))
            .into_iter()
            .next()
            .is_none()
    );
}

#[test]
fn the_loss_policy_is_due_its_after_past_the_loss_only_while_the_access_point_runs() {
    let policy = UpstreamLossPolicy::DEFAULT;
    let mut coordinator = running(ApFollowPolicy::DEFAULT, ghz2_4(6));
    // Connected: nothing is due.
    assert_eq!(coordinator.upstream_loss_due(policy), None);
    let lost = at(FIRST_TBTT + 7);
    coordinator.upstream_lost(lost);
    assert_eq!(
        coordinator.upstream_loss_due(policy),
        lost.checked_add(Duration::from_secs(30))
    );
    // Found again before then: the policy no longer runs.
    let found = actions(coordinator.upstream_found(ghz2_4(6)));
    assert!(found.is_empty());
    assert_eq!(coordinator.upstream_loss_due(policy), None);
    // A refused join keeps the original loss instant.
    coordinator.join_failed();
    assert_eq!(
        coordinator.upstream_loss_due(policy),
        lost.checked_add(Duration::from_secs(30))
    );
}

#[test]
fn a_rescan_that_finds_no_upstream_restarts_the_access_point_on_its_channel_and_counts_again() {
    let policy = UpstreamLossPolicy::DEFAULT;
    let mut coordinator = running(ApFollowPolicy::DEFAULT, ghz2_4(6));
    coordinator.upstream_lost(at(FIRST_TBTT));
    assert_eq!(
        actions(coordinator.upstream_rescan()),
        [CoordinatorAction::StopAccessPoint]
    );
    // The station scans alone: nothing is due meanwhile.
    assert_eq!(coordinator.upstream_loss_due(policy), None);
    assert!(coordinator.upstream_rescan().into_iter().next().is_none());
    let failed = at(FIRST_TBTT + 31_000_000);
    assert_eq!(
        actions(coordinator.upstream_rescan_failed(failed)),
        [CoordinatorAction::Retune {
            channel: ghz2_4(6),
            at: failed,
        }]
    );
    assert!(actions(coordinator.port_moved(ghz2_4(11))).is_empty());
    assert_eq!(
        actions(coordinator.port_moved(ghz2_4(6))),
        [CoordinatorAction::StartAccessPoint { channel: ghz2_4(6) }]
    );
    assert!(actions(coordinator.port_moved(ghz2_4(6))).is_empty());
    // Not due before the access point runs again, then `after` past the
    // failed scan.
    assert_eq!(coordinator.upstream_loss_due(policy), None);
    coordinator.access_point_running(schedule(ghz2_4(6)));
    assert_eq!(
        coordinator.upstream_loss_due(policy),
        failed.checked_add(policy.after)
    );
}

#[test]
fn a_rescan_that_joins_the_upstream_starts_the_access_point_on_its_new_channel() {
    let mut coordinator = running(ApFollowPolicy::DEFAULT, ghz2_4(6));
    coordinator.upstream_lost(at(FIRST_TBTT));
    assert_eq!(
        actions(coordinator.upstream_rescan()),
        [CoordinatorAction::StopAccessPoint]
    );
    assert_eq!(
        actions(coordinator.station_connected(ghz2_4(11))),
        [CoordinatorAction::StartAccessPoint {
            channel: ghz2_4(11)
        }]
    );
    assert_eq!(
        coordinator.upstream_loss_due(UpstreamLossPolicy::DEFAULT),
        None
    );
    // No rescan-failure path is left to take.
    assert!(
        coordinator
            .upstream_rescan_failed(at(FIRST_TBTT))
            .into_iter()
            .next()
            .is_none()
    );
}

#[test]
fn every_nonzero_follow_count_reaches_both_move_paths_unchanged() {
    for count in 1..=u8::MAX {
        let policy = ApFollowPolicy {
            announce_count: NonZeroU8::new(count).unwrap(),
            ..ApFollowPolicy::DEFAULT
        };
        let mut coordinator = running(policy, ghz2_4(6));
        assert_eq!(
            coordinator.station_joining_away(ghz2_4(11), at(FIRST_TBTT)),
            JoinAway::Follow { count }
        );
        coordinator.upstream_lost(at(FIRST_TBTT));
        assert_eq!(
            actions(coordinator.upstream_found(ghz2_4(11))),
            [CoordinatorAction::AnnounceSwitch {
                target: ghz2_4(11),
                mode: ChannelSwitchMode::Continue,
                count,
            }]
        );
    }
}
