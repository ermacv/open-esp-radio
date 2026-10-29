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

fn pcap(big_endian: bool, nanoseconds: bool, records: &[(u32, u32, &[u8])]) -> Vec<u8> {
    let word = |value: u32| {
        if big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        }
    };
    let mut bytes = word(if nanoseconds { 0xa1b23c4d } else { 0xa1b2c3d4 }).to_vec();
    bytes.extend_from_slice(&[0; 20]);
    for (seconds, fraction, data) in records {
        let length = u32::try_from(data.len()).expect("small record");
        for value in [*seconds, *fraction, length, length] {
            bytes.extend_from_slice(&word(value));
        }
        bytes.extend_from_slice(data);
    }
    bytes
}

fn format(bytes: &[u8]) -> PcapFormat {
    PcapFormat::of(bytes[..24].try_into().expect("header")).expect("pcap magic")
}

#[test]
fn pcap_scan_reports_the_last_complete_record_in_every_byte_order_and_resolution() {
    for big_endian in [false, true] {
        let bytes = pcap(big_endian, false, &[(1, 500, b"abc"), (2, 250_000, b"de")]);
        assert_eq!(
            format(&bytes).scan(&bytes, 24),
            (bytes.len(), Some(2_250_000))
        );
        let bytes = pcap(big_endian, true, &[(3, 7_000_999, b"x")]);
        assert_eq!(
            format(&bytes).scan(&bytes, 24),
            (bytes.len(), Some(3_007_000))
        );
    }
    assert!(PcapFormat::of(&[0; 24]).is_none());
}

#[test]
fn pcap_scan_stops_at_a_partial_record_and_resumes_from_it() {
    let bytes = pcap(false, false, &[(1, 0, b"abcd"), (2, 0, b"efgh")]);
    let first_end = 24 + 16 + 4;
    let format = format(&bytes);
    // A cut inside the second record's header or data keeps the first.
    for cut in [first_end + 3, bytes.len() - 1] {
        assert_eq!(format.scan(&bytes[..cut], 24), (first_end, Some(1_000_000)));
    }
    assert_eq!(
        format.scan(&bytes[..first_end], first_end),
        (first_end, None)
    );
    assert_eq!(
        format.scan(&bytes, first_end),
        (bytes.len(), Some(2_000_000))
    );
}

#[test]
fn tail_wait_ends_when_a_record_passes_the_moment_or_the_bound_expires() {
    let stopped = 10_000_000;
    // Times are passed in: the wait started at 0 with a 2 s bound.
    assert_eq!(tail_step(0, stopped, 0, 2_000_000), TailStep::Wait);
    assert_eq!(
        tail_step(stopped, stopped, 100_000, 2_000_000),
        TailStep::Wait
    );
    assert_eq!(
        tail_step(stopped + 1, stopped, 100_000, 2_000_000),
        TailStep::Covered
    );
    assert_eq!(
        tail_step(stopped + 1, stopped, 2_000_000, 2_000_000),
        TailStep::Covered
    );
    assert_eq!(
        tail_step(stopped - 1, stopped, 2_000_000, 2_000_000),
        TailStep::TimedOut
    );
}
