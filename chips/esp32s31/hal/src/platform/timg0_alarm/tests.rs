use core::cell::{Cell, RefCell};
use std::{vec, vec::Vec};

use oer_time::{Clock, Instant};
use oer_time_virtual::VirtualClock;

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Stop,
    Enable(bool),
    Observe,
    Clear,
    Load(u64),
    Start,
}

// Model only the sourced one-shot/RAW/ENA/CLR behavior. This is not a
// measurement of silicon collision ordering or interrupt latency.
struct Model {
    clock: VirtualClock,
    start: Instant,
    interval: u64,
    running: bool,
    start_elapsed: u64,
    armed: Cell<bool>,
    pending: Cell<bool>,
    enabled: bool,
    operations: RefCell<Vec<Operation>>,
}

impl Model {
    fn new() -> Self {
        let clock = VirtualClock::new();
        Self {
            start: clock.now(),
            clock,
            interval: 0,
            running: false,
            start_elapsed: 0,
            armed: Cell::new(false),
            pending: Cell::new(false),
            enabled: false,
            operations: RefCell::new(Vec::new()),
        }
    }

    fn update(&self) {
        if self.running
            && self.armed.get()
            && self.clock.now().as_micros() - self.start.as_micros() >= self.interval
        {
            self.pending.set(true);
            self.armed.set(false);
        }
    }

    fn advance(&self, micros: u64) {
        self.clock
            .advance(Duration::from_micros(micros))
            .expect("virtual test time fits");
        self.update();
    }

    fn record(&self, operation: Operation) {
        self.operations.borrow_mut().push(operation);
    }
}

impl Registers for Model {
    fn stop(&mut self) {
        self.update();
        self.running = false;
        self.armed.set(false);
        self.record(Operation::Stop);
    }

    fn enable(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.record(Operation::Enable(enabled));
    }

    fn observe(&self) -> Observation {
        self.update();
        self.record(Operation::Observe);
        Observation {
            pending: self.pending.get(),
            enabled: self.enabled,
            asserted: self.pending.get() && self.enabled,
        }
    }

    fn clear(&mut self) {
        self.pending.set(false);
        self.record(Operation::Clear);
    }

    fn load_interval(&mut self, micros: u64) {
        self.interval = micros;
        self.record(Operation::Load(micros));
    }

    fn start(&mut self) {
        self.start = self.clock.now();
        self.running = true;
        self.armed.set(true);
        self.record(Operation::Start);
        self.advance(self.start_elapsed);
    }
}

#[test]
fn expiry_is_one_shot_and_acknowledgement_does_not_rearm_or_stop_the_counter() {
    let mut model = Model::new();
    let old = arm_after(&mut model, Duration::from_micros(40)).expect("delay fits");
    assert!(!old.pending);
    model.advance(39);
    assert!(!model.observe().pending);
    model.advance(1);
    assert!(model.observe().asserted);
    assert!(!model.armed.get());
    assert!(model.running);
    model.advance(100);
    assert!(model.observe().asserted, "unacknowledged pending repeats");
    let event = acknowledge(&mut model);
    assert!(event.pending && event.enabled && event.asserted);
    assert!(model.running && model.enabled);
    model.advance(100);
    assert!(!model.observe().pending, "no periodic rearm");
}

#[test]
fn masking_and_stopping_retain_raw_and_unmasking_redelivers() {
    let mut model = Model::new();
    let _ = arm_after(&mut model, Duration::from_micros(1)).expect("delay fits");
    model.advance(1);
    model.stop();
    assert!(model.observe().asserted, "stop alone is no acknowledgement");
    model.enable(false);
    let masked = model.observe();
    assert!(masked.pending && !masked.enabled && !masked.asserted);
    model.enable(true);
    assert!(model.observe().asserted);
    model.enable(false);
    assert!(
        acknowledge(&mut model).pending,
        "RAW authorizes masked acknowledgement"
    );
    model.enable(true);
    assert!(!model.observe().asserted);
}

#[test]
fn disarm_stops_masks_then_samples_raw_and_clears_before_rearm() {
    let mut model = Model::new();
    let _ = arm_after(&mut model, Duration::from_micros(1)).expect("delay fits");
    model.advance(1);
    model.operations.borrow_mut().clear();
    let previous = arm_after(&mut model, Duration::from_micros(20)).expect("delay fits");
    assert!(previous.pending && !previous.enabled && !previous.asserted);
    assert_eq!(
        *model.operations.borrow(),
        [
            Operation::Stop,
            Operation::Enable(false),
            Operation::Observe,
            Operation::Clear,
            Operation::Load(20),
            Operation::Start,
            Operation::Enable(true)
        ]
    );
    assert!(!model.observe().pending);
    model.advance(20);
    assert!(model.observe().pending);
    assert!(disarm(&mut model).pending);
    model.advance(100);
    assert!(!model.running && !model.enabled && !model.observe().pending);
}

#[test]
fn expiry_between_start_and_unmask_is_retained_and_delivered() {
    let mut model = Model::new();
    model.start_elapsed = 1;
    let previous = arm_after(&mut model, Duration::from_micros(1)).expect("delay fits");
    assert!(!previous.pending);
    assert!(model.observe().asserted);
    assert!(!model.armed.get());
}

#[test]
fn zero_delay_is_refused_before_register_access() {
    let mut model = Model::new();
    let requested = Duration::from_micros(0);
    assert_eq!(
        arm_after(&mut model, requested),
        Err(InvalidDelay { requested })
    );
    assert!(model.operations.borrow().is_empty());
    assert!(!model.running && !model.enabled);
}

#[test]
fn maximum_delay_fits_and_larger_input_does_not_mutate_an_existing_alarm() {
    let mut model = Model::new();
    let _ = arm_after(&mut model, Duration::from_micros(MAX_DELAY_MICROS)).expect("maximum fits");
    model.operations.borrow_mut().clear();
    let requested = Duration::from_micros(MAX_DELAY_MICROS + 1);
    assert_eq!(
        arm_after(&mut model, requested),
        Err(InvalidDelay { requested })
    );
    assert!(model.operations.borrow().is_empty());
    assert_eq!(model.interval, MAX_DELAY_MICROS);
    assert!(model.armed.get() && model.running && model.enabled);
}

#[test]
fn spurious_acknowledgement_issues_no_clear_and_preserves_a_future_alarm() {
    let mut model = Model::new();
    let _ = arm_after(&mut model, Duration::from_micros(40)).expect("delay fits");
    model.operations.borrow_mut().clear();
    assert!(!acknowledge(&mut model).pending);
    assert_eq!(*model.operations.borrow(), [Operation::Observe]);
    model.advance(40);
    assert!(model.observe().asserted);
}

#[test]
fn pac_adapter_programs_the_interval_and_preserves_other_bank_and_reserved_lanes() {
    for fill in [0, u32::MAX] {
        // Separate memories model disjoint owners; Host itself does not emulate
        // W1C/CLR or one-shot side effects (the virtual model above does).
        let mut timer_words = vec![fill; instances::timg0::SIZE / size_of::<u32>()];
        let mut bank_words = timer_words.clone();
        {
            let mut registers = AlarmRegisters {
                timer: timg::Timg0Timer0::host(&mut timer_words).expect("timer memory fits"),
                bank: timg::Timg0InterruptBank::host(&mut bank_words).expect("bank memory fits"),
            };
            let micros = (u64::from(timg::t0alarmlo::T_ALARM_LO_MAX) + 1) + 17;
            let old = arm_after(&mut registers, Duration::from_micros(micros)).expect("delay fits");
            assert_eq!(old.pending, fill != 0);
            let config = registers.timer.read_t0config();
            assert!(config.t_en() && config.t_alarm_en() && config.t_increase());
            assert!(!config.t_autoreload());
            assert_eq!(registers.timer.read_t0alarmlo().t_alarm_lo(), 17);
            assert_eq!(registers.timer.read_t0alarmhi().t_alarm_hi().get(), 1);
            assert_eq!(registers.timer.read_t0loadlo().t_load_lo(), 0);
            assert_eq!(registers.timer.read_t0loadhi().t_load_hi().get(), 0);
            let enables = registers.bank.read_int_ena_timers();
            assert!(enables.t0_int_ena());
            assert_eq!(enables.t1_int_ena(), fill != 0);
            assert_eq!(enables.wdt_int_ena(), fill != 0);
            let _ = disarm(&mut registers);
            assert!(!registers.timer.read_t0config().t_en());
            assert!(!registers.bank.read_int_ena_timers().t0_int_ena());
        }
        assert_eq!(
            timer_words[timg::t0alarmhi::OFFSET / 4] & timg::t0alarmhi::RESERVED,
            fill & timg::t0alarmhi::RESERVED
        );
        assert_eq!(
            bank_words[timg::int_raw_timers::OFFSET / 4],
            fill,
            "never writes RAW"
        );
        if fill != 0 {
            assert_eq!(
                bank_words[timg::int_clr_timers::OFFSET / 4],
                timg::int_clr_timers::T0_INT_CLR_MASK,
                "only T0 CLR"
            );
        }
    }
}
