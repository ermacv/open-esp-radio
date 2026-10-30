use std::{
    cell::{Cell, RefCell},
    thread,
    time::Duration,
};

use oer_hil_link::{Link, test_support::Device};
use oer_hil_protocol::base::{GetHello, Hello};
use oer_lab_board::reset::{Ladder, RecoveryStep, Rung};

use super::{Board, Cleanup, Ending, Waits, finish};
use crate::{Outcome, Scenario, Verdict, run};

const IMAGE: u32 = 0x5eed_0002;
const ROM_LINE: &str = "rst:0x17 (CHIP_USB_UART_RESET),boot:0x5f (SPI_FAST_FLASH_BOOT)";
const WAITS: Waits = Waits {
    probe: Duration::from_millis(200),
    hello: Duration::from_millis(400),
};

fn hello() -> Hello {
    Hello {
        image_crc32: IMAGE,
        ..oer_hil_link::test_support::HELLO
    }
}

/// How a device on a fresh link behaves.
#[derive(Clone, Copy)]
enum Device_ {
    /// It says Hello and answers `GetHello`.
    Answers,
    /// It said Hello once and answers nothing more: a lost response.
    Mute,
    /// Nothing at all.
    Silent,
}

/// A device on its own thread behind the returned link.
fn device(boot_id: u64, behavior: Device_) -> Link {
    let (mut device, link) = Device::new(boot_id);
    if !matches!(behavior, Device_::Silent) {
        device.send(0, hello());
    }
    thread::spawn(move || {
        while let Some(request) = device.request() {
            if matches!(behavior, Device_::Answers) && request.decode::<GetHello>().is_some() {
                device.send(request.request_id, hello());
            }
        }
    });
    link
}

/// One rung of the fake board: whether the ROM and the image answer after it.
struct Step {
    step: RecoveryStep,
    rom: bool,
    image: bool,
}

/// A board whose ladder is `steps`, with an arbiter that `records` or not.
struct FakeBoard {
    steps: Vec<Step>,
    records: bool,
    image_up: Cell<bool>,
    resets: Cell<usize>,
    boots: Cell<u64>,
    quarantined: RefCell<Option<String>>,
    recovered: RefCell<Vec<RecoveryStep>>,
}

impl FakeBoard {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps,
            records: true,
            image_up: Cell::new(false),
            resets: Cell::new(0),
            boots: Cell::new(100),
            quarantined: RefCell::new(None),
            recovered: RefCell::new(Vec::new()),
        }
    }
}

struct FakeRung<'a>(&'a FakeBoard, &'a Step);

impl Rung for FakeRung<'_> {
    fn step(&self) -> RecoveryStep {
        self.1.step
    }
    fn reset(&self) -> oer_lab_board::Result<Option<String>> {
        self.0.resets.set(self.0.resets.get() + 1);
        self.0.image_up.set(self.1.image);
        Ok(self.1.rom.then(|| ROM_LINE.to_owned()))
    }
}

impl Board for FakeBoard {
    fn climb(&self, answers: &mut dyn FnMut() -> bool) -> Ladder {
        let rungs = self
            .steps
            .iter()
            .map(|step| FakeRung(self, step))
            .collect::<Vec<_>>();
        oer_lab_board::reset::climb(
            &rungs
                .iter()
                .map(|rung| rung as &dyn Rung)
                .collect::<Vec<_>>(),
            None,
            &String::new,
            answers,
        )
    }
    fn connect(&self) -> Result<Link, String> {
        self.boots.set(self.boots.get() + 1);
        let behavior = if self.image_up.get() {
            Device_::Answers
        } else {
            Device_::Silent
        };
        Ok(device(self.boots.get(), behavior))
    }
    fn quarantine(&self, reason: String) -> Result<(), String> {
        if !self.records {
            return Err("the arbiter is unavailable".to_owned());
        }
        *self.quarantined.borrow_mut() = Some(reason);
        Ok(())
    }
    fn recovered(&self, step: RecoveryStep, _reset_line: Option<String>) {
        self.recovered.borrow_mut().push(step);
    }
}

fn rts(rom: bool, image: bool) -> Step {
    Step {
        step: RecoveryStep::RtsReset,
        rom,
        image,
    }
}

fn power(rom: bool, image: bool) -> Step {
    Step {
        step: RecoveryStep::PowerCycle,
        rom,
        image,
    }
}

#[test]
fn a_device_running_the_image_needs_no_reset() {
    let board = FakeBoard::new(vec![rts(true, true)]);
    let (link, cleanup) = finish(
        &board,
        Some(device(1, Device_::Answers)),
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
        Some(device(1, Device_::Answers)),
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
fn a_lost_response_climbs_until_the_image_answers() {
    let board = FakeBoard::new(vec![rts(true, false), power(true, true)]);
    let (_, cleanup) = finish(
        &board,
        Some(device(1, Device_::Mute)),
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
        Some(device(1, Device_::Mute)),
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
    const SCENARIO: Scenario = Scenario {
        name: "test",
        image: "test-image",
        services: &[],
    };
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
