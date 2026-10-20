use super::*;
use crate::{
    analog::i2c::{PhyI2cError, analog_registers},
    calibration::cold::PhyColdI2cRequest,
};
use oer_time::Clock;
use oer_time_virtual::VirtualClock;
use std::{
    sync::Arc,
    task::{Context, Wake, Waker},
    vec::Vec,
};

struct WakeCount(std::sync::atomic::AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Io {
    actions: Vec<PhyColdI2cAction>,
    code: u8,
    byte: u8,
    busy: bool,
    atoms: usize,
}
impl StepIo for Io {
    fn start_i2c(&mut self, transaction: &mut PhyColdI2cTransaction) -> Result<bool, Failure> {
        self.atoms += 1;
        self.actions.push(transaction.action());
        if self.busy {
            return Ok(false);
        }
        match transaction.action() {
            PhyColdI2cAction::StartRead { .. } => transaction.read_started(),
            PhyColdI2cAction::StartWrite { value, .. } => {
                self.byte = value;
                transaction.write_started()
            }
            _ => return Err(Failure::Binding),
        }
        .map_err(|_| Failure::Binding)?;
        Ok(true)
    }
    fn sample_i2c(
        &mut self,
        transaction: &mut PhyColdI2cTransaction,
    ) -> Result<PhyColdI2cObservation, Failure> {
        self.atoms += 1;
        self.actions.push(transaction.action());
        match transaction.action() {
            PhyColdI2cAction::AwaitReadCompletionEdge { .. } => {
                transaction.observe_read_result(if self.busy {
                    Err(PhyI2cError::Busy)
                } else {
                    Ok(self.byte)
                })
            }
            PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                transaction.observe_write_result(if self.busy {
                    Err(PhyI2cError::Busy)
                } else {
                    Ok(())
                })
            }
            _ => return Err(Failure::Binding),
        }
        .map_err(|_| Failure::Binding)
    }
    fn start_pbus(&mut self, binding: &mut PhyPbusHardwareBinding) -> Result<(), Failure> {
        self.atoms += 1;
        binding.started().map_err(|_| Failure::Binding)
    }
    fn sample_pbus(
        &mut self,
        binding: &mut PhyPbusHardwareBinding,
    ) -> Result<PhyPbusHardwareObservation, Failure> {
        self.atoms += 1;
        binding
            .observe_completed(!self.busy)
            .map_err(|_| Failure::Binding)
    }
    fn temperature_code(&mut self) -> u8 {
        self.atoms += 1;
        self.code
    }
    fn clear_pbus_control(&mut self, _: bool) -> Result<bool, Failure> {
        self.atoms += 1;
        Ok(true)
    }
}

#[derive(Default)]
struct Stats(crate::tracking::observation::EngineTiming);
impl Sink for Stats {
    fn enabled(&self) -> bool {
        true
    }
    fn observe(&mut self, event: EngineEvent) {
        self.0.observe(event);
    }
}

fn context() -> (Waker, Arc<WakeCount>) {
    let wakes = Arc::new(WakeCount(std::sync::atomic::AtomicUsize::new(0)));
    (Waker::from(wakes.clone()), wakes)
}

#[test]
fn every_hardware_atom_yields_even_when_the_timer_is_already_ready() {
    let clock = VirtualClock::<1>::default();
    let mut io = Io::default();
    let request = PhyColdI2cRequest::write_field(analog_registers::TEMPERATURE_SENSOR_DAC, 15);
    let mut job =
        Job::i2c(PhyColdI2cTransaction::new(request), clock.now()).expect("deadline fits");
    let mut stats = Stats::default();
    let (waker, wakes) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = pin!(drive(&mut job, &mut io, &clock, &mut stats));
        // RMW: read start, read completion, write start, write completion.
        for _ in 0..4 {
            assert!(future.as_mut().poll(&mut cx).is_pending());
        }
        assert!(matches!(
            future.as_mut().poll(&mut cx),
            Poll::Ready(Ok(Outcome::I2c(_)))
        ));
    }
    assert_eq!(io.atoms, 4);
    assert_eq!(wakes.0.load(std::sync::atomic::Ordering::Relaxed), 4);
    assert_eq!((stats.0.polls, stats.0.pending, stats.0.resumes), (5, 4, 4));
    assert!(!stats.0.invalid);
}

#[test]
fn ready_at_deadline_is_accepted_but_a_late_read_is_never_consumed() {
    for late in [false, true] {
        let clock = VirtualClock::<1>::default();
        let mut job = Job::i2c(
            PhyColdI2cTransaction::new(PhyColdI2cRequest::read_field(
                analog_registers::TEMPERATURE_SENSOR_DAC_STATUS,
            )),
            clock.now(),
        )
        .expect("deadline fits");
        let mut io = Io::default();
        assert_eq!(job.advance_one(&mut io, clock.now()), Step::Runnable);
        clock
            .advance(Duration::from_micros(if late { 21 } else { 20 }))
            .expect("test time fits");
        let next = job.advance_one(&mut io, clock.now());
        if late {
            assert_eq!(next, Step::Failure(Failure::TimedOut));
            assert_eq!(io.atoms, 1);
        } else {
            assert_eq!(next, Step::Runnable);
            clock
                .advance(Duration::from_micros(100))
                .expect("test time fits");
            assert!(matches!(
                job.advance_one(&mut io, clock.now()),
                Step::Complete(Outcome::I2c(_))
            ));
            assert_eq!(io.atoms, 2);
        }
    }
}

#[test]
fn not_ready_uses_a_timer_and_a_deadline_without_another_atom() {
    let clock = VirtualClock::<1>::default();
    let mut job = Job::pbus(PhyPbusForceTest::new(1, 2, 3), clock.now()).expect("deadline fits");
    let mut io = Io {
        busy: true,
        ..Io::default()
    };
    assert_eq!(job.advance_one(&mut io, clock.now()), Step::Runnable);
    assert_eq!(
        job.advance_one(&mut io, clock.now()),
        Step::WaitAt(clock.now().checked_add(RETRY).expect("time fits"))
    );
    assert_eq!(io.atoms, 2);
    clock.advance(Duration::from_micros(21)).expect("time fits");
    assert!(matches!(
        job.advance_one(&mut io, clock.now()),
        Step::Complete(Outcome::Pbus {
            completed: false,
            ..
        })
    ));
    assert_eq!(io.atoms, 2);
}

#[test]
fn overflowing_deadlines_are_typed_failures_before_publication() {
    let clock = VirtualClock::<1>::starting_at(Instant::from_micros(u64::MAX - 10));
    assert!(matches!(
        Job::pbus(PhyPbusForceTest::new(0, 0, 0), clock.now()),
        Err(Failure::DeadlineOverflow)
    ));
}

fn old_temperature(io: &mut Io) -> (PhyTemperatureOutcome, Vec<PhyColdI2cAction>) {
    let mut transition = PhyTemperatureTransition::new();
    loop {
        match transition.action() {
            PhyTemperatureAction::Complete(outcome) => return (outcome, io.actions.clone()),
            PhyTemperatureAction::SampleCode => transition
                .advance(PhyTemperatureCompletion::CodeSampled {
                    value: io.temperature_code(),
                })
                .expect("sample matches"),
            action => {
                let mut binding =
                    PhyTemperatureI2cBinding::new(action).expect("valid temperature action");
                while !matches!(binding.action(), PhyColdI2cAction::Complete(_)) {
                    let transaction = binding.transaction_mut();
                    match transaction.action() {
                        PhyColdI2cAction::StartRead { .. }
                        | PhyColdI2cAction::StartWrite { .. } => {
                            io.start_i2c(transaction).expect("start succeeds");
                        }
                        _ => {
                            io.sample_i2c(transaction).expect("completion succeeds");
                        }
                    }
                }
                transition
                    .advance(binding.into_completion().expect("identity matches"))
                    .expect("completion matches");
            }
        }
    }
}

#[test]
fn sliced_temperature_keeps_the_old_writes_and_results_for_all_sensor_codes() {
    for dac in [0, 5, 7, 15, 11, 10] {
        for code in 0..=u8::MAX {
            let clock = VirtualClock::<1>::new();
            let mut old = Io {
                byte: dac,
                code,
                ..Io::default()
            };
            let expected = old_temperature(&mut old);
            let mut io = Io {
                byte: dac,
                code,
                ..Io::default()
            };
            let mut transition = PhyTemperatureTransition::new();
            let mut job = Job::temperature(&mut transition);
            loop {
                let before = io.atoms;
                let next = job.advance_one(&mut io, clock.now());
                assert!(io.atoms - before <= 1);
                match next {
                    Step::Runnable => {}
                    Step::WaitAt(at) => clock.advance_to(at),
                    Step::Complete(Outcome::Temperature(outcome)) => {
                        assert_eq!((outcome, io.actions.clone()), expected);
                        break;
                    }
                    _ => panic!("temperature completes"),
                }
            }
        }
    }
}
