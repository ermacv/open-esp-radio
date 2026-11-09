//! The rule over synthetic runs: a model of the air and the device's
//! clocks produces the records and the capture each hypothesis predicts;
//! each test states the outcome it expects.

use super::*;
use crate::capture::{AirFrame, Capture, Tag};
use crate::device::{Blocker, Completion, Record, Run, Sample, Stops};

const AP: [u8; 6] = [0x60, 0x55, 0xf9, 1, 2, 3];
const RUN: u32 = 0x0bad_c0de;
const ROUNDS_PER_KIND: u16 = 32;
const PERIOD_US: u64 = 100_000;
const BLOCKER_LEAD_US: u64 = 1_000;
/// From publication to the start of an idle-medium transmission.
const IDLE_START_US: u64 = 60;
const BLOCKER_AIRTIME_US: u64 = 12_416;
const SUBJECT_AIRTIME_US: u64 = 1_200;

/// What writes octets 24..32 in the model.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Writer {
    Caller,
    TsfAtAir,
    LocalAtAir,
    CallerPlusElapsed,
    TsfAtHandoff,
    /// The low word only, with the AP TSF at transmission.
    LowWordTsf,
    /// The AP TSF at transmission on even rounds, K on odd ones.
    Mixed,
    /// The AP TSF at transmission, 30 us late on delayed rounds.
    TsfAtAirSkewed,
    /// The owner's counterexample: the MAC local time at transmission in
    /// the low word under a constant high word 0xdeadbeef.
    LocalUnderDeadbeef,
}

#[derive(Clone, Copy)]
struct Model {
    writers: [Writer; 3],
    /// Sniffer rate error, ppm.
    sniffer_ppm: f64,
    /// The blocker reaches the air (false: lost from the capture).
    blocker_on_air: bool,
    /// The subject is published only after the blocker completed.
    late_subject: bool,
    /// Frames of the last kind missing from the capture after this many.
    captured_per_kind: u16,
    /// The SRAM read back holds the on-air value.
    write_back: bool,
    /// The MAC's RX buffer-full count the probe reports.
    rx_full: u32,
    /// The probe's cleanup: MAC, walker, final check.
    stops: [&'static str; 3],
    /// A sniffer timestamp scattered by this much around the air, as a
    /// host receive time would be.
    sniffer_scatter_us: u64,
}

const MODEL: Model = Model {
    writers: [Writer::TsfAtAir; 3],
    sniffer_ppm: 0.0,
    blocker_on_air: true,
    late_subject: false,
    captured_per_kind: ROUNDS_PER_KIND,
    write_back: false,
    rx_full: 0,
    stops: ["inactive", "stopped", "passed"],
    sniffer_scatter_us: 0,
};

/// K of round n: no zero octet and a non-zero high word, as the probe's.
fn k_of(n: u16) -> u64 {
    let mut k = 0u64;
    for octet in 0..8u64 {
        let value = 0x11 + ((u64::from(n) * (2 * octet + 3) + 7 * octet) % 0xc0);
        k |= value << (8 * octet);
    }
    k
}

/// The device: the MAC local time runs from 0 at true time 0; the AP TSF
/// restarts from zero at `TSF_RESET`.
const TSF_RESET: u64 = 2_500_000;
const LOCAL_ORIGIN: u64 = 0;
const SNIFFER_ORIGIN: u64 = 987_654_321;

fn local(t: u64) -> u32 {
    (LOCAL_ORIGIN + t) as u32
}

fn tsf(t: u64) -> u64 {
    t - TSF_RESET
}

fn sample(t: u64) -> Sample {
    Sample {
        local_before: local(t),
        tsf: tsf(t + 1),
        local_after: local(t + 2),
    }
}

fn kind_of(index: usize) -> Kind {
    Kind::ALL[index]
}

fn simulate(model: Model) -> (Run, Capture) {
    let sniffer = |t: u64| {
        let scatter = (t / 1_000 % 7) * model.sniffer_scatter_us / 3;
        SNIFFER_ORIGIN + (t as f64 * (1.0 + model.sniffer_ppm * 1e-6)) as u64 + scatter
    };
    let start = TSF_RESET + 10;
    let mut records = Vec::new();
    let mut frames = Vec::new();
    for n in 0..ROUNDS_PER_KIND * 3 {
        let block = usize::from(n / ROUNDS_PER_KIND);
        let kind = kind_of(block);
        let delayed = n % 2 == 1;
        let k = k_of(n);
        let round = start + 100_000 + u64::from(n) * PERIOD_US;
        let (blocker, published, air) = if delayed {
            let blocker_air = round + IDLE_START_US;
            let blocker_end = blocker_air + BLOCKER_AIRTIME_US;
            let published = if model.late_subject {
                blocker_end + 2_000
            } else {
                round + BLOCKER_LEAD_US
            };
            let air = if published < blocker_end {
                blocker_end + 50
            } else {
                published + IDLE_START_US
            };
            if model.blocker_on_air {
                frames.push(AirFrame {
                    tsft: sniffer(blocker_air),
                    frame_control: 0x08,
                    transmitter: AP,
                    stamp: Some([0xa5; 8]),
                    tag: Some(Tag {
                        kind: b'x',
                        arm: b'd',
                        n,
                        run: RUN,
                        k: 0,
                    }),
                });
            }
            let blocker = Blocker {
                published: sample(round),
                done: Some(Completion {
                    at: sample(blocker_end + 100),
                    status: 0,
                    detail: 0,
                }),
                queue: 3,
            };
            (Some(blocker), published, air)
        } else {
            (None, round, round + IDLE_START_US)
        };
        let writer = model.writers[block];
        let value = match writer {
            Writer::Caller => k,
            Writer::TsfAtAir => tsf(air),
            Writer::LocalAtAir => u64::from(local(air)),
            Writer::CallerPlusElapsed => k + (air - published),
            Writer::TsfAtHandoff => tsf(published + 10),
            Writer::LowWordTsf => (k & 0xffff_ffff_0000_0000) | (tsf(air) & 0xffff_ffff),
            Writer::TsfAtAirSkewed if delayed => tsf(air) + 30,
            Writer::TsfAtAirSkewed => tsf(air),
            Writer::LocalUnderDeadbeef => 0xdead_beef_0000_0000 | u64::from(local(air)),
            Writer::Mixed if n % 2 == 0 => tsf(air),
            Writer::Mixed => k,
        };
        if n % ROUNDS_PER_KIND < model.captured_per_kind {
            frames.push(AirFrame {
                tsft: sniffer(air),
                frame_control: kind.frame_control(),
                transmitter: AP,
                stamp: Some(value.to_le_bytes()),
                tag: Some(Tag {
                    kind: kind.code(),
                    arm: if delayed { b'd' } else { b'u' },
                    n,
                    run: RUN,
                    k,
                }),
            });
        }
        let done = air + SUBJECT_AIRTIME_US + 150;
        records.push(Record {
            n,
            kind,
            delayed,
            k,
            blocker,
            published: sample(published),
            handed: sample(published + 5),
            done: Some(Completion {
                at: sample(done),
                status: 0,
                detail: 0,
            }),
            readback: Some(if model.write_back {
                value.to_le_bytes()
            } else {
                k.to_le_bytes()
            }),
        });
    }
    let end = start + 100_000 + u64::from(ROUNDS_PER_KIND * 3) * PERIOD_US;
    // Another station's beacon in the capture is ignored.
    frames.push(AirFrame {
        tsft: sniffer(start),
        frame_control: 0x80,
        transmitter: [2, 0, 0, 0, 0, 9],
        stamp: Some([1; 8]),
        tag: None,
    });
    frames.sort_by_key(|frame| frame.tsft);
    let run = Run {
        run: RUN,
        channel: 13,
        ap: AP,
        absent: [2, 0x4f, 0x45, 0, 0, 1],
        blocker_body: 1500,
        start: Some(sample(start)),
        end: Some(sample(end)),
        reported: records.len(),
        records,
        error: None,
        rx_full: model.rx_full,
        stops: Stops {
            mac: model.stops[0].to_owned(),
            dma: model.stops[1].to_owned(),
            finish: model.stops[2].to_owned(),
        },
    };
    (
        run,
        Capture {
            frames,
            without_tsft: 0,
            bad_fcs: 0,
        },
    )
}

fn outcomes(model: Model) -> Vec<Outcome> {
    let (run, capture) = simulate(model);
    decide(&run, &capture)
        .1
        .into_iter()
        .map(|verdict| verdict.outcome)
        .collect()
}

fn verdicts(model: Model) -> Vec<Verdict> {
    let (run, capture) = simulate(model);
    decide(&run, &capture).1
}

#[test]
fn the_caller_value_kept_is_h1() {
    let model = Model {
        writers: [Writer::Caller; 3],
        ..MODEL
    };
    assert_eq!(outcomes(model), vec![Outcome::CallerValue; 3]);
}

#[test]
fn the_ap_tsf_at_transmission_is_h2_through_sniffer_drift() {
    for sniffer_ppm in [0.0, 45.0, -45.0] {
        let model = Model {
            sniffer_ppm,
            ..MODEL
        };
        let verdicts = verdicts(model);
        for verdict in &verdicts {
            assert_eq!(
                verdict.outcome,
                Outcome::ApTsfAtTransmission,
                "{sniffer_ppm}"
            );
            assert_eq!(verdict.evidence.matched, 32);
            assert_eq!(verdict.evidence.decisive, 31);
            assert!(verdict.evidence.min_delay_change_us > 10_000.0);
            assert!(verdict.evidence.worst_transmission_residual_us < 5.0);
            assert_eq!(verdict.evidence.readback_kept, 32);
        }
    }
}

#[test]
fn the_local_time_at_transmission_is_h3() {
    let model = Model {
        writers: [Writer::LocalAtAir; 3],
        ..MODEL
    };
    assert_eq!(outcomes(model), vec![Outcome::LocalTimeAtTransmission; 3]);
}

#[test]
fn k_plus_the_elapsed_time_is_h4_not_a_partial_overwrite() {
    let model = Model {
        writers: [Writer::CallerPlusElapsed; 3],
        ..MODEL
    };
    let verdicts = verdicts(model);
    for verdict in &verdicts {
        assert_eq!(verdict.outcome, Outcome::CallerPlusElapsed);
        // K's high octets stay: the family decides before the mask.
        assert_ne!(verdict.evidence.unchanged, 0);
    }
}

#[test]
fn a_tsf_taken_at_the_handoff_is_not_a_stamp_at_transmission() {
    // Every value lies inside its wide [publication, completion] bracket;
    // the delay dependence refutes transmission.
    let model = Model {
        writers: [Writer::TsfAtHandoff; 3],
        ..MODEL
    };
    let verdicts = verdicts(model);
    for verdict in &verdicts {
        assert_eq!(
            verdict.outcome,
            Outcome::Other(Other::StampedAtHandoff(Family::ApTsf))
        );
        assert!(verdict.evidence.worst_transmission_residual_us > 10_000.0);
    }
}

#[test]
fn a_value_off_the_transmission_instant_by_more_than_the_budget_is_unexplained() {
    let model = Model {
        writers: [Writer::TsfAtAirSkewed; 3],
        ..MODEL
    };
    for verdict in verdicts(model) {
        assert_eq!(verdict.outcome, Outcome::Other(Other::Unexplained));
        let residual = verdict.evidence.worst_transmission_residual_us;
        assert!((29.0..31.0).contains(&residual), "{residual}");
    }
}

#[test]
fn a_low_word_overwrite_is_a_partial_overwrite() {
    let model = Model {
        writers: [Writer::LowWordTsf; 3],
        ..MODEL
    };
    assert_eq!(
        outcomes(model),
        vec![Outcome::Other(Other::PartialOverwrite { unchanged: 0xf0 }); 3]
    );
}

#[test]
fn frames_of_one_kind_following_different_rules_are_inconsistent() {
    let model = Model {
        writers: [Writer::Mixed, Writer::Caller, Writer::Caller],
        ..MODEL
    };
    assert_eq!(
        outcomes(model),
        vec![
            Outcome::Other(Other::Inconsistent),
            Outcome::CallerValue,
            Outcome::CallerValue
        ]
    );
}

#[test]
fn each_kind_is_decided_on_its_own_frames() {
    let model = Model {
        writers: [Writer::TsfAtAir, Writer::TsfAtAir, Writer::Caller],
        ..MODEL
    };
    assert_eq!(
        outcomes(model),
        vec![
            Outcome::ApTsfAtTransmission,
            Outcome::ApTsfAtTransmission,
            Outcome::CallerValue
        ]
    );
}

#[test]
fn a_subject_published_after_the_blocker_ended_is_no_delay() {
    // Condition 1: the blocker went first, but the subject's publication
    // came after its end; no pair is decisive.
    let model = Model {
        late_subject: true,
        ..MODEL
    };
    for outcome in outcomes(model) {
        assert_eq!(
            outcome,
            Outcome::Insufficient(Shortfall::Pairs { decisive: 0 })
        );
    }
}

#[test]
fn a_blocker_the_capture_lacks_confirms_no_delay() {
    let model = Model {
        blocker_on_air: false,
        ..MODEL
    };
    for outcome in outcomes(model) {
        assert_eq!(
            outcome,
            Outcome::Insufficient(Shortfall::Pairs { decisive: 0 })
        );
    }
}

#[test]
fn a_sniffer_running_off_by_more_than_the_budget_is_insufficient() {
    let model = Model {
        sniffer_ppm: 250.0,
        ..MODEL
    };
    for outcome in outcomes(model) {
        assert!(
            matches!(outcome, Outcome::Insufficient(Shortfall::SnifferRate { ppm }) if (ppm - 250.0).abs() < 1.0),
            "{outcome:?}"
        );
    }
}

#[test]
fn a_capture_holding_too_few_frames_is_insufficient() {
    let model = Model {
        captured_per_kind: 6,
        ..MODEL
    };
    for verdict in verdicts(model) {
        assert_eq!(
            verdict.outcome,
            Outcome::Insufficient(Shortfall::Frames { matched: 6 })
        );
        assert_eq!(verdict.evidence.missing, 26);
    }
}

#[test]
fn a_tag_whose_k_differs_from_the_record_stops_the_kind() {
    let (mut run, capture) = simulate(MODEL);
    run.records[3].k ^= 1;
    let (_, verdicts) = decide(&run, &capture);
    assert_eq!(
        verdicts[0].outcome,
        Outcome::Insufficient(Shortfall::TagMismatch { n: 3 })
    );
    assert_eq!(verdicts[1].outcome, Outcome::ApTsfAtTransmission);
}

#[test]
fn a_retried_subject_is_left_out_and_counted() {
    let (run, mut capture) = simulate(MODEL);
    let copy = capture
        .frames
        .iter()
        .find(|frame| frame.tag.is_some_and(|tag| tag.kind == b'b' && tag.n == 4))
        .cloned()
        .unwrap();
    capture.frames.push(AirFrame {
        tsft: copy.tsft + 400,
        ..copy
    });
    let (_, verdicts) = decide(&run, &capture);
    assert_eq!(verdicts[0].evidence.duplicates, 1);
    assert_eq!(verdicts[0].evidence.matched, 31);
    assert_eq!(verdicts[0].outcome, Outcome::ApTsfAtTransmission);
}

#[test]
fn a_run_without_its_end_clock_or_clock_separation_is_insufficient() {
    let (mut run, capture) = simulate(MODEL);
    run.end = None;
    assert_eq!(decide(&run, &capture).0, Err(Shortfall::NoEndClock));
    let (mut run, capture) = simulate(MODEL);
    let start = run.start.as_mut().unwrap();
    start.tsf = u64::from(start.local_before) - 10;
    assert!(matches!(
        decide(&run, &capture).0,
        Err(Shortfall::ClockSeparation { .. })
    ));
}

#[test]
fn a_buffer_the_hardware_wrote_back_is_reported() {
    let model = Model {
        write_back: true,
        ..MODEL
    };
    for verdict in verdicts(model) {
        assert_eq!(verdict.evidence.readback_air, 32);
        assert_eq!(verdict.evidence.readback_kept, 0);
    }
}

#[test]
fn the_owners_local_time_under_a_constant_high_word_is_other_behaviour() {
    // The low word follows the MAC local time at transmission, but H3
    // predicts a zero high word: 0xdeadbeef is explained by nothing.
    let model = Model {
        writers: [Writer::LocalUnderDeadbeef; 3],
        ..MODEL
    };
    assert_eq!(outcomes(model), vec![Outcome::Other(Other::Unexplained); 3]);
}

#[test]
fn an_rx_overflow_withholds_every_outcome() {
    let model = Model {
        rx_full: 3,
        ..MODEL
    };
    for verdict in verdicts(model) {
        assert_eq!(
            verdict.outcome,
            Outcome::Insufficient(Shortfall::RxOverflow { count: 3 })
        );
        assert_eq!(
            verdict.evidence.withheld,
            Some(Outcome::ApTsfAtTransmission)
        );
    }
}

#[test]
fn an_unconfirmed_cleanup_or_a_probe_error_withholds_every_outcome() {
    let model = Model {
        stops: ["still-active", "unconfirmed", "not-run"],
        writers: [Writer::Caller; 3],
        ..MODEL
    };
    for verdict in verdicts(model) {
        assert!(
            matches!(
                &verdict.outcome,
                Outcome::Insufficient(Shortfall::DeviceRun { mac, dma, finish, error: None })
                    if mac == "still-active" && dma == "unconfirmed" && finish == "not-run"
            ),
            "{:?}",
            verdict.outcome
        );
        assert_eq!(verdict.evidence.withheld, Some(Outcome::CallerValue));
    }
    let (mut run, capture) = simulate(MODEL);
    run.error = Some("completion-timeout".to_owned());
    for verdict in decide(&run, &capture).1 {
        assert!(matches!(
            verdict.outcome,
            Outcome::Insufficient(Shortfall::DeviceRun { error: Some(_), .. })
        ));
    }
}

#[test]
fn a_sniffer_timestamp_that_does_not_follow_the_air_is_insufficient() {
    let model = Model {
        sniffer_scatter_us: 300,
        ..MODEL
    };
    let (run, capture) = simulate(model);
    assert!(
        matches!(decide(&run, &capture).0, Err(Shortfall::SnifferJitter { micros }) if micros > 50.0)
    );
}
