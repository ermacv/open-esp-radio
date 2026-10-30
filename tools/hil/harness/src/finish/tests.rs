use oer_lab_model::rom::RecoveryStep;

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
        Ending {
            unconfirmed: true,
            last_boot: None,
        },
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
        Ending {
            unconfirmed: true,
            last_boot: None,
        },
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
}

#[test]
fn a_silent_rom_after_the_ladder_leaves_the_board_to_a_person() {
    let board = FakeBoard::new(vec![rts(false, false), power(false, false)]);
    let (_, cleanup) = finish(
        &board,
        Some(device(1, Behavior::Mute)),
        IMAGE,
        Ending::default(),
        WAITS,
    );
    let Cleanup::Unreachable { reason } = &cleanup else {
        panic!("{cleanup:?}");
    };
    assert!(reason.contains("RtsReset, PowerCycle"), "{reason}");
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

#[test]
fn a_boot_seen_before_a_failed_reset_is_no_recovery() {
    let board = FakeBoard::new(vec![unmoved(rts(true, true)), power(true, true)]);
    board.image_up.set(true);
    let running = board.connect().unwrap();
    assert_eq!(running.boot(WAITS.hello).unwrap().boot_id, 100);
    let mut dut = crate::dut::Dut::new(Some(running), &board, IMAGE).with_hello(WAITS.hello);
    // The console stays silent past the wait: the reset fails without a link.
    board.image_up.set(false);
    assert!(crate::dut::Device::reset(&mut dut).is_err());
    assert!(crate::dut::Device::link(&dut).is_none());
    // Boot 100 answers again after a rung that did not restart it.
    board.image_up.set(true);
    let ending = Ending {
        unconfirmed: false,
        last_boot: dut.last_boot(),
    };
    let (link, cleanup) = finish(&board, dut.into_link(), IMAGE, ending, WAITS);
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
    assert_eq!(link.and_then(|link| link.running_boot_id()), Some(101));
}

#[test]
fn a_boot_a_rung_showed_without_its_hello_is_no_recovery_on_the_next_rung() {
    let board = FakeBoard::new(vec![
        unmoved(rts(true, true)),
        unmoved(rts(true, true)),
        power(true, true),
    ]);
    // Boot 101 runs, and its Hello is lost on the first rung's console.
    board.image_up.set(true);
    board.boots.set(101);
    board.hellos_lost.set(1);
    let ending = Ending {
        unconfirmed: true,
        last_boot: Some(100),
    };
    let (link, cleanup) = finish(&board, None, IMAGE, ending, WAITS);
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
    assert_eq!(link.and_then(|link| link.running_boot_id()), Some(102));
}

#[test]
fn a_boot_the_probe_saw_without_its_hello_is_no_recovery() {
    let board = FakeBoard::new(vec![unmoved(rts(true, true)), power(true, true)]);
    // Boot 100 answers its Hello, then restarts into boot 101 on the probe's
    // request and loses that boot's Hello.
    let (mut device, running) = oer_hil_link::test_support::Device::new(100);
    device.send(0, crate::fakes::hello());
    std::thread::spawn(move || {
        if device.request().is_some() {
            device.reboot_without_hello(101);
            device.send(
                0,
                oer_hil_protocol::base::Rejected(oer_hil_protocol::base::RejectReason::Busy),
            );
        }
        while device.request().is_some() {}
    });
    assert_eq!(running.boot(WAITS.hello).unwrap().boot_id, 100);
    board.image_up.set(true);
    board.boots.set(101);
    let (link, cleanup) = finish(&board, Some(running), IMAGE, Ending::default(), WAITS);
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
    assert_eq!(link.and_then(|link| link.running_boot_id()), Some(102));
}

#[test]
fn a_lease_the_lab_ended_unconfirmed_keeps_a_clean_device_from_passing() {
    assert_eq!(Cleanup::Clean.released(Ok(())), Cleanup::Clean);
    let problem = Cleanup::Clean
        .released(Err(
            "board:02:00:00:00:00:0a (its ROM did not answer)".to_owned()
        ))
        .problem()
        .unwrap();
    assert!(
        problem.contains("left clean") && problem.contains("its ROM did not answer"),
        "{problem}"
    );
}
