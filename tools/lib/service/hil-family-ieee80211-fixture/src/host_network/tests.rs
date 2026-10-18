use super::*;

#[test]
fn overlapping_link_parser_exposes_arp_flux_risk() {
    let links = overlapping_ipv4_links(
        "2: enp0: <UP> inet 192.168.178.129/24 scope global enp0\n\
         3: wlan0: <UP> inet 192.168.178.107/24 scope global wlan0\n\
         4: tailscale0: <UP> inet 100.64.0.1/32 scope global tailscale0\n",
        Ipv4Addr::new(192, 168, 178, 131),
    );

    assert_eq!(
        links,
        ["enp0=192.168.178.129/24", "wlan0=192.168.178.107/24"]
    );
}

#[test]
fn subnet_comparison_handles_boundary_prefixes() {
    assert!(same_ipv4_subnet(
        Ipv4Addr::new(1, 2, 3, 4),
        Ipv4Addr::new(203, 0, 113, 9),
        0
    ));
    assert!(!same_ipv4_subnet(
        Ipv4Addr::new(1, 2, 3, 4),
        Ipv4Addr::new(1, 2, 3, 5),
        32
    ));
    assert!(!same_ipv4_subnet(
        Ipv4Addr::LOCALHOST,
        Ipv4Addr::LOCALHOST,
        33
    ));
}

#[test]
fn route_parser_binds_interface_and_source() {
    let route = parse_ipv4_route(
        "192.168.178.127 dev enp0s20f0u2u4c2 src 192.168.178.129 uid 1000\n    cache\n",
        Ipv4Addr::new(192, 168, 178, 127),
    )
    .unwrap();

    assert_eq!(
        route,
        BenchmarkIpv4Route {
            interface: String::from("enp0s20f0u2u4c2"),
            source: Ipv4Addr::new(192, 168, 178, 129),
            medium: RouteMedium::Ethernet,
            expected_medium: None,
        }
    );
    route
        .verify_socket_source(Ipv4Addr::new(192, 168, 178, 129))
        .unwrap();
    assert!(
        route
            .verify_socket_source(Ipv4Addr::new(192, 168, 178, 107))
            .is_err()
    );
}

#[test]
fn route_parser_rejects_incomplete_kernel_output() {
    let device = Ipv4Addr::new(192, 168, 178, 127);

    assert!(parse_ipv4_route("", device).is_err());
    assert!(parse_ipv4_route("192.168.178.127 src 192.168.178.129", device).is_err());
    assert!(parse_ipv4_route("192.168.178.127 dev enp0", device).is_err());
}

#[test]
fn route_evidence_is_typed_and_requires_the_bound_socket_source() {
    let output = std::env::temp_dir().join(format!("open-radio-host-route-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&output);
    std::fs::create_dir(&output).unwrap();
    let route = BenchmarkIpv4Route {
        interface: String::from("enp0"),
        source: Ipv4Addr::new(192, 0, 2, 10),
        medium: RouteMedium::Ethernet,
        expected_medium: Some(RouteMedium::Ethernet),
    };

    route
        .record(
            &output,
            Ipv4Addr::new(192, 0, 2, 20),
            Ipv4Addr::new(192, 0, 2, 10),
        )
        .unwrap();
    let evidence: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("host-route.json")).unwrap()).unwrap();
    assert_eq!(evidence["medium"], "ethernet");
    assert_eq!(evidence["expected_medium"], "ethernet");
    assert_eq!(evidence["socket_source_assertion_passed"], true);
    assert!(
        route
            .record(
                &output,
                Ipv4Addr::new(192, 0, 2, 20),
                Ipv4Addr::new(192, 0, 2, 11),
            )
            .is_err()
    );
    std::fs::remove_dir_all(output).unwrap();
}
