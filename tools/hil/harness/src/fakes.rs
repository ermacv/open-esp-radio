//! Stand doubles for the harness's tests: devices behind real links and a
//! board whose reset ladder runs through `oer_lab_model::rom::climb`.

use std::{
    cell::{Cell, RefCell},
    thread,
    time::Duration,
};

use oer_hil_link::{Link, test_support::Device};
use oer_hil_protocol::base::{GetHello, Hello, RejectReason, Rejected};
use oer_lab_model::rom::{Ladder, RecoveryStep, Rung};

use crate::finish::{Board, Waits};

pub(crate) const IMAGE: u32 = 0x5eed_0002;
pub(crate) const ROM_LINE: &str = "rst:0x17 (CHIP_USB_UART_RESET),boot:0x5f (SPI_FAST_FLASH_BOOT)";
pub(crate) const WAITS: Waits = Waits {
    probe: Duration::from_millis(200),
    hello: Duration::from_millis(400),
};

pub(crate) fn hello() -> Hello {
    Hello {
        image_crc32: IMAGE,
        ..oer_hil_link::test_support::HELLO
    }
}

/// How a device on a fresh link behaves.
#[derive(Clone, Copy)]
pub(crate) enum Behavior {
    /// It says Hello and answers `GetHello`.
    Answers,
    /// It said Hello once and answers nothing more: a lost response.
    Mute,
    /// Nothing at all.
    Silent,
}

/// A device on its own thread behind the returned link.
pub(crate) fn device(boot_id: u64, behavior: Behavior) -> Link {
    let (mut device, link) = Device::new(boot_id);
    if !matches!(behavior, Behavior::Silent) {
        device.send(0, hello());
    }
    thread::spawn(move || answer(device, behavior));
    link
}

/// Serve `device` as `behavior` says until the host closes.
pub(crate) fn answer(mut device: Device, behavior: Behavior) {
    while let Some(request) = device.request() {
        if matches!(behavior, Behavior::Answers) && request.decode::<GetHello>().is_some() {
            device.send(request.request_id, hello());
        }
    }
}

/// One rung of the fake board: whether it restarts the device, and whether
/// the ROM and the image answer after it.
pub(crate) struct Step {
    pub step: RecoveryStep,
    pub restarts: bool,
    pub rom: bool,
    pub image: bool,
}

/// A board whose ladder is `steps`.
pub(crate) struct FakeBoard {
    pub steps: Vec<Step>,
    pub image_up: Cell<bool>,
    pub resets: Cell<usize>,
    /// The device's running boot.
    pub boots: Cell<u64>,
    /// Links the board opens next on which the running image's Hello is
    /// lost: the device shows its boot by another frame and answers nothing.
    pub hellos_lost: Cell<usize>,
    pub recovered: RefCell<Vec<RecoveryStep>>,
    /// How a device behaves on a link the board opens while its image runs.
    pub serve: fn(Device),
}

impl FakeBoard {
    pub(crate) fn new(steps: Vec<Step>) -> Self {
        Self {
            serve: |device| answer(device, Behavior::Answers),
            steps,
            image_up: Cell::new(false),
            resets: Cell::new(0),
            boots: Cell::new(100),
            hellos_lost: Cell::new(0),
            recovered: RefCell::new(Vec::new()),
        }
    }
}

struct FakeRung<'a>(&'a FakeBoard, &'a Step);

impl Rung for FakeRung<'_> {
    fn step(&self) -> RecoveryStep {
        self.1.step
    }
    fn reset(&self) -> oer_lab_model::Result<Option<String>> {
        self.0.resets.set(self.0.resets.get() + 1);
        if self.1.restarts {
            self.0.boots.set(self.0.boots.get() + 1);
            self.0.image_up.set(self.1.image);
        }
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
        oer_lab_model::rom::climb(
            &rungs
                .iter()
                .map(|rung| rung as &dyn Rung)
                .collect::<Vec<_>>(),
            None,
            &String::new,
            answers,
        )
    }
    fn reset(&self) -> Result<(), String> {
        let step = self.steps.first().ok_or("no rung")?;
        FakeRung(self, step)
            .reset()
            .map(drop)
            .map_err(|error| error.to_string())
    }
    fn connect(&self) -> Result<Link, String> {
        if !self.image_up.get() {
            return Ok(device(self.boots.get(), Behavior::Silent));
        }
        let (mut device, link) = Device::new(self.boots.get());
        if self.hellos_lost.get() > 0 {
            self.hellos_lost.set(self.hellos_lost.get() - 1);
            device.send(0, Rejected(RejectReason::Busy));
            thread::spawn(move || answer(device, Behavior::Mute));
            return Ok(link);
        }
        device.send(0, hello());
        let serve = self.serve;
        thread::spawn(move || serve(device));
        Ok(link)
    }
    fn recovered(&self, step: RecoveryStep, _reset_line: Option<String>) {
        self.recovered.borrow_mut().push(step);
    }
}

pub(crate) fn rts(rom: bool, image: bool) -> Step {
    Step {
        step: RecoveryStep::RtsReset,
        restarts: true,
        rom,
        image,
    }
}

pub(crate) fn power(rom: bool, image: bool) -> Step {
    Step {
        step: RecoveryStep::PowerCycle,
        restarts: true,
        rom,
        image,
    }
}

/// `step` taken without restarting the device: the running boot goes on.
pub(crate) fn unmoved(step: Step) -> Step {
    Step {
        restarts: false,
        ..step
    }
}
