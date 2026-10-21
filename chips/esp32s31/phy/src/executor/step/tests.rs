use super::*;
pub(crate) struct TestDelay;
impl crate::executor::wait::PhyShortDelay for TestDelay {
    const MAX_MICROS: u32 = 20;
    fn settle_micros(micros: u32) -> bool {
        (1..=20).contains(&micros)
    }
}

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
    rfpll_actions: Vec<crate::tracking::rfpll::Action>,
    rfpll_initial: u16,
    rfpll_statuses: Vec<crate::tracking::rfpll::search::Status>,
    rfpll_samples: usize,
    rfpll_failure: Option<usize>,
    code: u8,
    byte: u8,
    busy: Rc<Cell<bool>>,
    pub(crate) atoms: Rc<Cell<usize>>,
}
impl Io {
    pub(super) fn rfpll<D: crate::executor::wait::PhyShortDelay>(
        &mut self,
        action: crate::tracking::rfpll::Action,
    ) -> Result<crate::tracking::rfpll::Completion, Failure> {
        use crate::analog::frequency::{
            PhyFrequencyCapMemoryAction as Memory, PhyFrequencyCapMemoryCompletion as Done,
        };
        use crate::tracking::rfpll::{Action, Completion, search};
        self.atom();
        if self.rfpll_failure == Some(self.rfpll_actions.len()) {
            return Err(Failure::Binding);
        }
        self.rfpll_actions.push(action);
        Ok(match action {
            Action::Search(action) => Completion::Search(match action {
                search::Action::ReadInitialCap => {
                    search::Completion::InitialCap(self.rfpll_initial)
                }
                search::Action::EnableSearch => search::Completion::SearchEnabled,
                search::Action::WriteCap(cap) => search::Completion::CapWritten(cap),
                search::Action::DelayMicros(micros) => {
                    if !D::settle_micros(micros) {
                        return Err(Failure::Binding);
                    }
                    search::Completion::DelayElapsed(micros)
                }
                search::Action::ReadStatus => {
                    let status = self
                        .rfpll_statuses
                        .get(self.rfpll_samples)
                        .copied()
                        .unwrap_or(search::Status::Other);
                    self.rfpll_samples += 1;
                    search::Completion::Status(status)
                }
                search::Action::Complete(_) => return Err(Failure::Binding),
            }),
            Action::Memory(action) => Completion::Memory(match action {
                Memory::ReadMemory {
                    entry_index,
                    address,
                    mode,
                } => Done::MemoryRead {
                    entry_index,
                    address,
                    mode,
                    value: 0xa500_0000 | u32::from(entry_index),
                },
                Memory::WriteMemory {
                    entry_index,
                    address,
                    mode,
                    ..
                } => Done::MemoryWritten {
                    entry_index,
                    address,
                    mode,
                },
                Memory::RestoreChannelIndex { frequency_index } => {
                    Done::ChannelIndexRestored { frequency_index }
                }
                Memory::Complete(_) => return Err(Failure::Binding),
            }),
            Action::Complete(_) => return Err(Failure::Binding),
        })
    }
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
    fn advance<D: crate::executor::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        job: &mut Job<'_>,
        clock: &C,
        now: Instant,
    ) -> Step {
        job.advance_one::<D, _>(
            super::io::Io::Test(self, core::marker::PhantomData),
            clock,
            now,
        )
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
    let mut job =
        Job::i2c(&mut transaction, clock.now(), I2cTiming::Direct).expect("deadline fits");
    let mut stats = EngineTiming::default();
    let (waker, wakes) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job,
            &mut io,
            &clock,
            Some(&mut stats),
            None
        ));
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
        (5, 4, 0, 2)
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
    let mut job =
        Job::i2c(&mut transaction, clock.now(), I2cTiming::Temperature).expect("deadline fits");
    let mut stats = EngineTiming::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job,
            &mut io,
            &clock,
            Some(&mut stats),
            None
        ));
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
        let mut job =
            Job::i2c(&mut transaction, clock.now(), I2cTiming::Direct).expect("deadline fits");
        assert_eq!(
            io.advance::<TestDelay, _>(&mut job, &clock, clock.now()),
            Step::Runnable
        );
        clock
            .advance(Duration::from_micros(if late { 21 } else { 20 }))
            .expect("time fits");
        let next = io.advance::<TestDelay, _>(&mut job, &clock, clock.now());
        if late {
            assert_eq!(next, Step::Failure(Failure::TimedOut));
            assert_eq!(io.atoms.get(), 1);
        } else {
            assert_eq!(next, Step::Runnable);
            clock
                .advance(Duration::from_micros(100))
                .expect("time fits");
            assert!(matches!(
                io.advance::<TestDelay, _>(&mut job, &clock, clock.now()),
                Step::Complete
            ));
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
    assert_eq!(
        io.advance::<TestDelay, _>(&mut job, &clock, clock.now()),
        Step::Runnable
    );
    assert_eq!(
        io.advance::<TestDelay, _>(&mut job, &clock, clock.now()),
        Step::WaitAt(clock.now().checked_add(RETRY).expect("time fits"))
    );
    clock.advance(Duration::from_micros(21)).expect("time fits");
    assert!(matches!(
        io.advance::<TestDelay, _>(&mut job, &clock, clock.now()),
        Step::Complete
    ));
    assert_eq!(io.atoms.get(), 2);
}
#[test]
fn overflowing_deadlines_are_typed_failures_before_publication() {
    let clock = VirtualClock::<1>::starting_at(Instant::from_micros(u64::MAX - 10));
    let mut transaction = read();
    assert!(matches!(
        Job::i2c(&mut transaction, clock.now(), I2cTiming::Direct),
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
                let mut future = core::pin::pin!(super::temperature::sample::<TestDelay, _, _>(
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
        let step = io.advance::<TestDelay, _>(&mut job, &clock, clock.now());
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
    let mut job =
        Job::i2c(&mut transaction, clock.now(), I2cTiming::Direct).expect("deadline fits");
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
        &mut job, &mut io, &clock, None, None
    ));
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
        let mut job =
            Job::i2c(&mut transaction, clock.now(), I2cTiming::Temperature).expect("deadline fits");
        let mut stats = EngineTiming::default();
        let mut trace = ReadinessTrace::default();
        let (waker, _) = context();
        let mut cx = Context::from_waker(&waker);
        {
            let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
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
            clock
                .advance(Duration::from_micros(130))
                .expect("time fits");
            assert_eq!(
                future.as_mut().poll(&mut cx),
                Poll::Ready(Err(Failure::TimedOut))
            );
        }
        assert_eq!(trace.commands, 1);
        assert_eq!(trace.failure_busy, Some(0));
        assert_eq!(
            trace.maximum_first_check_micros,
            if not_ready_checks { 1 } else { 130 }
        );
        assert_eq!(
            trace.failure_elapsed_micros,
            if not_ready_checks { 133 } else { 130 }
        );
        assert_eq!(trace.failure_lateness_micros, 129);
        assert_eq!(stats.maximum_lateness_micros, 129);
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

#[test]
fn runnable_yields_without_constructing_or_polling_a_timer() {
    struct Probe<'a> {
        clock: &'a VirtualClock<1>,
        constructed: Cell<u32>,
        polled: Cell<u32>,
    }
    impl Clock for Probe<'_> {
        fn now(&self) -> Instant {
            self.clock.now()
        }
    }
    impl Timer for Probe<'_> {
        fn wait_until(&self, _: Instant) -> impl Future<Output = ()> {
            self.constructed.set(self.constructed.get() + 1);
            core::future::poll_fn(|_| {
                self.polled.set(self.polled.get() + 1);
                Poll::Ready(())
            })
        }
    }
    let clock = VirtualClock::<1>::new();
    let timer = Probe {
        clock: &clock,
        constructed: Cell::new(0),
        polled: Cell::new(0),
    };
    let mut io = Io::default();
    let mut job = Job::code(clock.now()).expect("deadline fits");
    let mut trace = ReadinessTrace::default();
    let (waker, wakes) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job,
            &mut io,
            &timer,
            None,
            Some(&mut trace)
        ));
        assert!(future.as_mut().poll(&mut cx).is_pending());
        clock.advance(Duration::from_micros(3)).expect("time fits");
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    assert_eq!((timer.constructed.get(), timer.polled.get()), (0, 0));
    assert_eq!(wakes.0.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(
        (
            trace.yields,
            trace.yield_total_micros,
            trace.yield_maximum_micros
        ),
        (1, 3, 3)
    );
}

struct CommandCost<'a> {
    io: Io,
    clock: &'a VirtualClock<1>,
}
impl Registers for CommandCost<'_> {
    fn advance<D: crate::executor::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        job: &mut Job<'_>,
        clock: &C,
        now: Instant,
    ) -> Step {
        let step = self.io.advance::<D, _>(job, clock, now);
        if job.first_wait {
            self.clock
                .advance(Duration::from_micros(3))
                .expect("time fits");
        }
        step
    }
    fn i2c_status(&mut self) -> u8 {
        self.io.i2c_status()
    }
}

#[test]
fn t7_answer_window_starts_after_publication_and_accepts_its_exact_boundary() {
    for late in [false, true] {
        let clock = VirtualClock::<1>::new();
        let mut registers = CommandCost {
            io: Io::default(),
            clock: &clock,
        };
        let mut transaction = read();
        let mut job =
            Job::i2c(&mut transaction, clock.now(), I2cTiming::Temperature).expect("deadline fits");
        let (waker, _) = context();
        let mut cx = Context::from_waker(&waker);
        {
            let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
                &mut job,
                &mut registers,
                &clock,
                None,
                None
            ));
            assert!(future.as_mut().poll(&mut cx).is_pending());
            clock
                .advance(Duration::from_micros(T7_ANSWER_MICROS + u64::from(late)))
                .expect("time fits");
            let result = future.as_mut().poll(&mut cx);
            if late {
                assert_eq!(result, Poll::Ready(Err(Failure::TimedOut)));
            } else {
                assert!(result.is_pending());
                assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
            }
        }
        assert_eq!(registers.io.actions.len(), if late { 1 } else { 2 });
    }
}

#[test]
fn overflow_after_publication_is_typed_and_does_not_sample_the_bus() {
    let clock =
        VirtualClock::<1>::starting_at(Instant::from_micros(u64::MAX - T7_ANSWER_MICROS - 2));
    let mut registers = CommandCost {
        io: Io::default(),
        clock: &clock,
    };
    let mut transaction = read();
    let mut job =
        Job::i2c(&mut transaction, clock.now(), I2cTiming::Temperature).expect("admission fits");
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job,
            &mut registers,
            &clock,
            None,
            None
        ));
        assert_eq!(
            future.as_mut().poll(&mut cx),
            Poll::Ready(Err(Failure::DeadlineOverflow))
        );
    }
    assert_eq!(registers.io.actions.len(), 1);
}

#[test]
fn a_voluntary_yield_between_rmw_commands_is_not_a_hardware_timeout() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let atoms = io.atoms.clone();
    let mut transaction = PhyColdI2cTransaction::new(PhyColdI2cRequest::write_field(
        analog_registers::TEMPERATURE_SENSOR_DAC,
        15,
    ));
    let mut job =
        Job::i2c(&mut transaction, clock.now(), I2cTiming::Temperature).expect("deadline fits");
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None, None
        ));
        for _ in 0..4 {
            let before = atoms.get();
            assert!(future.as_mut().poll(&mut cx).is_pending());
            assert_eq!(atoms.get() - before, 1);
            clock.advance(Duration::from_micros(40)).expect("time fits");
        }
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    assert_eq!(atoms.get(), 4);
    assert!(matches!(
        transaction.action(),
        PhyColdI2cAction::Complete(_)
    ));
}

#[test]
fn rfpll_preserves_signed_search_and_all_memory_effects_at_bounded_checkpoints() {
    use crate::analog::frequency::PhyFrequencyCapMemoryAction as Memory;
    use crate::tracking::rfpll::{Action, Correction, search::Status};
    // Exhausted search, directional boundaries, accepted negative requests,
    // nine-bit upper boundary, and a nonzero correction in each direction.
    let cases = [
        (100, std::vec![Status::Other; 60]),
        (100, std::vec![Status::Accepted; 60]),
        (
            0,
            std::vec![
                Status::Increase,
                Status::Accepted,
                Status::Increase,
                Status::Decrease,
                Status::Decrease
            ],
        ),
        (511, std::vec![Status::Accepted; 30]),
        (
            100,
            [
                std::vec![Status::Increase; 2],
                std::vec![Status::Accepted; 30],
            ]
            .concat(),
        ),
        (
            300,
            [
                std::vec![Status::Accepted; 30],
                std::vec![Status::Decrease; 2],
            ]
            .concat(),
        ),
    ];
    for (initial, statuses) in cases {
        for channel in [1, 13, 14] {
            let make_io = || Io {
                rfpll_initial: initial,
                rfpll_statuses: statuses.clone(),
                ..Io::default()
            };
            let mut old = make_io();
            let mut native = Correction::new(channel);
            let expected = loop {
                let action = native.action();
                if let Action::Complete(outcome) = action {
                    break outcome;
                }
                let completion = old
                    .rfpll::<TestDelay>(action)
                    .expect("native action completes");
                native
                    .advance(completion)
                    .expect("native sequence is valid");
            };
            let clock = VirtualClock::<1>::new();
            let mut io = make_io();
            let mut correction = Correction::new(channel);
            {
                let mut job = Job::rfpll(rfpll::Cursor::Correction(&mut correction), clock.now())
                    .expect("deadline fits");
                loop {
                    let before = io.rfpll_actions.len();
                    let result = io.advance::<TestDelay, _>(&mut job, &clock, clock.now());
                    let actions = &io.rfpll_actions[before..];
                    if result == Step::Complete {
                        assert!(actions.is_empty());
                        break;
                    }
                    assert_eq!(result, Step::Runnable);
                    assert!(!actions.is_empty());
                    match actions[0] {
                        Action::Search(crate::tracking::rfpll::search::Action::WriteCap(_))
                            if actions.len() > 1 =>
                        {
                            assert!(matches!(
                                actions,
                                [
                                    Action::Search(
                                        crate::tracking::rfpll::search::Action::WriteCap(_)
                                    ),
                                    Action::Search(
                                        crate::tracking::rfpll::search::Action::DelayMicros(5)
                                    ),
                                    Action::Search(
                                        crate::tracking::rfpll::search::Action::ReadStatus
                                    )
                                ]
                            ));
                        }
                        Action::Memory(Memory::ReadMemory { entry_index, .. }) => {
                            assert!(
                                matches!(actions, [_, Action::Memory(Memory::WriteMemory { entry_index: written, .. })] if *written == entry_index)
                            );
                        }
                        _ => assert_eq!(actions.len(), 1),
                    }
                    clock
                        .advance(Duration::from_micros(500))
                        .expect("time fits");
                }
            }
            assert_eq!(io.rfpll_actions, old.rfpll_actions);
            assert_eq!(correction.action(), Action::Complete(expected));
            assert_eq!(
                io.rfpll_actions
                    .iter()
                    .filter(|a| matches!(a, Action::Memory(Memory::WriteMemory { .. })))
                    .count(),
                if expected.search.delta() == 0 { 0 } else { 85 }
            );
        }
    }
}

#[test]
fn rfpll_pending_failure_and_cancellation_keep_the_thermal_envelope_unrestored() {
    use crate::tracking::rfpll::{
        Action,
        thermal::{Action as Thermal, Completion, Request, Transition},
    };
    for failure in [None, Some(0), Some(4), Some(8)] {
        let clock = VirtualClock::<1>::new();
        let mut child = Transition::new(Request {
            current_temperature: 20,
            reference_temperature: 0,
            current_channel: 13,
            threshold_override: None,
        });
        for completion in [
            Completion::GrantProtectSet { enabled: true },
            Completion::SoftwareControlSelected,
            Completion::Settled,
            Completion::BoundaryObserved,
        ] {
            child.advance(completion).expect("envelope entered");
        }
        let mut io = Io {
            rfpll_initial: 100,
            rfpll_failure: failure,
            ..Io::default()
        };
        let (waker, _) = context();
        let mut cx = Context::from_waker(&waker);
        let result = {
            let mut job =
                Job::rfpll(rfpll::Cursor::Thermal(&mut child), clock.now()).expect("deadline fits");
            let mut driver = core::pin::pin!(drive::<TestDelay, _, _>(
                &mut job, &mut io, &clock, None, None
            ));
            loop {
                if let Poll::Ready(result) = driver.as_mut().poll(&mut cx) {
                    break result;
                }
            }
        };
        if failure.is_some() {
            assert_eq!(result, Err(Failure::Binding));
            assert!(matches!(child.action(), Thermal::Correct(_)));
        } else {
            assert_eq!(result, Ok(()));
            assert_eq!(child.action(), Thermal::RestoreHardwareControl);
            child
                .advance(Completion::HardwareControlRestored)
                .expect("restored");
            assert_eq!(child.action(), Thermal::SetGrantProtect { enabled: false });
        }
        assert!(
            !io.rfpll_actions
                .iter()
                .any(|a| matches!(a, Action::Complete(_)))
        );
    }
    let clock = VirtualClock::<1>::new();
    let mut correction = crate::tracking::rfpll::Correction::new(13);
    let mut io = Io::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut job = Job::rfpll(rfpll::Cursor::Correction(&mut correction), clock.now())
            .expect("deadline fits");
        let mut driver = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None, None
        ));
        assert!(driver.as_mut().poll(&mut cx).is_pending());
    }
    assert!(!matches!(correction.action(), Action::Complete(_)));
}

#[test]
fn rfpll_driver_wakes_once_per_checkpoint_without_a_timer_or_gap_timeout() {
    struct NoWait<'a> {
        clock: &'a VirtualClock<1>,
        constructed: Cell<usize>,
    }
    impl Clock for NoWait<'_> {
        fn now(&self) -> Instant {
            self.clock.now()
        }
    }
    impl Timer for NoWait<'_> {
        fn wait_until(&self, _: Instant) -> impl Future<Output = ()> {
            self.constructed.set(self.constructed.get() + 1);
            core::future::ready(())
        }
    }
    let clock = VirtualClock::<1>::new();
    let timer = NoWait {
        clock: &clock,
        constructed: Cell::new(0),
    };
    let mut io = Io {
        rfpll_initial: 100,
        ..Io::default()
    };
    let mut correction = crate::tracking::rfpll::Correction::new(13);
    let mut stats = EngineTiming::default();
    let (waker, wakes) = context();
    let mut cx = Context::from_waker(&waker);
    let mut pending = 0;
    {
        let mut job = Job::rfpll(rfpll::Cursor::Correction(&mut correction), clock.now())
            .expect("deadline fits");
        let mut driver = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job,
            &mut io,
            &timer,
            Some(&mut stats),
            None
        ));
        loop {
            match driver.as_mut().poll(&mut cx) {
                Poll::Pending => {
                    pending += 1;
                    clock
                        .advance(Duration::from_micros(500))
                        .expect("time fits");
                }
                Poll::Ready(result) => {
                    assert_eq!(result, Ok(()));
                    break;
                }
            }
        }
    }
    assert_eq!(timer.constructed.get(), 0);
    assert_eq!(pending, 63); // Initial, enable, 60 candidates, final write.
    assert_eq!(wakes.0.load(std::sync::atomic::Ordering::Relaxed), pending);
    assert_eq!(
        (stats.pending as usize, stats.polls as usize, stats.resumes),
        (pending, pending + 1, 0)
    );
    assert!(matches!(
        correction.action(),
        crate::tracking::rfpll::Action::Complete(_)
    ));
}
