use core::cell::RefCell;
use std::vec::Vec;

use oer_espressif_watchdog::{
    Access, LOCK, MainWatchdogEnable, RtcWatchdogRegisters, SuperWatchdogRegisters, WRITE_KEY,
    WriteProtected,
};

use crate::watchdog::BootWatchdogs;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Dog {
    Main0,
    Rtc,
    Super,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Key(Dog, u32),
    Running(Dog, bool, bool),
    Latch(Dog),
    AutoFeed(bool),
}

struct Recorder<'a> {
    dog: Dog,
    log: &'a RefCell<Vec<Op>>,
}

impl Recorder<'_> {
    fn record(&mut self, op: Op) {
        self.log.borrow_mut().push(op);
    }
}

impl WriteProtected for Recorder<'_> {
    fn store_write_key(&mut self, _: Access<'_>, key: u32) {
        let dog = self.dog;
        self.record(Op::Key(dog, key));
    }
}

impl MainWatchdogEnable for Recorder<'_> {
    fn set_running(&mut self, _: Access<'_>, enabled: bool, flash_boot: bool) {
        let dog = self.dog;
        self.record(Op::Running(dog, enabled, flash_boot));
    }
    fn latch(&mut self, _: Access<'_>) {
        let dog = self.dog;
        self.record(Op::Latch(dog));
    }
}

impl RtcWatchdogRegisters for Recorder<'_> {
    fn set_running(&mut self, _: Access<'_>, enabled: bool, flash_boot: bool) {
        let dog = self.dog;
        self.record(Op::Running(dog, enabled, flash_boot));
    }
}

impl SuperWatchdogRegisters for Recorder<'_> {
    fn set_auto_feed(&mut self, _: Access<'_>, enabled: bool) {
        self.record(Op::AutoFeed(enabled));
    }
}

#[test]
fn the_rtc_watchdog_stays_armed_until_the_hand_off_disables_it_last() {
    let log = RefCell::new(Vec::new());
    let recorder = |dog| Recorder { dog, log: &log };
    let watchdogs = BootWatchdogs::take_over(
        recorder(Dog::Main0),
        recorder(Dog::Rtc),
        recorder(Dog::Super),
    );
    assert_eq!(
        log.take(),
        [
            Op::Key(Dog::Super, WRITE_KEY),
            Op::AutoFeed(true),
            Op::Key(Dog::Super, LOCK),
            Op::Key(Dog::Main0, WRITE_KEY),
            Op::Running(Dog::Main0, false, false),
            Op::Latch(Dog::Main0),
            Op::Key(Dog::Main0, LOCK),
        ]
    );
    let _handed_off = watchdogs.hand_off();
    assert_eq!(
        log.take(),
        [
            Op::Key(Dog::Rtc, WRITE_KEY),
            Op::Running(Dog::Rtc, false, false),
            Op::Key(Dog::Rtc, LOCK),
        ]
    );
}
