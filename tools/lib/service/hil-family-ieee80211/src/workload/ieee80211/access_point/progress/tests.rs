use super::*;

#[test]
fn link_and_cleanup_failures_do_not_erase_traffic_or_each_other() {
    let results = oer_hil_workload::results::Results::default();
    let mut progress = CycleProgress::new(2);
    progress.record(
        "traffic",
        &Ok::<_, &str>(serde_json::json!({"rx_bytes": 1234})),
    );
    progress.record(
        "secondary_client_link",
        &Err::<(), _>("no associated station"),
    );
    progress.record("client_restore", &Err::<(), _>("restore failed"));
    progress.record("station_restart", &Ok::<_, &str>(()));
    progress.save(&results);
    progress.record("late", &Ok::<_, &str>(()));
    progress.save(&results);
    let observations = results.snapshot().observations;
    assert_eq!(observations.len(), 1, "a cycle keeps one current progress");
    assert_eq!(observations[0].name, "cycle-002-progress");
    let saved = observations[0].value.clone();
    assert_eq!(saved["stages"]["late"]["status"], "available");
    assert_eq!(saved["cycle"], 2);
    assert_eq!(saved["stages"]["traffic"]["value"]["rx_bytes"], 1234);
    assert_eq!(
        saved["stages"]["secondary_client_link"]["error"],
        "no associated station"
    );
    assert_eq!(saved["stages"]["client_restore"]["error"], "restore failed");
    assert_eq!(saved["stages"]["station_restart"]["status"], "available");
}
