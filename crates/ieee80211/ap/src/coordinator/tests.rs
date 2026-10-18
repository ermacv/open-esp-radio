use oer_ieee80211_mac::channel::ChannelWidth;
use std::vec::Vec;

use super::*;

const INTERVAL: Duration = Duration::from_micros(102_400);
const FIRST_TBTT: u64 = 1_000_000;

fn ghz2_4(number: u8) -> Channel {
    Channel::ghz2_4(number, ChannelWidth::Mhz20).unwrap()
}

fn ghz5(number: u8) -> Channel {
    Channel::ghz5(number, ChannelWidth::Mhz20).unwrap()
}

const fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

fn schedule(channel: Channel) -> ApSchedule {
    ApSchedule {
        channel,
        next_tbtt: at(FIRST_TBTT),
        beacon_interval: INTERVAL,
    }
}

fn actions(actions: CoordinatorActions) -> Vec<CoordinatorAction> {
    actions.into_iter().collect()
}

static SEARCHED: [Channel; 3] = [
    match Channel::ghz2_4(1, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!(),
    },
    match Channel::ghz2_4(6, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!(),
    },
    match Channel::ghz2_4(11, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!(),
    },
];

/// A coordinator whose access point runs beside a station on `channel`.
fn running(policy: ApFollowPolicy, channel: Channel) -> ChannelCoordinator<'static> {
    let mut coordinator = ChannelCoordinator::new(policy, ApSearchPolicy::new(&SEARCHED));
    // Wanted before the station connects, the access point waits for the
    // upstream's channel.
    assert!(coordinator.want_access_point().is_empty());
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
    assert!(coordinator.port_moved(ghz2_4(11)).is_empty());
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
    let mut coordinator =
        ChannelCoordinator::new(ApFollowPolicy::DEFAULT, ApSearchPolicy::new(&SEARCHED));
    assert!(coordinator.station_connected(ghz2_4(6)).is_empty());
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
    assert!(coordinator.port_moved(ghz5(36)).is_empty());
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
        announce_count: 5,
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
fn a_lost_upstream_is_searched_densely_then_sparsely_around_the_access_point_s_beacons() {
    let mut coordinator = running(ApFollowPolicy::DEFAULT, ghz2_4(6));
    let lost = at(FIRST_TBTT - 50_000);
    coordinator.upstream_lost(lost);
    let search = ApSearchPolicy::new(&SEARCHED);
    // The first absence follows the next TBTT by the guard.
    let first = at(FIRST_TBTT + search.guard.as_micros());
    assert_eq!(coordinator.poll(lost), None);
    assert_eq!(coordinator.next_deadline(), Some(first));
    let mut absences = Vec::new();
    let mut now = first;
    while now < at(FIRST_TBTT + 35_000_000) {
        if let Some(CoordinatorAction::Absence {
            channel,
            start,
            until,
        }) = coordinator.poll(now)
        {
            assert_eq!(start, now);
            assert_eq!(until.saturating_duration_since(start), search.dwell);
            absences.push((start, channel));
        }
        now = coordinator.next_deadline().unwrap();
    }
    // Every absence starts the guard after a TBTT and never visits the
    // access point's channel; the channels take turns.
    for (start, _) in &absences {
        let since_first = start.saturating_duration_since(first).as_micros();
        assert_eq!(since_first % INTERVAL.as_micros(), 0);
    }
    assert!(absences.iter().all(|(_, channel)| *channel != ghz2_4(6)));
    assert_eq!(absences[0].1, ghz2_4(1));
    assert_eq!(absences[1].1, ghz2_4(11));
    assert_eq!(absences[2].1, ghz2_4(1));
    // Dense for 30 s: one absence each interval, then about one a second.
    let dense_end = lost.checked_add(search.dense_period).unwrap();
    let dense = absences
        .iter()
        .filter(|(start, _)| *start < dense_end)
        .count();
    let dense_expected =
        dense_end.saturating_duration_since(first).as_micros() / INTERVAL.as_micros() + 1;
    assert!(
        dense.abs_diff(dense_expected as usize) <= 1,
        "{dense} {dense_expected}"
    );
    let sparse: Vec<_> = absences
        .iter()
        .filter(|(start, _)| *start >= dense_end)
        .collect();
    assert!((4..=6).contains(&sparse.len()), "{}", sparse.len());
    for pair in sparse.windows(2) {
        let gap = pair[1].0.saturating_duration_since(pair[0].0);
        assert!(gap >= search.sparse_interval);
        assert!(gap < search.sparse_interval.checked_add(INTERVAL).unwrap());
    }
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
    assert_eq!(coordinator.next_deadline(), None);
    let due = at(FIRST_TBTT + 5 * INTERVAL.as_micros());
    assert_eq!(
        actions(coordinator.access_point_switch_due(ghz2_4(11), due)),
        [CoordinatorAction::Retune {
            channel: ghz2_4(11),
            at: due,
        }]
    );
    assert!(coordinator.port_moved(ghz2_4(1)).is_empty());
    assert_eq!(
        actions(coordinator.port_moved(ghz2_4(11))),
        [CoordinatorAction::JoinUpstream {
            channel: ghz2_4(11)
        }]
    );
    assert!(coordinator.port_moved(ghz2_4(11)).is_empty());
    assert!(coordinator.station_connected(ghz2_4(11)).is_empty());

    // Found on the access point's own channel, the station just joins.
    coordinator.upstream_lost(at(FIRST_TBTT));
    assert_eq!(
        actions(coordinator.upstream_found(ghz2_4(11))),
        [CoordinatorAction::JoinUpstream {
            channel: ghz2_4(11)
        }]
    );
}

#[test]
fn an_empty_search_list_waits_on_the_access_point_s_channel() {
    let mut coordinator =
        ChannelCoordinator::new(ApFollowPolicy::DEFAULT, ApSearchPolicy::new(&[]));
    coordinator.want_access_point();
    coordinator.station_connected(ghz2_4(6));
    coordinator.access_point_running(schedule(ghz2_4(6)));
    coordinator.upstream_lost(at(0));
    assert_eq!(coordinator.poll(at(FIRST_TBTT + 10_000)), None);
    assert_eq!(coordinator.next_deadline(), None);
}

#[test]
fn search_skips_the_home_primary_even_when_the_port_is_wider() {
    let home = Channel::ghz2_4(6, ChannelWidth::Mhz40Above).unwrap();
    let mut coordinator = running(ApFollowPolicy::DEFAULT, home);
    coordinator.upstream_lost(at(FIRST_TBTT));
    assert_eq!(coordinator.poll(at(FIRST_TBTT)), None);
    for expected in [ghz2_4(1), ghz2_4(11), ghz2_4(1)] {
        let now = coordinator.next_deadline().unwrap();
        assert!(
            matches!(coordinator.poll(now), Some(CoordinatorAction::Absence { channel, .. }) if channel == expected)
        );
    }
}

#[test]
fn a_late_owner_skips_expired_windows_without_replaying_them() {
    let mut coordinator = running(ApFollowPolicy::DEFAULT, ghz2_4(6));
    coordinator.upstream_lost(at(FIRST_TBTT - 1));
    assert_eq!(coordinator.poll(at(FIRST_TBTT - 1)), None);
    let late = at(FIRST_TBTT + 5 * INTERVAL.as_micros() + 30_000);
    assert_eq!(coordinator.poll(late), None);
    let next = coordinator.next_deadline().unwrap();
    assert!(next > late);
    assert!(next < late.checked_add(INTERVAL).unwrap());
    assert!(
        matches!(coordinator.poll(next), Some(CoordinatorAction::Absence { channel, start, .. })
        if channel == ghz2_4(1) && start == next)
    );
}

#[test]
fn a_failed_join_keeps_the_original_search_density() {
    let mut coordinator = running(ApFollowPolicy::DEFAULT, ghz2_4(6));
    coordinator.upstream_lost(at(FIRST_TBTT));
    assert!(coordinator.searching());
    coordinator.upstream_found(ghz2_4(6));
    assert!(!coordinator.searching());
    coordinator.upstream_join_failed();
    assert!(coordinator.searching());
    let now = at(FIRST_TBTT + 31_000_000);
    assert_eq!(coordinator.poll(now), None);
    let start = coordinator.next_deadline().unwrap();
    assert!(coordinator.poll(start).is_some());
    let gap = coordinator
        .next_deadline()
        .unwrap()
        .saturating_duration_since(start);
    assert!(gap >= Duration::from_secs(1));
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
    assert_eq!(
        found,
        [CoordinatorAction::JoinUpstream { channel: ghz2_4(6) }]
    );
    assert_eq!(coordinator.upstream_loss_due(policy), None);
    // A refused join keeps the original loss instant.
    coordinator.upstream_join_failed();
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
    assert!(coordinator.upstream_rescan().is_empty());
    let failed = at(FIRST_TBTT + 31_000_000);
    assert_eq!(
        actions(coordinator.upstream_rescan_failed(failed)),
        [
            CoordinatorAction::Retune {
                channel: ghz2_4(6),
                at: failed,
            },
            CoordinatorAction::StartAccessPoint { channel: ghz2_4(6) },
        ]
    );
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
            .is_empty()
    );
}
