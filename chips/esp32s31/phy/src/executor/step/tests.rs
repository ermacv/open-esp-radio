use super::*;
mod channel_gain;
mod dc_iq;
mod dcode;
mod power;
mod wifi_i2c;
pub(crate) struct TestDelay;
impl crate::executor::wait::PhyShortDelay for TestDelay {
    const MAX_MICROS: u32 = 20;
    fn settle_micros(micros: u32) -> bool {
        (1..=20).contains(&micros)
    }
}

use crate::{
    analog::{
        i2c::{PhyI2cError, fields},
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
    frequency_actions: Vec<crate::analog::rfpll::RfpllFrequencyAction>,
    frequency_failure: bool,
    i2c_write_publication: Option<(Rc<VirtualClock<1>>, u64)>,
    i2c_busy_after_write: bool,
    dc_iq_pending: usize,
    dc_iq_events: Vec<&'static str>,
    rfpll_clock: Option<Rc<VirtualClock<1>>>,
    rfpll_times: Vec<Instant>,
    rfpll_write_micros: u64,
    rfpll_command_micros: u64,
    rfpll_initial: u16,
    rfpll_statuses: Vec<crate::tracking::rfpll::search::Status>,
    rfpll_samples: usize,
    rfpll_failure: Option<usize>,
    power_actions: Vec<crate::tracking::power::PhyTxPowerTrackingAction>,
    power_failure: Option<usize>,
    power_gain_entries: Vec<oer_esp32s31_hal::types::PhyGainMemoryEntry>,
    power_gain_failure: Option<usize>,
    channel_base_reads: usize,
    code: u8,
    byte: u8,
    busy: Rc<Cell<bool>>,
    pub(crate) atoms: Rc<Cell<usize>>,
}
impl Io {
    pub(super) fn dc_iq<D: crate::executor::wait::PhyShortDelay>(
        &mut self,
        binding: crate::calibration::estimator::PhyDcIqReadinessBinding,
    ) -> Result<crate::calibration::estimator::PhyDcIqCompletion, Failure> {
        use crate::calibration::estimator::{
            PhyDcIqAccumulatorSnapshot, PhyDcIqEnablePhase, PhyDcIqReadinessSnapshot,
        };
        self.atom();
        binding.capture_with(
            crate::HARDWARE_EDGE_LIMIT,
            self,
            |io| {
                io.dc_iq_events.push("ready");
                let ready = io.dc_iq_pending == 0;
                io.dc_iq_pending = io.dc_iq_pending.saturating_sub(1);
                PhyDcIqReadinessSnapshot {
                    ready,
                    activity: !ready,
                }
            },
            |io| {
                io.dc_iq_events.push("accumulators");
                PhyDcIqAccumulatorSnapshot {
                    i: 0,
                    q: 0,
                    power: 1,
                }
            },
            |io, phase, enabled| {
                assert!(!enabled);
                io.dc_iq_events.push(match phase {
                    PhyDcIqEnablePhase::Measurement => "measurement-off",
                    PhyDcIqEnablePhase::Start => "start-off",
                });
            },
            |io, micros| {
                io.dc_iq_events.push("settle");
                D::settle_micros(micros)
                    .then_some(())
                    .ok_or(Failure::Binding)
            },
        )
    }
    pub(super) fn frequency<D: crate::executor::wait::PhyShortDelay>(
        &mut self,
        binding: crate::analog::rfpll::RfpllFrequencyExternalBinding,
    ) -> Result<crate::analog::rfpll::RfpllFrequencyCompletion, Failure> {
        use crate::analog::rfpll::{
            RfpllFrequencyAction as Action, RfpllFrequencyCompletion as Completion,
            RfpllFrequencyExternalBinding as Binding,
        };
        self.atom();
        if self.frequency_failure {
            return Err(Failure::Binding);
        }
        Ok(match binding {
            Binding::Mmio(binding) => {
                self.frequency_actions.push(binding.action());
                match binding.action() {
                    Action::StartChannelSwitch {
                        frequency_index,
                        crystal_selector,
                    } => Completion::ChannelSwitchStarted {
                        frequency_index,
                        crystal_selector,
                    },
                    Action::ClearChannelSwitch => Completion::ChannelSwitchCleared,
                    Action::ReadChannelReady { .. } => {
                        Completion::ChannelReadyObserved { ready: true }
                    }
                    Action::ConfigureNrx { frequency_mhz } => {
                        Completion::NrxConfigured { frequency_mhz }
                    }
                    _ => return Err(Failure::Binding),
                }
            }
            Binding::Timer(binding) => {
                if !D::settle_micros(binding.micros()) {
                    return Err(Failure::Binding);
                }
                binding.into_completion()
            }
            _ => return Err(Failure::Binding),
        })
    }
    pub(super) fn prepare_rfpll_command(
        &mut self,
        kind: crate::tracking::observation::RfpllCommand,
    ) {
        use crate::{
            analog::i2c::{fields, registers},
            tracking::observation::RfpllCommand as Kind,
        };
        self.byte = match kind {
            Kind::InitialLow => self.rfpll_initial as u8,
            Kind::InitialHigh => fields::RFPLL_CALIBRATED_CAPACITOR_HIGH
                .insert(0, (self.rfpll_initial >> 8) as u8)
                .unwrap(),
            Kind::EnableRead | Kind::CapHighRead => 0xa5,
            Kind::Status => {
                use crate::tracking::rfpll::search::Status;
                let value = match self
                    .rfpll_statuses
                    .get(self.rfpll_samples)
                    .copied()
                    .unwrap_or(Status::Other)
                {
                    Status::Accepted => 0,
                    Status::Increase => 1,
                    Status::Decrease => 2,
                    Status::Other => 3,
                };
                fields::RFPLL_CAPACITOR_SEARCH_STATUS
                    .insert(0, value)
                    .unwrap()
            }
            _ => self.byte,
        };
    }
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
        if let Some(clock) = &self.rfpll_clock {
            self.rfpll_times.push(clock.now());
        }
        Ok(match action {
            Action::Search(action) => Completion::Search(match action {
                search::Action::ReadInitialCap => {
                    search::Completion::InitialCap(self.rfpll_initial)
                }
                search::Action::EnableSearch => search::Completion::SearchEnabled,
                search::Action::WriteCap(cap) => {
                    if let Some(clock) = &self.rfpll_clock {
                        clock
                            .advance(Duration::from_micros(self.rfpll_write_micros))
                            .expect("write duration fits");
                    }
                    search::Completion::CapWritten(cap)
                }
                search::Action::DelayMicros(micros) => {
                    if !D::settle_micros(micros) {
                        return Err(Failure::Binding);
                    }
                    if let Some(clock) = &self.rfpll_clock {
                        clock
                            .advance(Duration::from_micros(u64::from(micros)))
                            .expect("settle fits");
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
                if let Some((clock, micros)) = &self.i2c_write_publication {
                    clock
                        .advance(Duration::from_micros(*micros))
                        .expect("publication fits virtual time");
                }
                if self.i2c_busy_after_write {
                    self.busy.set(true);
                }
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
        if let Some(clock) = &self.rfpll_clock {
            clock
                .advance(Duration::from_micros(self.rfpll_command_micros))
                .expect("test command time fits");
        }
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
    fn advance_power<D: crate::executor::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        child: &mut crate::tracking::parameters::PhyParamTrackingTxPowerTransition<'_>,
        gain: &mut super::power::Cursor,
        action: crate::tracking::power::PhyTxPowerTrackingAction,
        clock: &C,
        measured: bool,
    ) -> Step {
        super::power::advance::<D, _>(
            &mut super::io::Io::Test(self, core::marker::PhantomData),
            child,
            gain,
            action,
            clock,
            measured,
        )
        .unwrap_or_else(Step::Failure)
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
        fields::SARADC_I2C_SARADC_TSENS_DAC,
    ))
}
#[test]
fn every_hardware_atom_yields_even_when_the_timer_is_already_ready() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let atoms = io.atoms.clone();
    let mut transaction = PhyColdI2cTransaction::new(PhyColdI2cRequest::write_field(
        fields::SARADC_I2C_SARADC_TSENS_DAC,
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
            None,
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
            None,
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
        &mut job, &mut io, &clock, None, None, None
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
                Some(&mut trace),
                None
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
            Some(&mut trace),
            None
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
        fields::SARADC_I2C_SARADC_TSENS_DAC,
        15,
    ));
    let mut job =
        Job::i2c(&mut transaction, clock.now(), I2cTiming::Temperature).expect("deadline fits");
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None, None, None
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
                    let bus_before = io.actions.len();
                    let result = io.advance::<TestDelay, _>(&mut job, &clock, clock.now());
                    let actions = &io.rfpll_actions[before..];
                    if result == Step::Complete {
                        assert!(actions.is_empty());
                        break;
                    }
                    assert_eq!(result, Step::Runnable);
                    let bus = &io.actions[bus_before..];
                    assert!(
                        bus.iter()
                            .filter(|a| matches!(
                                a,
                                PhyColdI2cAction::StartRead { .. }
                                    | PhyColdI2cAction::StartWrite { .. }
                            ))
                            .count()
                            <= 1
                    );
                    if actions.is_empty() {
                        assert!(!bus.is_empty());
                        clock
                            .advance(Duration::from_micros(500))
                            .expect("time fits");
                        continue;
                    }
                    match actions[0] {
                        Action::Search(crate::tracking::rfpll::search::Action::DelayMicros(_)) => {
                            assert!(matches!(
                                actions,
                                [
                                    _,
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
            // Scheduler gaps supply the minimum settle in the sliced path;
            // every hardware write/read and its order still matches native.
            let effects = |actions: Vec<Action>| {
                actions
                    .into_iter()
                    .filter(|a| {
                        !matches!(
                            a,
                            Action::Search(crate::tracking::rfpll::search::Action::DelayMicros(_))
                        )
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                effects(io.rfpll_actions.clone()),
                effects(old.rfpll_actions)
            );
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
fn rfpll_status_waits_from_the_completed_write_and_only_settles_the_remainder() {
    use crate::tracking::rfpll::{Action, search};
    for gap in [0, 2, 5, 500] {
        let clock = Rc::new(VirtualClock::<1>::new());
        let started_at = clock.now();
        let mut io = Io {
            rfpll_clock: Some(clock.clone()),
            rfpll_write_micros: 7,
            rfpll_initial: 100,
            ..Io::default()
        };
        let mut search = search::Search::new();
        let mut job =
            Job::rfpll(rfpll::Cursor::Search(&mut search), clock.now()).expect("deadline fits");
        // Two initial reads, two enable RMW commands and three cap writes.
        for _ in 0..7 {
            assert_eq!(
                io.advance::<TestDelay, _>(&mut job, &*clock, clock.now()),
                Step::Runnable
            );
        }
        assert!(matches!(
            io.rfpll_actions.as_slice(),
            [
                Action::Search(search::Action::ReadInitialCap),
                Action::Search(search::Action::EnableSearch),
                Action::Search(search::Action::WriteCap(_))
            ]
        ));
        let written_at = clock.now();
        assert_eq!(
            written_at,
            started_at
                .checked_add(Duration::from_micros(7))
                .expect("write fits")
        );
        clock.advance(Duration::from_micros(gap)).expect("gap fits");
        assert_eq!(
            io.advance::<TestDelay, _>(&mut job, &*clock, clock.now()),
            Step::Runnable
        );
        let remainder = 5_u64.saturating_sub(gap);
        let after_write = &io.rfpll_actions[3..];
        if remainder == 0 {
            assert_eq!(after_write, [Action::Search(search::Action::ReadStatus)]);
        } else {
            assert_eq!(
                after_write,
                [
                    Action::Search(search::Action::DelayMicros(remainder as u32)),
                    Action::Search(search::Action::ReadStatus)
                ]
            );
        }
        assert_eq!(
            io.rfpll_times.last().copied().expect("status was sampled"),
            written_at
                .checked_add(Duration::from_micros(gap.max(5)))
                .expect("time fits")
        );
    }
}

#[test]
fn rfpll_settle_deadline_overflow_cannot_read_status() {
    use crate::tracking::rfpll::{Action, search};
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let mut search = search::Search::new();
    {
        let mut job = Job::rfpll(rfpll::Cursor::Search(&mut search), clock.now())
            .expect("initial deadline fits");
        for _ in 0..6 {
            assert_eq!(
                io.advance::<TestDelay, _>(&mut job, &clock, clock.now()),
                Step::Runnable
            );
        }
        clock
            .advance(Duration::from_micros(
                u64::MAX - clock.now().as_micros() - 2,
            ))
            .expect("time fits");
        assert_eq!(
            io.advance::<TestDelay, _>(&mut job, &clock, clock.now()),
            Step::Failure(Failure::DeadlineOverflow)
        );
    }
    assert_eq!(search.action(), search::Action::DelayMicros(5));
    assert!(
        !io.rfpll_actions
            .contains(&Action::Search(search::Action::ReadStatus))
    );
}

#[test]
fn rfpll_rmw_keeps_the_captured_byte_and_model_owner_across_the_checkpoint() {
    use crate::tracking::rfpll::{Action, search};
    let clock = VirtualClock::<1>::new();
    let mut io = Io {
        rfpll_initial: 100,
        ..Io::default()
    };
    let mut search = search::Search::new();
    {
        let mut job =
            Job::rfpll(rfpll::Cursor::Search(&mut search), clock.now()).expect("deadline fits");
        for _ in 0..3 {
            assert_eq!(
                io.advance::<TestDelay, _>(&mut job, &clock, clock.now()),
                Step::Runnable
            );
        }
        // Only the initial read has completed at the pure model boundary.
        assert_eq!(
            io.rfpll_actions,
            [Action::Search(search::Action::ReadInitialCap)]
        );
        let captured = fields::RFPLL_CAPACITOR_SEARCH_ENABLE.insert_truncated(0xa5, 1);
        io.byte = 0;
        let before = io.actions.len();
        assert_eq!(
            io.advance::<TestDelay, _>(&mut job, &clock, clock.now()),
            Step::Runnable
        );
        assert!(
            matches!(io.actions[before..], [PhyColdI2cAction::StartWrite { value, .. }, PhyColdI2cAction::AwaitWriteCompletionEdge { .. }] if value == captured)
        );
        assert_eq!(
            io.rfpll_actions.last(),
            Some(&Action::Search(search::Action::EnableSearch))
        );
        for _ in 0..2 {
            assert_eq!(
                io.advance::<TestDelay, _>(&mut job, &clock, clock.now()),
                Step::Runnable
            );
        }
        // Cancel after the high-bit read: no CapWritten, settle or status.
    }
    assert!(matches!(search.action(), search::Action::WriteCap(_)));
    assert_eq!(io.rfpll_actions.len(), 2);
}

#[test]
fn rfpll_busy_command_times_out_without_consuming_a_partial_helper() {
    struct Progress<'a>(&'a VirtualClock<1>);
    impl Clock for Progress<'_> {
        fn now(&self) -> Instant {
            let now = self.0.now();
            self.0.advance(Duration::from_micros(1)).expect("time fits");
            now
        }
    }
    let source = VirtualClock::<1>::new();
    let clock = Progress(&source);
    let mut io = Io::default();
    io.busy.set(true);
    let mut search = crate::tracking::rfpll::search::Search::new();
    {
        let mut job =
            Job::rfpll(rfpll::Cursor::Search(&mut search), clock.now()).expect("deadline fits");
        assert_eq!(
            io.advance::<TestDelay, _>(&mut job, &clock, clock.now()),
            Step::Failure(Failure::TimedOut)
        );
    }
    assert_eq!(
        search.action(),
        crate::tracking::rfpll::search::Action::ReadInitialCap
    );
    assert!(io.rfpll_actions.is_empty());
}

#[test]
fn rfpll_standalone_search_returns_after_its_last_write() {
    use crate::tracking::rfpll::search;
    let clock = VirtualClock::<1>::new();
    let mut io = Io {
        rfpll_initial: 100,
        ..Io::default()
    };
    let mut search = search::Search::new();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut job =
            Job::rfpll(rfpll::Cursor::Search(&mut search), clock.now()).expect("deadline fits");
        let mut driver = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None, None, None
        ));
        loop {
            if let Poll::Ready(result) = driver.as_mut().poll(&mut cx) {
                assert_eq!(result, Ok(()));
                break;
            }
            clock
                .advance(Duration::from_micros(500))
                .expect("time fits");
        }
    }
    assert!(matches!(search.action(), search::Action::Complete(_)));
    assert_eq!(io.rfpll_samples, 60);
    assert!(matches!(
        io.rfpll_actions.last(),
        Some(crate::tracking::rfpll::Action::Search(
            search::Action::WriteCap(100)
        ))
    ));
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
            let mut job = Job::rfpll(
                rfpll::Cursor::Correction(child.correction_mut().expect("correction admitted")),
                clock.now(),
            )
            .expect("deadline fits");
            let mut driver = core::pin::pin!(drive::<TestDelay, _, _>(
                &mut job, &mut io, &clock, None, None, None
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
            assert!(matches!(child.action(), Thermal::Correct(_)));
            child.finish_correction().expect("correction completed");
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
            &mut job, &mut io, &clock, None, None, None
        ));
        assert!(driver.as_mut().poll(&mut cx).is_pending());
    }
    assert_eq!(
        correction.action(),
        Action::Search(crate::tracking::rfpll::search::Action::WriteCap(0))
    );
    assert!(!io.rfpll_actions.contains(&Action::Search(
        crate::tracking::rfpll::search::Action::ReadStatus
    )));
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
    for observed in [false, true] {
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
        let mut trace = crate::tracking::observation::RfpllEngineTrace::default();
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
                None,
                observed.then_some(&mut trace)
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
        // Per-command tracing retains individual checkpoints. The quiet route
        // batches four commands, including terminal recognition in the last poll.
        assert_eq!(
            pending,
            if observed {
                247
            } else {
                247 / usize::from(BATCH_COMMANDS)
            }
        );
        assert_eq!(wakes.0.load(std::sync::atomic::Ordering::Relaxed), pending);
        assert_eq!(
            (stats.pending as usize, stats.polls as usize, stats.resumes),
            (pending, pending + 1, 0)
        );
        assert!(matches!(
            correction.action(),
            crate::tracking::rfpll::Action::Complete(_)
        ));
        assert!(!trace.invalid);
    }
}

#[test]
fn rfpll_batch_stops_at_elapsed_budget_before_its_command_count_limit() {
    let clock = Rc::new(VirtualClock::<1>::new());
    let mut io = Io {
        rfpll_clock: Some(clock.clone()),
        rfpll_command_micros: 10,
        rfpll_initial: 100,
        ..Io::default()
    };
    let mut search = crate::tracking::rfpll::search::Search::new();
    let mut timing = EngineTiming::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut job =
            Job::rfpll(rfpll::Cursor::Search(&mut search), clock.now()).expect("clock fits");
        let mut driver = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job,
            &mut io,
            clock.as_ref(),
            Some(&mut timing),
            None,
            None
        ));
        for _ in 0..3 {
            let before = clock.now();
            assert!(driver.as_mut().poll(&mut cx).is_pending());
            assert_eq!(
                clock.now().saturating_duration_since(before).as_micros(),
                20
            );
            clock
                .advance(Duration::from_micros(500))
                .expect("clock fits");
        }
    }
    // The third batch contains the low-byte write and the masked high-byte
    // read. Dropping it cannot publish the high byte or finish the model.
    assert_eq!(io.actions.len(), 12);
    assert_eq!(io.rfpll_samples, 0);
    assert!(matches!(
        search.action(),
        crate::tracking::rfpll::search::Action::WriteCap(100)
    ));
    assert_eq!(
        (timing.polls, timing.pending, timing.maximum_micros),
        (3, 3, 20)
    );
}

#[test]
fn cold_rfpll_keeps_the_first_sample_wait_and_the_native_attempt_budget() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io {
        byte: 15,
        ..Io::default()
    };
    let atoms = io.atoms.clone();
    let mut transaction = read();
    let mut job = Job::counted_i2c(&mut transaction, clock.now()).expect("deadline fits");
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut driver = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None, None, None
        ));
        assert!(driver.as_mut().poll(&mut cx).is_pending()); // publish
        assert_eq!(atoms.get(), 1);
        assert!(driver.as_mut().poll(&mut cx).is_pending()); // register the first 1-us wait
        assert_eq!(atoms.get(), 1);
        assert!(driver.as_mut().poll(&mut cx).is_pending()); // no early observation
        assert_eq!(atoms.get(), 1);
        clock.advance(Duration::from_micros(1)).expect("time fits");
        assert!(driver.as_mut().poll(&mut cx).is_pending()); // consume ready
        assert_eq!(atoms.get(), 2);
        assert_eq!(driver.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    assert!(job.finished);
    // The old for-loop needs a software terminal-recognition attempt after
    // observing readiness; using that last attempt for hardware still fails.
    let mut transaction = read();
    let mut cursor = i2c::Counted::new(&mut transaction);
    cursor.remaining = 2;
    let mut hw = super::io::Io::Test(&mut io, core::marker::PhantomData);
    assert_eq!(cursor.advance_one(&mut hw, clock.now()), Ok(Step::Runnable));
    assert!(matches!(
        cursor.advance_one(&mut hw, clock.now()),
        Ok(Step::WaitAt(_))
    ));
    clock.advance(Duration::from_micros(1)).expect("time fits");
    assert_eq!(cursor.advance_one(&mut hw, clock.now()), Ok(Step::Runnable));
    assert_eq!(
        cursor.advance_one(&mut hw, clock.now()),
        Err(Failure::TimedOut)
    );
}

#[test]
fn cold_and_direct_rfpll_share_the_same_rmw_atoms_and_preserve_identity() {
    use crate::analog::rfpll::{RfpllFrequencyAction, RfpllFrequencyI2cBinding};
    let clock = VirtualClock::<1>::new();
    let action = RfpllFrequencyAction::WriteMasked {
        field: fields::RFPLL_CAPACITOR_HIGH,
        value: 1,
    };
    let mut direct = Io {
        byte: 0xa5,
        ..Io::default()
    };
    let mut cold = Io {
        byte: 0xa5,
        ..Io::default()
    };
    let mut direct_binding = RfpllFrequencyI2cBinding::new(action).expect("valid RFPLL write");
    rfpll::direct_i2c(
        &mut super::io::Io::Test(&mut direct, core::marker::PhantomData),
        &clock,
        direct_binding.transaction_mut(),
        None,
        false,
    )
    .expect("direct command completes");
    let expected = direct_binding
        .into_completion()
        .expect("direct identity retained");
    let mut binding = RfpllFrequencyI2cBinding::new(action).expect("valid RFPLL write");
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut job =
            Job::counted_i2c(binding.transaction_mut(), clock.now()).expect("deadline fits");
        let mut driver = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job, &mut cold, &clock, None, None, None
        ));
        loop {
            match driver.as_mut().poll(&mut cx) {
                Poll::Ready(result) => {
                    assert_eq!(result, Ok(()));
                    break;
                }
                Poll::Pending => {
                    clock.advance_to_next_deadline();
                }
            }
        }
    }
    assert_eq!(
        binding.into_completion().expect("cold command completes"),
        expected
    );
    assert_eq!(cold.byte, direct.byte);
    assert_eq!(cold.actions, direct.actions);
}

#[test]
fn cold_rfpll_waits_between_busy_retries_and_fails_overflow_before_publishing() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    io.busy.set(true);
    let mut transaction = read();
    let mut cursor = i2c::Counted::new(&mut transaction);
    cursor.remaining = 2;
    let mut hw = super::io::Io::Test(&mut io, core::marker::PhantomData);
    for _ in 0..2 {
        assert!(matches!(
            cursor.advance_one(&mut hw, clock.now()),
            Ok(Step::WaitAt(_))
        ));
        clock.advance(Duration::from_micros(1)).expect("time fits");
    }
    assert_eq!(
        cursor.advance_one(&mut hw, clock.now()),
        Err(Failure::TimedOut)
    );
    assert_eq!(io.actions.len(), 2);
    assert!(matches!(
        transaction.action(),
        PhyColdI2cAction::StartRead { .. }
    ));
    assert!(matches!(
        Job::counted_i2c(&mut transaction, Instant::from_micros(u64::MAX)),
        Err(Failure::DeadlineOverflow)
    ));
}
