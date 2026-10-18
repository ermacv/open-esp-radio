use super::*;

#[test]
fn reported_rates_preserve_the_existing_host_and_target_gate_resolutions() {
    let recorder = oer_hil_workload::measurements::Recorder::default();
    record_rates(&recorder, 1, 1_998, 1_999);
    let values = recorder.snapshot();
    let target = values
        .iter()
        .find(|v| v.name == "udp.rx.target-rate")
        .unwrap();
    let host = values
        .iter()
        .find(|v| v.name == "udp.rx.host-offer-rate")
        .unwrap();
    assert_eq!(
        target.verdict,
        Some(oer_hil_run_bundle_format::run::MeasurementVerdict::Passed)
    );
    assert_eq!(
        host.verdict,
        Some(oer_hil_run_bundle_format::run::MeasurementVerdict::Failed)
    );
    assert_eq!(target.threshold.unwrap().value, 1_000);
    assert_eq!(host.threshold.unwrap().value, 1_999);
}

#[test]
fn typed_configuration_preserves_workload_bounds() {
    assert!(
        Config {
            minimum_rate_bps: Some(20_000_000),
            rate_bps: 10_000_000,
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            maximum_idle_channel_utilization_255: Some(0),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}
