use super::*;

#[cfg(unix)]
#[test]
fn association_timeout_is_distinct_from_remote_setup_failure() {
    use std::os::unix::process::ExitStatusExt;
    for (code, kind) in [
        (
            1,
            oer_hil_run_bundle_format::run::FailureKind::Infrastructure,
        ),
        (
            255,
            oer_hil_run_bundle_format::run::FailureKind::Infrastructure,
        ),
        (
            ASSOCIATION_TIMEOUT,
            oer_hil_run_bundle_format::run::FailureKind::Scenario,
        ),
    ] {
        let error = require_association(std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: b"injected failure".to_vec(),
        })
        .unwrap_err();
        assert_eq!(oer_hil_workload::failure::classify(&*error).kind, kind);
    }
}

#[test]
fn ssid_is_encoded_without_shell_or_wpa_quoting() {
    assert_eq!(encode_hex(b"lab \\\" ap"), "6c6162205c22206170");
}

#[test]
fn busybox_ping_summary_is_typed() {
    assert_eq!(
        parse_ping_summary(
            "10 packets transmitted, 10 packets received, 0% packet loss\nround-trip min/avg/max = 1.0/2.0/3.0 ms",
        ),
        Some(SecondaryClientProbeEvidence {
            transmitted: 10,
            received: 10,
        }),
    );
}

#[test]
fn forwarding_address_is_typed() {
    assert_eq!(
        tagged_ipv4("forward_address=192.168.178.2\n", "forward_address").unwrap(),
        Ipv4Addr::new(192, 168, 178, 2),
    );
}

#[test]
fn link_snapshot_parser_and_counter_delta_are_strict() {
    let output = "rx_bytes=200\nrx_packets=30\nrx_duration=88\nrx_bitrate=150.0 MBit/s MCS 7 40MHz short GI\ntx_bytes=100\ntx_packets=20\ntx_bitrate=135.0 MBit/s MCS 6 40MHz short GI\ntx_retries=3\ntx_failed=1\ntx_duration=77\ntid0_aqm_drops=0\n";
    assert_eq!(tagged_u64(output, "rx_packets").unwrap(), 30);
    assert_eq!(
        tagged_optional_u64(output, "rx_duration").unwrap(),
        Some(88)
    );
    assert_eq!(
        tagged_optional_string(output, "rx_bitrate").as_deref(),
        Some("150.0 MBit/s MCS 7 40MHz short GI"),
    );
    assert_eq!(tagged_u64(output, "tx_packets").unwrap(), 20);
    assert_eq!(
        tagged_optional_string(output, "tx_bitrate").as_deref(),
        Some("135.0 MBit/s MCS 6 40MHz short GI"),
    );
    assert_eq!(counter_delta("packets", 20, 27).unwrap(), 7);
    assert!(counter_delta("packets", 27, 20).is_err());
    assert_eq!(
        optional_counter_delta("duration", Some(20), Some(27)).unwrap(),
        Some(7),
    );
    assert!(optional_counter_delta("duration", Some(20), None).is_err());
}

#[cfg(unix)]
#[test]
fn failed_link_snapshot_keeps_exit_status_even_without_remote_diagnostics() {
    use std::os::unix::process::ExitStatusExt;
    let error = parse_link_snapshot(std::process::Output {
        status: std::process::ExitStatus::from_raw(1 << 8),
        stdout: Vec::new(),
        stderr: Vec::new(),
    })
    .err()
    .unwrap();
    let text = error.to_string();
    assert!(text.contains("exit status: 1"), "{text}");
    assert!(text.contains("stdout=\"\" stderr=\"\""), "{text}");
}

#[cfg(unix)]
#[test]
fn link_snapshot_carries_client_signal_and_operating_channel_survey() {
    use std::os::unix::process::ExitStatusExt;
    let snapshot = |stdout: &str| {
        parse_link_snapshot(std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        })
    };
    let counters = "rx_bytes=1\nrx_packets=1\ntx_bytes=1\ntx_packets=1\ntx_retries=0\n\
                    tx_failed=0\ntx_duration=1\ntid0_aqm_drops=0\nrx_drop_misc=4\n";
    let survey = "survey_active=1000\nsurvey_busy=400\nsurvey_receive=300\n\
                  survey_bss_receive=250\nsurvey_transmit=20\n";
    let parsed = snapshot(&format!(
        "{counters}signal_avg=-41\nsurvey_noise=-92\n{survey}"
    ))
    .unwrap();
    assert_eq!(parsed.rx_drop_misc, 4);
    assert_eq!(parsed.signal_avg_dbm, Some(-41));
    assert_eq!(
        parsed.survey,
        OpenWrtClientChannelSurvey {
            active_millis: 1_000,
            busy_millis: 400,
            receive_millis: 300,
            bss_receive_millis: 250,
            transmit_millis: 20,
            noise_dbm: Some(-92),
        }
    );
    // Signal and noise are optional driver reports; channel times are not.
    let parsed = snapshot(&format!("{counters}signal_avg=\n{survey}")).unwrap();
    assert_eq!(parsed.signal_avg_dbm, None);
    assert_eq!(parsed.survey.noise_dbm, None);
    assert!(snapshot(&format!("{counters}survey_active=1000\n")).is_err());
}
