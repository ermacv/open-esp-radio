use super::*;
use oer_ieee80211_mac::station_beacon::StaBeaconProtection;

const BEACON: StaBeaconObservation = StaBeaconObservation {
    timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(10),
    interval_tu: 100,
    capability_information: 0,
    tim: None,
    protection: StaBeaconProtection::UNPROTECTED,
    channel_switch: None,
};

#[test]
fn exact_deadline_is_lost_but_an_observation_refreshes_the_window() {
    let config =
        StaBeaconLossConfig::new(100, 3, oer_time::Duration::from_micros(307_200)).unwrap();
    let mut monitor = StaBeaconMonitor::new(config);
    monitor.arm(oer_time::Instant::from_micros(1_000)).unwrap();
    assert_eq!(
        monitor.deadline(),
        Some(oer_time::Instant::from_micros(308_200))
    );
    assert!(!monitor.expired(oer_time::Instant::from_micros(308_199)));

    monitor
        .observe(oer_time::Instant::from_micros(308_200), BEACON)
        .unwrap();
    assert!(!monitor.expired(oer_time::Instant::from_micros(308_200)));
    assert_eq!(
        monitor.deadline(),
        Some(oer_time::Instant::from_micros(615_400))
    );
    assert_eq!(monitor.observed(), 1);
    assert_eq!(monitor.last_observation(), Some(BEACON));
    assert!(monitor.expired(oer_time::Instant::from_micros(615_400)));
}

#[test]
fn construction_rejects_unbounded_or_vacuous_policy() {
    assert_eq!(
        StaBeaconLossConfig::new(0, 3, oer_time::Duration::from_micros(1)),
        Err(StaBeaconLossConfigError::ZeroInterval)
    );
    assert_eq!(
        StaBeaconLossConfig::new(100, 0, oer_time::Duration::from_micros(1)),
        Err(StaBeaconLossConfigError::ZeroMissLimit)
    );
    assert_eq!(
        StaBeaconLossConfig::new(100, 3, oer_time::Duration::from_micros(0)),
        Err(StaBeaconLossConfigError::ZeroTimeout)
    );
}

#[test]
fn active_reachability_refresh_does_not_fabricate_a_beacon() {
    let config =
        StaBeaconLossConfig::new(100, 3, oer_time::Duration::from_micros(307_200)).unwrap();
    let mut monitor = StaBeaconMonitor::new(config);
    monitor.arm(oer_time::Instant::from_micros(1_000)).unwrap();
    monitor
        .observe_reachability(oer_time::Instant::from_micros(308_200))
        .unwrap();

    assert_eq!(
        monitor.deadline(),
        Some(oer_time::Instant::from_micros(615_400))
    );
    assert_eq!(monitor.observed(), 0);
    assert_eq!(monitor.last_observation(), None);
}

fn link() -> StaLinkMonitor {
    StaLinkMonitor::new(
        StaBeaconLossConfig::new(100, 10, Duration::from_secs(6)).unwrap(),
        StaLinkProbePolicy {
            interval: Duration::from_millis(500),
            attempts: 3,
            directed: 2,
        },
    )
}

fn at(millis: u64) -> Instant {
    Instant::from_micros(millis * 1_000)
}

#[test]
fn silent_beacons_lead_to_bounded_probes_then_loss() {
    let mut link = link();
    link.arm(at(0)).unwrap();
    assert_eq!(link.due(at(5_999)), None);
    assert_eq!(
        link.due(at(6_000)),
        Some(StaLinkAction::Probe { directed: true })
    );
    link.probe_sent(at(6_000)).unwrap();
    assert_eq!(link.deadline(), Some(at(6_500)));
    assert_eq!(link.due(at(6_499)), None);
    assert_eq!(
        link.due(at(6_500)),
        Some(StaLinkAction::Probe { directed: true })
    );
    link.probe_sent(at(6_500)).unwrap();
    // Past the directed probes the station broadcasts.
    assert_eq!(
        link.due(at(7_000)),
        Some(StaLinkAction::Probe { directed: false })
    );
    link.probe_sent(at(7_000)).unwrap();
    assert_eq!(link.probes_sent(), 3);
    assert_eq!(link.due(at(7_500)), Some(StaLinkAction::Lost));
}

#[test]
fn a_beacon_or_a_probe_response_keeps_the_link() {
    let mut link = link();
    link.arm(at(0)).unwrap();
    link.probe_sent(at(6_000)).unwrap();
    // The access point answers the probe: a fresh beacon window, no probes.
    link.observe_probe_response(at(6_100)).unwrap();
    assert_eq!(link.probes_sent(), 0);
    assert_eq!(link.deadline(), Some(at(12_100)));
    assert_eq!(link.beacons().last_observation(), None);
    link.probe_sent(at(12_100)).unwrap();
    // A beacon does the same and is kept as the last observation.
    link.observe_beacon(at(12_200), BEACON).unwrap();
    assert_eq!(link.probes_sent(), 0);
    assert_eq!(link.due(at(18_199)), None);
    assert_eq!(link.beacons().last_observation(), Some(BEACON));
}
