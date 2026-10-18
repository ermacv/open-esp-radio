use super::*;
use crate::wifi::test_support::{
    AP_TEST_CAPABILITIES, STA_AP_TEST_CAPABILITIES, TEST_CAPABILITIES,
};
use oer_ieee80211_softmac::{WifiConfig, WifiMacAddress, WifiStationConfig};
use {
    oer_ieee80211_mac::channel::WifiChannelWidth,
    oer_ieee80211_mac::station::association::Preference,
};

fn station_request() -> StationRequest {
    StationRequest::new(
        WifiSsid::new(b"test-network").unwrap(),
        StationSecurity::wpa2_personal(b"password", b"test-network").unwrap(),
        StaReconnectPolicy::new(3, 100, 1_000, 100).unwrap(),
        StationScanPolicy::new(
            StationScanChannels::CHANNELS_1_TO_13,
            NonZeroU16::new(20).unwrap(),
            Preference::Automatic,
        ),
    )
}

fn access_point_request() -> AccessPointRequest {
    AccessPointRequest::new(
        WifiSsid::new(b"test-access-point").unwrap(),
        AccessPointSecurity::wpa2_personal(Pmk::derive(b"password", b"test-access-point").unwrap()),
        WifiChannel::mhz20(6).unwrap(),
        AccessPointClientLimit::new(4).unwrap(),
    )
    .unwrap()
}

#[test]
fn station_debug_never_formats_key_material() {
    let request = station_request();
    let debug = std::format!("{request:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("password"));
}

#[test]
fn access_point_request_is_narrow_and_never_formats_key_material() {
    let request = access_point_request();
    assert_eq!(request.channel().primary(), 6);
    assert_eq!(AccessPointRequest::BEACON_INTERVAL_TU, 100);
    assert_eq!(AccessPointRequest::DTIM_PERIOD, 2);
    assert_eq!(AccessPointRequest::PEER_CAPACITY, 15);
    assert_eq!(request.client_limit().get(), 4);
    assert_eq!(request.inactive_timeout().seconds(), 300);
    let debug = std::format!("{request:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("password"));

    let wide = AccessPointRequest::new(
        WifiSsid::new(b"wide").unwrap(),
        AccessPointSecurity::wpa2_personal(Pmk::derive(b"password", b"wide").unwrap()),
        WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap(),
        AccessPointClientLimit::new(4).unwrap(),
    )
    .unwrap();
    assert_eq!(wide.channel().width(), WifiChannelWidth::Mhz40Above);

    let request = access_point_request().with_inactive_timeout(
        AccessPointInactiveTimeout::new(30).expect("valid inactivity timeout"),
    );
    assert_eq!(request.inactive_timeout().seconds(), 30);
    let (_, _, _, _, timeout, beacon_interval, dtim_period) = request.into_parts();
    assert_eq!(timeout.seconds(), 30);
    assert_eq!(beacon_interval.tu(), AccessPointRequest::BEACON_INTERVAL_TU);
    assert_eq!(dtim_period.get(), AccessPointRequest::DTIM_PERIOD);
}

#[test]
fn combined_request_requires_one_checked_same_channel_owner_graph() {
    let station = WifiStationConfig::new(WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap());
    let access_point =
        WifiAccessPointConfig::new(WifiMacAddress::new([0x02, 0, 0, 0, 0, 2]).unwrap());
    let configuration = WifiSupervisorConfiguration::new(STA_AP_TEST_CAPABILITIES)
        .with_station(station)
        .with_access_point(access_point);
    let service = configuration
        .plan_station_access_point(StationAccessPointRequest::new(
            station_request(),
            access_point_request(),
        ))
        .unwrap();

    assert!(service.plan().station().is_some());
    assert!(service.plan().access_point().is_some());
    assert_eq!(
        service
            .station_access_point_request()
            .unwrap()
            .required_channel(),
        WifiChannel::mhz20(6).unwrap()
    );
}

#[test]
fn monitor_request_separates_capture_from_dma_capacity() {
    let request = MonitorRequest::new(
        WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz20).unwrap(),
        WifiMonitorConfig::normalized(),
    )
    .with_capture_policy(MonitorCapturePolicy::truncate_at(
        NonZeroU16::new(512).unwrap(),
    ));
    assert_eq!(request.channel().primary(), 6);
    assert_eq!(request.capture_policy().snapshot_length(), Some(512));
}

#[test]
fn service_request_checks_runtime_policy_against_topology() {
    let address = WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap();
    let station_plan = WifiConfig::station(WifiStationConfig::new(address))
        .validate(TEST_CAPABILITIES)
        .unwrap();
    let service = WifiServiceRequest::station(station_plan, station_request()).unwrap();
    assert!(service.station_request().is_some());
    assert!(service.monitor_request().is_none());

    let channel = WifiChannel::mhz20(6).unwrap();
    let monitor = MonitorRequest::new(channel, WifiMonitorConfig::normalized());
    let failure = WifiServiceRequest::standalone_monitor(station_plan, monitor).unwrap_err();
    assert_eq!(
        failure.error,
        WifiServiceRequestError::NotStandaloneMonitorTopology
    );
    assert_eq!(failure.request.channel().primary(), 6);
}

#[test]
fn supervisor_provisions_sequential_station_and_monitor_topologies() {
    let address = WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap();
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES)
        .with_station(WifiStationConfig::new(address))
        .with_standalone_monitor();

    let station = configuration.plan_station(station_request()).unwrap();
    assert!(station.plan().station().is_some());
    assert!(station.plan().monitor().is_none());

    let monitor_request = MonitorRequest::new(
        WifiChannel::mhz20(11).unwrap(),
        WifiMonitorConfig::normalized(),
    );
    let monitor = configuration.plan_monitor(monitor_request).unwrap();
    assert!(monitor.plan().station().is_none());
    assert!(monitor.plan().standalone_monitor().is_some());
}

#[test]
fn supervisor_provisions_an_exclusive_access_point_epoch() {
    let address = WifiMacAddress::new([0x02, 0, 0, 0, 0, 2]).unwrap();
    let configuration = WifiSupervisorConfiguration::new(AP_TEST_CAPABILITIES)
        .with_access_point(WifiAccessPointConfig::new(address));

    let service = configuration
        .plan_access_point(access_point_request())
        .unwrap();
    assert!(service.plan().station().is_none());
    assert!(service.plan().access_point().is_some());
    assert!(service.access_point_request().is_some());
}

#[test]
fn supervisor_provisions_finite_scan_on_the_station_owner_graph() {
    let address = WifiMacAddress::new([0x02, 0, 0, 0, 0, 1]).unwrap();
    let request = WifiScanRequest::new(
        StationScanChannels::CHANNELS_1_TO_13,
        NonZeroU16::new(20).unwrap(),
    );
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES)
        .with_station(WifiStationConfig::new(address))
        .with_standalone_scan();

    let scan = configuration.plan_scan(request).unwrap();
    assert!(scan.plan().station().is_some());
    assert!(scan.scan_request().is_some());
    assert!(scan.station_request().is_none());
}

#[test]
fn unprovisioned_scan_is_rejected_before_materialization() {
    let request = WifiScanRequest::new(
        StationScanChannels::CHANNELS_1_TO_13,
        NonZeroU16::new(37).unwrap(),
    );
    let failure = WifiSupervisorConfiguration::new(TEST_CAPABILITIES)
        .plan_scan(request)
        .unwrap_err();
    assert_eq!(failure.error, WifiServicePlanningError::ScanNotProvisioned);
    assert_eq!(failure.request.dwell_millis(), 37);
}

#[test]
fn unprovisioned_role_rejection_returns_the_exact_request() {
    let configuration = WifiSupervisorConfiguration::new(TEST_CAPABILITIES);
    let request = MonitorRequest::new(
        WifiChannel::mhz20(9).unwrap(),
        WifiMonitorConfig::normalized(),
    );
    let failure = configuration.plan_monitor(request).unwrap_err();
    assert_eq!(
        failure.error,
        WifiServicePlanningError::MonitorNotProvisioned
    );
    assert_eq!(failure.request.channel().primary(), 9);
}
