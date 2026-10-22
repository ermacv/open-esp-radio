use super::*;

#[test]
fn image_report_round_trips_through_the_host_contract_on_success_and_failure() {
    use oer_firmware_apps_w3::report::{Lease as ImageLease, Report as ImageReport};
    let mut image = ImageReport::new(6, [2, 0, 0, 0, 0, 8], [2, 0, 0, 0, 0, 1]);
    image.authenticated = true;
    image.associated = true;
    image.lease = Some(ImageLease {
        address: Ipv4Addr::new(192, 0, 2, 8),
        router: Ipv4Addr::new(192, 0, 2, 1),
        prefix: 24,
    });
    image.sent = 100;
    image.received = 100;
    image.peer_requests = 100;
    image.peer_replies = 100;
    image.irq_free = 4096;
    image.deauth_sent = true;
    let mut console = String::new();
    image.print(&mut console).unwrap();
    assert!(
        check(&parse(&console).unwrap(), &evidence())
            .unwrap()
            .passed()
    );

    image.received = 99;
    console.clear();
    image.print(&mut console).unwrap();
    let mut observations = evidence();
    observations.echoes.retain(|echo| {
        !(echo.source == image.lease.unwrap().router
            && echo.destination == image.lease.unwrap().address
            && echo.reply
            && echo.identifier == 50
            && echo.sequence == 100)
    });
    let report = parse(&console).unwrap();
    assert_eq!(report.ping.received, 99);
    assert!(!report.claims[2]);
    assert!(!report.result);
    assert!(!check(&report, &observations).unwrap().passed());
}
use oer_lab_router::w3::{Echo, Lease, Station};

const CONSOLE: &str = "boot diagnostic\n\
OER-W3-BEGIN v=1\n\
w3.config channel=6 target=xx:xx:xx:xx:00:08 stand=xx:xx:xx:xx:00:01\n\
w3.association authenticated=1 associated=1\n\
w3.lease acquired=1 address=192.0.2.8 router=192.0.2.1 prefix=24\n\
w3.ping sent=100 received=100 interval_ms=1000\n\
w3.peer_ping requests=100 replies=100\n\
w3.stats tx_timeout=0 queue_collision=0 irq_free=4096\n\
w3.departure deauth_sent=1\n\
w3.gate id=a pass=1\n\
w3.gate id=b pass=1\n\
w3.gate id=c pass=1\n\
w3.gate id=d pass=1\n\
w3.gate id=f pass=1\n\
OER-W3-END lines=12\n\
OER-RESULT: pass w3\n";

pub(super) fn evidence() -> Evidence {
    let address = Ipv4Addr::new(192, 0, 2, 8);
    let router = Ipv4Addr::new(192, 0, 2, 1);
    let echoes = (1..=100)
        .flat_map(|sequence| {
            [false, true].into_iter().flat_map(move |direction| {
                [false, true].map(move |reply| {
                    let (source, destination) = if direction {
                        (router, address)
                    } else {
                        (address, router)
                    };
                    Echo {
                        at_us: u64::from(sequence) * 1_000_000 + u64::from(reply) * 1000,
                        source: if reply { destination } else { source },
                        destination: if reply { source } else { destination },
                        reply,
                        identifier: if direction { 60 } else { 50 },
                        sequence,
                    }
                })
            })
        })
        .collect();
    Evidence {
        schema: 1,
        target: "xx:xx:xx:xx:00:08".to_owned(),
        bssid: "xx:xx:xx:xx:00:01".to_owned(),
        channel: 6,
        open: true,
        ht20: true,
        he_disabled: true,
        before: Station::default(),
        joined: Station {
            present: true,
            authenticated: true,
            associated: true,
            authorized: true,
        },
        lease: Some(Lease {
            address,
            remaining_seconds: 3600,
        }),
        router_address: router,
        router_ping: Ping {
            sent: 100,
            received: 100,
            interval_ms: 1000,
        },
        echoes,
        capture_complete: true,
        disconnect_event: true,
        after: Station::default(),
    }
}

#[test]
fn passing_report_is_recomputed_from_router_observations() {
    let report = parse(CONSOLE).unwrap();
    let checked = check(&report, &evidence()).unwrap();
    assert!(checked.passed());
    assert_eq!(checked.irq_free, 4096);
    // IRQ free bytes are reported, not an unrequested extra W3 threshold.
    assert!(
        check(
            &parse(&CONSOLE.replace("irq_free=4096", "irq_free=1")).unwrap(),
            &evidence()
        )
        .unwrap()
        .passed()
    );
}

#[test]
fn both_firmware_ping_directions_use_adjacent_intervals_without_absolute_phase() {
    for (interval, accepted) in [(1_024_000, true), (749_999, false), (1_250_001, false)] {
        let mut e = evidence();
        for echo in &mut e.echoes {
            echo.at_us = u64::from(echo.sequence) * interval + u64::from(echo.reply) * 1000;
        }
        let mut report = parse(CONSOLE).unwrap();
        if !accepted {
            assert!(
                check(&report, &e).is_err(),
                "unchanged passing claims must contradict bad timing"
            );
            report.claims[2] = false;
            report.claims[3] = false;
            report.result = false;
        }
        let result = check(&report, &e).unwrap();
        assert_eq!(result.gates[2], accepted);
        assert_eq!(result.gates[3], accepted);
    }
}

#[test]
fn each_host_gate_failure_is_a_failure_with_honest_claims_and_a_contradiction_otherwise() {
    type Case = (usize, fn(&mut Evidence));
    let cases: [Case; 11] = [
        (0, |e| e.joined.authenticated = false),
        (0, |e| e.joined.associated = false),
        (0, |e| e.joined.authorized = false),
        (1, |e| {
            e.lease = None;
            e.echoes.clear();
        }),
        (1, |e| e.lease.as_mut().unwrap().remaining_seconds = 0),
        (2, |e| {
            e.echoes.remove(0);
        }),
        (3, |e| e.router_ping.received = 99),
        (3, |e| {
            e.echoes.remove(2);
        }),
        (4, |e| e.disconnect_event = false),
        (4, |e| e.after.present = true),
        (2, |e| e.capture_complete = false),
    ];
    for (gate, mutate) in cases {
        let mut e = evidence();
        mutate(&mut e);
        let mut report = parse(CONSOLE).unwrap();
        assert!(check(&report, &e).is_err());
        report.claims[gate] = false;
        if gate == 1 {
            report.claims[2] = false;
            report.claims[3] = false;
        }
        if !e.capture_complete {
            report.claims[3] = false;
        }
        report.result = false;
        let checked = check(&report, &e).unwrap();
        assert!(!checked.passed());
        assert!(!checked.gates[gate]);
    }
}

#[test]
fn firmware_failures_cannot_hide_behind_good_router_observations() {
    for (gate, from, to) in [
        (0, "authenticated=1", "authenticated=0"),
        (0, "associated=1", "associated=0"),
        (2, "sent=100 received=100", "sent=100 received=99"),
        (2, "interval_ms=1000", "interval_ms=500"),
        (3, "requests=100 replies=100", "requests=100 replies=99"),
        (4, "tx_timeout=0", "tx_timeout=1"),
        (4, "queue_collision=0", "queue_collision=1"),
        (4, "deauth_sent=1", "deauth_sent=0"),
    ] {
        let mut report = parse(&CONSOLE.replace(from, to)).unwrap();
        assert!(check(&report, &evidence()).is_err());
        report.claims[gate] = false;
        report.result = false;
        assert!(!check(&report, &evidence()).unwrap().passed());
    }
}

#[test]
fn strict_console_parser_rejects_each_truncation_duplicates_garbage_and_wrong_versions() {
    let lines = CONSOLE.lines().collect::<Vec<_>>();
    for end in 0..lines.len() {
        assert!(
            parse(&lines[..end].join("\n")).is_err(),
            "truncated at {end}"
        );
    }
    for text in [
        CONSOLE.replace("v=1", "v=2"),
        CONSOLE.replace("lines=12", "lines=11"),
        CONSOLE.replace("channel=6", "channel=6 channel=6"),
        CONSOLE.replace("channel=6", "channel=6 extra=1"),
        CONSOLE.replace("channel=6", "channel=0"),
        CONSOLE.replace("authenticated=1", "authenticated=2"),
        CONSOLE.replace("target=xx:xx:xx:xx:00:08", "target=02:00:00:00:00:08"),
        CONSOLE.replace("id=b", "id=a"),
        CONSOLE.replace("queue_collision=0", "queue_collision=4294967296"),
        CONSOLE.replace("received=100", "received=101"),
        CONSOLE.replace("OER-RESULT: pass w3", "OER-RESULT: pass w3 garbage"),
        format!("{CONSOLE}{CONSOLE}"),
        format!("{CONSOLE}garbage\n"),
        CONSOLE.replace("w3.ping sent=100", "garbage sent=100"),
    ] {
        assert!(parse(&text).is_err());
    }
}

#[test]
fn host_evidence_is_strict_and_cannot_be_reused_for_another_profile_or_target() {
    let report = parse(CONSOLE).unwrap();
    for mutate in [
        |e: &mut Evidence| e.schema = 2,
        |e: &mut Evidence| e.target = "xx:xx:xx:xx:00:09".to_owned(),
        |e: &mut Evidence| e.open = false,
        |e: &mut Evidence| e.he_disabled = false,
        |e: &mut Evidence| e.ht20 = false,
        |e: &mut Evidence| e.before.present = true,
        |e: &mut Evidence| e.channel = 1,
        |e: &mut Evidence| e.echoes[0].source = Ipv4Addr::new(192, 0, 2, 9),
    ] {
        let mut e = evidence();
        mutate(&mut e);
        assert!(check(&report, &e).is_err());
    }
    let mut json = serde_json::to_value(evidence()).unwrap();
    json["unknown"] = true.into();
    assert!(serde_json::from_value::<Evidence>(json).is_err());
}

#[test]
fn input_limits_apply_before_utf8_and_json_allocation() {
    assert!(read_bounded(&b"valid"[..]).is_ok());
    assert!(read_bounded(&[0xff][..]).is_err());
    assert!(read_bounded(&vec![b'x'; 256 * 1024][..]).is_ok());
    assert!(read_bounded(&vec![b'x'; 256 * 1024 + 1][..]).is_err());
}

#[test]
fn missing_dhcp_is_an_agreed_failure_and_final_marker_cannot_override_gates() {
    let text = CONSOLE
        .replace(
            "acquired=1 address=192.0.2.8 router=192.0.2.1 prefix=24",
            "acquired=0 address=0.0.0.0 router=0.0.0.0 prefix=0",
        )
        .replace("id=b pass=1", "id=b pass=0")
        .replace("id=c pass=1", "id=c pass=0")
        .replace("id=d pass=1", "id=d pass=0")
        .replace("OER-RESULT: pass w3", "OER-RESULT: fail w3");
    let report = parse(&text).unwrap();
    let mut e = evidence();
    e.lease = None;
    e.echoes.clear();
    assert_eq!(
        check(&report, &e).unwrap().gates,
        [true, false, false, false, true]
    );
    assert!(parse(&text.replace("address=0.0.0.0", "address=192.0.2.8")).is_err());
    let report = parse(&CONSOLE.replace("OER-RESULT: pass w3", "OER-RESULT: fail w3")).unwrap();
    assert!(check(&report, &evidence()).is_err());
    let mut e = evidence();
    e.joined.present = false;
    assert!(check(&parse(CONSOLE).unwrap(), &e).is_err());
}
