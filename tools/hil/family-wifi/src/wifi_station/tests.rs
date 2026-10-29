use super::*;

#[test]
fn failed_work_stops_the_station_before_returning_and_cleanup_failure_is_visible() {
    let active = std::cell::Cell::new(true);
    let result: Result<()> = stop_on_error(Err("DHCP missing".into()), || {
        active.set(false);
        Ok(None)
    });
    assert!(!active.get());
    assert_eq!(
        result.expect_err("original failure").to_string(),
        "DHCP missing"
    );
    let result = stop_on_error(Ok(42), || {
        active.set(true);
        Ok(None)
    });
    assert_eq!(result.expect("already stopped successful workload"), 42);
    assert!(!active.get());
    let result: Result<()> = stop_on_error(Err("DHCP missing".into()), || {
        Err("private remote error".into())
    });
    assert_eq!(
        result.expect_err("cleanup failed").to_string(),
        "DHCP missing; Wi-Fi station StopStation cleanup failed"
    );
    let result: Result<()> =
        stop_on_error(Err("no reconnection".into()), || Ok(Some(service_end())));
    assert_eq!(
        result.expect_err("service ended").to_string(),
        format!("no reconnection; {}", ended(&service_end()))
    );
}

fn service_end() -> StationServiceEnded {
    StationServiceEnded {
        epoch: 2,
        end: StationServiceEnd::Failed(StationServiceFault::encode(&17_u64).unwrap()),
    }
}

/// A station whose answers are fixed: `ended` before and after the stop,
/// the stop's refusal, and what the stop's wait returns.
struct Fake {
    before: Option<StationServiceEnded>,
    after: Option<StationServiceEnded>,
    refused: bool,
    waited: Option<StopEnd>,
    calls: std::cell::RefCell<Vec<&'static str>>,
}
impl Fake {
    fn new() -> Self {
        Self {
            before: None,
            after: None,
            refused: false,
            waited: None,
            calls: Default::default(),
        }
    }
}
impl StationStop for Fake {
    type Pending = ();
    fn ended(&self) -> Result<Option<StationServiceEnded>> {
        let stopped = self.calls.borrow().contains(&"stop");
        self.calls.borrow_mut().push("ended");
        Ok(if stopped {
            self.after.clone()
        } else {
            self.before.clone()
        })
    }
    fn stop(&self) -> Result<()> {
        self.calls.borrow_mut().push("stop");
        if self.refused {
            Err("device rejected wifi/station/stop: InvalidState".into())
        } else {
            Ok(())
        }
    }
    fn stopped(&self, (): ()) -> Result<StopEnd> {
        self.calls.borrow_mut().push("stopped");
        match &self.waited {
            Some(StopEnd::Stopped(counters)) => Ok(StopEnd::Stopped(*counters)),
            Some(StopEnd::Ended(end)) => Ok(StopEnd::Ended(end.clone())),
            None => Err("device did not complete wifi/station/stop".into()),
        }
    }
}

#[test]
fn a_station_whose_service_ended_is_not_stopped_or_waited_for() {
    let station = Fake {
        before: Some(service_end()),
        ..Fake::new()
    };
    assert_eq!(
        stop_station(&station).expect("ended"),
        StopEnd::Ended(service_end())
    );
    assert_eq!(*station.calls.borrow(), ["ended"]);
}

#[test]
fn a_stop_refused_after_the_service_ended_returns_the_end_at_once() {
    let station = Fake {
        after: Some(service_end()),
        refused: true,
        ..Fake::new()
    };
    assert_eq!(
        stop_station(&station).expect("ended"),
        StopEnd::Ended(service_end())
    );
    assert_eq!(*station.calls.borrow(), ["ended", "stop", "ended"]);
    // A refusal without a service end remains the stop's failure.
    let station = Fake {
        refused: true,
        ..Fake::new()
    };
    assert!(stop_station(&station).is_err());
    assert_eq!(*station.calls.borrow(), ["ended", "stop", "ended"]);
}

#[test]
fn an_admitted_stop_ends_with_its_counters_or_the_service_end() {
    let counters = StationCounters {
        deauth_transmissions: 1,
        ..Default::default()
    };
    let station = Fake {
        waited: Some(StopEnd::Stopped(counters)),
        ..Fake::new()
    };
    assert_eq!(
        stop_station(&station).expect("stopped"),
        StopEnd::Stopped(counters)
    );
    assert_eq!(*station.calls.borrow(), ["ended", "stop", "stopped"]);
    let station = Fake {
        waited: Some(StopEnd::Ended(service_end())),
        ..Fake::new()
    };
    assert_eq!(
        stop_station(&station).expect("ended"),
        StopEnd::Ended(service_end())
    );
}

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
        sequence: None,
        packet_number: None,
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
    arp.sequence = Some(123);
    arp.packet_number = Some(1);
    out.push(arp);
    let mut reply = frame(41_002_000, Direction::FromStation, 0x28);
    reply.protected = true;
    reply.key_id = Some(0);
    reply.sequence = Some(124);
    reply.packet_number = Some(200);
    out.push(reply);
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
    let password = "wifi-station-secret-passphrase-never-log";
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

#[test]
fn unrelated_group_traffic_or_missing_station_reply_cannot_prove_arp() {
    for mutation in 0..5 {
        let mut frames = air();
        match mutation {
            0 => frames.retain(|f| f.at_us != 41_000_000),
            1 => frames.retain(|f| f.at_us != 41_002_000),
            2 => {
                frames
                    .iter_mut()
                    .find(|f| f.at_us == 41_000_000)
                    .expect("request")
                    .packet_number = Some(2)
            }
            3 => {
                frames
                    .iter_mut()
                    .find(|f| f.at_us == 41_002_000)
                    .expect("reply")
                    .sequence = Some(125)
            }
            _ => {
                frames
                    .iter_mut()
                    .find(|f| f.at_us == 41_002_000)
                    .expect("reply")
                    .protected = false
            }
        }
        let mut e = crate::evidence::test_evidence();
        project(&mut e, &frames, 0, &connected(), 139_000_000).expect("projection");
        assert!(!evaluate(&e)[2].passed());
    }
}
