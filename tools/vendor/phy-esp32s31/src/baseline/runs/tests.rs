use super::*;
#[test]
fn partial_production_series_cannot_become_a_match_from_the_completed_prefix() {
    let mut rep = serde_json::json!({"outcome":"failed"});
    let mut observations = serde_json::json!({"observations":[{"name":"comparison","value":{"verdict":"diff","production-captures":2}}]});
    production_series(&rep, &observations, 2).unwrap();
    assert!(production_series(&rep, &observations, 1).is_err());
    rep["outcome"] = "broken".into();
    assert!(production_series(&rep, &observations, 2).is_err());
    rep["outcome"] = "failed".into();
    observations["observations"][0]["value"]["verdict"] = "incomplete".into();
    assert!(production_series(&rep, &observations, 2).is_err());
    observations["observations"][0]["value"]["schema"] = compare::SCHEMA.into();
    observations["observations"][0]["value"]["analog"] =
        serde_json::json!({"same-run-control":[{"verdict":"incomplete","vendor-captures":0}]});
    production_series(&rep, &observations, 2).unwrap();
    assert!(production_series(&rep, &observations, 1).is_err());
}
#[test]
fn attachments_cannot_change_or_escape_and_discovery_is_not_a_primary_series() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("boot.json"), b"{}").unwrap();
    let r = serde_json::json!({"attachments":[{"path":"boot.json","size_bytes":2,"sha256":oer_os_durable::sha256_bytes(b"{}")} ]});
    assert_eq!(attached(dir.path(), &r, "boot.json").unwrap(), b"{}");
    std::fs::write(dir.path().join("boot.json"), b"[]").unwrap();
    assert!(attached(dir.path(), &r, "boot.json").is_err());
    assert!(attached(dir.path(), &r, "../boot.json").is_err());
    assert_eq!(count(&serde_json::json!({"observations":[{"name":"vendor-readability-discovery","value":{}}]}),"vendor-boot-").unwrap(),0);
    assert!(
        count(
            &serde_json::json!({"observations":[{"name":"vendor-boot-002","value":{}}]}),
            "vendor-boot-"
        )
        .is_err()
    );
}
