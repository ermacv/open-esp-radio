use super::*;

#[test]
fn parses_only_an_associated_mac_identity() {
    assert_eq!(
        parse_bssid("Connected to xx:xx:xx:xx:f6:d1 (on wlan0)\n").unwrap(),
        "xx:xx:xx:xx:f6:d1"
    );
    assert!(parse_bssid("Not connected.\n").is_err());
    assert!(parse_bssid("Connected to not-a-mac (on wlan0)\n").is_err());
}

#[test]
fn unknown_states_are_not_misreported_as_discovery_failures() {
    assert_eq!(connection_stage(Some("DISCONNECTED")), "unknown");
    assert_eq!(connection_stage(None), "unknown");
    assert_eq!(connection_stage(Some("4WAY_HANDSHAKE")), "key-negotiation");
}

#[test]
fn every_cycle_contributes_its_associated_station_address_once() {
    let output = tempfile::tempdir().unwrap();
    let record = |cycle: &str, address: Option<&str>| {
        let directory = output.path().join(cycle).join("linux-client");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("connection.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": 2, "connected": true, "station_address": address,
            }))
            .unwrap(),
        )
        .unwrap();
    };
    record("cycle-00", Some("02:00:00:00:48:F0"));
    record("cycle-01", Some("02:00:00:00:48:f0"));
    record("cycle-02", None);
    assert_eq!(
        connected_station_addresses(output.path()).unwrap(),
        ["02:00:00:00:48:f0"]
    );
    record("cycle-03", Some("02:00:00:00:dc:c3"));
    assert_eq!(
        connected_station_addresses(output.path()).unwrap(),
        ["02:00:00:00:dc:c3", "02:00:00:00:48:f0"]
    );
}

const STATION_DUMP: &str = "Station 02:00:00:00:f6:d0 (on wlan0)
	inactive time:	12 ms
	rx bytes:	2850112
	rx packets:	4512
	tx bytes:	3002910
	tx packets:	2391
	tx retries:	87
	tx failed:	2
	rx drop misc:	1
	signal:  	-41 dBm
	tx bitrate:	150.0 MBit/s MCS 7 40MHz short GI
	rx bitrate:	135.0 MBit/s MCS 7 40MHz
";

const AQM: &str = "tid0_aqm_drops=3\ntid0_aqm_overlimit=1\n";

#[test]
fn the_laptop_client_link_snapshot_reads_its_one_station() {
    let snapshot = LaptopLinkSnapshot::parse(STATION_DUMP, AQM, 5).unwrap();
    assert_eq!(
        (
            snapshot.tx_packets,
            snapshot.tx_retries,
            snapshot.tx_failed,
            snapshot.rx_packets,
            snapshot.rx_drop_misc,
        ),
        (2391, 87, 2, 4512, 1)
    );
    assert_eq!(snapshot.tx_bitrate, "150.0 MBit/s MCS 7 40MHz short GI");
    assert_eq!(
        (
            snapshot.tid0_aqm_drops,
            snapshot.tid0_aqm_overlimit,
            snapshot.interface_tx_dropped
        ),
        (3, 1, 5)
    );
    // A managed client lists its AP alone; two stations are not its link.
    let two = format!("{STATION_DUMP}{STATION_DUMP}");
    assert!(LaptopLinkSnapshot::parse(&two, AQM, 0).is_err());
    assert!(LaptopLinkSnapshot::parse("", AQM, 0).is_err());
    // Counters the helper did not print are no evidence.
    assert!(LaptopLinkSnapshot::parse(STATION_DUMP, "tid0_aqm_drops=3\n", 0).is_err());
}
