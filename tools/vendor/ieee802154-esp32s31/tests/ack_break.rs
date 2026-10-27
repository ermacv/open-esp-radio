use oer_vendor_ieee802154_esp32s31::{Record, Verdict, catalog, claim, compare_scenario, port};

#[test]
fn external_ack_break_notification_survives_the_comparison_filter() {
    let scenario = catalog::SCENARIOS
        .iter()
        .find(|s| s.name == "ack-transmit-abort-reasons")
        .unwrap();
    let vendor = claim(scenario);
    let port = port::run(scenario).unwrap();
    let notification = |record: &&Record| {
        matches!(record,
        Record::External { name, .. } if name == "esp_coex_ieee802154_coex_break_notify")
    };
    for trace in [&vendor, &port] {
        assert_eq!(
            trace.iter().filter(notification).count(),
            usize::from(cfg!(feature = "sw-coex"))
        );
    }
    assert_eq!(
        compare_scenario(scenario, &vendor, Ok(port.clone())).exit_code(),
        0
    );
    let mut changed = port;
    if cfg!(feature = "sw-coex") {
        changed.retain(|record| !notification(&record));
    } else {
        changed.push(Record::External {
            name: "esp_coex_ieee802154_coex_break_notify".into(),
            arguments: vec![],
        });
    }
    assert!(matches!(
        compare_scenario(scenario, &vendor, Ok(changed)),
        Verdict::Diff { .. }
    ));
}
