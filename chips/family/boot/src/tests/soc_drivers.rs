//! Behavioral order, readback and error boundaries of the extracted drivers.
use super::Ticking;
use crate::{
    Access, BootFailure, ClockFailure,
    drivers::{access_control, brownout, clock, cores, entropy, soc},
    wait::{BootBudget, BootDeadline, BootWait, WaitFailure},
};
use core::cell::{Cell, RefCell};
use oer_espressif_analog::{AnalogField, AnalogRegisterBus, Busy};
use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;
use std::{rc::Rc, vec, vec::Vec};

fn wait(name: &'static str, micros: u64) -> BootWait {
    BootWait::new(name, Duration::from_micros(micros)).expect("nonzero test bound")
}
fn budget() -> BootBudget {
    BootBudget::below(Duration::from_millis(10), Duration::from_secs(9)).expect("below watchdog")
}

struct Lp<'a> {
    time: &'a VirtualClock,
    observed: u32,
    writes: Vec<(u32, Instant)>,
    reads: RefCell<Vec<Instant>>,
}
impl clock::LpFastRegisters for Lp<'_> {
    type Source = u32;
    fn write_source(&mut self, _: Access<'_>, source: u32) {
        self.writes.push((source, self.time.now()));
    }
    fn source(&self, _: Access<'_>) -> u32 {
        self.reads.borrow_mut().push(self.time.now());
        self.observed
    }
    fn source_code(source: u32) -> u32 {
        source
    }
}
#[test]
fn lp_fast_writes_once_then_settles_before_readback_including_unnamed_codes() {
    for observed in [0, 1, 2, 3, u32::MAX] {
        let time = VirtualClock::new();
        let ticking = Ticking::new(&time);
        let deadline = BootDeadline::start(&ticking, budget());
        let mut registers = Lp {
            time: &time,
            observed,
            writes: vec![],
            reads: RefCell::new(vec![]),
        };
        let profile = clock::LpFastProfile {
            source: 1,
            settle: wait("lp-fast-switch", 3),
        };
        assert_eq!(
            clock::select_lp_fast(&mut registers, profile, &deadline),
            if observed == 1 {
                Ok(())
            } else {
                Err(BootFailure::Clock(ClockFailure::LpFastReadback {
                    expected: 1,
                    observed,
                }))
            }
        );
        assert_eq!(registers.writes.len(), 1);
        assert_eq!(registers.writes[0].0, 1);
        let reads = registers.reads.borrow();
        assert_eq!(reads.len(), 1);
        assert!(
            reads[0].saturating_duration_since(registers.writes[0].1) >= Duration::from_micros(3)
        );
    }
}
#[test]
fn lp_fast_deadline_refusal_keeps_the_selector_write_and_skips_readback() {
    let time = VirtualClock::new();
    let ticking = Ticking::new(&time);
    let deadline = BootDeadline::start(&ticking, budget());
    time.advance(budget().duration()).expect("exhaust budget");
    let mut registers = Lp {
        time: &time,
        observed: 1,
        writes: vec![],
        reads: RefCell::new(vec![]),
    };
    assert_eq!(
        clock::select_lp_fast(
            &mut registers,
            clock::LpFastProfile {
                source: 1,
                settle: wait("lp-fast-switch", 3)
            },
            &deadline
        ),
        Err(WaitFailure::DeadlineExceeded("lp-fast-switch").into())
    );
    assert_eq!(registers.writes.len(), 1);
    assert!(registers.reads.borrow().is_empty());
}

type Log = Rc<RefCell<Vec<&'static str>>>;
fn settings() -> brownout::Settings<u16> {
    brownout::Settings {
        interrupt_wait: 2,
        reset_wait: 1023,
        close_flash: true,
        power_down_rf: true,
        reset_enabled: true,
        system_reset: true,
    }
}
fn brownout_profile() -> brownout::Profile<u16> {
    brownout::Profile {
        settings: settings(),
        threshold: 7,
        threshold_wait: wait("bod-threshold", 1000),
    }
}
struct Detector {
    log: Log,
    observed: brownout::Settings<u16>,
    enabled: bool,
    ignore_enable: bool,
    mode1_software: bool,
    mode1_reset: bool,
}
impl Detector {
    fn new(log: Log) -> Self {
        Self {
            log,
            observed: settings(),
            enabled: false,
            ignore_enable: false,
            mode1_software: true,
            mode1_reset: false,
        }
    }
}
impl brownout::Registers for Detector {
    type Wait = u16;
    type Address = u8;
    fn configure_mode0(&mut self, _: Access<'_>, settings: brownout::Settings<u16>) {
        assert_eq!(settings, crate::tests::soc_drivers::settings());
        self.log.borrow_mut().push("configure");
    }
    fn enable(&mut self, _: Access<'_>) {
        self.log.borrow_mut().push("enable");
        self.enabled = !self.ignore_enable;
    }
    fn mode0(&self, _: Access<'_>) -> brownout::Mode0<u16> {
        self.log.borrow_mut().push("mode0");
        brownout::Mode0 {
            settings: self.observed,
            enabled: self.enabled,
        }
    }
    fn mode1_software(&self, _: Access<'_>) -> bool {
        self.log.borrow_mut().push("mode1-software");
        self.mode1_software
    }
    fn mode1_reset(&self, _: Access<'_>) -> bool {
        self.log.borrow_mut().push("mode1-reset");
        self.mode1_reset
    }
    fn threshold_field() -> AnalogField<u8> {
        AnalogField::new(5, 2, 0).expect("three-bit test field")
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Refusal {
    None,
    ReadStart,
    ReadFinish,
    WriteStart,
    WriteFinish,
}
struct Bus {
    log: Log,
    byte: u8,
    refusal: Refusal,
}
impl AnalogRegisterBus for Bus {
    type Address = u8;
    fn try_start_read(&mut self, address: u8) -> Result<(), Busy> {
        assert_eq!(address, 5);
        if self.refusal == Refusal::ReadStart {
            return Err(Busy);
        }
        self.log.borrow_mut().push("read");
        Ok(())
    }
    fn try_finish_read(&self, address: u8) -> Result<u8, Busy> {
        assert_eq!(address, 5);
        if self.refusal == Refusal::ReadFinish {
            Err(Busy)
        } else {
            Ok(self.byte)
        }
    }
    fn try_start_write(&mut self, address: u8, value: u8) -> Result<(), Busy> {
        assert_eq!(address, 5);
        if self.refusal == Refusal::WriteStart {
            return Err(Busy);
        }
        self.log.borrow_mut().push("write");
        self.byte = value;
        Ok(())
    }
    fn try_finish_write(&self, address: u8) -> Result<(), Busy> {
        assert_eq!(address, 5);
        if self.refusal == Refusal::WriteFinish {
            Err(Busy)
        } else {
            Ok(())
        }
    }
}
fn brownout_model(refusal: Refusal) -> (Detector, Bus, Log) {
    let log = Rc::new(RefCell::new(vec![]));
    (
        Detector::new(log.clone()),
        Bus {
            log: log.clone(),
            byte: 0xf0,
            refusal,
        },
        log,
    )
}
#[test]
fn brownout_finishes_threshold_before_arming_then_samples_the_same_words_in_order() {
    let time = VirtualClock::new();
    let ticking = Ticking::new(&time);
    let deadline = BootDeadline::start(&ticking, budget());
    let (mut detector, mut bus, log) = brownout_model(Refusal::None);
    assert_eq!(
        brownout::apply(&mut detector, &mut bus, brownout_profile(), &deadline),
        Ok(())
    );
    assert_eq!(bus.byte, 0xf7, "neighbor analog lanes preserved");
    assert_eq!(
        *log.borrow(),
        [
            "configure",
            "read",
            "write",
            "enable",
            "mode0",
            "mode1-software",
            "mode1-reset"
        ]
    );
}
#[test]
fn brownout_event_enable_that_does_not_stick_is_a_readback_failure() {
    let time = VirtualClock::new();
    let ticking = Ticking::new(&time);
    let deadline = BootDeadline::start(&ticking, budget());
    let (mut detector, mut bus, log) = brownout_model(Refusal::None);
    detector.ignore_enable = true;
    assert_eq!(
        brownout::apply(&mut detector, &mut bus, brownout_profile(), &deadline),
        Err(brownout::BrownoutFailure::Readback)
    );
    assert!(!detector.enabled);
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|&&event| event == "enable")
            .count(),
        1
    );
}

#[test]
fn brownout_port_refusals_timeout_before_event_enable_or_readback() {
    for refusal in [
        Refusal::ReadStart,
        Refusal::ReadFinish,
        Refusal::WriteStart,
        Refusal::WriteFinish,
    ] {
        let time = VirtualClock::new();
        let ticking = Ticking::new(&time);
        let deadline = BootDeadline::start(&ticking, budget());
        let (mut detector, mut bus, log) = brownout_model(refusal);
        assert_eq!(
            brownout::apply(&mut detector, &mut bus, brownout_profile(), &deadline),
            Err(brownout::BrownoutFailure::Threshold(WaitFailure::TimedOut(
                "bod-threshold"
            )))
        );
        assert!(!detector.enabled);
        assert!(!log.borrow().contains(&"mode0"));
    }
}
#[test]
fn brownout_common_deadline_and_invalid_threshold_are_typed_failures() {
    let time = VirtualClock::new();
    let ticking = Ticking::new(&time);
    let deadline = BootDeadline::start(&ticking, budget());
    time.advance(budget().duration()).expect("exhaust budget");
    let (mut detector, mut bus, _) = brownout_model(Refusal::ReadFinish);
    assert_eq!(
        brownout::apply(&mut detector, &mut bus, brownout_profile(), &deadline),
        Err(brownout::BrownoutFailure::Threshold(
            WaitFailure::DeadlineExceeded("bod-threshold")
        ))
    );
    assert!(!detector.enabled);
    let mut profile = brownout_profile();
    profile.threshold = 8;
    assert_eq!(
        brownout::apply(&mut detector, &mut bus, profile, &deadline),
        Err(brownout::BrownoutFailure::ThresholdValue(
            oer_espressif_analog::ValueTooWide
        ))
    );
}
#[test]
fn brownout_readback_mismatches_keep_the_armed_owner_without_retry_or_fallback() {
    for mismatch in 0..8 {
        let time = VirtualClock::new();
        let ticking = Ticking::new(&time);
        let deadline = BootDeadline::start(&ticking, budget());
        let (mut detector, mut bus, log) = brownout_model(Refusal::None);
        match mismatch {
            0 => detector.observed.interrupt_wait = 3,
            1 => detector.observed.reset_wait = 1022,
            2 => detector.observed.close_flash = false,
            3 => detector.observed.power_down_rf = false,
            4 => detector.observed.reset_enabled = false,
            5 => detector.observed.system_reset = false,
            6 => detector.mode1_software = false,
            _ => detector.mode1_reset = true,
        }
        assert_eq!(
            brownout::apply(&mut detector, &mut bus, brownout_profile(), &deadline),
            Err(brownout::BrownoutFailure::Readback)
        );
        assert!(detector.enabled);
        assert_eq!(
            log.borrow()
                .iter()
                .filter(|&&event| event == "configure")
                .count(),
            1
        );
        assert_eq!(
            log.borrow()
                .iter()
                .filter(|&&event| event == "mode0")
                .count(),
            1
        );
    }
}

#[derive(Debug, PartialEq)]
enum Event {
    Stall(u8),
    Hold(u8),
    Clear(u8),
    Lp,
    Hp,
    Memory,
    BusClock,
    Clock,
    Reset(bool),
    Noise(u8, u8, u16, u16),
    Health(u16),
    Sample,
    Start,
}
struct Soc<'a> {
    log: Vec<Event>,
    time: &'a VirtualClock,
    advance: Cell<bool>,
}
impl cores::Registers for Soc<'_> {
    type Secondary = u8;
    const SECONDARIES: &'static [u8] = &[1, 2];
    fn request_stall(&mut self, _: Access<'_>, hart: u8) {
        self.log.push(Event::Stall(hart));
        if self.advance.replace(false) {
            self.time
                .advance(budget().duration())
                .expect("advance after admission");
        }
    }
    fn hold(&mut self, _: Access<'_>, hart: u8) {
        self.log.push(Event::Hold(hart));
    }
    fn clear_boot_address(&mut self, _: Access<'_>, hart: u8) {
        self.log.push(Event::Clear(hart));
    }
}
impl access_control::Registers for Soc<'_> {
    fn open_lp_filter(&mut self, _: Access<'_>) {
        self.log.push(Event::Lp);
    }
    fn open_hp_filter(&mut self, _: Access<'_>) {
        self.log.push(Event::Hp);
    }
    fn open_memory_filter(&mut self, _: Access<'_>) {
        self.log.push(Event::Memory);
    }
}
impl entropy::Registers for Soc<'_> {
    type Noise = u8;
    type Cutoff = u16;
    fn open_bus_clock(&mut self, _: Access<'_>) {
        self.log.push(Event::BusClock);
    }
    fn open_clock(&mut self, _: Access<'_>) {
        self.log.push(Event::Clock);
    }
    fn reset(&mut self, _: Access<'_>, asserted: bool) {
        self.log.push(Event::Reset(asserted));
    }
    fn configure_noise(&mut self, _: Access<'_>, p: &entropy::Profile<u8, u16>) {
        self.log.push(Event::Noise(
            p.noise_source,
            p.noise_position,
            p.repetition_cutoff,
            p.adaptive_cutoff,
        ));
    }
    fn configure_health_test(&mut self, _: Access<'_>, samples: u16) {
        self.log.push(Event::Health(samples));
    }
    fn enable_sampling(&mut self, _: Access<'_>) {
        self.log.push(Event::Sample);
    }
    fn start_health_test(&mut self, _: Access<'_>) {
        self.log.push(Event::Start);
    }
}
fn entropy_profile() -> entropy::Profile<u8, u16> {
    entropy::Profile {
        noise_source: 9,
        noise_position: 10,
        repetition_cutoff: 11,
        adaptive_cutoff: 12,
        startup_samples: 100,
    }
}
#[test]
fn soc_contains_every_secondary_opens_filters_then_starts_entropy_without_another_deadline_check() {
    for advance in [false, true] {
        let time = VirtualClock::new();
        let ticking = Ticking::new(&time);
        let deadline = BootDeadline::start(&ticking, budget());
        let mut registers = Soc {
            log: vec![],
            time: &time,
            advance: Cell::new(advance),
        };
        assert_eq!(
            soc::initialize(&mut registers, &entropy_profile(), &deadline),
            Ok(())
        );
        assert_eq!(
            registers.log,
            [
                Event::Stall(1),
                Event::Hold(1),
                Event::Clear(1),
                Event::Stall(2),
                Event::Hold(2),
                Event::Clear(2),
                Event::Lp,
                Event::Hp,
                Event::Memory,
                Event::BusClock,
                Event::Clock,
                Event::Reset(true),
                Event::Reset(false),
                Event::Noise(9, 10, 11, 12),
                Event::Health(100),
                Event::Sample,
                Event::Start
            ]
        );
    }
}
#[test]
fn soc_expired_deadline_has_no_register_effects() {
    let time = VirtualClock::new();
    let ticking = Ticking::new(&time);
    let deadline = BootDeadline::start(&ticking, budget());
    time.advance(budget().duration()).expect("exhaust budget");
    let mut registers = Soc {
        log: vec![],
        time: &time,
        advance: Cell::new(false),
    };
    assert_eq!(
        soc::initialize(&mut registers, &entropy_profile(), &deadline),
        Err(soc::SocFailure::Deadline(WaitFailure::DeadlineExceeded(
            "soc-init"
        )))
    );
    assert!(registers.log.is_empty());
}
