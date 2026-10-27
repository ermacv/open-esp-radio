//! Private PHY transport jobs. Hardware ownership stays with the caller.
mod channel_gain;
mod i2c;
mod io;
pub(crate) mod power;
pub(crate) mod rfpll;
pub(crate) mod temperature;
pub(crate) mod wifi_i2c;
pub(crate) use io::Registers;
use oer_cooperate::CooperativeTimer as Timer;

use crate::{
    analog::pbus::{
        PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusClearOutcome, PhyPbusClearTransition,
        PhyPbusForceTest, PhyPbusHardwareAction, PhyPbusHardwareBinding,
        PhyPbusHardwareObservation,
    },
    calibration::cold::{
        PhyColdI2cAction, PhyColdI2cObservation, PhyColdI2cOutcome, PhyColdI2cTransaction,
    },
    tracking::observation::{EngineTiming, ReadinessTrace},
};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use io::Io;
use oer_time::{Clock, Duration, Instant};

const EDGE_MICROS: u64 = super::wait::poll::OBSERVATION_MICROS;
const RETRY: Duration = Duration::from_micros(1);

// Leave 100 us for admission, one <=50-us atom and the return path.
const POLL_ADMISSION_MICROS: u64 = oer_cooperate::POLL_ADMISSION_MICROS as u64;

// T7's answer window: the hardware boundary is 20 us. It starts when command
// publication returns, yields the CPU, and does not delay a ready result. A
// late resume never fails an edge on its own: the bus is sampled before the
// window decides a timeout, so the window bounds only how long a busy bus is
// awaited, not the executor's resume lateness.
pub(crate) const T7_ANSWER_MICROS: u64 = 100;

#[derive(Clone, Copy)]
pub(crate) enum I2cTiming {
    Direct,
    Temperature,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Failure {
    DeadlineOverflow,
    TimedOut,
    Binding,
    #[cfg(target_arch = "riscv32")]
    Target(crate::target_executor::PhyTargetPortError),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Outcome {
    I2c(PhyColdI2cOutcome),
    Code(u8),
    Pbus {
        transaction: PhyPbusForceTest,
        completed: bool,
    },
    PbusClear(PhyPbusClearOutcome),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Step {
    Runnable,
    Rfpll(crate::tracking::observation::RfpllCommandSample),
    WaitAt(Instant),
    Complete,
    Failure(Failure),
}

enum Work<'a> {
    Channel {
        binding: &'a mut Option<crate::channel::PhyChipChannelExternalBinding>,
        completion: &'a mut Option<crate::channel::PhyChipChannelCompletion>,
        gain: channel_gain::Cursor,
    },
    Dcode {
        transition: &'a mut crate::analog::dcode::PhyDcodeTransition,
        lock: &'a mut Option<bool>,
        remaining: u32,
        i2c: Option<crate::analog::dcode::PhyDcodeI2cBinding>,
    },
    Rfpll(rfpll::Run<'a>),
    CountedI2c(i2c::Counted<'a>),
    I2c(&'a mut PhyColdI2cTransaction),
    Code(Option<u8>),
    Pbus(&'a mut PhyPbusHardwareBinding),
    PbusClear {
        transition: &'a mut PhyPbusClearTransition,
        command: Option<PhyPbusHardwareBinding>,
    },
}

pub(crate) struct Job<'a> {
    work: Work<'a>,
    // Transport answer deadline, or RFPLL's earliest status time after a
    // completed capacitor write. RFPLL never arms a timer from this field.
    deadline: Instant,
    temperature: bool,
    rfpll_observed: bool,
    first_wait: bool,
    finished: bool,
    start_pending: bool,
    /// One completed edge, consumed by the poll recorder at its exit.
    ready_started: Option<Instant>,
}
impl<'a> Job<'a> {
    fn new(work: Work<'a>, now: Instant, temperature: bool) -> Result<Self, Failure> {
        Ok(Self {
            work,
            deadline: deadline(now)?,
            temperature,
            rfpll_observed: false,
            first_wait: false,
            finished: false,
            start_pending: temperature,
            ready_started: None,
        })
    }
    pub(crate) fn i2c(
        transaction: &'a mut PhyColdI2cTransaction,
        now: Instant,
        timing: I2cTiming,
    ) -> Result<Self, Failure> {
        Self::new(
            Work::I2c(transaction),
            now,
            matches!(timing, I2cTiming::Temperature),
        )
    }
    pub(crate) fn rfpll(cursor: rfpll::Cursor<'a>, now: Instant) -> Result<Self, Failure> {
        Self::new(Work::Rfpll(rfpll::Run::new(cursor)), now, false)
    }
    pub(crate) fn counted_i2c(
        transaction: &'a mut PhyColdI2cTransaction,
        now: Instant,
    ) -> Result<Self, Failure> {
        let mut job = Self::new(Work::CountedI2c(i2c::Counted::new(transaction)), now, false)?;
        // Counted paths use the existing attempt budget rather than an elapsed answer
        // window; Driver must not clamp its real waits to a direct deadline.
        job.deadline = Instant::from_micros(u64::MAX);
        Ok(job)
    }
    pub(crate) fn code(now: Instant) -> Result<Self, Failure> {
        Self::new(Work::Code(None), now, false)
    }
    pub(crate) fn dcode(
        transition: &'a mut crate::analog::dcode::PhyDcodeTransition,
        lock: &'a mut Option<bool>,
        now: Instant,
    ) -> Result<Self, Failure> {
        Self::new(
            Work::Dcode {
                transition,
                lock,
                remaining: 100_000,
                i2c: None,
            },
            now,
            false,
        )
    }
    pub(crate) fn channel(
        binding: &'a mut Option<crate::channel::PhyChipChannelExternalBinding>,
        completion: &'a mut Option<crate::channel::PhyChipChannelCompletion>,
        now: Instant,
    ) -> Result<Self, Failure> {
        let mut job = Self::new(
            Work::Channel {
                binding,
                completion,
                gain: channel_gain::Cursor::default(),
            },
            now,
            false,
        )?;
        job.deadline = Instant::from_micros(u64::MAX);
        Ok(job)
    }

    pub(crate) fn channel_maximum_entries(&self) -> [u8; 2] {
        match &self.work {
            Work::Channel { gain, .. } => gain.maximum_entries(),
            _ => [0; 2],
        }
    }

    pub(crate) fn pbus(
        binding: &'a mut PhyPbusHardwareBinding,
        now: Instant,
    ) -> Result<Self, Failure> {
        Self::new(Work::Pbus(binding), now, false)
    }
    pub(crate) fn clear_pbus(
        transition: &'a mut PhyPbusClearTransition,
        now: Instant,
    ) -> Result<Self, Failure> {
        Self::new(
            Work::PbusClear {
                transition,
                command: None,
            },
            now,
            false,
        )
    }

    pub(crate) fn code_value(&self) -> Result<u8, Failure> {
        if let Work::Code(Some(value)) = self.work {
            if self.finished {
                return Ok(value);
            }
        }
        Err(Failure::Binding)
    }
    pub(crate) fn outcome(&self) -> Option<Outcome> {
        if !self.finished {
            return None;
        }
        Some(match &self.work {
            Work::I2c(transaction) => match transaction.action() {
                PhyColdI2cAction::Complete(outcome) => Outcome::I2c(outcome),
                _ => return None,
            },
            Work::Code(Some(value)) => Outcome::Code(*value),
            Work::Pbus(binding) => match binding.action() {
                PhyPbusHardwareAction::Complete(transaction) => Outcome::Pbus {
                    transaction,
                    completed: true,
                },
                PhyPbusHardwareAction::Start(transaction)
                | PhyPbusHardwareAction::AwaitCompletionEdge(transaction) => Outcome::Pbus {
                    transaction,
                    completed: false,
                },
            },
            Work::PbusClear { transition, .. } => match transition.action() {
                PhyPbusClearAction::Complete(outcome) => Outcome::PbusClear(outcome),
                _ => return None,
            },
            _ => return None,
        })
    }

    /// One admitted hardware checkpoint, or terminal software-only recognition.
    /// RFPLL retains partial helpers across complete bus-command checkpoints
    /// and keeps complete memory entries inside a checkpoint;
    /// temperature conversion, state publication and observers are outside it.
    #[inline(never)]
    fn advance_one<D: super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        io: Io<'_>,
        clock: &C,
        now: Instant,
    ) -> Step {
        match self.advance::<D, _>(io, clock, now) {
            Ok(mut step) => {
                if matches!(step, Step::Rfpll(_)) && !self.rfpll_observed {
                    step = Step::Runnable;
                }
                if matches!(step, Step::Complete) {
                    self.finished = true;
                }
                step
            }
            Err(error) => Step::Failure(error),
        }
    }
    fn advance<D: super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        mut io: Io<'_>,
        clock: &C,
        now: Instant,
    ) -> Result<Step, Failure> {
        self.ready_started = None;
        self.first_wait = false;
        match &mut self.work {
            Work::Channel {
                binding,
                completion,
                gain,
            } => {
                if binding.is_none() {
                    return Ok(Step::Complete);
                }
                self.ready_started = Some(clock.now());
                return if io.channel::<D, _>(clock, binding, completion, gain)? {
                    Ok(Step::WaitAt(
                        now.checked_add(RETRY).ok_or(Failure::DeadlineOverflow)?,
                    ))
                } else {
                    Ok(Step::Runnable)
                };
            }
            Work::Dcode {
                transition,
                lock,
                remaining,
                i2c,
            } => {
                use crate::analog::dcode::{
                    PhyDcodeAction as Action, PhyDcodeCompletion as Completion,
                    PhyDcodeExternalBinding as Binding,
                };
                if lock.is_some()
                    || matches!(transition.action(), Action::Complete(_) | Action::Failed(_))
                {
                    return Ok(Step::Complete);
                }
                if *remaining == 0 {
                    #[cfg(target_arch = "riscv32")]
                    return Err(Failure::Target(
                        crate::target_executor::PhyTargetPortError::RfOperationLimit,
                    ));
                    #[cfg(not(target_arch = "riscv32"))]
                    return Err(Failure::Binding);
                }
                *remaining -= 1;
                self.ready_started = Some(clock.now());
                let next = if i2c.is_none() {
                    Some(Binding::lower(transition.action()).map_err(|_| Failure::Binding)?)
                } else {
                    None
                };
                let completion = match next {
                    Some(Binding::Rfpll(binding)) => {
                        Some(Completion::Rfpll(io.frequency::<D, _>(clock, binding)?))
                    }
                    Some(Binding::I2c(binding)) => {
                        // Retain both RMW bytes before publishing the first.
                        *i2c = Some(binding);
                        None
                    }
                    None => None,
                };
                let completion = if let Some(completion) = completion {
                    completion
                } else {
                    let binding = i2c.as_mut().ok_or(Failure::Binding)?;
                    // A byte command retains its original twenty-us settle;
                    // previous bytes and executor suspension use none of it.
                    rfpll::direct_i2c(
                        &mut io,
                        clock,
                        binding.transaction_mut(),
                        None,
                        false,
                        rfpll::I2cBoundary::Command,
                    )?;
                    if !matches!(binding.action(), PhyColdI2cAction::Complete(_)) {
                        return Ok(Step::Runnable);
                    }
                    i2c.take()
                        .ok_or(Failure::Binding)?
                        .into_completion()
                        .map_err(|_| Failure::Binding)?
                };
                if let Completion::Rfpll(
                    crate::analog::rfpll::RfpllFrequencyCompletion::MaskedRead {
                        field: crate::analog::i2c::analog_i2c::rfpll::lock_status::LOCK_STATUS,
                        value,
                    },
                ) = completion
                {
                    **lock = Some(value != 0);
                }
                transition
                    .advance(completion)
                    .map_err(|_| Failure::Binding)?;
                return Ok(Step::Runnable);
            }
            Work::Rfpll(run) => {
                return io.advance_rfpll::<D, _>(
                    clock,
                    run,
                    &mut self.deadline,
                    self.rfpll_observed,
                );
            }
            Work::CountedI2c(cursor) => return cursor.advance_one(&mut io, now),
            Work::Code(code) => {
                if code.is_some() {
                    return Ok(Step::Complete);
                }
                *code = Some(io.temperature_code());
                return Ok(Step::Runnable);
            }
            Work::I2c(transaction) => {
                if matches!(transaction.action(), PhyColdI2cAction::Complete(_)) {
                    return Ok(Step::Complete);
                }
                // No command is in flight while Runnable waits for the next
                // poll. Bound a busy-at-start retry from its first attempt,
                // rather than charging that voluntary yield to hardware.
                if self.start_pending {
                    self.deadline = deadline(now)?;
                    self.start_pending = false;
                }
                // The bus is sampled before the deadline is judged: a resume
                // late after the hardware answered consumes that answer, and
                // a start busy at its deadline gets this last attempt. Only
                // a bus still busy at or after the deadline times out below.
                match transaction.action() {
                    PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                        if io.i2c(transaction, true)? == PhyColdI2cObservation::EdgeConsumed {
                            self.deadline = if self.temperature {
                                answer_deadline(now)?
                            } else {
                                deadline(now)?
                            };
                            if !self.temperature {
                                return Ok(Step::Runnable);
                            }
                            self.first_wait = true;
                        }
                    }
                    _ => {
                        if io.i2c(transaction, false)? == PhyColdI2cObservation::EdgeConsumed {
                            // The checked command answer deadline retains its publication timestamp.
                            self.ready_started = Some(Instant::from_micros(
                                self.deadline.as_micros().wrapping_sub(if self.temperature {
                                    T7_ANSWER_MICROS
                                } else {
                                    EDGE_MICROS
                                }),
                            ));
                            if !matches!(transaction.action(), PhyColdI2cAction::Complete(_)) {
                                self.deadline = deadline(now)?;
                                self.start_pending = self.temperature;
                            }
                            return Ok(Step::Runnable);
                        }
                    }
                }
            }
            Work::Pbus(binding) => {
                if matches!(binding.action(), PhyPbusHardwareAction::Complete(_)) {
                    return Ok(Step::Complete);
                }
                if now > self.deadline {
                    return Ok(Step::Complete);
                }
                if matches!(binding.action(), PhyPbusHardwareAction::Start(_)) {
                    io.pbus(binding, true)?;
                    self.deadline = deadline(now)?;
                    return Ok(Step::Runnable);
                }
                if io.pbus(binding, false)? == PhyPbusHardwareObservation::EdgeConsumed {
                    // Every deadline was checked as start + EDGE_MICROS, so this cannot underflow.
                    self.ready_started = Some(Instant::from_micros(
                        self.deadline.as_micros().wrapping_sub(EDGE_MICROS),
                    ));
                    return Ok(Step::Runnable);
                }
                if now == self.deadline {
                    return Ok(Step::Complete);
                }
            }
            Work::PbusClear {
                transition,
                command,
            } => {
                if let Some(binding) = command {
                    let transaction = match binding.action() {
                        PhyPbusHardwareAction::Start(t)
                        | PhyPbusHardwareAction::AwaitCompletionEdge(t)
                        | PhyPbusHardwareAction::Complete(t) => t,
                    };
                    let completed = matches!(binding.action(), PhyPbusHardwareAction::Complete(_));
                    if completed || now > self.deadline {
                        transition
                            .advance(if completed {
                                PhyPbusClearCompletion::ForceTestCompleted(transaction)
                            } else {
                                PhyPbusClearCompletion::ForceTestTimedOut(transaction)
                            })
                            .map_err(|_| Failure::Binding)?;
                        *command = None;
                        return Ok(Step::Runnable);
                    }
                    if matches!(binding.action(), PhyPbusHardwareAction::Start(_)) {
                        io.pbus(binding, true)?;
                        self.deadline = deadline(now)?;
                        return Ok(Step::Runnable);
                    }
                    if io.pbus(binding, false)? == PhyPbusHardwareObservation::EdgeConsumed {
                        self.ready_started = Some(Instant::from_micros(
                            self.deadline.as_micros().wrapping_sub(EDGE_MICROS),
                        ));
                        return Ok(Step::Runnable);
                    }
                    if now == self.deadline {
                        transition
                            .advance(PhyPbusClearCompletion::ForceTestTimedOut(transaction))
                            .map_err(|_| Failure::Binding)?;
                        *command = None;
                        return Ok(Step::Runnable);
                    }
                } else {
                    let completion = match transition.action() {
                        PhyPbusClearAction::Complete(_) => {
                            return Ok(Step::Complete);
                        }
                        PhyPbusClearAction::ForceTest(transaction) => {
                            *command = Some(PhyPbusHardwareBinding::new(transaction));
                            self.deadline = deadline(now)?;
                            return Ok(Step::Runnable);
                        }
                        PhyPbusClearAction::ConfigureDebugMode => {
                            io.pbus_control(false)?;
                            PhyPbusClearCompletion::DebugModeConfigured
                        }
                        PhyPbusClearAction::ConfigureWorkMode => {
                            let settled = io.pbus_control(true)?;
                            transition
                                .advance(PhyPbusClearCompletion::WorkModeConfigured {
                                    settle_required: settled,
                                })
                                .map_err(|_| Failure::Binding)?;
                            if settled {
                                for completion in [
                                    PhyPbusClearCompletion::DelayElapsed,
                                    PhyPbusClearCompletion::WorkModePulseConfigured,
                                    PhyPbusClearCompletion::DelayElapsed,
                                    PhyPbusClearCompletion::WorkModePulseCleared,
                                ] {
                                    transition
                                        .advance(completion)
                                        .map_err(|_| Failure::Binding)?;
                                }
                            }
                            return Ok(Step::Runnable);
                        }
                        _ => return Err(Failure::Binding),
                    };
                    transition
                        .advance(completion)
                        .map_err(|_| Failure::Binding)?;
                    return Ok(Step::Runnable);
                }
            }
        }
        if now >= self.deadline {
            return Err(Failure::TimedOut);
        }
        Ok(Step::WaitAt(
            now.checked_add(RETRY)
                .ok_or(Failure::DeadlineOverflow)?
                .min(self.deadline),
        ))
    }
}
fn answer_deadline(now: Instant) -> Result<Instant, Failure> {
    now.checked_add(Duration::from_micros(T7_ANSWER_MICROS))
        .ok_or(Failure::DeadlineOverflow)
}
fn deadline(now: Instant) -> Result<Instant, Failure> {
    now.checked_add(Duration::from_micros(EDGE_MICROS))
        .ok_or(Failure::DeadlineOverflow)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Gate {
    Idle,
    Waiting,
    Ready,
    Yield,
}

pin_project_lite::pin_project! {
    #[project = RouteProj]
    enum Route<'a, 'j, F> {
        Transport {
            job: &'a mut Job<'j>,
            stats: Option<&'a mut EngineTiming>,
            #[pin]
            wait: Option<F>,
            wait_at: Instant,
        },
        Power {
            child: Option<crate::tracking::parameters::PhyParamTrackingTxPowerTransition<'a>>,
            timing: EngineTiming,
            gain: power::Cursor,
            measured: bool,
        },
    }
}
enum Input<'a, 'j> {
    Transport {
        job: &'a mut Job<'j>,
        stats: Option<&'a mut EngineTiming>,
    },
    Power {
        child: crate::tracking::parameters::PhyParamTrackingTxPowerTransition<'a>,
        measured: bool,
    },
}

pin_project_lite::pin_project! {
    struct Driver<'a, 'j, R, T, F, M, D> {
        delay: core::marker::PhantomData<D>,
        #[pin]
        route: Route<'a, 'j, F>,
        registers: &'a mut R,
        timer: &'a T,
        rfpll: Option<&'a mut crate::tracking::observation::RfpllEngineTrace>,
        trace: Option<&'a mut ReadinessTrace>,
        make_wait: M,
        gate: Gate,
    }
}

/// The power adapter only converts the shared engine's terminal result.
/// Its owner and observations live inside Driver, never in an outer async frame.
trait Engine: Future<Output = Result<(), Failure>> {
    fn observe_calibration(self: Pin<&mut Self>);
    fn finish_power(
        self: Pin<&mut Self>,
        result: Result<(), Failure>,
    ) -> (
        Result<crate::tracking::parameters::PhyParamTrackingCompletion, Failure>,
        EngineTiming,
        crate::tracking::observation::GainTiming,
    );
}
impl<
    'a,
    R: Registers,
    T: Timer,
    F: Future<Output = ()>,
    M: Fn(&'a T, Instant) -> F,
    D: super::wait::PhyShortDelay,
> Engine for Driver<'a, '_, R, T, F, M, D>
{
    fn observe_calibration(self: Pin<&mut Self>) {
        if let RouteProj::Transport {
            job,
            stats: Some(timing),
            ..
        } = self.project().route.project()
        {
            let terminal = job.finished;
            match &mut job.work {
                Work::Channel { gain, .. } => gain.observe(timing, terminal),
                _ => {}
            }
        }
    }
    fn finish_power(
        self: Pin<&mut Self>,
        result: Result<(), Failure>,
    ) -> (
        Result<crate::tracking::parameters::PhyParamTrackingCompletion, Failure>,
        EngineTiming,
        crate::tracking::observation::GainTiming,
    ) {
        match self.project().route.project() {
            RouteProj::Power {
                child,
                timing,
                gain,
                ..
            } => {
                let result = result.and_then(|()| {
                    child
                        .take()
                        .ok_or(Failure::Binding)?
                        .commit()
                        .map_err(|_| Failure::Binding)
                });
                (result, *timing, gain.timing)
            }
            RouteProj::Transport { .. } => (
                Err(Failure::Binding),
                EngineTiming::default(),
                Default::default(),
            ),
        }
    }
}
impl<
    'a,
    R: Registers,
    T: Timer,
    F: Future<Output = ()>,
    M: Fn(&'a T, Instant) -> F,
    D: super::wait::PhyShortDelay,
> Future for Driver<'a, '_, R, T, F, M, D>
{
    type Output = Result<(), Failure>;
    oer_esp32s31_hal::place_phy_engine_hot_path! {
        /// One engine poll per timer/register type, shared by all families.
        #[inline(never)]
        fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            let mut this = self.project();
            let mut route = this.route.as_mut().project();
            let clock = &rfpll::OutlinedClock(*this.timer);
            let started = match &route {
                RouteProj::Transport { stats, .. } => stats.as_ref().map(|_| clock.now()),
                RouteProj::Power { measured, .. } => measured.then(|| clock.now()),
            };
            let mut resumed = None;
            let mut advanced = false;
            let mut ready_elapsed = None;
            let mut ready_started = None;
            let mut rfpll_sample = None;
            let batch_started = started.unwrap_or_else(|| clock.now());
            let result = loop {
                if *this.gate == Gate::Yield {
                    if let Some(trace) = this.trace.as_deref_mut() {
                        if let RouteProj::Transport { wait_at, .. } = &route {
                            trace.yield_resumed(clock.now().saturating_duration_since(**wait_at).as_micros());
                        }
                    }
                    *this.gate = Gate::Idle;
                }
                if *this.gate != Gate::Idle {
                    let RouteProj::Transport { wait_at, .. } = &route else { break Poll::Ready(Err(Failure::Binding)); };
                    let at = **wait_at;
                    let ready = if *this.gate == Gate::Ready {
                        true
                    } else if let RouteProj::Transport { wait, .. } = &mut route {
                        if let Some(wait) = wait.as_mut().as_pin_mut() {
                            poll_timer(wait, context).is_ready()
                        } else { break Poll::Ready(Err(Failure::Binding)); }
                    } else { break Poll::Ready(Err(Failure::Binding)); };
                    if advanced {
                        *this.gate = if ready { Gate::Ready } else { Gate::Waiting };
                        if ready { context.waker().wake_by_ref(); }
                        break Poll::Pending;
                    }
                    if !ready { break Poll::Pending; }
                    resumed = Some((at, clock.now()));
                    *this.gate = Gate::Idle;
                }
                let (step, now) = match &mut route {
                    RouteProj::Transport { job, stats, .. } => {
                        let sample = this.trace.as_ref().and_then(|_| match &job.work {
                            Work::I2c(t) => match t.action() {
                                PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => Some(None),
                                PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                                | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => Some(Some(job.first_wait)),
                                _ => None,
                            },
                            _ => None,
                        });
                        let now = if matches!(&job.work, Work::Rfpll(_) | Work::Dcode { .. }) { Instant::EPOCH } else { clock.now() };
                        let mut step = this.registers.advance::<D, _>(job, *this.timer, now);
                        if job.first_wait {
                            match answer_deadline(clock.now()) {
                                Ok(at) => job.deadline = at,
                                Err(error) => step = Step::Failure(error),
                            }
                        }
                        if stats.is_some() {
                            if let Some(at) = job.ready_started.take() {
                                let at = *ready_started.get_or_insert(at);
                                ready_elapsed = Some(clock.now().saturating_duration_since(at));
                            }
                        }
                        if let (Some(trace), Some(sample)) = (this.trace.as_deref_mut(), sample) {
                            if let Some(first) = sample {
                                trace_checked(trace, *this.registers, now, first, step, resumed);
                            } else if job.first_wait { trace.command_started(clock.now()); }
                        }
                        (step, now)
                    }
                    RouteProj::Power { child, measured, gain: cursor, .. } => {
                        let Some(child) = child.as_mut() else { break Poll::Ready(Err(Failure::Binding)); };
                        use crate::tracking::power::PhyTxPowerTrackingAction as Action;
                        let action = child.action();
                        let gain = **measured && matches!(action,
                            Action::RegenerateWifiGain { .. } | Action::RegenerateBluetoothIeee802154Gain { .. });
                        let at = gain.then(|| clock.now());
                        // This owning route calls stepped control/gain atoms directly;
                        // no monolithic gain-image lowering is reachable here.
                        let step = this.registers.advance_power::<D, _>(child, cursor, action, clock, gain);
                        if let Some(at) = at { ready_elapsed = Some(clock.now().saturating_duration_since(at)); }
                        (step, Instant::EPOCH)
                    }
                };
                let at = match step {
                    Step::Complete => break Poll::Ready(Ok(())),
                    Step::Failure(error) => break Poll::Ready(Err(error)),
                    Step::Rfpll(_) | Step::Runnable => {
                        if let Step::Rfpll(sample) = step { rfpll_sample = Some(sample); }
                        if this.rfpll.is_none() && !this.timer.should_yield()
                            && clock.now().saturating_duration_since(batch_started).as_micros() < POLL_ADMISSION_MICROS
                        {
                            continue;
                        }
                        *this.gate = Gate::Yield;
                        context.waker().wake_by_ref();
                        if this.trace.is_some() {
                            if let RouteProj::Transport { wait_at, .. } = &mut route { **wait_at = clock.now(); }
                        }
                        break Poll::Pending;
                    }
                    Step::WaitAt(at) => {
                        let completed = clock.now();
                        let Some(at) = completed.checked_add(at.saturating_duration_since(now)) else {
                            break Poll::Ready(Err(Failure::DeadlineOverflow));
                        };
                        let RouteProj::Transport { job, .. } = &route else {
                            break Poll::Ready(Err(Failure::Binding));
                        };
                        if job.first_wait { at } else { at.min(job.deadline) }
                    }
                };
                let RouteProj::Transport { wait_at, .. } = &mut route else { break Poll::Ready(Err(Failure::Binding)); };
                **wait_at = at;
                if at <= clock.now() { *this.gate = Gate::Ready; }
                else {
                    let RouteProj::Transport { wait, .. } = &mut route else {
                        break Poll::Ready(Err(Failure::Binding));
                    };
                    wait.as_mut().set(Some((this.make_wait)(*this.timer, at)));
                    *this.gate = Gate::Waiting;
                }
                advanced = true;
            };
            let stats = match route {
                RouteProj::Transport { stats, .. } => stats.as_deref_mut(),
                RouteProj::Power { timing, measured, .. } => measured.then_some(timing),
            };
            if let (Some(stats), Some(started)) = (stats, started) {
                stats.poll_finished(clock, started, result.is_pending(), resumed,
                    ready_elapsed, this.rfpll.as_deref_mut().zip(rfpll_sample));
            }
            result
        }
    }
}

// Keep failure/status accounting off the hardware dispatch path.
#[inline(never)]
fn trace_checked<R: Registers>(
    trace: &mut ReadinessTrace,
    registers: &mut R,
    now: Instant,
    first: bool,
    step: Step,
    resumed: Option<(Instant, Instant)>,
) {
    let failure = if step == Step::Failure(Failure::TimedOut) {
        Some((
            registers.i2c_status(),
            resumed.map_or(0, |(at, now)| now.saturating_duration_since(at).as_micros()),
        ))
    } else {
        None
    };
    trace.checked(
        now,
        first,
        matches!(step, Step::WaitAt(_)) || failure.is_some_and(|(busy, _)| busy != 0),
        failure,
    );
}

#[inline(never)]
fn poll_timer<F: Future<Output = ()>>(wait: Pin<&mut F>, context: &mut Context<'_>) -> Poll<()> {
    wait.poll(context)
}

fn driver<'a, 'j, D: super::wait::PhyShortDelay + 'a, R: Registers, T: Timer>(
    input: Input<'a, 'j>,
    registers: &'a mut R,
    timer: &'a T,
    trace: Option<&'a mut ReadinessTrace>,
    rfpll: Option<&'a mut crate::tracking::observation::RfpllEngineTrace>,
) -> impl Engine + 'a {
    let route = match input {
        Input::Transport { job, stats } => {
            job.rfpll_observed = rfpll.is_some();
            Route::Transport {
                job,
                stats,
                wait: None,
                wait_at: Instant::EPOCH,
            }
        }
        Input::Power { child, measured } => Route::Power {
            child: Some(child),
            timing: EngineTiming::default(),
            gain: power::Cursor::default(),
            measured,
        },
    };
    Driver {
        delay: core::marker::PhantomData::<D>,
        route,
        registers,
        timer,
        rfpll,
        trace,
        make_wait: |timer: &'a T, at| timer.wait_until(at),
        gate: Gate::Idle,
    }
}
pub(crate) fn drive<'a, 'j, D: super::wait::PhyShortDelay + 'a, R: Registers, T: Timer>(
    job: &'a mut Job<'j>,
    registers: &'a mut R,
    timer: &'a T,
    stats: Option<&'a mut EngineTiming>,
    trace: Option<&'a mut ReadinessTrace>,
    rfpll: Option<&'a mut crate::tracking::observation::RfpllEngineTrace>,
) -> impl Future<Output = Result<(), Failure>> + 'a {
    driver::<D, _, _>(
        Input::Transport { job, stats },
        registers,
        timer,
        trace,
        rfpll,
    )
}

pin_project_lite::pin_project! {
    struct CalibrationTrace<F> { #[pin] inner: F }
}
impl<F: Engine> Future for CalibrationTrace<F> {
    type Output = Result<(), Failure>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        let result = this.inner.as_mut().poll(cx);
        this.inner.observe_calibration();
        result
    }
}

pub(crate) fn drive_calibration<
    'a,
    'j,
    D: super::wait::PhyShortDelay + 'a,
    R: Registers,
    T: Timer,
>(
    job: &'a mut Job<'j>,
    registers: &'a mut R,
    timer: &'a T,
    stats: Option<&'a mut EngineTiming>,
) -> impl Future<Output = Result<(), Failure>> + 'a {
    CalibrationTrace {
        inner: driver::<D, _, _>(
            Input::Transport { job, stats },
            registers,
            timer,
            None,
            None,
        ),
    }
}

#[cfg(test)]
pub(crate) mod tests;
