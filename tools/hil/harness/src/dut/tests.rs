use super::{Device, Dut};
use crate::fakes::{FakeBoard, HELLO, IMAGE, rts, unmoved};
use crate::finish::Board;

#[test]
fn a_reset_counts_only_when_another_boot_answers() {
    let board = FakeBoard::new(vec![unmoved(rts(true, true))]);
    board.image_up.set(true);
    let link = board.connect().unwrap();
    assert_eq!(link.boot(HELLO).unwrap().boot_id, 100);
    let mut dut = Dut::new(Some(link), &board, IMAGE).with_hello(HELLO);
    let refused = dut.reset();
    assert!(
        refused
            .as_ref()
            .is_err_and(|reason| reason.contains("did not restart")),
        "{refused:?}"
    );
    // The device runs on, and its link serves the cases that follow.
    assert_eq!(
        dut.link().and_then(oer_hil_link::Link::running_boot_id),
        Some(100)
    );
}

#[test]
fn a_reset_into_a_new_boot_of_the_image_continues_on_it() {
    let board = FakeBoard::new(vec![rts(true, true)]);
    board.image_up.set(true);
    let link = board.connect().unwrap();
    let mut dut = Dut::new(Some(link), &board, IMAGE).with_hello(HELLO);
    assert_eq!(dut.reset().map(|boot| boot.boot_id), Ok(101));
}

#[test]
fn a_failed_reset_keeps_the_boot_a_later_reset_must_leave() {
    let board = FakeBoard::new(vec![unmoved(rts(true, true))]);
    board.image_up.set(true);
    let link = board.connect().unwrap();
    assert_eq!(link.boot(HELLO).unwrap().boot_id, 100);
    let mut dut = Dut::new(Some(link), &board, IMAGE).with_hello(HELLO);
    // The console stays silent past the wait: the reset fails without a link.
    board.image_up.set(false);
    assert!(dut.reset().is_err());
    assert!(dut.link().is_none());
    // Boot 100 answers the next reset, which did not restart it.
    board.image_up.set(true);
    let refused = dut.reset();
    assert!(
        refused
            .as_ref()
            .is_err_and(|reason| reason.contains("did not restart")),
        "{refused:?}"
    );
}

#[test]
fn a_boot_seen_without_its_hello_is_the_one_a_later_reset_must_leave() {
    let board = FakeBoard::new(vec![unmoved(rts(true, true))]);
    board.image_up.set(true);
    let link = board.connect().unwrap();
    assert_eq!(link.boot(HELLO).unwrap().boot_id, 100);
    let mut dut = Dut::new(Some(link), &board, IMAGE).with_hello(HELLO);
    // The device restarts into boot 101 during the reset, whose Hello is lost.
    board.boots.set(101);
    board.hellos_lost.set(1);
    assert!(dut.reset().is_err());
    assert_eq!(dut.last_boot(), Some(101));
    // Boot 101 answers the next reset, which did not restart it.
    let refused = dut.reset();
    assert!(
        refused
            .as_ref()
            .is_err_and(|reason| reason.contains("did not restart")),
        "{refused:?}"
    );
}

#[test]
fn a_restart_of_the_device_continues_on_its_new_boot_without_a_board_reset() {
    let board = FakeBoard::new(vec![rts(true, true)]);
    board.image_up.set(true);
    let link = board.connect().unwrap();
    assert_eq!(link.boot(HELLO).unwrap().boot_id, 100);
    let mut dut = Dut::new(Some(link), &board, IMAGE).with_hello(HELLO);
    // The device restarts by itself into boot 101.
    board.boots.set(101);
    assert_eq!(dut.follow_restart().map(|boot| boot.boot_id), Ok(101));
    assert_eq!(board.resets.get(), 0);
}

#[test]
fn a_device_that_did_not_restart_keeps_its_boot_and_link() {
    let board = FakeBoard::new(vec![rts(true, true)]);
    board.image_up.set(true);
    let link = board.connect().unwrap();
    assert_eq!(link.boot(HELLO).unwrap().boot_id, 100);
    let mut dut = Dut::new(Some(link), &board, IMAGE).with_hello(HELLO);
    let refused = dut.follow_restart();
    assert!(
        refused
            .as_ref()
            .is_err_and(|reason| reason.contains("did not restart")),
        "{refused:?}"
    );
    assert_eq!(
        dut.link().and_then(oer_hil_link::Link::running_boot_id),
        Some(100)
    );
    assert_eq!(board.resets.get(), 0);
}
