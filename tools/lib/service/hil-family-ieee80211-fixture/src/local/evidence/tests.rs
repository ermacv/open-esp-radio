use super::*;

#[cfg(unix)]
#[test]
fn failed_or_malformed_capture_is_infrastructure() {
    for script in [
        "echo 'injected dumpcap failure' >&2; exit 7",
        "echo 'Packets captured: 4' >&2",
    ] {
        let mut command = oer_process::command("sh");
        command.args(["-c", &format!("echo 'ready' >&2; read command; {script}")]);
        let child = Capture::start(&mut command, "ready".into(), Duration::from_secs(5)).unwrap();
        let error = LocalPacketCapture { child: Some(child) }
            .finish()
            .unwrap_err();
        assert_eq!(
            oer_hil_workload::failure::classify(&*error).kind,
            oer_hil_run_bundle_format::run::FailureKind::Infrastructure
        );
    }
}

#[test]
fn parses_iw_station_fields_without_depending_on_column_alignment() {
    let input = "\ttx packets:\t25013\n\ttx retries:\t4\n\ttx failed:\t0\n\ttx duration:\t2567438 us\n\ttx bitrate:\t150.0 MBit/s MCS 7 40MHz short GI\n\trx bitrate:\t135.0 MBit/s MCS 7 40MHz\n";
    assert_eq!(tagged_u64(input, "tx packets:").unwrap(), 25_013);
    assert_eq!(tagged_u64(input, "tx duration:").unwrap(), 2_567_438);
    assert_eq!(
        tagged_text(input, "tx bitrate:").unwrap(),
        "150.0 MBit/s MCS 7 40MHz short GI"
    );
}

#[test]
fn parses_and_enforces_the_observed_channel_width() {
    assert_eq!(
        channel_width("channel 1 (2412 MHz), width: 40 MHz, center1: 2422 MHz\n").unwrap(),
        40
    );
    assert!(require_width(PhyExpectation::Ht40, 40).is_ok());
    assert!(require_width(PhyExpectation::Ht40, 20).is_err());
}

#[test]
fn parses_dumpcap_packet_and_drop_counts() {
    let summary = "Capturing on 'wlan0'\nPackets captured: 141940\nPackets received/dropped on interface 'wlan0': 141940/0 (pcap:0/dumpcap:0) (100.0%)\n";
    assert_eq!(dumpcap_captured(summary).unwrap(), 141_940);
    assert_eq!(dumpcap_dropped(summary).unwrap(), 0);
}
