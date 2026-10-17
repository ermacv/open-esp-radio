use super::*;

#[test]
fn each_activity_replaces_the_wifi_status_with_its_own_bit() {
    assert_eq!(WifiCoexActivity::Idle.status_update().status, 0);
    assert_eq!(
        WifiCoexActivity::Scanning.status_update().status,
        wifi_status::SCAN
    );
    assert_eq!(
        WifiCoexActivity::Connecting {
            reconnecting: false
        }
        .status_update()
        .status,
        wifi_status::CONNECTING
    );
    assert_eq!(
        WifiCoexActivity::Connected {
            beacon_interval_tu: 100
        }
        .status_update()
        .status,
        wifi_status::CONNECTED
    );
}

#[test]
fn the_interval_follows_the_activity_and_a_connected_beacon_interval() {
    let interval = |activity: WifiCoexActivity| activity.status_update().interval;
    assert_eq!(interval(WifiCoexActivity::Idle), 1_000);
    assert_eq!(interval(WifiCoexActivity::Scanning), 100);
    assert_eq!(
        interval(WifiCoexActivity::Connecting {
            reconnecting: false
        }),
        100
    );
    assert_eq!(
        interval(WifiCoexActivity::Connecting { reconnecting: true }),
        240
    );
    // 100 TU = 102.4 ms, in whole 100 µs units.
    assert_eq!(
        interval(WifiCoexActivity::Connected {
            beacon_interval_tu: 100
        }),
        1_024
    );
    // An access point without a beacon interval runs on 100 TU.
    assert_eq!(
        interval(WifiCoexActivity::Connected {
            beacon_interval_tu: 0
        }),
        1_024
    );
    assert_eq!(
        interval(WifiCoexActivity::Connected {
            beacon_interval_tu: 200
        }),
        2_048
    );
}

#[test]
fn only_a_connected_station_leaves_the_restart_to_its_beacon() {
    for activity in [
        WifiCoexActivity::Idle,
        WifiCoexActivity::Scanning,
        WifiCoexActivity::Connecting {
            reconnecting: false,
        },
    ] {
        assert!(activity.status_update().restart_when_shared);
    }
    assert!(
        !WifiCoexActivity::Connected {
            beacon_interval_tu: 100
        }
        .status_update()
        .restart_when_shared
    );
}

#[test]
fn a_shared_scan_dwells_ten_milliseconds_per_period() {
    assert_eq!(shared_scan_dwell_millis(20, 6), 60);
    assert_eq!(shared_scan_dwell_millis(120, 6), 60);
    // Longer requests scale with the requested dwell over 120 ms.
    assert_eq!(shared_scan_dwell_millis(240, 6), 120);
    assert_eq!(shared_scan_dwell_millis(360, 1), 30);
}

#[test]
fn a_phy_request_maps_to_the_vendor_primary_and_secondary_offset() {
    assert_eq!(
        WifiCoexChannel::from_phy_request(6, 0),
        Some(WifiCoexChannel {
            primary: 6,
            secondary: 0
        })
    );
    // Channel 13 HT40-: centre 2462 MHz, primary 2472 MHz.
    assert_eq!(
        WifiCoexChannel::from_phy_request(2_462, 3),
        Some(WifiCoexChannel {
            primary: 13,
            secondary: 2
        })
    );
    // Channel 1 HT40+: centre 2422 MHz, primary 2412 MHz.
    assert_eq!(
        WifiCoexChannel::from_phy_request(2_422, 2),
        Some(WifiCoexChannel {
            primary: 1,
            secondary: 1
        })
    );
    assert_eq!(WifiCoexChannel::from_phy_request(0, 0), None);
    assert_eq!(WifiCoexChannel::from_phy_request(2_463, 3), None);
    assert_eq!(WifiCoexChannel::from_phy_request(2_462, 1), None);
}
