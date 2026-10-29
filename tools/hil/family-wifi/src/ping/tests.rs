use super::*;
use oer_lab_router::station::{PingReply, PingRtt};

#[test]
fn each_window_records_reported_statistics_and_all_replies_as_information() {
    let results = Results::default();
    let measurements = Recorder::default();
    let ping = Ping {
        sent: 100,
        received: 99,
        interval_ms: 1000,
        replies: vec![
            PingReply {
                sequence: 1,
                rtt_us: 1,
                duplicate: false,
            },
            PingReply {
                sequence: 3,
                rtt_us: 2_000_000,
                duplicate: false,
            },
            PingReply {
                sequence: 3,
                rtt_us: 3_000_000,
                duplicate: true,
            },
        ],
        rtt: Some(PingRtt {
            min_us: 1,
            avg_us: 1_666_667,
            max_us: 3_000_000,
            mdev_us: Some(1_247_219),
        }),
    };
    for window in [Window::GroupKeyChange, Window::AfterReconnect] {
        record(&results, &measurements, window, &ping);
    }
    let saved = results.snapshot();
    assert_eq!(saved.observations.len(), 2);
    assert_eq!(
        saved.observations[0].name,
        "wifi-station.ping.group-key-change"
    );
    assert_eq!(
        saved.observations[1].name,
        "wifi-station.ping.after-reconnect"
    );
    for observation in saved.observations {
        let decoded: Ping = serde_json::from_value(observation.value).unwrap();
        assert_eq!(decoded, ping);
    }
    let measured = measurements.snapshot();
    assert_eq!(measured.len(), 8);
    for window in ["group-key-change", "after-reconnect"] {
        for (statistic, value) in [
            ("min", 1),
            ("avg", 1_666_667),
            ("max", 3_000_000),
            ("mdev", 1_247_219),
        ] {
            let row = measured
                .iter()
                .find(|row| row.name == format!("wifi-station.ping.{window}.rtt-us.{statistic}"))
                .unwrap();
            assert_eq!(row.value, value);
            assert_eq!(row.unit, MeasurementUnit::Microseconds);
            assert_eq!((row.better, row.threshold, row.verdict), (None, None, None));
        }
    }
}

#[test]
fn missing_statistics_stay_absent_and_reply_observations_survive() {
    let results = Results::default();
    let measurements = Recorder::default();
    let mut ping = Ping {
        replies: vec![PingReply {
            sequence: 1,
            rtt_us: 123,
            duplicate: false,
        }],
        ..Ping::default()
    };
    record(&results, &measurements, Window::GroupKeyChange, &ping);
    assert!(measurements.snapshot().is_empty());
    assert_eq!(
        results.snapshot().observations[0].value["replies"][0]["rtt_us"],
        123
    );
    ping.rtt = Some(PingRtt {
        min_us: 123,
        avg_us: 123,
        max_us: 123,
        mdev_us: None,
    });
    record(&results, &measurements, Window::AfterReconnect, &ping);
    assert_eq!(measurements.snapshot().len(), 3);
    assert!(
        measurements
            .snapshot()
            .iter()
            .all(|row| !row.name.ends_with(".mdev"))
    );
}
