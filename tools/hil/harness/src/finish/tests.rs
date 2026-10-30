use oer_lab_board::reset::RecoveryStep;

use super::{Cleanup, Ending, finish};
use crate::fakes::{Behavior, FakeBoard, IMAGE, ROM_LINE, WAITS, device, power, rts, unmoved};
use crate::finish::Board;
use crate::{Outcome, Scenario, Verdict, run};

#[test]
fn a_device_running_the_image_needs_no_reset() {
    let board = FakeBoard::new(vec![rts(true, true)]);
    let (link, cleanup) = finish(
        &board,
        Some(device(1, Behavior::Answers)),
        IMAGE,
        Ending::default(),
        WAITS,
    );
    assert_eq!(cleanup, Cleanup::Clean);
    assert!(link.is_some());
    assert_eq!(board.resets.get(), 0);
}

#[test]
fn an_unconfirmed_request_is_ended_by_the_first_rung_that_brings_the_image_back() {
    let board = FakeBoard::new(vec![rts(true, true), power(true, true)]);
    let (link, cleanup) = finish(
        &board,
        Some(device(1, Behavior::Answers)),
        IMAGE,
        Ending { unconfirmed: true },
        WAITS,
    );
    assert!(
        matches!(
            cleanup,
            Cleanup::Recovered {
                step: RecoveryStep::RtsReset,
                ..
            }
        ),
        "{cleanup:?}"
    );
    assert_eq!(link.unwrap().boot(WAITS.hello).unwrap().boot_id, 101);
    assert_eq!(*board.recovered.borrow(), [RecoveryStep::RtsReset]);
}

#[test]
fn a_rung_that_did_not_restart_the_device_is_no_recovery() {
    let board = FakeBoard::new(vec![unmoved(rts(true, true)), power(true, true)]);
    board.image_up.set(true);
    let running = board.connect().unwrap();
    assert_eq!(running.boot(WAITS.hello).unwrap().boot_id, 100);
    let (link, cleanup) = finish(
        &board,
        Some(running),
        IMAGE,
        Ending { unconfirmed: true },
        WAITS,
    );
    assert!(
        matches!(
            cleanup,
            Cleanup::Recovered {
                step: RecoveryStep::PowerCycle,
                ..
            }
        ),
        "{cleanup:?}"
    );
    assert_eq!(link.unwrap().boot(WAITS.hello).unwrap().boot_id, 101);
}

#[test]
fn a_lost_response_climbs_until_the_image_answers() {
    let board = FakeBoard::new(vec![rts(true, false), power(true, true)]);
    let (_, cleanup) = finish(
        &board,
        Some(device(1, Behavior::Mute)),
        IMAGE,
        Ending::default(),
        WAITS,
    );
    assert!(
        matches!(
            cleanup,
            Cleanup::Recovered {
                step: RecoveryStep::PowerCycle,
                ..
            }
        ),
        "{cleanup:?}"
    );
    assert_eq!(board.resets.get(), 2);
    assert!(board.quarantined.borrow().is_none());
}

#[test]
fn a_rom_that_answers_keeps_the_board_in_service_when_the_image_does_not() {
    let board = FakeBoard::new(vec![rts(true, false), power(true, false)]);
    let (link, cleanup) = finish(&board, None, IMAGE, Ending::default(), WAITS);
    assert_eq!(
        cleanup,
        Cleanup::ImageFailed {
            reset_line: ROM_LINE.to_owned()
        }
    );
    assert!(link.is_none());
    assert!(board.quarantined.borrow().is_none());
}

#[test]
fn a_silent_rom_after_the_ladder_quarantines_the_board() {
    let board = FakeBoard::new(vec![rts(false, false), power(false, false)]);
    let (_, cleanup) = finish(
        &board,
        Some(device(1, Behavior::Mute)),
        IMAGE,
        Ending::default(),
        WAITS,
    );
    let Cleanup::Quarantined { reason } = &cleanup else {
        panic!("{cleanup:?}");
    };
    assert_eq!(board.quarantined.borrow().as_deref(), Some(reason.as_str()));
    assert!(reason.contains("RtsReset, PowerCycle"), "{reason}");
}

#[test]
fn a_quarantine_the_arbiter_did_not_record_is_kept_in_the_result() {
    let mut board = FakeBoard::new(vec![rts(false, false)]);
    board.records = false;
    let (_, cleanup) = finish(&board, None, IMAGE, Ending::default(), WAITS);
    assert!(
        matches!(&cleanup, Cleanup::Unrecorded { cause, .. } if cause.contains("unavailable")),
        "{cleanup:?}"
    );
}

#[test]
fn a_cleanup_keeps_a_pass_from_passing_and_never_makes_a_failure_pass() {
    const SCENARIO: Scenario = Scenario::new("test", "test-image", &[]);
    let outcome = |verdict| Outcome {
        case: Some("a"),
        verdict,
    };
    let recovered = Cleanup::Recovered {
        step: RecoveryStep::RtsReset,
        reset_line: None,
    };
    let mut out = Vec::new();
    let passed = run::summary(
        &SCENARIO,
        &[outcome(Verdict::Pass)],
        Some(&recovered),
        &mut out,
    );
    assert_eq!(passed.exit_code(), 2);
    assert!(String::from_utf8(out).unwrap().starts_with("cleanup: "));
    let failed = run::summary(
        &SCENARIO,
        &[outcome(Verdict::Fail("wrong".into()))],
        Some(&recovered),
        &mut Vec::new(),
    );
    assert_eq!(failed, Verdict::Fail("wrong".into()));
    let clean = run::summary(
        &SCENARIO,
        &[outcome(Verdict::Pass)],
        Some(&Cleanup::Clean),
        &mut Vec::new(),
    );
    assert_eq!(clean, Verdict::Pass);
}
