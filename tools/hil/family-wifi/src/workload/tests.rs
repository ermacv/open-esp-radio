use super::*;

fn frame(at: u64, direction: Direction, subtype: u8) -> StationFrame {
    StationFrame {
        at_us: at,
        frequency_mhz: 2437,
        direction,
        subtype,
        protected: false,
        eapol: false,
        handshake_message: None,
        replay_counter: None,
        status: None,
        key_id: None,
    }
}
fn handshake(at: u64) -> Vec<StationFrame> {
    let mut assoc = frame(at, Direction::ToStation, 1);
    assoc.status = Some(0);
    let mut out = vec![assoc];
    for message in 1..=4 {
        let mut f = frame(
            at + u64::from(message) * 10_000,
            if message % 2 == 0 {
                Direction::FromStation
            } else {
                Direction::ToStation
            },
            0x28,
        );
        f.eapol = true;
        f.handshake_message = Some(message);
        f.replay_counter = Some(if message <= 2 { 1 } else { 2 });
        out.push(f);
    }
    out
}
fn air() -> Vec<StationFrame> {
    let mut out = vec![frame(500_000, Direction::GroupFromAp, 8)];
    out.extend(handshake(1_000_000));
    let mut data = frame(10_000_000, Direction::FromStation, 0x28);
    data.protected = true;
    out.push(data);
    let mut arp = frame(41_000_000, Direction::GroupFromAp, 0x28);
    arp.protected = true;
    arp.key_id = Some(2);
    out.push(arp);
    out.extend(handshake(121_000_000));
    out.push(frame(140_000_000, Direction::FromStation, 12));
    out
}
fn connected() -> [Connected; 2] {
    [
        Connected {
            epoch: 1,
            at_us: 1_050_000,
            sequence: 20,
            associated_us: 1_005_000,
            router_authorized: true,
        },
        Connected {
            epoch: 2,
            at_us: 121_050_000,
            sequence: 50,
            associated_us: 121_005_000,
            router_authorized: true,
        },
    ]
}
#[test]
fn captured_four_way_and_new_key_group_data_are_projected_without_device_verdicts() {
    let mut e = crate::evidence::test_evidence();
    project(&mut e, &air(), 0, &connected(), 139_000_000).expect("projection");
    assert!(evaluate(&e).iter().all(Gate::passed));
}
#[test]
fn missing_m4_or_wrong_replay_counter_cannot_authorize_an_earlier_association() {
    for mutation in 0..3 {
        let mut frames = air();
        match mutation {
            0 => frames.retain(|f| f.at_us != 1_040_000),
            1 => {
                frames
                    .iter_mut()
                    .find(|f| f.at_us == 1_040_000)
                    .expect("M4")
                    .replay_counter = Some(99)
            }
            _ => frames.clear(),
        }
        let mut e = crate::evidence::test_evidence();
        project(&mut e, &frames, 0, &connected(), 139_000_000).expect("projection");
        assert!(!evaluate(&e)[1].passed());
    }
}
#[test]
fn plaintext_nonempty_air_and_wrong_group_key_id_fail_their_own_gate() {
    let mut frames = air();
    frames
        .iter_mut()
        .find(|f| f.at_us == 10_000_000)
        .expect("data")
        .protected = false;
    let mut e = crate::evidence::test_evidence();
    project(&mut e, &frames, 0, &connected(), 139_000_000).expect("projection");
    assert!(!evaluate(&e)[3].passed());
    let mut frames = air();
    frames
        .iter_mut()
        .find(|f| f.at_us == 41_000_000)
        .expect("group ARP")
        .key_id = Some(1);
    let mut e = crate::evidence::test_evidence();
    project(&mut e, &frames, 0, &connected(), 139_000_000).expect("projection");
    assert!(!evaluate(&e)[2].passed());
}
#[test]
fn capture_loss_and_missing_final_deauth_cannot_become_complete_air_evidence() {
    for mutation in 0..2 {
        let mut frames = air();
        if mutation == 0 {
            frames.retain(|f| f.subtype != 12);
        }
        let mut e = crate::evidence::test_evidence();
        project(&mut e, &frames, mutation, &connected(), 139_000_000).expect("projection");
        assert!(!evaluate(&e)[3].passed());
    }
}
#[test]
fn credentials_and_key_material_have_no_place_in_the_run_observation_or_errors() {
    let e = crate::evidence::test_evidence();
    let password = "w4-secret-passphrase-never-log";
    let command = StartStation(StationStart {
        credentials: NetworkCredentials::try_new(b"private-ssid", password.as_bytes())
            .expect("credentials"),
        power_save: StationPowerSave::Off,
    });
    let logs = format!("{command:?} {e:?} {:?}", evaluate(&e));
    let saved = serde_json::to_string(&e).expect("observation");
    for secret in [
        password,
        "PMK-secret-32-bytes",
        "PTK-secret-48-bytes",
        "GTK-secret-16-bytes",
    ] {
        assert!(!logs.contains(secret));
        assert!(!saved.contains(secret));
    }
    for field in ["passphrase", "pmk", "ptk", "gtk"] {
        let mut injected = serde_json::to_value(&e).expect("observation");
        injected[field] = serde_json::json!(password);
        let error = serde_json::from_value::<Evidence>(injected)
            .expect_err("key material is not an observation");
        assert!(!error.to_string().contains(password));
    }
}
