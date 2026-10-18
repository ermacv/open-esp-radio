use super::*;

fn parse(text: &str) -> WifiScenario {
    toml::from_str(text).unwrap_or_else(|error| panic!("{error}\n{text}"))
}

fn valid(text: &str) -> WifiScenario {
    let scenario = parse(text);
    scenario
        .validate()
        .unwrap_or_else(|error| panic!("{error}\n{text}"));
    scenario
}

fn invalid(text: &str) {
    assert!(parse(text).validate().is_err(), "accepted\n{text}");
}

const UDP_RX: &str = "image = 'correctness'\n[workload]\nkind = 'station-udp'\nlink = { phy = 'ht40' }\nduration_seconds = 16\npayload_bytes = 1472\noffer = { rx_bps = 50000000 }\n";

fn udp_rx(extra: &str) -> String {
    format!("{UDP_RX}{extra}")
}

#[test]
fn station_udp_requirements_follow_the_offer_and_observers() {
    let rx = valid(UDP_RX).plan();
    assert!(rx.requirements.station_network && rx.requirements.station_udp_rx_capture);
    assert!(!rx.requirements.station_udp_tx_capture && !rx.requirements.openwrt_tx_monitor);
    assert_eq!(
        rx.wifi,
        WifiLabUse {
            link: Some(PhyExpectation::Ht40),
            management_frame_protection: Default::default(),
            access_point_security: Default::default(),
            access_point: false,
            access_point_beacon: None,
        }
    );
    let observed = valid(&udp_rx(
        "[workload.observation]\nopenwrt_tx_monitor = true\nindependent_air_monitor = true\n",
    ))
    .plan();
    assert!(observed.requirements.openwrt_tx_monitor && observed.requirements.laptop_air_monitor);
    let both = valid(&UDP_RX.replace(
        "offer = { rx_bps = 50000000 }",
        "offer = { rx_bps = 50000000, tx_bps = 50000000 }",
    ))
    .plan();
    assert!(both.requirements.station_udp_rx_capture && both.requirements.station_udp_tx_capture);
}

#[test]
fn station_udp_criteria_must_measure_an_offered_direction() {
    for extra in [
        "[workload.criteria]\nminimum_tx_bps = 1\n",
        "[workload.criteria]\nminimum_rx_bps = 50000001\n",
        "[workload.criteria]\nminimum_combined_bps = 1\n",
        "[workload.criteria]\nmaximum_rx_silence_ms = 0\n",
        "[workload.observation]\nindependent_air_monitor = true\n",
    ] {
        invalid(&udp_rx(extra));
    }
    invalid(&UDP_RX.replace("offer = { rx_bps = 50000000 }", "offer = {}"));
    invalid(
        &UDP_RX
            .replace(
                "offer = { rx_bps = 50000000 }",
                "offer = { tx_bps = 50000000 }",
            )
            .replace("'ht40'", "'ht20'"),
    );
    invalid(
        &udp_rx("[workload.criteria]\nexact_delivery = true\n")
            .replace("correctness", "performance"),
    );
    invalid(
        &udp_rx("[workload.criteria]\nrequire_no_beacon_loss = true\n")
            .replace("correctness", "performance"),
    );
    assert!(
        toml::from_str::<WifiScenario>(&udp_rx("[workload.criteria]\nmaximum_lost = 1\n")).is_err()
    );
}

#[test]
fn receive_checks_are_published_only_for_their_criteria() {
    assert!(valid(UDP_RX).plan().checks.is_empty());
    let checks = valid(&udp_rx(
        "[workload.criteria]\nminimum_rx_bps = 1000\nmaximum_rx_silence_ms = 50\n",
    ))
    .plan()
    .checks;
    assert_eq!(
        checks,
        [
            "udp.rx.host-offer-rate",
            "udp.rx.maximum-silence",
            "udp.rx.target-rate"
        ]
    );
}

#[test]
fn link_expectations_require_driver_observation_and_a_matching_phy() {
    for (link, image) in [
        ("{ phy = 'ht40', minimum_mcs = 8 }", "correctness"),
        ("{ phy = 'ht40', minimum_mcs = 7 }", "performance"),
        ("{ phy = 'he20', guard_interval = 'short' }", "correctness"),
        (
            "{ phy = 'ht40', guard_interval = 'short' }",
            "diagnostic-task-residence",
        ),
    ] {
        invalid(
            &UDP_RX
                .replace("{ phy = 'ht40' }", link)
                .replace("correctness", image),
        );
    }
    valid(&UDP_RX.replace("{ phy = 'ht40' }", "{ phy = 'he20', minimum_mcs = 9 }"));
}

#[test]
fn fixture_guard_interval_mutation_requires_a_strict_expectation_and_both_observers() {
    let strict = UDP_RX.replace(
        "{ phy = 'ht40' }",
        "{ phy = 'ht40', guard_interval = 'short' }",
    );
    let observers =
        "[workload.observation]\nopenwrt_tx_monitor = true\nindependent_air_monitor = true\n";
    let mutated = strict.replace(
        "payload_bytes = 1472\n",
        "payload_bytes = 1472\nfixed_fixture_guard_interval = true\n",
    );
    valid(&format!("{mutated}{observers}"));
    invalid(&mutated);
    invalid(&format!(
        "{}{observers}",
        UDP_RX.replace(
            "payload_bytes = 1472\n",
            "payload_bytes = 1472\nfixed_fixture_guard_interval = true\n",
        )
    ));
}

#[test]
fn datapath_diagnostics_are_restricted_to_their_measurements() {
    let rx = |image: &str, datapath: &str| {
        format!(
            "{}[datapath]\n{datapath}\n",
            UDP_RX
                .replace("correctness", image)
                .replace("duration_seconds = 16", "duration_seconds = 12")
        )
    };
    valid(&rx(
        "diagnostic-task-poll",
        "rx_checksum = 'assume-valid-diagnostic'",
    ));
    invalid(&rx(
        "correctness",
        "rx_checksum = 'assume-valid-diagnostic'",
    ));
    valid(&rx(
        "diagnostic-core0-rx-coarse",
        "rx_continuation = 'adaptive-probe-diagnostic'",
    ));
    invalid(&rx(
        "diagnostic-task-poll",
        "rx_continuation = 'adaptive-probe-diagnostic'",
    ));
    valid(&rx(
        "diagnostic-core0-rx-cycles",
        "l1_cache_counters = true",
    ));
    invalid(&rx(
        "diagnostic-core0-rx-coarse",
        "l1_cache_counters = true",
    ));
    invalid(&rx(
        "diagnostic-task-poll",
        "tx_buffer = 'psram-direct-dma-diagnostic'",
    ));
    invalid(
        &rx("diagnostic-core0-rx-cycles", "")
            .replace("duration_seconds = 12", "duration_seconds = 13"),
    );
}

#[test]
fn images_accept_only_their_workloads() {
    invalid(&UDP_RX.replace("correctness", "bluetooth-dtm"));
    invalid(&UDP_RX.replace("correctness", "diagnostic-rx-ownership"));
    invalid(
        "image = 'performance'\n[workload]\nkind = 'station-reconnect'\nlink = { phy = 'ht40' }\ncycles = 1\nboots = 1\ntimeout_seconds = 30\n",
    );
}

#[test]
fn station_control_workloads_publish_distinct_checks() {
    let loss = valid("image = 'correctness'\n[workload]\nkind = 'station-ap-loss'\nlink = { phy = 'ht40' }\ntimeout_seconds = 60\nrequire_recovery_echo = true\n").plan();
    assert!(loss.requirements.station_control);
    assert_eq!(
        loss.checks,
        [
            "wifi.station.ap-loss-reconnected",
            "wifi.station.control-responsive",
            "wifi.station.recovered-ip-exchange",
        ]
    );
    let absence = |initially_absent: bool| {
        valid(&format!("image = 'correctness'\n[workload]\nkind = 'station-ap-absence'\nlink = {{ phy = 'ht40' }}\ntimeout_seconds = 60\ninitially_absent = {initially_absent}\n")).plan().checks
    };
    assert!(absence(true).contains(&"wifi.station.initial-retry-exhausted"));
    assert!(!absence(true).contains(&"wifi.station.recovery-retry-exhausted"));
    assert!(absence(false).contains(&"wifi.station.recovery-retry-exhausted"));
}

#[test]
fn role_operations_own_their_parameters() {
    let role = |operation: &str| {
        format!(
            "image = 'correctness'\n[workload]\nkind = 'role'\nlink = {{ phy = 'ht40' }}\ntimeout_seconds = 60\noperation = {operation}\n"
        )
    };
    valid(&role(
        "{ kind = 'restart', cycles = 3, rf = 'closed-and-woken' }",
    ));
    valid(&role(
        "{ kind = 'roundtrip', channel = 6, dwell_seconds = 5, snapshot_length = 128 }",
    ));
    invalid(&role(
        "{ kind = 'restart', cycles = 11, rf = 'closed-and-woken' }",
    ));
    invalid(&role("{ kind = 'monitor', channel = 14 }"));
    assert!(toml::from_str::<WifiScenario>(&role("{ kind = 'scan', cycles = 3 }")).is_err());
}

const AP: &str = "image = 'correctness'\n[workload]\nkind = 'access-point'\ncycles = 1\nboots = 1\ntimeout_seconds = 60\n";

fn ap(fields: &str, traffic: &str) -> String {
    format!("{AP}{fields}[workload.traffic]\n{traffic}")
}

const AP_UDP_RX: &str =
    "kind = 'udp'\nduration_seconds = 10\npayload_bytes = 1472\noffer = { rx_bps = 10000000 }\n";

#[test]
fn access_point_clients_select_fixture_services() {
    let laptop = valid(&ap("", "kind = 'none'\n")).plan();
    assert!(laptop.requirements.laptop_client && !laptop.requirements.openwrt_client);
    assert!(laptop.requirements.station_network);
    assert_eq!(
        laptop.wifi,
        WifiLabUse {
            link: None,
            management_frame_protection: Default::default(),
            access_point_security: Default::default(),
            access_point: true,
            access_point_beacon: None,
        }
    );
    let openwrt = valid(&ap("clients = { kind = 'openwrt' }\n", AP_UDP_RX)).plan();
    assert!(!openwrt.requirements.laptop_client && openwrt.requirements.openwrt_client);
    let both = valid(&ap(
        "clients = { kind = 'laptop-and-openwrt' }\n",
        "kind = 'none'\n",
    ))
    .plan();
    assert!(both.requirements.laptop_client && both.requirements.openwrt_client);
    let scheduled = valid(&ap(
        "link = { phy = 'ht20' }\nscheduler = 'rr-ht-response24'\n",
        "kind = 'none'\n",
    ))
    .plan();
    assert_eq!(
        scheduled.settings.ap_scheduler,
        WifiApScheduler::RrHtResponse24
    );
    assert_eq!(scheduled.wifi.link, Some(PhyExpectation::Ht20));
}

#[test]
fn laptop_clients_carry_their_advertised_phy() {
    use super::access_point::{AccessPointClients, LaptopPhy};
    let clients = |text: &str| toml::from_str::<AccessPointClients>(text).unwrap();
    assert_eq!(clients("kind = 'laptop'").laptop_phy(), Some(LaptopPhy::Ht));
    assert_eq!(
        clients("kind = 'laptop'\nlaptop_phy = 'non-ht'").laptop_phy(),
        Some(LaptopPhy::NonHt)
    );
    assert_eq!(
        clients("kind = 'laptop-and-openwrt'\nlaptop_phy = 'non-ht'").laptop_phy(),
        Some(LaptopPhy::NonHt)
    );
    assert_eq!(clients("kind = 'openwrt'").laptop_phy(), None);
    let non_ht = valid(&ap(
        "clients = { kind = 'laptop', laptop_phy = 'non-ht' }\n",
        AP_UDP_RX,
    ))
    .plan();
    assert!(non_ht.requirements.laptop_client && !non_ht.requirements.openwrt_client);
}

#[test]
fn access_point_mutations_and_observers_require_their_evidence() {
    invalid(&ap("security = 'open'\n", "kind = 'none'\n"));
    invalid(&ap("clients = { kind = 'openwrt' }\n", "kind = 'none'\n"));
    invalid(&ap("independent_air_monitor = true\n", AP_UDP_RX));
    invalid(&ap("link = { phy = 'he20' }\n", "kind = 'none'\n"));
    invalid(&ap("scheduler = 'rr-ht-response24'\n", "kind = 'none'\n"));
    invalid(&ap(
        "link = { phy = 'ht40', minimum_mcs = 7 }\n",
        "kind = 'none'\n",
    ));
    let mutated = "link = { phy = 'ht40' }\nclients = { kind = 'openwrt', fixed_ht_mcs = 7 }\n";
    invalid(&ap(mutated, AP_UDP_RX));
    let mutated = format!("{mutated}independent_air_monitor = true\n");
    valid(&ap(&mutated, AP_UDP_RX));
    invalid(&ap(&mutated, AP_UDP_RX).replace("correctness", "performance"));
    invalid(&ap(
        "link = { phy = 'ht40' }\nclients = { kind = 'openwrt', fixed_guard_interval = true }\nindependent_air_monitor = true\n",
        AP_UDP_RX,
    ));
    let observed = valid(&ap(
        "clients = { kind = 'openwrt' }\nindependent_air_monitor = true\n",
        AP_UDP_RX,
    ))
    .plan();
    assert!(observed.requirements.laptop_air_monitor);
}

const MULTI_TX: &str = "kind = 'udp-multi-client'\nduration_seconds = 12\npayload_bytes = 1472\noffer = { tx_bps = 20000000 }\n[workload.traffic.criteria]\nminimum_bps_per_flow = 1000000\n";

#[test]
fn multi_client_traffic_needs_both_clients_and_consistent_offers() {
    let both = "clients = { kind = 'laptop-and-openwrt' }\n";
    valid(&ap(both, MULTI_TX));
    invalid(&ap("", MULTI_TX));
    invalid(&ap(
        both,
        &MULTI_TX.replace(
            "minimum_bps_per_flow = 1000000",
            "minimum_bps_per_flow = 20000001",
        ),
    ));
    invalid(&ap(
        both,
        &MULTI_TX.replace(
            "[workload.traffic.criteria]",
            "[workload.traffic.secondary]\nrx_bps = 1000000\n[workload.traffic.criteria]",
        ),
    ));
    invalid(&ap(
        both,
        &format!("{MULTI_TX}maximum_secondary_tx_interarrival_ms = 100\n"),
    ));
    let paced = MULTI_TX.replace(
        "[workload.traffic.criteria]",
        "[workload.traffic.secondary]\ntx_bps = 1000000\ntx_pacing_group_datagrams = 4\n[workload.traffic.criteria]",
    );
    valid(&ap(
        both,
        &format!(
            "{paced}maximum_secondary_tx_interarrival_ms = 100\nminimum_secondary_tx_datagrams = 8\n"
        ),
    ));
    invalid(&ap(
        both,
        &format!("{paced}maximum_flow_skew_percent = 10\n"),
    ));
    let probe = format!("{both}probe_load = true\n");
    valid(&ap(&probe, MULTI_TX).replace("correctness", "diagnostic-task-poll"));
    invalid(&ap(&probe, MULTI_TX));
    let requirements = parse(&ap(&probe, MULTI_TX).replace("correctness", "diagnostic-task-poll"))
        .plan()
        .requirements;
    assert!(requirements.probe_load && requirements.openwrt_tx_monitor);
}

#[test]
fn paired_station_access_point_uses_the_laptop_client() {
    let paired = valid("image = 'correctness'\n[workload]\nkind = 'station-access-point'\nlink = { phy = 'ht40' }\ntimeout_seconds = 60\nduration_seconds = 10\ndirection = 'bidirectional'\nrate_bps_per_flow = 10000000\nminimum_bps_per_flow = 1000000\nmaximum_fairness_skew_percent = 50\npayload_bytes = 1472\n").plan();
    assert!(paired.requirements.laptop_client && !paired.requirements.station_control);
    let reconnect = valid("image = 'correctness'\n[workload]\nkind = 'station-access-point-reconnect'\nlink = { phy = 'ht40' }\ntimeout_seconds = 60\n").plan();
    assert!(reconnect.requirements.laptop_client && reconnect.requirements.station_control);
}

#[test]
fn induced_protection_needs_ht_transmission_and_publishes_air_checks() {
    let tx = UDP_RX.replace(
        "offer = { rx_bps = 50000000 }",
        "offer = { tx_bps = 16000000 }",
    );
    let induced = |peer: &str, percent: u8| {
        format!(
            "{tx}[workload.induced_protection]\npeer = '{peer}'\nminimum_protected_ppdu_percent = {percent}\n"
        )
    };
    let scenario = valid(&induced("non-ht-member", 95));
    let plan = scenario.plan();
    assert!(plan.requirements.non_ht_member && plan.requirements.air_observer);
    assert!(!plan.requirements.legacy_bss);
    assert_eq!(
        plan.checks,
        [
            "wifi.protection.control-rate",
            "wifi.protection.nav-covers-exchange",
            "wifi.protection.rts-cts-before-data",
        ]
    );
    let legacy = valid(&induced("overlapping-legacy-bss", 90)).plan();
    assert!(legacy.requirements.legacy_bss && !legacy.requirements.non_ht_member);
    invalid(&induced("non-ht-member", 0));
    invalid(&format!(
        "{UDP_RX}[workload.induced_protection]\npeer = 'non-ht-member'\nminimum_protected_ppdu_percent = 95\n"
    ));
    invalid(&induced("non-ht-member", 95).replace("'ht40'", "'he20'"));
    invalid(&format!(
        "{}[workload.observation]\nopenwrt_tx_monitor = true\nindependent_air_monitor = true\n",
        induced("non-ht-member", 95)
    ));
}

#[test]
fn access_point_protection_needs_a_non_ht_laptop_and_ht_transmission() {
    let protected = |clients: &str, traffic: &str| {
        ap(
            &format!(
                "link = {{ phy = 'ht40' }}\nclients = {clients}\n[workload.protection]\nminimum_protected_ppdu_percent = 95\n"
            ),
            traffic,
        )
    };
    let non_ht = "{ kind = 'laptop-and-openwrt', laptop_phy = 'non-ht' }";
    let plan = valid(&protected(non_ht, MULTI_TX)).plan();
    assert!(plan.requirements.air_observer && plan.requirements.laptop_client);
    assert!(plan.requirements.openwrt_client);
    assert_eq!(
        plan.checks,
        [
            "wifi.protection.control-rate",
            "wifi.protection.nav-covers-exchange",
            "wifi.protection.rts-cts-before-data",
        ]
    );
    invalid(&protected("{ kind = 'laptop-and-openwrt' }", MULTI_TX));
    invalid(&protected(non_ht, &MULTI_TX.replace("tx_bps", "rx_bps")));
    invalid(&protected(non_ht, MULTI_TX).replace("'ht40'", "'he20'"));
    // A non-HT laptop without an observed protection contract is ordinary.
    valid(&ap(&format!("clients = {non_ht}\n"), MULTI_TX));
}

/// Hardware time bounds iteration: a Wi-Fi scenario runs at most three
/// traffic phases (repetitions times cycles), each at most 16 seconds long.
/// Only scenarios whose purpose is duration itself, tagged `soak` or
/// `thermal`, are exempt.
#[test]
fn catalog_scenarios_fit_the_hardware_run_budget() {
    fn files(directory: &std::path::Path, found: &mut std::vec::Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files(&path, found);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "toml")
            {
                found.push(path);
            }
        }
    }
    fn longest_duration(value: &toml::Value) -> i64 {
        match value {
            toml::Value::Table(table) => table
                .iter()
                .map(|(key, value)| match value {
                    toml::Value::Integer(seconds) if key == "duration_seconds" => *seconds,
                    value => longest_duration(value),
                })
                .max()
                .unwrap_or(0),
            _ => 0,
        }
    }
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scenarios/ieee80211");
    let mut paths = std::vec::Vec::new();
    files(&root, &mut paths);
    assert!(!paths.is_empty());
    let mut over = std::vec::Vec::new();
    for path in paths {
        let scenario: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let exempt = scenario["tags"].as_array().is_some_and(|tags| {
            tags.iter()
                .any(|tag| matches!(tag.as_str(), Some("soak" | "thermal")))
        });
        if exempt {
            continue;
        }
        let repetitions = scenario
            .get("repetitions")
            .and_then(toml::Value::as_integer)
            .unwrap_or(1);
        let wifi = &scenario["wifi"];
        let cycles = wifi
            .get("workload")
            .and_then(|workload| workload.get("cycles"))
            .and_then(toml::Value::as_integer)
            .unwrap_or(1);
        let duration = longest_duration(wifi);
        if repetitions * cycles > 3 || duration > 16 {
            over.push(format!(
                "{}: repetitions={repetitions} cycles={cycles} duration_seconds={duration}",
                path.display()
            ));
        }
    }
    assert!(
        over.is_empty(),
        "scenarios exceed the run budget:\n{}",
        over.join("\n")
    );
}

#[test]
fn air_observers_are_refused_on_the_performance_image_and_admitted_on_station_exit() {
    let observers =
        "[workload.observation]\nopenwrt_tx_monitor = true\nindependent_air_monitor = true\n";
    invalid(&format!(
        "{}{observers}",
        UDP_RX.replace("image = 'correctness'", "image = 'performance'")
    ));
    valid(&format!(
        "{}{observers}",
        UDP_RX.replace("image = 'correctness'", "image = 'diagnostic-station-exit'")
    ));
}

/// A station tracking hold needs its image, a receive-only offer long
/// enough for every branch set's due ticks, and the timer left to the step.
#[test]
fn a_station_tracking_hold_needs_its_image_window_and_receive_offer() {
    let hold = UDP_RX.replace("'correctness'", "'diagnostic-phy-tracking-hold'");
    valid(&format!("{hold}phy_tracking_hold = 2\n"));
    for broken in [
        // Another image.
        format!("{UDP_RX}phy_tracking_hold = 2\n"),
        // No ticks, or more than the bound.
        format!("{hold}phy_tracking_hold = 0\n"),
        format!("{hold}phy_tracking_hold = 11\n"),
        // Traffic shorter than three branch sets' windows.
        format!("{hold}phy_tracking_hold = 3\n"),
        // A transmitting offer.
        format!(
            "{}phy_tracking_hold = 2\n",
            hold.replace(
                "offer = { rx_bps = 50000000 }",
                "offer = { rx_bps = 50000000, tx_bps = 50000000 }"
            )
        ),
        // The scenario suspending tracking itself.
        format!("{hold}phy_tracking_hold = 2\nsuspend_phy_tracking = true\n"),
    ] {
        invalid(&broken);
    }
}

/// The interrupt-latency probe needs its image and a receive-only offer, and
/// excludes the tracking hold.
#[test]
fn a_station_interrupt_latency_probe_needs_its_image_and_receive_offer() {
    let probe = UDP_RX.replace("'correctness'", "'diagnostic-interrupt-latency'");
    valid(&format!("{probe}interrupt_latency_probe = 512\n"));
    for broken in [
        format!("{UDP_RX}interrupt_latency_probe = 512\n"),
        format!("{probe}interrupt_latency_probe = 0\n"),
        format!("{probe}interrupt_latency_probe = 1025\n"),
        format!(
            "{}interrupt_latency_probe = 512\n",
            probe.replace(
                "offer = { rx_bps = 50000000 }",
                "offer = { rx_bps = 50000000, tx_bps = 50000000 }"
            )
        ),
        format!("{probe}interrupt_latency_probe = 512\nphy_tracking_hold = 2\n"),
    ] {
        invalid(&broken);
    }
}
