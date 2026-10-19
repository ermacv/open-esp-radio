use super::*;

fn config() -> w1::Config {
    w1::Config {
        channel: 13,
        country: *b"DE",
        bssid: Mac::new([2, 0, 0, 0, 0, 2]).expect("AP"),
        ssid: w1::Ssid::new(b"Lab").expect("SSID"),
    }
}
fn target() -> Mac {
    Mac::new([2, 0, 0, 0, 0, 1]).expect("target")
}
fn console() -> Vec<String> {
    let mut rows = vec!["OER-W2-BEGIN v=1".to_owned(),
        "w2.config channel=13 country=DE cap=10 antenna_mbm=233 chip_mbm=2100 target=xx:xx:xx:xx:00:01 stand=xx:xx:xx:xx:00:02".to_owned()];
    for channel in 1..=13 {
        rows.push(format!("w2.channel channel={channel} sequence={} allowed=1 sent=1 success=1 responses={} rate_kbps=1000 retune_us=1100 power=10,0,10,10 ceiling=10",channel-1,u8::from(channel==13)));
    }
    rows.push("w2.basic supported=8284 extended=-".to_owned());
    rows.push("w2.unicast rate_kbps=1000 sequence=13 success=1 responses=1 attempts=1 raw=0 power=10,0,10,10 ceiling=10".to_owned());
    rows.push(
        "w2.stats timeouts=0 collisions=0 colors=0 errors=0 irq_free=16000 error=-".to_owned(),
    );
    for gate in ['a', 'b', 'c', 'd', 'e'] {
        rows.push(format!("w2.gate name={gate} pass=1"));
    }
    rows.push("OER-W2-END lines=22".to_owned());
    rows
}
fn air() -> Vec<Frame> {
    let target = target().octets();
    let ap = config().bssid.octets();
    let broadcast = Frame {
        at_us: 1_000_000,
        frequency_mhz: 2472,
        rate_mbps: Some(1.0),
        subtype: 4,
        source: Some(target),
        receiver: [255; 6],
        bssid: Some([255; 6]),
        sequence: Some(12),
        retry: false,
        ssid: b"Lab".to_vec(),
    };
    let mut individual = broadcast.clone();
    individual.receiver = ap;
    individual.bssid = Some(ap);
    individual.sequence = Some(13);
    individual.at_us += 100_000;
    let mut ack = individual.clone();
    ack.at_us += 600;
    ack.subtype = 29;
    ack.source = None;
    ack.receiver = target;
    ack.bssid = None;
    ack.sequence = None;
    ack.ssid.clear();
    let mut response = individual.clone();
    response.at_us += 2000;
    response.subtype = 5;
    response.source = Some(ap);
    response.receiver = target;
    vec![broadcast, individual, ack, response]
}
#[test]
fn host_recomputes_all_gates_and_requires_independent_air_evidence() {
    let checked = check(
        &console(),
        "OER-RESULT: pass w2",
        &config(),
        target(),
        &air(),
    )
    .expect("report");
    assert!(checked.passed);
    assert!(
        !check(&console(), "OER-RESULT: pass w2", &config(), target(), &[])
            .expect("report without air")
            .passed
    );
}
#[test]
fn a_wrong_rate_retry_ack_or_response_fails_the_air_gates() {
    for index in 0..4 {
        let mut air = air();
        match index {
            0 => air[0].rate_mbps = Some(6.0),
            1 => air[1].retry = true,
            2 => air[2].receiver = [3; 6],
            _ => air[3].ssid = b"Other".to_vec(),
        }
        assert!(
            !check(&console(), "OER-RESULT: pass w2", &config(), target(), &air)
                .expect("report")
                .passed
        );
    }
}
#[test]
fn power_rate_country_or_gate_contradictions_are_rejected() {
    for (old, new) in [
        ("power=10,0,10,10", "power=11,0,10,10"),
        ("supported=8284", "supported=8c98"),
        ("country=DE", "country=US"),
        ("raw=0", "raw=5"),
        ("name=d pass=1", "name=d pass=0"),
    ] {
        let rows = console()
            .into_iter()
            .map(|line| line.replace(old, new))
            .collect::<Vec<_>>();
        assert!(check(&rows, "OER-RESULT: pass w2", &config(), target(), &air()).is_err());
    }
}
#[test]
fn duplicate_truncated_and_malformed_rows_never_pass() {
    let mut rows = console();
    rows[2] = rows[3].clone();
    assert!(check(&rows, "OER-RESULT: pass w2", &config(), target(), &air()).is_err());
    rows = console();
    rows.pop();
    assert!(check(&rows, "OER-RESULT: pass w2", &config(), target(), &air()).is_err());
    for value in ["", "w2.config", "w2.config channel=x", "OER-W2-BEGIN v=1"] {
        assert!(
            check(
                &[value.to_owned()],
                "OER-RESULT: pass w2",
                &config(),
                target(),
                &air()
            )
            .is_err()
        );
    }
}

#[test]
fn an_adjacent_channels_probe_is_correlated_by_sequence_not_observer_frequency() {
    let mut air = air();
    let mut adjacent = air[0].clone();
    adjacent.sequence = Some(11);
    air.insert(0, adjacent);
    assert!(
        check(&console(), "OER-RESULT: pass w2", &config(), target(), &air)
            .expect("sequence correlation")
            .passed
    );
}

#[test]
fn an_unplanned_or_repeated_broadcast_cannot_satisfy_the_scan_policy() {
    for sequence in [12, 14] {
        let mut air = air();
        let mut extra = air[0].clone();
        extra.sequence = Some(sequence);
        air.push(extra);
        assert!(
            !check(&console(), "OER-RESULT: pass w2", &config(), target(), &air)
                .expect("scan evidence")
                .passed
        );
    }
}
