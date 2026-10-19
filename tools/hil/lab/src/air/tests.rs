use super::*;

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

#[test]
fn tshark_decodes_a_radiotap_probe_and_ack_from_memory() {
    // A synthetic pcap: radiotap RATE + CHANNEL, an ordinary probe and ACK.
    let mut pcap = Vec::new();
    for word in [0xa1b2c3d4u32, 0x00040002, 0, 0, 65535, 127] {
        pcap.extend_from_slice(&word.to_le_bytes());
    }
    let radiotap = [0, 0, 14, 0, 0x0c, 0, 0, 0, 2, 0, 0xa8, 9, 0xa0, 0];
    let mut probe = radiotap.to_vec();
    probe.extend_from_slice(&[0x40, 0x08, 0, 0]);
    probe.extend_from_slice(&AP);
    probe.extend_from_slice(&TARGET);
    probe.extend_from_slice(&AP);
    probe.extend_from_slice(&[0x80, 0, 0, 3, b'L', b'a', b'b']);
    let mut ack = radiotap.to_vec();
    ack.extend_from_slice(&[0xd4, 0, 0, 0]);
    ack.extend_from_slice(&TARGET);
    for (us, frame) in [(0u32, probe), (600, ack)] {
        for word in [1, us, frame.len() as u32, frame.len() as u32] {
            pcap.extend_from_slice(&word.to_le_bytes());
        }
        pcap.extend_from_slice(&frame);
    }
    let frames = decode(&pcap).expect("tshark decode");
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].rate_mbps, Some(1.0));
    assert!(frames[0].retry);
    assert_eq!(frames[0].source, Some(TARGET));
    assert_eq!(frames[0].ssid, b"Lab");
    assert_eq!(acknowledged_probes(&frames, TARGET, AP), 1);
}
