use super::*;

#[test]
fn union_includes_every_selected_owner_and_empty_needs_nothing() {
    assert_eq!(Requirements::union([]), Requirements::default());
    let peer = Requirements {
        peer: true,
        ..Requirements::default()
    };
    let station = Requirements {
        station_network: true,
        station_udp_rx_capture: true,
        ..Requirements::default()
    };
    let access_point = Requirements {
        station_network: true,
        laptop_client: true,
        openwrt_client: true,
        probe_load: true,
        ..Requirements::default()
    };
    let union = Requirements::union([peer, station, access_point]);
    assert!(union.peer && union.station_network && union.station_udp_rx_capture);
    assert!(union.laptop_client && union.openwrt_client && union.probe_load);
    assert!(!union.station_udp_tx_capture && !union.laptop_air_monitor);
    assert!(union.network() && union.local_radio());
    assert!(!peer.network());
}
