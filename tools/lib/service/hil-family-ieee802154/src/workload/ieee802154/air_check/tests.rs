use super::*;

fn request() -> Ieee802154AirCheckRequest {
    Ieee802154AirCheckRequest {
        channel: 15,
        cycles: 2,
        energy_scan_micros: 5_000,
        receive_window_millis: 200,
        scheduled_lead_micros: 20_000,
        scheduled_window_micros: 50_000,
    }
}

fn transmit(requested_at_micros: u64, done_at_micros: u64) -> Ieee802154AirTransmit {
    Ieee802154AirTransmit {
        outcome: Ieee802154AirTxOutcome::Success,
        requested_at_micros,
        done_at_micros,
    }
}

fn cycle() -> Ieee802154AirCycle {
    Ieee802154AirCycle {
        energy: Ieee802154AirEnergyOutcome::Energy(-92),
        cca: Ieee802154AirCcaOutcome::Clear,
        direct: transmit(1_000, 2_200),
        scheduled: [transmit(30_000, 31_100), transmit(60_000, 61_050)],
        received_frames: 0,
        strongest_rssi_dbm: None,
        scheduled_window: quiet_window(),
    }
}

fn quiet_window() -> Ieee802154AirWindow {
    Ieee802154AirWindow {
        ended: true,
        start_micros: 300_000,
        end_micros: 350_000,
        done_at_micros: 350_400,
        received_frames: 0,
        first_frame_at_micros: None,
    }
}

/// A quiet window ends at its end; a frame ends it early; frames before it
/// opens, an end outside its bounds or no end fail.
#[test]
fn scheduled_windows_end_at_their_end_or_first_frame() {
    validate_window(&quiet_window()).unwrap();
    let with_frame = Ieee802154AirWindow {
        received_frames: 1,
        first_frame_at_micros: Some(310_000),
        done_at_micros: 311_000,
        ..quiet_window()
    };
    validate_window(&with_frame).unwrap();

    for broken in [
        Ieee802154AirWindow {
            ended: false,
            ..quiet_window()
        },
        Ieee802154AirWindow {
            done_at_micros: 349_000,
            ..quiet_window()
        },
        Ieee802154AirWindow {
            done_at_micros: 350_000 + SCHEDULED_COMPLETION_BOUND_MICROS + 1,
            ..quiet_window()
        },
        Ieee802154AirWindow {
            first_frame_at_micros: Some(299_000),
            done_at_micros: 300_000,
            ..with_frame
        },
        Ieee802154AirWindow {
            received_frames: 0,
            ..with_frame
        },
    ] {
        assert!(validate_window(&broken).is_err(), "{broken:?}");
    }
    let mut late = nominal();
    late.cycles[1].scheduled_window.ended = false;
    assert!(validate(late, request()).is_err());
}

fn nominal() -> Ieee802154AirCheckEvidence {
    let mut evidence = Ieee802154AirCheckEvidence {
        stop: Ieee802154AirCheckStop::Complete,
        completed_cycles: 2,
        ..Default::default()
    };
    evidence.cycles[0] = cycle();
    evidence.cycles[1] = cycle();
    evidence
}

#[test]
fn nominal_cycles_pass_and_a_busy_channel_is_an_outcome() {
    validate(nominal(), request()).unwrap();
    let mut busy = nominal();
    busy.cycles[1].cca = Ieee802154AirCcaOutcome::Busy;
    busy.cycles[1].received_frames = 3;
    busy.cycles[1].strongest_rssi_dbm = Some(-40);
    validate(busy, request()).unwrap();
}

#[test]
fn incomplete_runs_fail() {
    let mut stopped = nominal();
    stopped.stop = Ieee802154AirCheckStop::StartFailed;
    assert!(validate(stopped, request()).is_err());
    let mut short = nominal();
    short.completed_cycles = 1;
    assert!(validate(short, request()).is_err());
}

#[test]
fn failed_operations_fail_their_cycle() {
    let mut scan = nominal();
    scan.cycles[0].energy = Ieee802154AirEnergyOutcome::Failed;
    assert!(validate(scan, request()).is_err());
    let mut implausible = nominal();
    implausible.cycles[0].energy = Ieee802154AirEnergyOutcome::Energy(40);
    assert!(validate(implausible, request()).is_err());
    let mut cca = nominal();
    cca.cycles[1].cca = Ieee802154AirCcaOutcome::Failed;
    assert!(validate(cca, request()).is_err());
    let mut direct = nominal();
    direct.cycles[0].direct.outcome = Ieee802154AirTxOutcome::HardwareFailure;
    assert!(validate(direct, request()).is_err());
}

#[test]
fn scheduled_transmits_must_honour_their_start() {
    let mut early = nominal();
    early.cycles[0].scheduled[0] = transmit(30_000, 29_999);
    assert!(validate(early, request()).is_err());
    let mut late = nominal();
    late.cycles[0].scheduled[1] = transmit(60_000, 60_000 + SCHEDULED_COMPLETION_BOUND_MICROS + 1);
    assert!(validate(late, request()).is_err());
    let mut overlapping = nominal();
    overlapping.cycles[0].scheduled[1] = transmit(31_000, 32_000);
    assert!(validate(overlapping, request()).is_err());
}

#[test]
fn the_command_timeout_covers_every_cycle() {
    let one = command_timeout(Ieee802154AirCheckRequest {
        cycles: 1,
        ..request()
    });
    let four = command_timeout(Ieee802154AirCheckRequest {
        cycles: 4,
        ..request()
    });
    assert!(four > one);
    assert!(one >= Duration::from_secs(25));
}
