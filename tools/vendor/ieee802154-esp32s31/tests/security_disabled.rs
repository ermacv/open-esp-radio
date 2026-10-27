use oer_vendor_ieee802154_esp32s31::{
    Inputs, Record, Scenario, Step, Verdict, claim, compare_scenario, port,
};

fn inputs() -> Inputs {
    Inputs::default()
}
fn steps() -> Vec<Step> {
    vec![Step::SetTransmitSecurity {
        frame: vec![5, 0x02, 0x20, 1, 0, 0],
        key: [7; 16],
        address: [9; 8],
    }]
}

#[test]
fn security_disabled_is_a_vendor_assertion_not_invalid_argument() {
    let scenario = Scenario {
        name: "security-disabled",
        inputs,
        steps,
        expected_difference: None,
    };
    // SOURCE: pinned esp_ieee802154_sec.c:19 asserts before parsing/configuring.
    let vendor = claim(&scenario);
    assert!(
        vendor
            .iter()
            .any(|record| matches!(record, Record::Event { name, .. } if name == "assert_failed"))
    );
    let port = port::run(&scenario);
    assert!(
        port.as_ref()
            .is_err_and(|error| error.contains("security-enabled"))
    );
    assert_eq!(
        compare_scenario(&scenario, &vendor, port),
        Verdict::Incomplete("the vendor driver asserted".to_owned())
    );
}
