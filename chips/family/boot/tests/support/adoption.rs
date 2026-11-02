//! Test-only owned ports shared by the driver and its staged runner tests.
#![allow(clippy::panic, clippy::unwrap_used)]
use oer_espressif_boot::adoption::{self, InitError, Owners, Port, WatchdogParts, WatchdogProfile};
use oer_espressif_watchdog::{
    Access, MainConfig, MainWatchdogEnable, MainWatchdogRegisters, MicrosecondTick, RtcStageAction,
    RtcWatchdogArm, RtcWatchdogRegisters, SuperWatchdogRegisters, WriteProtected,
};
use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;
use std::{cell::RefCell, marker::PhantomData, vec::Vec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Cpu,
    Console,
    Clocks,
    Cores,
    Debug,
    Timer0Clock,
    Main1Clock,
    Key(u8, u32),
    Running(u8, bool, bool),
    Latch(u8),
    Hold(u32),
    Action(RtcStageAction),
    Feed,
    Finish,
    BeforeMapping,
    Mapping,
}

pub struct Log {
    pub clock: VirtualClock,
    pub events: RefCell<Vec<(Instant, Op)>>,
    pub refusal: Option<InitError>,
}
impl Log {
    pub fn new(refusal: Option<InitError>) -> Self {
        Self {
            clock: VirtualClock::new(),
            events: RefCell::new(Vec::new()),
            refusal,
        }
    }
    pub fn push(&self, op: Op) {
        self.events.borrow_mut().push((self.clock.now(), op));
        self.clock.advance(Duration::from_micros(1)).unwrap();
    }
    pub fn operations(&self) -> Vec<Op> {
        self.events.borrow().iter().map(|(_, op)| *op).collect()
    }
    pub fn owners(&self) -> Owners<Fake<'_>> {
        let part = |id| Part { log: self, id };
        Owners {
            cpu_clock: part(10),
            console: part(11),
            clocks: part(12),
            core_control: part(13),
            debug: part(14),
            watchdogs: WatchdogParts {
                main0: part(0),
                main1: part(1),
                rtc: part(2),
                super_watchdog: part(3),
            },
            resources: part(15),
        }
    }
}

pub struct Part<'a> {
    pub log: &'a Log,
    pub id: u8,
}
pub struct Fake<'a>(PhantomData<&'a ()>);
pub struct Hardware<'a> {
    pub clock: Part<'a>,
    pub resources: Part<'a>,
    pub watchdogs: adoption::Watchdogs<Part<'a>, Part<'a>, Part<'a>, Part<'a>>,
}
impl<'a> Port for Fake<'a> {
    type CpuClock = Part<'a>;
    type Console = Part<'a>;
    type ClockParts = Part<'a>;
    type Clocks = Part<'a>;
    type CoreControl = Part<'a>;
    type Debug = Part<'a>;
    type Resources = Part<'a>;
    type Main0 = Part<'a>;
    type Main1 = Part<'a>;
    type Rtc = Part<'a>;
    type Super = Part<'a>;
    type Output = Hardware<'a>;
    const WATCHDOGS: WatchdogProfile = WatchdogProfile {
        tick: match MicrosecondTick::from_source_hz(40_000_000) {
            Some(tick) => tick,
            None => panic!("integer tick"),
        },
        rtc_stage0_hold: 612_000,
    };
    fn install_console(part: Part<'a>) -> Result<(), InitError> {
        part.log.push(Op::Console);
        if part.log.refusal == Some(InitError::ConsoleInstalled) {
            Err(InitError::ConsoleInstalled)
        } else {
            Ok(())
        }
    }
    fn adopt_clocks(part: Part<'a>) -> Part<'a> {
        part.log.push(Op::Clocks);
        part
    }
    fn install_core_control(part: Part<'a>) -> Result<(), InitError> {
        part.log.push(Op::Cores);
        if part.log.refusal == Some(InitError::CoreControlInstalled) {
            Err(InitError::CoreControlInstalled)
        } else {
            Ok(())
        }
    }
    fn publish_debug_status(part: Part<'a>) {
        part.log.push(Op::Debug);
    }
    fn finish(
        resources: Part<'a>,
        clock: Part<'a>,
        watchdogs: adoption::Watchdogs<Part<'a>, Part<'a>, Part<'a>, Part<'a>>,
    ) -> Hardware<'a> {
        resources.log.push(Op::Finish);
        Hardware {
            resources,
            clock,
            watchdogs,
        }
    }
}
impl WriteProtected for Part<'_> {
    fn store_write_key(&mut self, _: Access<'_>, key: u32) {
        self.log.push(Op::Key(self.id, key));
    }
}
impl MainWatchdogEnable for Part<'_> {
    fn set_running(&mut self, _: Access<'_>, on: bool, flash_boot: bool) {
        self.log.push(Op::Running(self.id, on, flash_boot));
    }
    fn latch(&mut self, _: Access<'_>) {
        self.log.push(Op::Latch(self.id));
    }
}
impl MainWatchdogRegisters for Part<'_> {
    fn configure(&mut self, _: Access<'_>, _: MainConfig) {
        panic!("adoption must only stop main watchdogs");
    }
    fn set_prescale(&mut self, _: Access<'_>, _: u16) {
        panic!("adoption must not change the prescaler");
    }
    fn reset_divider(&mut self, _: Access<'_>) {
        panic!("adoption must not reset the divider");
    }
    fn set_stage0_timeout(&mut self, _: Access<'_>, _: u32) {
        panic!("adoption must not arm a main watchdog");
    }
    fn feed(&mut self, _: Access<'_>) {
        panic!("adoption must not feed a main watchdog");
    }
}
impl adoption::Timer0Clock for Part<'_> {
    fn start_timer0_clock(&mut self) {
        self.log.push(Op::Timer0Clock);
    }
}
impl adoption::Main1Clock for Part<'_> {
    fn start_clock(&mut self) {
        self.log.push(Op::Main1Clock);
    }
}
impl RtcWatchdogRegisters for Part<'_> {
    fn set_running(&mut self, _: Access<'_>, on: bool, flash_boot: bool) {
        self.log.push(Op::Running(self.id, on, flash_boot));
    }
}
impl RtcWatchdogArm for Part<'_> {
    fn set_stage0_hold(&mut self, _: Access<'_>, hold: u32) {
        self.log.push(Op::Hold(hold));
    }
    fn set_stage0_action(&mut self, _: Access<'_>, action: RtcStageAction) {
        self.log.push(Op::Action(action));
    }
    fn feed(&mut self, _: Access<'_>) {
        self.log.push(Op::Feed);
    }
}
impl SuperWatchdogRegisters for Part<'_> {
    fn set_auto_feed(&mut self, _: Access<'_>, _: bool) {
        panic!("leave bootstrap super-watchdog policy unchanged");
    }
}

impl adoption::CpuClock for Part<'_> {
    fn sample(&self) -> adoption::CpuRate {
        self.log.push(Op::Cpu);
        adoption::CpuRate {
            fractional: false,
            integer_hz: Some(160_000_000),
        }
    }
    fn publish(self, rate_hz: Option<u32>) {
        assert_eq!(rate_hz, Some(160_000_000));
    }
}
