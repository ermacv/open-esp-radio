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
fn ping_never_infers_success_from_an_absent_summary() {
    assert_eq!(parse_ping("100 100\n").unwrap().received, 100);
    assert_eq!(parse_ping("100 99\n").unwrap().received, 99);
    for text in ["", "100 101", "101 100", "100 100\n100 100", "garbage"] {
        assert!(parse_ping(text).is_err());
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
