use super::*;

#[test]
fn capture_requires_one_explicit_drop_counter_and_never_repeats_diagnostics() {
    let summary = "Packets received/dropped on interface 'mon0': 450/0 (pcap:0/dumpcap:0/flushed:0/ps_ifdrop:0) (100.0%)\n";
    assert_eq!(capture_drops(summary).expect("statistics"), 0);
    assert_eq!(
        capture_drops(&summary.replace("450/0", "450/2")).expect("drops"),
        2
    );
    assert!(capture_drops("").is_err());
    assert!(capture_drops(&summary.repeat(2)).is_err());
    let secret = "sensitive-passphrase";
    assert!(
        !capture_drops(secret)
            .expect_err("invalid statistics")
            .to_string()
            .contains(secret)
    );
}

const TARGET: [u8; 6] = [2, 0, 0, 0, 0, 1];
const AP: [u8; 6] = [2, 0, 0, 0, 0, 2];

fn frames() -> Vec<Frame> {
    parse(concat!(
        "1.000000000\t2472\t1\t0x0004\t02:00:00:00:00:01\tff:ff:ff:ff:ff:ff\tff:ff:ff:ff:ff:ff\t7\t0\t4c6162\n",
        "1.010000000\t2472\t6\t0x0004\t02:00:00:00:00:01\t02:00:00:00:00:02\t02:00:00:00:00:02\t8\t1\t4c6162\n",
        "1.010200000\t2472\t6\t0x001d\t\t02:00:00:00:00:01\t\t\t0\t\n",
        "1.012000000\t2472\t6\t0x0005\t02:00:00:00:00:02\t02:00:00:00:00:01\t02:00:00:00:00:02\t9\t0\t4c6162\n",
    )).expect("decoded rows")
}

#[test]
fn decode_preserves_source_rate_retry_ack_and_response() {
    let rows = frames();
    assert_eq!(rows[0].receiver, [255; 6]);
    assert_eq!(rows[0].rate_mbps, Some(1.0));
    assert!(!rows[0].retry);
    assert!(rows[1].retry);
    assert_eq!(rows[1].sequence, Some(8));
    assert_eq!(rows[3].ssid, b"Lab");
    assert_eq!(rows[3].subtype, 5);
    assert_eq!(acknowledged_probes(&rows, TARGET, AP), 1);
}

#[test]
fn unrelated_late_and_other_channel_acks_do_not_prove_the_probe() {
    let mut rows = frames();
    rows[2].receiver = AP;
    assert_eq!(acknowledged_probes(&rows, TARGET, AP), 0);
    rows[2].receiver = TARGET;
    rows[2].frequency_mhz = 2412;
    assert_eq!(acknowledged_probes(&rows, TARGET, AP), 0);
    rows[2].frequency_mhz = 2472;
    rows[2].at_us += 1001;
    assert_eq!(acknowledged_probes(&rows, TARGET, AP), 0);
}

#[test]
fn malformed_tshark_input_is_an_error() {
    assert!(parse("broken").is_err());
    assert!(parse("1.0\t2472\t1\t0x4\t\tbad\t\t0\t0\t").is_err());
}
