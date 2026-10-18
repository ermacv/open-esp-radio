use core::num::NonZeroU16;

use oer_ieee80211_mac::station::association::Preference;

use oer_ieee80211_sta::request::{StationScanChannels, StationScanPolicy};

use super::*;

#[test]
fn owned_scan_plan_is_the_only_source_of_discovery_policy() {
    let discovery = StationDiscovery::new(
        WifiSsid::new(b"portable").unwrap(),
        StationScanPolicy::new(
            StationScanChannels::from_primary_channels(&[1, 6, 11]).unwrap(),
            NonZeroU16::new(40).unwrap(),
            Preference::Automatic,
        ),
    );
    let plan = StationScanPlan::new(discovery, Some(11));
    assert_eq!(plan.channels(), [11, 1, 6]);
    assert_eq!(plan.target_ssid(), b"portable");
    let request = plan.request([2, 0, 0, 0, 0, 1]);
    assert_eq!(request.config.dwell_ticks(), 40);
    assert_eq!(request.supported_rates, ESP32S31_STATION_PROBE_RATES);
    assert_eq!(
        request.descriptor_capacity,
        Some(ESP32S31_STATION_PROBE_DESCRIPTOR_CAPACITY)
    );
}

#[test]
fn scan_failure_policy_is_terminal_only_at_unsafe_owner_frontiers() {
    assert_eq!(
        esp32s31_station_scan_failure_disposition(&StaScanError::ActiveProbe(ScanPortError::<
            u8,
            u8,
            u8,
        >::Transmit(
            1
        ),)),
        StaFailureDisposition::Terminal,
    );
    assert_eq!(
        esp32s31_station_scan_failure_disposition(&StaScanError::ReceiveStop(ScanPortError::<
            u8,
            u8,
            u8,
        >::Receive(2),)),
        StaFailureDisposition::Terminal,
    );
    assert_eq!(
        esp32s31_station_scan_failure_disposition(&StaScanError::ChannelSwitch(ScanPortError::<
            u8,
            u8,
            u8,
        >::ChannelSwitch(
            3
        ),)),
        StaFailureDisposition::RefreshCandidate,
    );
}
