use oer_vendor_ieee802154_esp32s31::{Record, Verdict, catalog, claim, compare_scenario, port};

#[test]
fn malformed_security_returns_invalid_argument_without_ll_writes() {
    let scenario = catalog::SCENARIOS
        .iter()
        .find(|s| s.name == "malformed-security")
        .unwrap();
    let vendor = claim(scenario);
    let port = port::run(scenario).unwrap();
    for trace in [&vendor, &port] {
        assert!(
            !trace
                .iter()
                .any(|record| matches!(record, Record::Ll { .. }))
        );
        assert!(
            trace.iter().any(|record| matches!(record,
            Record::Return { name, value: 258 } if name == "esp_ieee802154_set_transmit_security"))
        );
    }
    assert_eq!(
        compare_scenario(scenario, &vendor, Ok(port)),
        Verdict::Match
    );
}
