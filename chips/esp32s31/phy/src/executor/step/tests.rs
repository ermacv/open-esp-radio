use super::*;
use crate::{
    analog::{
        i2c::{PhyI2cError, analog_registers},
        temperature::{
            PhyTemperatureAction, PhyTemperatureCompletion, PhyTemperatureI2cBinding,
            PhyTemperatureOutcome, PhyTemperatureTransition,
        },
    },
    calibration::cold::PhyColdI2cRequest,
};
use oer_time::{Clock, Duration};
use oer_time_virtual::VirtualClock;
use std::{
    cell::Cell,
    rc::Rc,
    sync::Arc,
    task::{Wake, Waker},
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
pub(crate) struct Io {
    actions: Vec<PhyColdI2cAction>,
    code: u8,
    byte: u8,
    busy: Rc<Cell<bool>>,
    pub(crate) atoms: Rc<Cell<usize>>,
}
impl Io {
    fn atom(&self) {
        self.atoms.set(self.atoms.get() + 1);
    }
    pub(super) fn start_i2c(
        &mut self,
        transaction: &mut PhyColdI2cTransaction,
    ) -> Result<bool, Failure> {
        self.atom();
        self.actions.push(transaction.action());
        if self.busy.get() {
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
    pub(super) fn sample_i2c(
        &mut self,
        transaction: &mut PhyColdI2cTransaction,
    ) -> Result<PhyColdI2cObservation, Failure> {
        self.atom();
        self.actions.push(transaction.action());
        match transaction.action() {
            PhyColdI2cAction::AwaitReadCompletionEdge { .. } => {
                transaction.observe_read_result(if self.busy.get() {
                    Err(PhyI2cError::Busy)
                } else {
                    Ok(self.byte)
                })
            }
            PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                transaction.observe_write_result(if self.busy.get() {
                    Err(PhyI2cError::Busy)
                } else {
                    Ok(())
                })
            }
            _ => return Err(Failure::Binding),
        }
        .map_err(|_| Failure::Binding)
    }
    pub(super) fn start_pbus(
        &mut self,
        binding: &mut PhyPbusHardwareBinding,
    ) -> Result<(), Failure> {
        self.atom();
        binding.started().map_err(|_| Failure::Binding)
    }
    pub(super) fn sample_pbus(
        &mut self,
        binding: &mut PhyPbusHardwareBinding,
    ) -> Result<PhyPbusHardwareObservation, Failure> {
        self.atom();
        binding
            .observe_completed(!self.busy.get())
            .map_err(|_| Failure::Binding)
    }
    pub(super) fn temperature_code(&mut self) -> u8 {
        self.atom();
        self.code
    }
    pub(super) fn clear_pbus_control(&mut self, _: bool) -> Result<bool, Failure> {
        self.atom();
        Ok(true)
    }
}
impl Registers for Io {
    fn advance(&mut self, job: &mut Job<'_>, now: Instant) -> Step {
        job.advance_one(super::io::Io::Test(self, core::marker::PhantomData), now)
    }
    fn i2c_status(&mut self) -> u8 {
        self.atom();
        u8::from(self.busy.get())
    }
}
fn context() -> (Waker, Arc<WakeCount>) {
    let wakes = Arc::new(WakeCount(std::sync::atomic::AtomicUsize::new(0)));
    (Waker::from(wakes.clone()), wakes)
}
fn read() -> PhyColdI2cTransaction {
    PhyColdI2cTransaction::new(PhyColdI2cRequest::read_field(
        analog_registers::TEMPERATURE_SENSOR_DAC_STATUS,
    ))
}
#[test]
fn every_hardware_atom_yields_even_when_the_timer_is_already_ready() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let atoms = io.atoms.clone();
    let mut transaction = PhyColdI2cTransaction::new(PhyColdI2cRequest::write_field(
        analog_registers::TEMPERATURE_SENSOR_DAC,
        15,
    ));
    let mut job = Job::i2c(&mut transaction, clock.now(), false).expect("deadline fits");
    let mut stats = EngineTiming::default();
    let (waker, wakes) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(drive(&mut job, &mut io, &clock, Some(&mut stats), None));
        for _ in 0..4 {
            let before = atoms.get();
            assert!(future.as_mut().poll(&mut cx).is_pending());
            assert_eq!(atoms.get() - before, 1);
        }
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(()))));
    }
    assert_eq!(atoms.get(), 4);
    assert_eq!(wakes.0.load(std::sync::atomic::Ordering::Relaxed), 4);
    assert_eq!(
        (stats.polls, stats.pending, stats.resumes, stats.ready_edges),
        (5, 4, 4, 2)
    );
    assert!(!stats.invalid);
}
#[test]
fn timer_wait_does_no_atoms_until_the_deadline_and_records_resume_lateness() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let busy = io.busy.clone();
    let atoms = io.atoms.clone();
    let mut transaction = read();
    let mut job = Job::i2c(&mut transaction, clock.now(), true).expect("deadline fits");
    let mut stats = EngineTiming::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(drive(&mut job, &mut io, &clock, Some(&mut stats), None));
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(atoms.get(), 1);
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(atoms.get(), 1);
        busy.set(true);
        clock.advance(Duration::from_micros(1)).expect("time fits");
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(atoms.get(), 2);
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(atoms.get(), 2);
        busy.set(false);
        clock.advance(Duration::from_micros(3)).expect("time fits");
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(atoms.get(), 3);
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }
    assert_eq!(stats.maximum_ready_micros, 4);
    assert_eq!(stats.maximum_lateness_micros, 2);
    assert_eq!((stats.polls, stats.pending, stats.ready_edges), (6, 5, 1));
}
#[test]
fn ready_at_deadline_is_accepted_but_a_late_read_is_never_consumed() {
    for late in [false, true] {
        let clock = VirtualClock::<1>::new();
        let mut io = Io::default();
        let mut transaction = read();
        let mut job = Job::i2c(&mut transaction, clock.now(), false).expect("deadline fits");
        assert_eq!(io.advance(&mut job, clock.now()), Step::Runnable);
        clock
            .advance(Duration::from_micros(if late { 21 } else { 20 }))
            .expect("time fits");
        let next = io.advance(&mut job, clock.now());
        if late {
            assert_eq!(next, Step::Failure(Failure::TimedOut));
            assert_eq!(io.atoms.get(), 1);
        } else {
            assert_eq!(next, Step::Runnable);
            clock
                .advance(Duration::from_micros(100))
                .expect("time fits");
            assert!(matches!(io.advance(&mut job, clock.now()), Step::Complete));
            assert_eq!(io.atoms.get(), 2);
        }
    }
}
#[test]
fn pbus_timeout_keeps_its_source_completion_and_never_consumes_a_late_result() {
    let clock = VirtualClock::<1>::new();
    let mut binding = PhyPbusHardwareBinding::new(PhyPbusForceTest::new(1, 2, 3));
    let mut job = Job::pbus(&mut binding, clock.now()).expect("deadline fits");
    let mut io = Io::default();
    io.busy.set(true);
    assert_eq!(io.advance(&mut job, clock.now()), Step::Runnable);
    assert_eq!(
        io.advance(&mut job, clock.now()),
        Step::WaitAt(clock.now().checked_add(RETRY).expect("time fits"))
    );
    clock.advance(Duration::from_micros(21)).expect("time fits");
    assert!(matches!(io.advance(&mut job, clock.now()), Step::Complete));
    assert_eq!(io.atoms.get(), 2);
}
#[test]
fn overflowing_deadlines_are_typed_failures_before_publication() {
    let clock = VirtualClock::<1>::starting_at(Instant::from_micros(u64::MAX - 10));
    let mut transaction = read();
    assert!(matches!(
        Job::i2c(&mut transaction, clock.now(), false),
        Err(Failure::DeadlineOverflow)
    ));
}
fn old_temperature(io: &mut Io) -> PhyTemperatureOutcome {
    let mut transition = PhyTemperatureTransition::new();
    loop {
        match transition.action() {
            PhyTemperatureAction::Complete(outcome) => return outcome,
            PhyTemperatureAction::SampleCode => transition
                .advance(PhyTemperatureCompletion::CodeSampled {
                    value: io.temperature_code(),
                })
                .expect("sample matches"),
            action => {
                let mut binding = PhyTemperatureI2cBinding::new(action).expect("valid action");
                while !matches!(binding.action(), PhyColdI2cAction::Complete(_)) {
                    let t = binding.transaction_mut();
                    match t.action() {
                        PhyColdI2cAction::StartRead { .. }
                        | PhyColdI2cAction::StartWrite { .. } => {
                            io.start_i2c(t).expect("start succeeds");
                        }
                        _ => {
                            io.sample_i2c(t).expect("completion succeeds");
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
// The complete T7 orchestration is tested below through the production helper.
#[test]
fn sliced_t7_matches_the_old_action_write_and_result_sequence_for_all_sensor_codes() {
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
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
            let atoms = io.atoms.clone();
            let mut transition = PhyTemperatureTransition::new();
            let actual = {
                let mut future = core::pin::pin!(super::temperature::sample(
                    &mut transition,
                    &mut io,
                    &clock,
                    None,
                    None
                ));
                loop {
                    let before = atoms.get();
                    let next = future.as_mut().poll(&mut cx);
                    assert!(atoms.get() - before <= 1, "one atom per enclosing T7 poll");
                    if let Poll::Ready(outcome) = next {
                        break outcome.expect("T7 completes");
                    }
                    clock.advance_to_next_deadline();
                }
            };
            assert_eq!(actual, expected);
            assert_eq!(io.actions, old.actions);
        }
    }
}
#[test]
fn pbus_clear_preserves_all_commands_and_the_conditional_settle_tail() {
    let clock = VirtualClock::<1>::new();
    let mut transition = PhyPbusClearTransition::new();
    let mut job = Job::clear_pbus(&mut transition, clock.now()).expect("deadline fits");
    let mut io = Io::default();
    for _ in 0..100 {
        let before = io.atoms.get();
        let step = io.advance(&mut job, clock.now());
        assert!(io.atoms.get() - before <= 1);
        if step == Step::Complete {
            assert_eq!(
                job.outcome(),
                Some(Outcome::PbusClear(PhyPbusClearOutcome::Cleared))
            );
            assert_eq!(io.atoms.get(), 26);
            return;
        }
        if let Step::WaitAt(at) = step {
            clock.advance_to(at);
        }
    }
    panic!("PBus clear completes within its finite command count");
}

#[test]
fn an_immediately_ready_async_timer_is_not_polled_after_completion() {
    struct AsyncTimer(VirtualClock<1>);
    impl Clock for AsyncTimer {
        fn now(&self) -> Instant {
            self.0.now()
        }
    }
    impl Timer for AsyncTimer {
        async fn wait_until(&self, at: Instant) {
            self.0.wait_until(at).await;
        }
    }
    let clock = AsyncTimer(VirtualClock::<1>::new());
    let mut io = Io::default();
    let atoms = io.atoms.clone();
    let mut transaction = read();
    let mut job = Job::i2c(&mut transaction, clock.now(), false).expect("deadline fits");
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    let mut future = core::pin::pin!(drive(&mut job, &mut io, &clock, None, None));
    assert!(future.as_mut().poll(&mut cx).is_pending());
    assert_eq!(atoms.get(), 1);
    assert!(future.as_mut().poll(&mut cx).is_pending());
    assert_eq!(atoms.get(), 2);
    assert!(future.as_mut().poll(&mut cx).is_ready());
    assert_eq!(atoms.get(), 2);
}

#[test]
fn preparation_trace_survives_a_late_failure_without_consuming_the_result() {
    for not_ready_checks in [false, true] {
        let clock = VirtualClock::<1>::new();
        let mut io = Io::default();
        let busy = io.busy.clone();
        let mut transaction = read();
        let mut job = Job::i2c(&mut transaction, clock.now(), true).expect("deadline fits");
        let mut stats = EngineTiming::default();
        let mut trace = ReadinessTrace::default();
        let (waker, _) = context();
        let mut cx = Context::from_waker(&waker);
        {
            let mut future = core::pin::pin!(drive(
                &mut job,
                &mut io,
                &clock,
                Some(&mut stats),
                Some(&mut trace)
            ));
            assert!(future.as_mut().poll(&mut cx).is_pending());
            if not_ready_checks {
                busy.set(true);
                clock.advance(Duration::from_micros(1)).expect("time fits");
                assert!(future.as_mut().poll(&mut cx).is_pending());
                clock.advance(Duration::from_micros(2)).expect("time fits");
                assert!(future.as_mut().poll(&mut cx).is_pending());
                busy.set(false);
            }
            clock.advance(Duration::from_micros(30)).expect("time fits");
            assert_eq!(
                future.as_mut().poll(&mut cx),
                Poll::Ready(Err(Failure::TimedOut))
            );
        }
        assert_eq!(trace.commands, 1);
        assert_eq!(trace.failure_busy, Some(0));
        assert_eq!(
            trace.maximum_first_check_micros,
            if not_ready_checks { 1 } else { 30 }
        );
        assert_eq!(
            trace.failure_elapsed_micros,
            if not_ready_checks { 33 } else { 30 }
        );
        assert_eq!(trace.failure_lateness_micros, 29);
        assert_eq!(stats.maximum_lateness_micros, 29);
        assert_eq!(
            trace
                .not_ready_samples()
                .iter()
                .map(|s| s.elapsed_micros)
                .collect::<Vec<_>>(),
            if not_ready_checks {
                std::vec![1, 3]
            } else {
                std::vec![]
            }
        );
        assert!(trace.complete());
        assert_eq!(io.actions.len(), if not_ready_checks { 3 } else { 1 });
        assert!(matches!(
            transaction.action(),
            PhyColdI2cAction::AwaitReadCompletionEdge { .. }
        ));
    }
}
