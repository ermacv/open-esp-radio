use super::*;
use std::vec;

fn interval(recorder: &mut Recorder, scope: Scope, kind: Kind, requested: u64, elapsed: u64) {
    recorder.observe(
        scope,
        kind,
        Event::Started {
            requested_micros: requested,
        },
    );
    recorder.observe(
        scope,
        kind,
        Event::Completed {
            elapsed_micros: elapsed,
            lateness_micros: elapsed - requested,
        },
    );
}

#[test]
fn separates_wait_causes_and_existing_sar_samples() {
    let mut recorder = Recorder::default();
    interval(&mut recorder, Scope::Pbus, Kind::BusBusy, 1, 3);
    interval(&mut recorder, Scope::Pbus, Kind::Completion, 1, 5);
    interval(&mut recorder, Scope::Search, Kind::Settle, 10, 14);
    interval(&mut recorder, Scope::Tone, Kind::Settle, 1, 2);
    interval(&mut recorder, Scope::Sar, Kind::Settle, 2, 4);
    interval(&mut recorder, Scope::Root, Kind::Settle, 1, 7);
    recorder.sar_ready(false);
    recorder.sar_ready(false);
    recorder.sar_ready(true);
    assert!(recorder.is_complete());
    let report = recorder.report();
    assert_eq!(report.pbus.bus_busy, 1);
    assert_eq!(report.pbus.timing.count, 2);
    assert_eq!(report.pbus.timing.elapsed_micros, 8);
    assert_eq!(report.pbus.timing.maximum_lateness_micros, 4);
    assert_eq!(report.search.requested_micros, 10);
    assert_eq!(report.tone.elapsed_micros, 2);
    assert_eq!(report.sar.elapsed_micros, 4);
    assert_eq!(report.root.elapsed_micros, 7);
    assert_eq!((report.sar_ready, report.sar_not_ready), (1, 2));
}

#[test]
fn incomplete_mismatched_or_impossible_intervals_fail_closed() {
    let start = Event::Started {
        requested_micros: 1,
    };
    let end = Event::Completed {
        elapsed_micros: 3,
        lateness_micros: 2,
    };
    for events in [
        vec![(Scope::Tone, Kind::Settle, start)],
        vec![(Scope::Tone, Kind::Settle, end)],
        vec![
            (Scope::Tone, Kind::Settle, start),
            (Scope::Sar, Kind::Settle, end),
        ],
        vec![
            (Scope::Tone, Kind::Settle, start),
            (Scope::Tone, Kind::Settle, start),
            (Scope::Tone, Kind::Settle, end),
        ],
        vec![(Scope::Pbus, Kind::Settle, start)],
        vec![(Scope::Tone, Kind::Completion, start)],
        vec![
            (Scope::Tone, Kind::Settle, start),
            (
                Scope::Tone,
                Kind::Settle,
                Event::Completed {
                    elapsed_micros: 0,
                    lateness_micros: 0,
                },
            ),
        ],
    ] {
        let mut recorder = Recorder::default();
        for (scope, kind, event) in events {
            recorder.observe(scope, kind, event);
        }
        assert!(!recorder.is_complete());
    }
}

#[test]
fn counter_and_duration_overflow_fail_closed() {
    let mut recorder = Recorder::default();
    for _ in 0..=u16::MAX {
        recorder.sar_ready(false);
    }
    assert!(!recorder.is_complete());
    let mut recorder = Recorder::default();
    interval(
        &mut recorder,
        Scope::Search,
        Kind::Settle,
        u64::from(u32::MAX),
        u64::from(u32::MAX),
    );
    interval(&mut recorder, Scope::Search, Kind::Settle, 1, 1);
    assert!(!recorder.is_complete());
}
