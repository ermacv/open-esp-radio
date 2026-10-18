use std::{net::Ipv4Addr, time::Duration};

use oer_hil_run_bundle_format::run::MeasurementVerdict;

use oer_hil_net_traffic::icmp::Summary;

use super::{Config, measurements};

#[test]
fn measurements_preserve_each_acceptance_verdict() {
    let options = Config {
        device: Ipv4Addr::LOCALHOST,
        count: 10,
        interval: Duration::from_millis(1),
        timeout: Duration::from_millis(10),
        payload_bytes: 32,
        maximum_lost: 1,
        maximum_p95: Some(Duration::from_millis(100)),
    };
    let summary = Summary {
        transmitted: 10,
        received: 8,
        readiness_attempts: 1,
        lost_sequences: vec![2, 4],
        minimum_us: 10,
        average_us: 20,
        p50_us: 15,
        p95_us: 101_000,
        p99_us: 110_000,
        maximum_us: 110_000,
    };
    let measurements = measurements(options, &summary);
    let verdict = |name| {
        measurements
            .iter()
            .find(|measurement| measurement.name == name)
            .and_then(|measurement| measurement.verdict)
    };
    assert_eq!(
        verdict("icmp.replies.received"),
        Some(MeasurementVerdict::Failed)
    );
    assert_eq!(
        verdict("icmp.replies.lost"),
        Some(MeasurementVerdict::Failed)
    );
    assert_eq!(verdict("icmp.rtt.p95"), Some(MeasurementVerdict::Failed));
}
