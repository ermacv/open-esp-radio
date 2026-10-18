use super::*;

#[test]
fn parses_tcpdump_summary_without_using_received_by_filter() {
    let summary =
        "12 packets captured\n25 packets received by filter\n0 packets dropped by kernel\n";
    assert_eq!(parse_summary_value(summary, "packets captured"), Some(12));
    assert_eq!(
        parse_summary_value(summary, "packets dropped by kernel"),
        Some(0)
    );
}

#[test]
fn counter_reset_is_not_misreported_as_a_wrapping_delta() {
    assert_eq!(delta("counter", 10, 14).unwrap(), 4);
    assert!(delta("counter", 14, 10).is_err());
}

#[test]
fn station_mac_is_strictly_parsed_for_bridge_capture_reuse() {
    assert_eq!(
        parse_mac("xx:xx:xx:xx:f6:d0").unwrap(),
        [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0]
    );
    assert!(parse_mac("02:00:00:00:f6").is_err());
    assert!(parse_mac("xx:xx:xx:xx:f6:d0:00").is_err());
    assert!(parse_mac("02:00:00:00:f6:zz").is_err());
}

#[test]
fn ceiling_rejects_busy_pre_workload_channel() {
    require_pre_workload_channel_utilization(Some(64), 64).unwrap();
    assert!(require_pre_workload_channel_utilization(Some(64), 65).is_err());
    require_pre_workload_channel_utilization(None, 255).unwrap();
    assert_eq!(scale_channel_utilization(12_002, 2_837).unwrap(), 61);
    assert_eq!(scale_channel_utilization(12_002, 3_790).unwrap(), 81);
    assert!(scale_channel_utilization(0, 0).is_err());
    assert!(scale_channel_utilization(10, 11).is_err());
}
