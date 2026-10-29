use super::*;

#[test]
fn station_requires_the_target_and_all_association_flags() {
    assert!(parse_station("1 1 1 1\n").unwrap().joined());
    assert!(!parse_station("1 1 0 1\n").unwrap().joined());
    assert_eq!(parse_station("0 0 0 0\n").unwrap(), Station::default());
    for text in ["2 1 1 1", "0 1 0 0", "1 1 1", "1 1 1 1 garbage", ""] {
        assert!(parse_station(text).is_err(), "{text}");
    }
}

#[test]
fn leases_are_unique_for_the_exact_target_and_keep_expiration() {
    assert_eq!(parse_lease("").unwrap(), None);
    assert_eq!(
        parse_lease("192.0.2.8 0")
            .unwrap()
            .unwrap()
            .remaining_seconds,
        0
    );
    assert_eq!(
        parse_lease("192.0.2.8 3600").unwrap().unwrap().address,
        Ipv4Addr::new(192, 0, 2, 8)
    );
    for text in [
        "192.0.2.8 10\n192.0.2.9 20",
        "invalid",
        "0.0.0.0 20",
        "224.0.0.1 20",
        "192.0.2.8 -1",
        "192.0.2.8 20 hostname",
    ] {
        assert!(parse_lease(text).is_err(), "{text}");
    }
}

#[test]
fn native_ping_preserves_sequence_gaps_and_every_duplicate_rtt() {
    let ping = parse_ping(include_str!("fixtures/ping-iputils.txt")).unwrap();
    assert_eq!((ping.sent, ping.received, ping.interval_ms), (4, 3, 1000));
    assert_eq!(
        ping.replies
            .iter()
            .map(|reply| (reply.sequence, reply.rtt_us, reply.duplicate))
            .collect::<Vec<_>>(),
        [
            (1, 123, false),
            (3, 124, false),
            (3, 125, true),
            (4, 128, false)
        ]
    );
    assert_eq!(
        ping.rtt,
        Some(PingRtt {
            min_us: 123,
            avg_us: 125,
            max_us: 128,
            mdev_us: Some(2),
        })
    );
}

#[test]
fn busybox_native_output_keeps_its_zero_based_sequences_and_absent_mdev() {
    let ping = parse_ping(include_str!("fixtures/ping-busybox.txt")).unwrap();
    assert_eq!((ping.sent, ping.received), (3, 2));
    assert_eq!(
        ping.replies
            .iter()
            .map(|reply| (reply.sequence, reply.rtt_us, reply.duplicate))
            .collect::<Vec<_>>(),
        [(0, 1234, false), (2, 2500, false), (2, 3000, true)]
    );
    assert_eq!(
        ping.rtt,
        Some(PingRtt {
            min_us: 1234,
            avg_us: 2244,
            max_us: 3000,
            mdev_us: None,
        })
    );
}

#[test]
fn missing_rtt_summary_keeps_replies_without_synthesizing_statistics() {
    let text = include_str!("fixtures/ping-iputils.txt");
    let without_rtt = text.split("rtt min/").next().unwrap();
    let ping = parse_ping(without_rtt).unwrap();
    assert_eq!(ping.replies.len(), 4);
    assert_eq!(ping.rtt, None);
    let lost = parse_ping(
        "PING 192.0.2.8 (192.0.2.8) 56(84) bytes of data.\n\
         From 192.0.2.1 icmp_seq=1 Destination Host Unreachable\n\
         --- 192.0.2.8 ping statistics ---\n\
         100 packets transmitted, 0 received, +100 errors, 100% packet loss, time 99000ms\n",
    )
    .unwrap();
    assert_eq!((lost.sent, lost.received), (100, 0));
    assert!(lost.replies.is_empty());
    assert_eq!(lost.rtt, None);
}

#[test]
fn missing_or_invalid_ping_data_returns_typed_errors() {
    let text = include_str!("fixtures/ping-iputils.txt");
    for (input, error) in [
        ("".to_owned(), PingParseError::MissingPacketSummary),
        (
            "64 bytes from 192.0.2.8: icmp_seq=1 ttl=64 time=1 ms\n".to_owned(),
            PingParseError::MissingPacketSummary,
        ),
        (
            format!("{text}{text}"),
            PingParseError::RepeatedPacketSummary,
        ),
        (
            text.replace("4 packets transmitted", "101 packets transmitted"),
            PingParseError::ContradictoryCounts,
        ),
        (
            text.replace("3 received", "5 received"),
            PingParseError::ContradictoryCounts,
        ),
        (
            text.replace("3 received", "invalid received"),
            PingParseError::InvalidPacketSummary,
        ),
        (
            text.replace("icmp_seq=1", "icmp_seq=65536"),
            PingParseError::InvalidReply,
        ),
        (
            text.replace("time=0.123", "time<0.123"),
            PingParseError::InvalidReply,
        ),
        (
            text.replace("0.123/0.125/0.128/0.002", "0.123/0.125/0.128"),
            PingParseError::InvalidRoundTripSummary,
        ),
        (
            format!("{text}rtt min/avg/max/mdev = 0/0/0/0 ms\n"),
            PingParseError::RepeatedRoundTripSummary,
        ),
    ] {
        assert_eq!(parse_ping(&input), Err(error), "{input}");
    }
}

#[test]
fn ping_decimal_times_are_exact_and_invalid_or_overflowing_values_are_errors() {
    for (text, value) in [("0", 0), ("1", 1000), ("12.3", 12300), ("0.001000", 1)] {
        assert_eq!(ping_time_us(text), Ok(value));
    }
    for text in [
        "-1",
        "NaN",
        "inf",
        "1e3",
        ".1",
        "1.2.3",
        "0.0001",
        "18446744073709551615",
    ] {
        assert_eq!(
            ping_time_us(text),
            Err(PingParseError::InvalidRoundTripTime)
        );
    }
}

#[test]
fn numeric_echo_parser_refuses_garbage_and_truncation() {
    let line =
        "1792590000.000123 IP 192.0.2.8 > 192.0.2.1: ICMP echo request, id 25, seq 1, length 64";
    let parsed = parse_echoes(line).unwrap();
    assert_eq!(parsed[0].at_us, 1_792_590_000_000_123);
    assert!(!parsed[0].reply);
    for text in [
        line.replace("request,", "unreachable,"),
        line.replace(".000123", ".1"),
        line.replace("seq 1,", "seq 65536,"),
        format!("{line}\ngarbage"),
        line.replace("length 64", "length 2"),
    ] {
        assert!(parse_echoes(&text).is_err());
    }
}
