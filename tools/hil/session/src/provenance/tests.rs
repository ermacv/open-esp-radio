use super::*;

#[test]
fn before_observation_accepts_down_vifs_but_does_not_hide_probe_errors() {
    assert!(!openwrt_has_active_channel(Some(42), "").unwrap());
    assert!(!openwrt_has_active_channel(Some(0), "Interface phy0-ap0\n\ttype AP\n").unwrap());
    assert!(openwrt_has_active_channel(Some(0), "\tchannel 13 (2472 MHz)\n").unwrap());
    assert!(openwrt_has_active_channel(Some(1), "").is_err());
    assert!(openwrt_has_active_channel(Some(255), "").is_err());
}

#[test]
fn inactive_before_state_is_valid_provenance_for_a_network_scenario() {
    let lab = LabConfig::for_test();
    let mut provenance = capture(&lab, Default::default()).unwrap();
    provenance.scope = ObservationScope::Network;
    provenance.fixture = FixtureObservation::OpenWrtInactive {
        wireless_interface: "phy0-ap0".into(),
    };
    provenance
        .validate_binding(lab.cell_id(), &lab.dut.id, 0, None)
        .unwrap();
    provenance.fixture = FixtureObservation::OpenWrtInactive {
        wireless_interface: "another-radio".into(),
    };
    assert!(
        provenance
            .validate_binding(lab.cell_id(), &lab.dut.id, 0, None)
            .is_err()
    );
}

#[test]
fn system_observation_does_not_contact_the_configured_access_point() {
    let mut lab = LabConfig::for_test();
    let StationFixtureConfig::OpenWrt(config) = &mut lab.station_fixture else {
        unreachable!()
    };
    config.ssh_target = "unused-fixture.invalid".into();
    let provenance = capture(&lab, Default::default()).unwrap();
    assert_eq!(provenance.scope, ObservationScope::System);
    assert_eq!(provenance.fixture, FixtureObservation::NotUsed);
    assert!(provenance.host.ipv4_routes.is_empty());
    assert!(
        provenance
            .host
            .interfaces
            .iter()
            .all(|entry| entry.wireless_link.is_none() && entry.ipv4_addresses.is_empty())
    );
    provenance
        .validate_binding(lab.cell_id(), &lab.dut.id, 0, None)
        .unwrap();
}

#[test]
fn definition_omits_credentials_and_transport_endpoint() {
    let definition = definition(&LabConfig::for_test());
    let json = serde_json::to_string(&definition).unwrap();

    assert!(!json.contains("test-password"));
    assert!(!json.contains("test-network"));
    assert!(!json.contains("open-radio-ap"));
    assert!(json.contains("phy0-ap0"));
    assert!(json.contains("sensitive_network_values"));
}

#[test]
fn host_route_parser_keeps_replay_relevant_fields() {
    let routes = parse_host_ipv4_routes(
        "default via 192.0.2.1 dev enp0 proto dhcp src 192.0.2.10 metric 100\n\
         10.43.0.0/24 dev wlan0 proto kernel scope link src 10.43.0.2\n",
    )
    .unwrap();

    assert_eq!(routes.len(), 2);
    assert_eq!(routes[0].destination, "default");
    assert_eq!(routes[0].gateway, Some(Ipv4Addr::new(192, 0, 2, 1)));
    assert_eq!(routes[0].metric, Some(100));
    assert_eq!(routes[1].interface.as_deref(), Some("wlan0"));
    assert_eq!(
        routes[1].preferred_source,
        Some(Ipv4Addr::new(10, 43, 0, 2))
    );
}

#[test]
fn openwrt_parser_records_radio_geometry_and_vifs() {
    let observation = parse_openwrt(
        "release=OpenWrt 24.10.2\n\
         revision=r28739-d9340319c6\n\
         kernel=6.6.93\n\
         machine=aarch64\n\
         boot_id=31e71ea2-7bf5-4f7f-9f24-725cbb6f86fd\n\
         ingress_operstate=up\n\
         ingress_ipv4=192.0.2.1/24\n\
         wiphy=0\n\
         interface_type=AP\n\
         channel=13\n\
         frequency_mhz=2472\n\
         width_mhz=40\n\
         center1_mhz=2462\n\
         tx_power_dbm=20.00\n\
         country=DE\n\
         driver=mt76x2e\n\
         firmware=0.0.00\n\
         associated_stations=1\n\
         vif=phy0-ap0|AP\n\
         vif=open-radio-mon|monitor\n",
        "phy0-ap0",
        "br-lan",
    )
    .unwrap();

    assert_eq!(observation.channel, 13);
    assert_eq!(observation.width_mhz, 40);
    assert_eq!(observation.tx_power_milli_dbm, 20_000);
    assert_eq!(observation.associated_stations, 1);
    assert_eq!(observation.concurrent_interfaces.len(), 2);
}
