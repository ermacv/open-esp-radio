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
    assert!(parse_ping("100 100\n").unwrap().passed());
    assert!(!parse_ping("100 99\n").unwrap().passed());
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

pub(super) fn echoes() -> Vec<Echo> {
    (1..=100)
        .flat_map(|sequence| {
            [false, true].map(|reply| Echo {
                at_us: u64::from(sequence) * 1_000_000 + u64::from(reply) * 1000,
                source: if reply {
                    Ipv4Addr::new(192, 0, 2, 1)
                } else {
                    Ipv4Addr::new(192, 0, 2, 8)
                },
                destination: if reply {
                    Ipv4Addr::new(192, 0, 2, 8)
                } else {
                    Ipv4Addr::new(192, 0, 2, 1)
                },
                reply,
                identifier: 50,
                sequence,
            })
        })
        .collect()
}

#[test]
fn hundred_distinct_echo_pairs_require_replies_after_the_requests() {
    let source = Ipv4Addr::new(192, 0, 2, 8);
    let destination = Ipv4Addr::new(192, 0, 2, 1);
    let good = echoes();
    assert!(echo_gate(&good, source, destination));
    let mut bad = good.clone();
    bad.remove(9);
    assert!(!echo_gate(&bad, source, destination));
    let mut bad = good.clone();
    bad[9] = bad[7].clone();
    assert!(!echo_gate(&bad, source, destination));
    let mut bad = good.clone();
    bad[9].at_us = 0;
    assert!(!echo_gate(&bad, source, destination));
    let mut bad = good.clone();
    bad[8].identifier = 51;
    assert!(!echo_gate(&bad, source, destination));
    let mut bad = good.clone();
    bad[9].identifier = 51;
    assert!(!echo_gate(&bad, source, destination));
    let mut bad = good.clone();
    for echo in &mut bad {
        echo.at_us /= 1000;
    }
    assert!(
        !echo_gate(&bad, source, destination),
        "an image's interval claim cannot hide fast echoes"
    );
    let mut bad = good.clone();
    bad[8].at_us += CADENCE_TOLERANCE_US + 1;
    assert!(!echo_gate(&bad, source, destination));
}

#[test]
fn runtime_profile_verification_requires_ht_and_refuses_wpa_wep_or_he() {
    struct Fake(&'static str);
    impl Remote for Fake {
        fn run(&mut self, script: &str) -> Result<String> {
            assert!(script.contains("/var/run/hostapd-$phy.conf"));
            assert!(script.contains("wep_(default_key|key[0-3])"));
            Ok(self.0.to_owned())
        }
    }
    assert!(verify_open_ht20(&mut Fake("1 1 0 0 0 0\n"), "radio9", "made-up-interface").is_ok());
    for flags in [
        "0 1 0 0 0 0",
        "1 0 0 0 0 0",
        "1 1 1 0 0 0",
        "1 1 0 1 0 0",
        "1 1 0 0 2 0",
        "1 1 0 0 0 1",
        "garbage",
    ] {
        assert!(verify_open_ht20(&mut Fake(flags), "radio9", "made-up-interface").is_err());
    }
}

#[test]
fn remote_scripts_filter_full_identities_before_returning_and_quote_inputs() {
    struct Fake {
        scripts: Vec<String>,
    }
    impl Remote for Fake {
        fn run(&mut self, script: &str) -> Result<String> {
            self.scripts.push(script.to_owned());
            Ok(if script.contains("station dump") {
                "1 1 1 1"
            } else {
                "192.0.2.8 1000"
            }
            .to_owned())
        }
        fn pause(&mut self, _: Duration) {
            panic!("observations never poll internally");
        }
    }
    let mut fake = Fake {
        scripts: Vec::new(),
    };
    let target = Mac::parse("02:00:00:00:00:08").unwrap();
    assert!(
        station(&mut fake, "made-up'iface", target)
            .unwrap()
            .joined()
    );
    assert!(lease(&mut fake, target).unwrap().is_some());
    assert!(
        fake.scripts
            .iter()
            .all(|s| !s.contains("cat /tmp/dhcp.leases"))
    );
    assert!(fake.scripts[0].contains(&quote("made-up'iface").unwrap()));
    assert!(fake.scripts[1].contains(network::LEASE_FILE));
}
