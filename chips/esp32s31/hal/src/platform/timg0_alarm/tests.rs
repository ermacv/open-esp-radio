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
    ClearTimer0,
    Load(u64),
    Start,
}

// Virtual hardware is external to its two software owners. It models the
// sourced RAW/ST/ENA/CLR effects; reload is an IDF assumption, not a silicon test.
struct Hardware {
    clock: VirtualClock,
    start: Cell<Instant>,
    interval: Cell<u64>,
    running: Cell<bool>,
    start_elapsed: Cell<u64>,
    armed: Cell<bool>,
    timer0_pending: Cell<bool>,
    timer1_pending: Cell<bool>,
    watchdog_pending: Cell<bool>,
    enabled: Cell<bool>,
    operations: RefCell<Vec<Operation>>,
}

struct Program<'a>(&'a Hardware);
struct Interrupt<'a>(&'a Hardware);

impl Hardware {
    fn new() -> Self {
        let clock = VirtualClock::new();
        Self {
            start: Cell::new(clock.now()),
            clock,
            interval: Cell::new(0),
            running: Cell::new(false),
            start_elapsed: Cell::new(0),
            armed: Cell::new(false),
            timer0_pending: Cell::new(false),
            timer1_pending: Cell::new(false),
            watchdog_pending: Cell::new(false),
            enabled: Cell::new(false),
            operations: RefCell::new(Vec::new()),
        }
    }

    fn owners(&self) -> (Program<'_>, Interrupt<'_>) {
        (Program(self), Interrupt(self))
    }

    fn update(&self) {
        if self.running.get()
            && self.armed.get()
            && self.clock.now().as_micros() - self.start.get().as_micros() >= self.interval.get()
        {
            self.timer0_pending.set(true);
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

impl ProgramRegisters for Program<'_> {
    fn stop(&mut self) {
        self.0.update();
        self.0.running.set(false);
        self.0.armed.set(false);
        self.0.record(Operation::Stop);
    }

    fn enable(&mut self, enabled: bool) {
        self.0.enabled.set(enabled);
        self.0.record(Operation::Enable(enabled));
    }

    fn load_interval(&mut self, micros: u64) {
        self.0.interval.set(micros);
        self.0.record(Operation::Load(micros));
    }

    fn start(&mut self) {
        self.0.start.set(self.0.clock.now());
        self.0.running.set(true);
        self.0.armed.set(true);
        self.0.record(Operation::Start);
        self.0.advance(self.0.start_elapsed.get());
    }
}

impl InterruptRegisters for Interrupt<'_> {
    fn observe(&self) -> Observation {
        self.0.update();
        self.0.record(Operation::Observe);
        Observation {
            pending: self.0.timer0_pending.get(),
            asserted: self.0.timer0_pending.get() && self.0.enabled.get(),
        }
    }

    fn clear_timer0(&mut self) {
        self.0.timer0_pending.set(false);
        self.0.record(Operation::ClearTimer0);
    }
}

impl Interrupt<'_> {
    fn timer1_pending(&self) -> bool {
        self.0.timer1_pending.get()
    }
    fn watchdog_pending(&self) -> bool {
        self.0.watchdog_pending.get()
    }
}

#[test]
fn expiry_is_one_shot_and_irq_acknowledgement_does_not_rearm_or_stop_program() {
    let hardware = Hardware::new();
    let (mut program, mut irq) = hardware.owners();
    arm_after(&mut program, Duration::from_micros(40)).expect("delay fits");
    hardware.advance(39);
    assert!(!irq.observe().pending);
    hardware.advance(1);
    assert!(irq.observe().asserted);
    assert!(!hardware.armed.get() && hardware.running.get());
    hardware.advance(100);
    assert!(irq.observe().asserted, "unacknowledged pending repeats");
    assert!(acknowledge(&mut irq).pending);
    assert!(hardware.running.get() && hardware.enabled.get());
    hardware.advance(100);
    assert!(!irq.observe().pending, "no periodic rearm");
}

#[test]
fn program_stop_and_mask_retain_raw_for_permanent_irq_owner() {
    let hardware = Hardware::new();
    let (mut program, mut irq) = hardware.owners();
    arm_after(&mut program, Duration::from_micros(1)).expect("delay fits");
    hardware.advance(1);
    program.stop();
    assert!(irq.observe().asserted, "stop is no acknowledgement");
    program.enable(false);
    assert!(irq.observe().pending && !irq.observe().asserted);
    program.enable(true);
    assert!(irq.observe().asserted, "unmask redelivers retained RAW");
    disarm(&mut program);
    assert!(irq.observe().pending && !irq.observe().asserted);
    assert!(acknowledge(&mut irq).pending);
    program.enable(true);
    assert!(!irq.observe().asserted);
}

#[test]
fn irq_acknowledges_timer0_and_preserves_other_pending_causes() {
    let hardware = Hardware::new();
    hardware.timer0_pending.set(true);
    hardware.timer1_pending.set(true);
    hardware.watchdog_pending.set(true);
    let (_, mut irq) = hardware.owners();
    assert!(acknowledge(&mut irq).pending);
    assert!(!irq.observe().pending);
    assert!(irq.timer1_pending() && irq.watchdog_pending());
}

#[test]
fn handler_acknowledges_then_rearms_the_movable_program_owner() {
    let hardware = Hardware::new();
    let (mut program, mut irq) = hardware.owners();
    arm_after(&mut program, Duration::from_micros(1)).expect("delay fits");
    hardware.advance(1);
    hardware.operations.borrow_mut().clear();
    assert!(acknowledge(&mut irq).pending);
    arm_after(&mut program, Duration::from_micros(20)).expect("delay fits");
    assert_eq!(
        *hardware.operations.borrow(),
        [
            Operation::Observe,
            Operation::ClearTimer0,
            Operation::Stop,
            Operation::Enable(false),
            Operation::Load(20),
            Operation::Start,
            Operation::Enable(true)
        ]
    );
    assert!(!irq.observe().pending);
    hardware.advance(20);
    assert!(irq.observe().pending);
}

#[test]
fn program_rearm_cannot_acknowledge_an_existing_irq_cause() {
    let hardware = Hardware::new();
    let (mut program, irq) = hardware.owners();
    hardware.timer0_pending.set(true);
    arm_after(&mut program, Duration::from_micros(40)).expect("delay fits");
    assert!(irq.observe().pending);
}

#[test]
fn expiry_between_start_and_unmask_is_retained_and_delivered() {
    let hardware = Hardware::new();
    hardware.start_elapsed.set(1);
    let (mut program, irq) = hardware.owners();
    arm_after(&mut program, Duration::from_micros(1)).expect("delay fits");
    assert!(irq.observe().asserted && !hardware.armed.get());
}

#[test]
fn invalid_intervals_do_not_mutate_an_existing_alarm() {
    let hardware = Hardware::new();
    let (mut program, _) = hardware.owners();
    arm_after(&mut program, Duration::from_micros(MAX_DELAY_MICROS)).expect("maximum fits");
    for micros in [0, MAX_DELAY_MICROS + 1] {
        hardware.operations.borrow_mut().clear();
        let requested = Duration::from_micros(micros);
        assert_eq!(
            arm_after(&mut program, requested),
            Err(InvalidDelay { requested })
        );
        assert!(hardware.operations.borrow().is_empty());
        assert_eq!(hardware.interval.get(), MAX_DELAY_MICROS);
        assert!(hardware.armed.get() && hardware.running.get() && hardware.enabled.get());
    }
}

#[test]
fn spurious_irq_acknowledgement_preserves_a_future_alarm() {
    let hardware = Hardware::new();
    let (mut program, mut irq) = hardware.owners();
    arm_after(&mut program, Duration::from_micros(40)).expect("delay fits");
    hardware.operations.borrow_mut().clear();
    assert!(!acknowledge(&mut irq).pending);
    assert_eq!(*hardware.operations.borrow(), [Operation::Observe]);
    hardware.advance(40);
    assert!(irq.observe().asserted);
}

#[test]
fn pac_program_adapter_sets_target_reload_and_preserves_sibling_enables() {
    for fill in [0, u32::MAX] {
        let mut words = vec![fill; instances::timg0::SIZE / size_of::<u32>()];
        let mut program =
            Timg0AlarmProgram(timg::Timg0Timer0::host(&mut words).expect("memory fits"));
        let micros = 0x1_0000_0011;
        arm_after(&mut program, Duration::from_micros(micros)).expect("delay fits");
        let config = program.0.read_t0config();
        assert!(config.t_en() && config.t_alarm_en() && config.t_increase());
        assert!(!config.t_autoreload());
        assert_eq!(program.0.read_t0alarmlo().t_alarm_lo(), 17);
        assert_eq!(program.0.read_t0alarmhi().t_alarm_hi().get(), 1);
        assert_eq!(program.0.read_t0loadlo().t_load_lo(), 0);
        assert_eq!(program.0.read_t0loadhi().t_load_hi().get(), 0);
        assert!(program.enabled());
        let enables = program.0.read_int_ena_timers();
        assert_eq!(enables.t1_int_ena(), fill != 0);
        assert_eq!(enables.wdt_int_ena(), fill != 0);
        program.disarm();
        assert!(!program.0.read_t0config().t_en() && !program.enabled());
        assert_eq!(program.0.read_int_ena_timers().t1_int_ena(), fill != 0);
        assert_eq!(program.0.read_int_ena_timers().wdt_int_ena(), fill != 0);
    }
}

#[test]
fn pac_irq_adapter_observes_raw_independently_of_programming() {
    let mut words = vec![u32::MAX; instances::timg0::SIZE / size_of::<u32>()];
    let mut irq =
        Timg0AlarmInterrupt(timg::Timg0InterruptBank::host(&mut words).expect("memory fits"));
    let event = irq.acknowledge();
    assert!(event.pending && event.asserted);
    // Plain Host memory has no hardware CLR effects. Side-effect behavior,
    // including preservation of sibling causes, is checked by the model above.
    assert!(irq.0.read_int_raw_timers().t1_int_raw());
    assert!(irq.0.read_int_raw_timers().wdt_int_raw());
}
