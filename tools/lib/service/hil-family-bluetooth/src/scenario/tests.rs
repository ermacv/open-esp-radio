use super::*;

fn parse(text: &str) -> BluetoothScenario {
    toml::from_str(text).unwrap()
}

#[test]
fn each_workload_selects_its_image() {
    for (text, image) in [
        ("kind = 'gatt'", ImageClass::BluetoothGatt),
        (
            "kind = 'gatt'\ntrace = true",
            ImageClass::DiagnosticBluetoothGattTrace,
        ),
        (
            "kind = 'secure-gatt'\nshutdown = 'hci-read-failure'",
            ImageClass::BluetoothSecureGatt,
        ),
        (
            "kind = 'dtm'\nboots = 1\nminimum_packets = 10",
            ImageClass::BluetoothDtm,
        ),
        ("kind = 'scannable-advertising'", ImageClass::BluetoothDtm),
        ("kind = 'directed-advertising'", ImageClass::BluetoothDtm),
        (
            "kind = 'dtm-peer'\nminimum_packets = 100",
            ImageClass::BluetoothDtm,
        ),
    ] {
        let scenario = parse(text);
        scenario.validate().unwrap();
        let plan = scenario.plan();
        assert_eq!(plan.image, image, "{text}");
        assert!(!plan.requirements.network());
        assert_eq!(
            plan.requirements.bluetooth_adapter,
            scenario.adapter_preflight().is_some(),
            "{text}"
        );
    }
}

#[test]
fn the_peer_scenario_claims_the_peer_board_and_no_linux_adapter() {
    let scenario = parse("kind = 'dtm-peer'\nminimum_packets = 100");
    let plan = scenario.plan();
    assert!(plan.requirements.peer && !plan.requirements.bluetooth_adapter);
    assert_eq!(
        scenario.peer_image(),
        Some(crate::fixture::dtm_peer::DTM_PEER_IMAGE)
    );
    assert!(
        !parse("kind = 'dtm'\nboots = 1\nminimum_packets = 10")
            .plan()
            .requirements
            .peer
    );
    assert!(
        parse("kind = 'dtm-peer'\nminimum_packets = 0")
            .validate()
            .is_err()
    );
}

#[test]
fn out_of_range_or_retired_workloads_are_rejected() {
    assert!(
        parse("kind = 'dtm'\nboots = 0\nminimum_packets = 1")
            .validate()
            .is_err()
    );
    for text in [
        "kind = 'gatt'\nimage = 'bluetooth-gatt'",
        "kind = 'peripheral'\nboots = 1\nconnections = 2\nhold_millis = 0",
        "kind = 'maintenance-deadline'",
        "kind = 'watchdog-reset'",
    ] {
        assert!(toml::from_str::<BluetoothScenario>(text).is_err(), "{text}");
    }
}

#[test]
fn only_the_traced_gatt_scenario_publishes_its_trace_check() {
    assert_eq!(
        parse("kind = 'gatt'\ntrace = true").plan().checks,
        ["bluetooth.trace.connection-steps"]
    );
    assert!(parse("kind = 'gatt'").plan().checks.is_empty());
}
