use super::*;

fn assessed(energy_dbm: i8, busy: bool, count: usize) -> Vec<Assessed> {
    vec![Assessed { energy_dbm, busy }; count]
}

/// A busy channel as the peer's burst makes it: strong on its channel,
/// quiet on the far one and before and after it.
fn busy_channel() -> Measurements {
    Measurements {
        baseline: assessed(-95, false, 4),
        baseline_far: assessed(-94, false, 4),
        burst: assessed(-45, true, 4),
        burst_far: assessed(-93, false, 4),
        burst_back: assessed(-46, true, 4),
        after: assessed(-94, false, 4),
        assessed_transmits: vec!["ChannelBusy".into(); 4],
        burst_rssi_dbm: vec![-44, -43, -45],
        burst_frames_received: 30,
        peer_sent: 120,
        peer_failed: 0,
    }
}

#[test]
fn a_busy_channel_that_retunes_and_clears_passes() {
    let summary = evaluate(&busy_channel()).unwrap();
    assert_eq!(summary.burst_dbm, -45);
    assert_eq!(summary.burst_rssi_dbm, -44);
    assert_eq!(summary.burst_busy, 4);
}

/// One way a measurement can miss an effect of the burst.
type Change = fn(&mut Measurements);

#[test]
fn each_missing_effect_of_the_burst_fails_the_cell() {
    let cases: [(&str, Change); 9] = [
        ("no rise", |m| m.burst = assessed(-90, true, 4)),
        ("no retune away", |m| m.burst_far = assessed(-50, false, 4)),
        ("no retune back", |m| m.burst_back = assessed(-90, true, 4)),
        ("burst never ended", |m| m.after = assessed(-50, false, 4)),
        ("energy disagrees with rssi", |m| {
            m.burst_rssi_dbm = vec![-70]
        }),
        ("no frame received", |m| m.burst_rssi_dbm.clear()),
        ("assessments not busy", |m| {
            m.burst = assessed(-45, false, 4)
        }),
        ("transmits not busy", |m| {
            m.assessed_transmits = vec!["Success".into(); 4]
        }),
        ("peer sent nothing", |m| m.peer_sent = 0),
    ];
    for (name, change) in cases {
        let mut measurements = busy_channel();
        change(&mut measurements);
        assert!(evaluate(&measurements).is_err(), "{name}");
    }
}

#[test]
fn busy_assessments_must_exceed_the_baseline_and_be_the_majority() {
    let mut measurements = busy_channel();
    measurements.burst = [assessed(-45, true, 2), assessed(-45, false, 2)].concat();
    // Two of four are the majority when the baseline was never busy.
    evaluate(&measurements).unwrap();
    measurements.baseline = assessed(-95, true, 4);
    assert!(evaluate(&measurements).is_err());
}

#[test]
fn an_incomplete_assessment_is_refused() {
    let done = Ieee802154SessionAssessment {
        result: oer_hil_protocol::ieee802154::Ieee802154SessionResult::Done,
        energy: Ieee802154AirEnergyOutcome::Energy(-60),
        cca: Ieee802154AirCcaOutcome::Busy,
    };
    assert_eq!(
        Assessed::from_assessment(done).unwrap(),
        Assessed {
            energy_dbm: -60,
            busy: true
        }
    );
    for incomplete in [
        Ieee802154SessionAssessment {
            energy: Ieee802154AirEnergyOutcome::Failed,
            ..done
        },
        Ieee802154SessionAssessment {
            cca: Ieee802154AirCcaOutcome::Failed,
            ..done
        },
        Ieee802154SessionAssessment {
            result: oer_hil_protocol::ieee802154::Ieee802154SessionResult::EventTimeout,
            ..done
        },
    ] {
        assert!(Assessed::from_assessment(incomplete).is_err());
    }
}

#[test]
fn the_burst_frame_fills_the_psdu_and_reaches_the_device() {
    let frame = burst_frame();
    assert_eq!(frame.len() + 2, 127);
    // No acknowledgement request, addressed to the device's short address.
    assert_eq!(frame[0] & 0x20, 0);
    assert_eq!(&frame[5..7], &DEVICE_SHORT.to_le_bytes());
}
