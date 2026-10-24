use super::*;

fn traffic(count: u16, begin: u64) -> Traffic {
    let address = Ipv4Addr::new(10, 231, 0, 100);
    let router = Ipv4Addr::new(10, 231, 0, 1);
    let mut echoes = Vec::new();
    for sequence in 1..=count {
        for reverse in [false, true] {
            for reply in [false, true] {
                let (source, destination) = if reverse ^ reply {
                    (router, address)
                } else {
                    (address, router)
                };
                echoes.push(Echo {
                    at_us: begin
                        + u64::from(sequence - 1) * 1_000_000
                        + u64::from(reverse) * 2000
                        + u64::from(reply) * 1000,
                    source,
                    destination,
                    reply,
                    identifier: if reverse { 8 } else { 7 },
                    sequence,
                });
            }
        }
    }
    Traffic {
        count,
        address,
        router,
        target_sent: count,
        target_received: count,
        target_peer_requests: count,
        target_peer_replies: count,
        router_sent: count,
        router_received: count,
        echoes,
    }
}
pub(super) fn full() -> Evidence {
    use oer_ieee80211_port::conformance::{BarrierCase, Execution, catalog::Case};
    let prepared = oer_hil_protocol::lower_mac::Prepared {
        ampdu: false,
        basic_rates: [0x8c; 16],
        basic_rates_len: 1,
    };
    let hardware = Case::all()
        .enumerate()
        .filter(|(_, case)| {
            matches!(case, Case::Group(_) | Case::Barrier(BarrierCase::RemoveKey))
                && case.execution() == Execution::Hardware
        })
        .map(|(index, case)| oer_hil_protocol::lower_mac::CaseTrace {
            case: oer_hil_protocol::lower_mac::RunCase(
                u8::try_from(index).expect("catalog ordinal"),
            ),
            trace: oer_hil_family_lower_mac::model_trace(case, &prepared, 6)
                .expect("reference trace"),
        })
        .collect();
    let joined = Station {
        present: true,
        authenticated: true,
        associated: true,
        authorized: true,
    };
    Evidence {
        baseline: Some(Baseline {
            channel: 6,
            wpa2_ccmp: true,
            ht20: true,
            he_disabled: true,
            pmf_disabled: true,
            before: Station::default(),
            joined,
            lease: Lease {
                address: Ipv4Addr::new(10, 231, 0, 100),
                remaining_seconds: 800,
            },
            target_address: Ipv4Addr::new(10, 231, 0, 100),
            target_gateway: Ipv4Addr::new(10, 231, 0, 1),
            target_prefix: 24,
            tx_timeouts: 0,
            queue_collisions: 0,
            deauth_transmissions: 1,
            final_departure_us: 140_000_000,
            after: Station::default(),
        }),
        handshake: Some(Handshake {
            epoch: 1,
            association_us: 1_000_000,
            m1_us: 1_010_000,
            m2_us: 1_020_000,
            m3_us: 1_030_000,
            m4_us: 1_040_000,
            connected_us: 1_050_000,
            router_authorized: true,
        }),
        traffic: Some(traffic(100, 10_000_000)),
        rekey: Some(Rekey {
            completed_us: 40_000_000,
            old_key_id: 1,
            new_key_id: 2,
            epoch: 1,
            previous_generation: 1,
            generation: 2,
            triggered_us: 40_500_000,
            request: Some(arp(false, 41_000_000)),
            reply: Some(arp(true, 41_002_000)),
            arp_frames: 1,
        }),
        air: Some(Air {
            protected_station_frames: 400,
            plaintext_station_frames: 0,
            first_us: 500_000,
            last_us: 141_000_000,
            captured_frames: 450,
            dropped_frames: 0,
        }),
        forced_deauth_us: Some(120_000_000),
        reconnect: Some(Handshake {
            epoch: 2,
            association_us: 121_000_000,
            m1_us: 121_010_000,
            m2_us: 121_020_000,
            m3_us: 121_030_000,
            m4_us: 121_040_000,
            connected_us: 121_050_000,
            router_authorized: true,
        }),
        resumed: Some(traffic(10, 125_000_000)),
        keys: Some(KeyEvidence {
            channel: 6,
            prepared,
            hardware,
        }),
    }
}
fn fails(e: &Evidence, gate: usize) {
    let gates = evaluate(e);
    assert!(
        !gates[gate].passed(),
        "{} unexpectedly passed",
        gates[gate].id
    );
    assert!(!gates[gate].reason.expect("failure reason").is_empty());
}
#[test]
fn all_six_gates_require_complete_independent_evidence() {
    assert!(evaluate(&full()).iter().all(Gate::passed));
    for gate in 0..6 {
        fails(&Evidence::default(), gate);
    }
    for gate in 0..6 {
        let mut e = full();
        match gate {
            0 => e.baseline = None,
            1 => e.handshake = None,
            2 => e.rekey = None,
            3 => e.air = None,
            4 => e.reconnect = None,
            _ => e.keys = None,
        }
        fails(&e, gate);
    }
}
#[test]
fn association_and_lease_must_agree_with_the_router_and_final_departure() {
    for mutation in 0..7 {
        let mut e = full();
        let b = e.baseline.as_mut().expect("baseline");
        match mutation {
            0 => b.joined.authorized = false,
            1 => b.target_address = Ipv4Addr::LOCALHOST,
            2 => b.lease.remaining_seconds = 0,
            3 => b.tx_timeouts = 1,
            4 => b.queue_collisions = 1,
            5 => b.deauth_transmissions = 0,
            _ => b.after.present = true,
        }
        fails(&e, 0);
    }
}
#[test]
fn no_m4_or_connected_cannot_pass_even_when_router_authorized() {
    for mutation in 0..5 {
        let mut e = full();
        let h = e.handshake.as_mut().expect("handshake");
        match mutation {
            0 => h.m4_us = 0,
            1 => h.m4_us = h.association_us + 1_000_001,
            2 => h.connected_us = 0,
            3 => h.m2_us = h.m1_us - 1,
            _ => h.router_authorized = false,
        }
        fails(&e, 1);
    }
}
#[test]
fn rekey_timer_or_duplicate_echoes_are_not_successful_traffic_or_rekey() {
    for mutation in 0..6 {
        let mut e = full();
        match mutation {
            0 => e.rekey = None,
            1 => e.rekey.as_mut().expect("GTK").new_key_id = 1,
            2 => e.rekey.as_mut().expect("GTK").arp_frames = 0,
            3 => e.rekey.as_mut().expect("GTK").triggered_us = 39_000_000,
            4 => e.traffic.as_mut().expect("traffic").echoes.clear(),
            _ => {
                let t = e.traffic.as_mut().expect("traffic");
                t.echoes[4] = t.echoes[0].clone();
            }
        }
        fails(&e, 2);
    }
}
#[test]
fn empty_truncated_or_plaintext_air_is_invalid() {
    for mutation in 0..5 {
        let mut e = full();
        let air = e.air.as_mut().expect("air");
        match mutation {
            0 => *air = Air::default(),
            1 => air.protected_station_frames = 0,
            2 => air.plaintext_station_frames = 1,
            3 => air.last_us = 100_000_000,
            _ => air.dropped_frames = 1,
        }
        fails(&e, 3);
    }
}
#[test]
fn reconnect_requires_a_fresh_handshake_after_forced_deauth_and_ten_pairs() {
    for mutation in 0..5 {
        let mut e = full();
        match mutation {
            0 => e.forced_deauth_us = None,
            1 => e.reconnect.as_mut().expect("reconnect").epoch = 1,
            2 => e.reconnect.as_mut().expect("reconnect").m4_us = 0,
            3 => e.resumed.as_mut().expect("traffic").router_received = 9,
            _ => e.resumed.as_mut().expect("traffic").echoes.clear(),
        }
        fails(&e, 4);
    }
}
#[test]
fn conformance_is_regenerated_from_the_model_and_cannot_pass_empty_or_duplicate_cases() {
    for mutation in 0..3 {
        let mut e = full();
        let keys = e.keys.as_mut().expect("key traces");
        match mutation {
            0 => keys.hardware.clear(),
            1 => keys.hardware[1] = keys.hardware[0].clone(),
            _ => keys.prepared.basic_rates_len = 0,
        }
        fails(&e, 5);
    }
}

fn arp(transmitted: bool, at: u64) -> ArpObservation {
    ArpObservation {
        received_us: at + 1000,
        air_us: Some(at),
        frame: oer_hil_protocol::wifi::StationArpFrame {
            epoch: 1,
            generation: 2,
            transmitted,
            group_addressed: !transmitted,
            operation: if transmitted { 2 } else { 1 },
            sender: if transmitted {
                [10, 231, 0, 100]
            } else {
                [10, 231, 0, 1]
            },
            target: if transmitted {
                [10, 231, 0, 1]
            } else {
                [10, 231, 0, 100]
            },
            sequence: if transmitted { 124 } else { 123 },
            packet_number: if transmitted { 200 } else { 1 },
            key_id: if transmitted { 0 } else { 2 },
        },
    }
}
#[test]
fn rekey_requires_both_exact_arp_frames_in_the_same_epoch_and_generation() {
    for mutation in 0..12 {
        let mut e = full();
        let r = e.rekey.as_mut().expect("GTK");
        match mutation {
            0 => r.request = None,
            1 => r.reply = None,
            2 => r.request.as_mut().expect("request").air_us = None,
            3 => r.reply.as_mut().expect("reply").air_us = None,
            4 => r.request.as_mut().expect("request").frame.operation = 2,
            5 => r.request.as_mut().expect("request").frame.key_id = 1,
            6 => r.request.as_mut().expect("request").frame.group_addressed = false,
            7 => r.reply.as_mut().expect("reply").frame.sender = [127, 0, 0, 1],
            8 => r.reply.as_mut().expect("reply").frame.generation = 1,
            9 => r.reply.as_mut().expect("reply").frame.epoch = 2,
            10 => r.reply.as_mut().expect("reply").air_us = Some(40_999_999),
            _ => r.reply.as_mut().expect("reply").air_us = Some(46_000_001),
        }
        fails(&e, 2);
    }
}

#[test]
fn rekey_orders_air_stamps_only_against_each_other() {
    let ok = |q: u64, a: u64, received: u64| {
        let mut e = full();
        let r = e.rekey.as_mut().expect("GTK");
        r.request.as_mut().expect("request").air_us = Some(q);
        r.request.as_mut().expect("request").received_us = received;
        r.reply.as_mut().expect("reply").air_us = Some(a);
        evaluate(&e)[2].passed()
    };
    // Air stamp 0.5 ms after the host received the note.
    assert!(ok(41_000_500, 41_002_000, 41_000_000));
    // Batched capture stamps may coincide.
    assert!(ok(41_001_500, 41_001_500, 41_001_000));
    // Reply before request fails.
    assert!(!ok(41_002_001, 41_002_000, 41_001_000));
}
