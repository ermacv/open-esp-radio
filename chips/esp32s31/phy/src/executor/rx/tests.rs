use super::*;
use crate::executor::test_clock::VirtualClock;
use crate::{
    calibration::estimator::{
        PhyDcIqAccumulatorSnapshot, PhyDcIqEnablePhase, PhyDcIqMmioBinding,
        PhyDcIqReadinessBinding, PhyDcIqReadinessSnapshot,
    },
    rx::program::tests::{Oracle, at_estimator, at_field, parameters},
};
use oer_time::Timer;
use std::{cell::Cell, rc::Rc, vec::Vec};

struct Delay;
impl PhyShortDelay for Delay {
    const MAX_MICROS: u32 = 20;
    fn settle_micros(micros: u32) -> bool {
        micros > 0 && micros <= 20
    }
}
struct Fake<'a> {
    clock: &'a VirtualClock<2>,
    calls: Rc<Cell<usize>>,
    oracle: Oracle,
    pending: usize,
    events: Vec<&'static str>,
    entry_failure: bool,
    i2c_failure: bool,
    entries: usize,
}
impl<'a> Fake<'a> {
    fn new(clock: &'a VirtualClock<2>) -> Self {
        Self {
            clock,
            calls: Rc::new(Cell::new(0)),
            oracle: Oracle::default(),
            pending: 0,
            events: Vec::new(),
            entry_failure: false,
            i2c_failure: false,
            entries: 0,
        }
    }
    fn atom(&self) {
        self.calls.set(self.calls.get() + 1);
        self.clock
            .advance(Duration::from_micros(5))
            .expect("virtual atom fits");
    }
    fn ready(&mut self) -> PhyDcIqReadinessSnapshot {
        self.events.push("ready");
        let ready = self.pending == 0;
        self.pending = self.pending.saturating_sub(1);
        PhyDcIqReadinessSnapshot {
            ready,
            activity: !ready,
        }
    }
    fn accumulators(&mut self) -> PhyDcIqAccumulatorSnapshot {
        self.events.push("accumulators");
        PhyDcIqAccumulatorSnapshot {
            i: 0,
            q: 0,
            power: 256,
        }
    }
    fn enable(&mut self, phase: PhyDcIqEnablePhase, enabled: bool) {
        self.events.push(match (phase, enabled) {
            (PhyDcIqEnablePhase::Start, true) => "start-on",
            (PhyDcIqEnablePhase::Measurement, true) => "measurement-on",
            (PhyDcIqEnablePhase::Measurement, false) => "measurement-off",
            (PhyDcIqEnablePhase::Start, false) => "start-off",
        });
    }
    fn settle(&mut self, micros: u32) -> Result<(), RxError> {
        assert!(Delay::settle_micros(micros));
        self.events.push("settle");
        self.clock
            .advance(Duration::from_micros(u64::from(micros)))
            .expect("settle fits");
        Ok(())
    }
}
impl Hardware for Fake<'_> {
    fn control<D: PhyShortDelay>(&mut self, _: ControlCommand) -> Result<(), RxError> {
        self.atom();
        Ok(())
    }
    fn pbus<C: Clock>(&mut self, _: &C, _: PhyPbusForceTest) -> Result<bool, RxError> {
        self.atom();
        Ok(true)
    }
    fn read_pbus(&mut self, _: u8, _: u8) -> Result<u32, RxError> {
        self.atom();
        Ok(self.oracle.read_pbus())
    }
    fn entry(&mut self, _: PhyGainMemoryEntry) -> Result<(), RxError> {
        self.atom();
        if self.entry_failure {
            Err(RxError::Hardware)
        } else {
            self.entries += 1;
            Ok(())
        }
    }
    fn estimator<D: PhyShortDelay>(
        &mut self,
        command: PhyDcIqAction,
    ) -> Result<PhyDcIqCompletion, RxError> {
        self.atom();
        if matches!(command, PhyDcIqAction::Configure(_)) {
            PhyDcIqMmioBinding::new(command)
                .expect("configure binding")
                .capture_with(
                    self,
                    |io, _| io.events.push("configure"),
                    Self::enable,
                    Self::settle,
                    Self::ready,
                    Self::accumulators,
                )
        } else {
            PhyDcIqReadinessBinding::new(command)
                .expect("readiness binding")
                .capture_with(
                    crate::HARDWARE_EDGE_LIMIT,
                    self,
                    Self::ready,
                    Self::accumulators,
                    Self::enable,
                    Self::settle,
                )
        }
    }
    fn frequency<D: PhyShortDelay, C: Clock>(
        &mut self,
        _: &C,
        command: RfpllFrequencyAction,
    ) -> Result<RfpllFrequencyCompletion, RxError> {
        self.atom();
        Ok(self.oracle.frequency(command))
    }
    fn direct_i2c<C: Clock>(
        &mut self,
        _: &C,
        transaction: &mut PhyColdI2cTransaction,
    ) -> Result<(), RxError> {
        self.atom();
        if self.i2c_failure {
            Err(RxError::Hardware)
        } else {
            self.oracle.bus(transaction);
            Ok(())
        }
    }
    fn i2c(
        &mut self,
        transaction: &mut PhyColdI2cTransaction,
        start: bool,
    ) -> Result<PhyColdI2cObservation, RxError> {
        self.atom();
        if self.i2c_failure {
            return Err(RxError::Hardware);
        }
        let action = transaction.action();
        if start {
            match action {
                PhyColdI2cAction::StartRead { .. } => transaction.read_started(),
                PhyColdI2cAction::StartWrite { .. } => transaction.write_started(),
                _ => panic!("not a publication"),
            }
            .expect("starts");
            Ok(PhyColdI2cObservation::EdgeConsumed)
        } else {
            match action {
                PhyColdI2cAction::AwaitReadCompletionEdge { .. } => {
                    transaction.observe_read_result(Ok(0xa5))
                }
                PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                    transaction.observe_write_result(Ok(()))
                }
                _ => panic!("not an observation"),
            }
            .map_err(|_| RxError::Hardware)
        }
    }
}

#[test]
fn an_unpolled_driver_touches_neither_hardware_nor_the_retained_owner() {
    let clock = VirtualClock::<2>::new();
    let mut io = Fake::new(&clock);
    let mut program = RxProgram::new(parameters());
    let before = std::format!("{program:?}");
    let mut execution = RxGainExecution::default();
    drop(drive::<Delay, _, _, _>(
        &mut program,
        &mut io,
        &clock,
        &mut execution,
        None,
        |_, _| {},
    ));
    assert_eq!(io.calls.get(), 0);
    assert_eq!(std::format!("{program:?}"), before);
}

#[test]
fn one_driver_publishes_both_banks_across_bounded_polls_and_counts_each_poll() {
    let clock = VirtualClock::<2>::new();
    let mut io = Fake::new(&clock);
    let calls = io.calls.clone();
    let mut p = parameters();
    p.dc_calibrated = true;
    let mut program = RxProgram::new(p);
    let mut execution = RxGainExecution::default();
    let mut timing = EngineTiming::default();
    let mut phases = Vec::new();
    let mut polls = 0;
    {
        let mut future = core::pin::pin!(drive::<Delay, _, _, _>(
            &mut program,
            &mut io,
            &clock,
            &mut execution,
            Some(&mut timing),
            |op, event| phases.push((op, event))
        ));
        let mut cx = Context::from_waker(core::task::Waker::noop());
        loop {
            let before = calls.get();
            let started = clock.now();
            polls += 1;
            let result = future.as_mut().poll(&mut cx);
            assert!(calls.get() - before <= 1);
            assert!(clock.now().saturating_duration_since(started).as_micros() <= 50);
            if let Poll::Ready(result) = result {
                result.expect("publisher succeeds");
                break;
            }
        }
    }
    assert_eq!(io.entries, 148);
    assert!(polls >= io.calls.get());
    assert_eq!(usize::from(timing.polls), polls);
    assert_eq!(timing.pending + 1, timing.polls);
    assert!(!timing.invalid);
    assert_eq!(
        phases,
        [
            (Operation::RxGainPublishPhase, Event::Started),
            (Operation::RxGainPublishPhase, Event::Completed),
            (Operation::RxGainControlPhase, Event::Started),
            (Operation::RxGainControlPhase, Event::Completed)
        ]
    );
    assert!(
        program
            .terminal()
            .expect("terminal")
            .expect("successful program")
            .dc
            .is_none()
    );
}

#[test]
fn false_ready_registers_a_real_timer_and_preserves_the_atomic_capture_tail() {
    let clock = VirtualClock::<2>::new();
    let mut io = Fake::new(&clock);
    io.pending = 1;
    let calls = io.calls.clone();
    let mut program = at_estimator();
    let mut execution = RxGainExecution::default();
    {
        let mut future = core::pin::pin!(drive::<Delay, _, _, _>(
            &mut program,
            &mut io,
            &clock,
            &mut execution,
            None,
            |_, _| {}
        ));
        let mut cx = Context::from_waker(core::task::Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        let before = calls.get();
        assert!(clock.next_deadline().is_some());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(calls.get(), before, "no observation before the timer");
        clock.advance(Duration::from_micros(1)).expect("timer fits");
        assert!(future.as_mut().poll(&mut cx).is_pending());
    }
    assert_eq!(
        io.events,
        [
            "configure",
            "start-on",
            "settle",
            "measurement-on",
            "ready",
            "ready",
            "accumulators",
            "measurement-off",
            "settle",
            "start-off"
        ]
    );
    assert_eq!(execution.minimum_searches, 1);
    assert_eq!(execution.settle_1us, 2);
    assert!(program.terminal().is_none());
}

#[test]
fn an_immediately_ready_timer_is_consumed_once_and_resumes_in_a_later_poll() {
    struct Immediate<'a>(&'a VirtualClock<2>);
    impl Clock for Immediate<'_> {
        fn now(&self) -> Instant {
            self.0.now()
        }
    }
    impl oer_cooperate::CooperativeTimer for Immediate<'_> {
        fn should_yield(&self) -> bool {
            true
        }
    }
    impl Timer for Immediate<'_> {
        async fn wait_until(&self, at: Instant) {
            self.0.advance_to(at);
        }
    }
    let clock = VirtualClock::<2>::new();
    let timer = Immediate(&clock);
    let mut io = Fake::new(&clock);
    io.pending = 1;
    let calls = io.calls.clone();
    let mut program = at_estimator();
    let mut execution = RxGainExecution::default();
    let mut future = core::pin::pin!(drive::<Delay, _, _, _>(
        &mut program,
        &mut io,
        &timer,
        &mut execution,
        None,
        |_, _| {}
    ));
    let mut cx = Context::from_waker(core::task::Waker::noop());
    assert!(future.as_mut().poll(&mut cx).is_pending());
    assert_eq!(calls.get(), 1);
    assert!(future.as_mut().poll(&mut cx).is_pending());
    assert!(calls.get() > 1);
}

#[test]
fn ready_capture_and_disable_finish_before_the_poll_can_yield() {
    let clock = VirtualClock::<2>::new();
    let mut io = Fake::new(&clock);
    let mut program = at_estimator();
    let mut execution = RxGainExecution::default();
    {
        let mut future = core::pin::pin!(drive::<Delay, _, _, _>(
            &mut program,
            &mut io,
            &clock,
            &mut execution,
            None,
            |_, _| {}
        ));
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(core::task::Waker::noop()))
                .is_pending()
        );
    }
    assert_eq!(
        io.events,
        [
            "configure",
            "start-on",
            "settle",
            "measurement-on",
            "ready",
            "accumulators",
            "measurement-off",
            "settle",
            "start-off"
        ]
    );
    assert_eq!(execution.minimum_operations, 9);
}

#[test]
fn an_i2c_error_is_committed_before_the_driver_returns_the_fault() {
    let clock = VirtualClock::<2>::new();
    let mut io = Fake::new(&clock);
    io.i2c_failure = true;
    let mut program = at_field();
    let mut execution = RxGainExecution::default();
    {
        let mut future = core::pin::pin!(drive::<Delay, _, _, _>(
            &mut program,
            &mut io,
            &clock,
            &mut execution,
            None,
            |_, _| {}
        ));
        assert_eq!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(core::task::Waker::noop())),
            Poll::Ready(Err(RxError::Hardware))
        );
    }
    assert!(matches!(program.step(), Err(RxError::Hardware)));
    assert!(program.terminal().is_none());
    assert_eq!(io.calls.get(), 1);
}

#[test]
fn abandoning_a_pending_driver_retains_its_cursor_and_prevents_restart() {
    let clock = VirtualClock::<2>::new();
    let mut io = Fake::new(&clock);
    let mut program = at_estimator();
    io.pending = 1;
    let mut execution = RxGainExecution::default();
    {
        let mut future = core::pin::pin!(drive::<Delay, _, _, _>(
            &mut program,
            &mut io,
            &clock,
            &mut execution,
            None,
            |_, _| {}
        ));
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(core::task::Waker::noop()))
                .is_pending()
        );
    }
    assert_eq!(program.action_tag(), 24);
    assert!(program.terminal().is_none());
    // The outer physical owner must fail-stop: cancellation is not permission
    // to recreate a timer or restart the attempt allowance under a new driver.
    assert!(matches!(program.step(), Err(RxError::Hardware)));
}

#[test]
fn no_peer_continues_until_guard_and_new_peer_stops_at_the_next_atom() {
    for peer_after in [None, Some(23)] {
        let clock = VirtualClock::<2>::new();
        clock.set_ready_peers(false);
        if let Some(delay) = peer_after {
            clock.ready_peer_at(
                clock
                    .now()
                    .checked_add(Duration::from_micros(delay))
                    .expect("peer time fits"),
            );
        }
        let mut io = Fake::new(&clock);
        let mut program = RxProgram::new(parameters());
        let mut execution = RxGainExecution::default();
        let mut timing = EngineTiming::default();
        let mut cx = Context::from_waker(core::task::Waker::noop());
        let started = clock.now();
        {
            let mut driver = core::pin::pin!(drive::<Delay, _, _, _>(
                &mut program,
                &mut io,
                &clock,
                &mut execution,
                Some(&mut timing),
                |_, _| {}
            ));
            assert!(driver.as_mut().poll(&mut cx).is_pending());
        }
        let elapsed = clock.now().saturating_duration_since(started).as_micros();
        if let Some(at) = peer_after {
            assert!((at..=at + 50).contains(&elapsed));
        } else {
            assert!((900..=950).contains(&elapsed));
        }
        assert!(timing.maximum_micros <= 1000);
    }
}
