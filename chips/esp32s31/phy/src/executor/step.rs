//! Private, single-atom PHY jobs. The physical borrow outlives every checkpoint.

use crate::{
    analog::{
        pbus::{
            PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusClearOutcome,
            PhyPbusClearTransition, PhyPbusForceTest, PhyPbusHardwareBinding,
            PhyPbusHardwareObservation,
        },
        temperature::{
            PhyTemperatureAction, PhyTemperatureCompletion, PhyTemperatureI2cBinding,
            PhyTemperatureOutcome, PhyTemperatureTransition,
        },
    },
    calibration::cold::{PhyColdI2cAction, PhyColdI2cObservation, PhyColdI2cTransaction},
    tracking::observation::EngineEvent,
};
use core::{
    cell::RefCell,
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};
use oer_time::{Duration, Instant, Timer};

/// The direct tier's existing named observation protection (wait::poll).
const EDGE_MICROS: u64 = 20;
const RETRY: Duration = Duration::from_micros(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Failure {
    DeadlineOverflow,
    TimedOut,
    Binding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Outcome {
    Temperature(PhyTemperatureOutcome),
    I2c(crate::calibration::cold::PhyColdI2cOutcome),
    Pbus {
        transaction: PhyPbusForceTest,
        completed: bool,
    },
    PbusClear(PhyPbusClearOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Step {
    Runnable,
    WaitAt(Instant),
    Complete(Outcome),
    Failure(Failure),
}

/// Only synchronous hardware edges cross this object-safe boundary. No
/// algorithm, timer future or observer type is monomorphized into the job.
pub(crate) trait StepIo {
    fn start_i2c(&mut self, transaction: &mut PhyColdI2cTransaction) -> Result<bool, Failure>;
    fn sample_i2c(
        &mut self,
        transaction: &mut PhyColdI2cTransaction,
    ) -> Result<PhyColdI2cObservation, Failure>;
    fn start_pbus(&mut self, binding: &mut PhyPbusHardwareBinding) -> Result<(), Failure>;
    fn sample_pbus(
        &mut self,
        binding: &mut PhyPbusHardwareBinding,
    ) -> Result<PhyPbusHardwareObservation, Failure>;
    fn temperature_code(&mut self) -> u8;
    /// Work-mode's 1-us settle and pulse/set/2-us/clear remain adjacent.
    fn clear_pbus_control(&mut self, work_mode: bool) -> Result<bool, Failure>;
}

pub(crate) trait Sink {
    fn enabled(&self) -> bool;
    fn observe(&mut self, event: EngineEvent);
}

enum Work<'a> {
    Temperature(&'a mut PhyTemperatureTransition),
    I2c,
    Pbus,
    PbusClear(&'a mut PhyPbusClearTransition),
}

enum Command {
    I2c(PhyColdI2cTransaction),
    Temperature(PhyTemperatureI2cBinding),
    Pbus(PhyPbusHardwareBinding),
}

struct Edge {
    command: Command,
    deadline: Instant,
    /// Cold/temperature first observations retain their 1-us delay. Direct
    /// callers use a Runnable boundary before the first immediate sample.
    delayed: bool,
}

pub(crate) struct Job<'a> {
    work: Work<'a>,
    edge: Option<Edge>,
}

impl<'a> Job<'a> {
    pub(crate) fn temperature(transition: &'a mut PhyTemperatureTransition) -> Self {
        Self {
            work: Work::Temperature(transition),
            edge: None,
        }
    }

    pub(crate) fn i2c(transaction: PhyColdI2cTransaction, now: Instant) -> Result<Self, Failure> {
        Ok(Self {
            work: Work::I2c,
            edge: Some(Edge::new(Command::I2c(transaction), now, false)?),
        })
    }

    pub(crate) fn pbus(transaction: PhyPbusForceTest, now: Instant) -> Result<Self, Failure> {
        Ok(Self {
            work: Work::Pbus,
            edge: Some(Edge::new(
                Command::Pbus(PhyPbusHardwareBinding::new(transaction)),
                now,
                false,
            )?),
        })
    }

    pub(crate) fn clear_pbus(transition: &'a mut PhyPbusClearTransition) -> Self {
        Self {
            work: Work::PbusClear(transition),
            edge: None,
        }
    }

    /// A single shared dispatcher, including terminal and source-owned timeout
    /// commits. In particular it never starts the next bus operation after a
    /// ready read in the same call.
    #[inline(never)]
    pub(crate) fn advance_one(&mut self, io: &mut dyn StepIo, now: Instant) -> Step {
        match self.advance(io, now) {
            Ok(step) => step,
            Err(error) => Step::Failure(error),
        }
    }

    fn advance(&mut self, io: &mut dyn StepIo, now: Instant) -> Result<Step, Failure> {
        if let Some(edge) = &mut self.edge {
            let result = edge.advance(io, now)?;
            if let EdgeStep::Checkpoint(step) = result {
                return Ok(step);
            }
            let completed = matches!(result, EdgeStep::Complete);
            let edge = self.edge.take().ok_or(Failure::Binding)?;
            return match (edge.command, &mut self.work) {
                (Command::Temperature(binding), Work::Temperature(transition)) if completed => {
                    transition
                        .advance(binding.into_completion().map_err(|_| Failure::Binding)?)
                        .map_err(|_| Failure::Binding)?;
                    Ok(Step::Runnable)
                }
                (Command::I2c(transaction), Work::I2c) if completed => {
                    let PhyColdI2cAction::Complete(outcome) = transaction.action() else {
                        return Err(Failure::Binding);
                    };
                    Ok(Step::Complete(Outcome::I2c(outcome)))
                }
                (Command::Pbus(binding), work) => {
                    let transaction = if completed {
                        binding.into_transaction().map_err(|_| Failure::Binding)?
                    } else {
                        binding.into_timeout_transaction()
                    };
                    if let Work::PbusClear(transition) = work {
                        transition
                            .advance(if completed {
                                PhyPbusClearCompletion::ForceTestCompleted(transaction)
                            } else {
                                PhyPbusClearCompletion::ForceTestTimedOut(transaction)
                            })
                            .map_err(|_| Failure::Binding)?;
                        Ok(Step::Runnable)
                    } else {
                        Ok(Step::Complete(Outcome::Pbus {
                            transaction,
                            completed,
                        }))
                    }
                }
                _ => Err(Failure::TimedOut),
            };
        }
        match &mut self.work {
            Work::Temperature(transition) => match transition.action() {
                PhyTemperatureAction::Complete(outcome) => {
                    Ok(Step::Complete(Outcome::Temperature(outcome)))
                }
                PhyTemperatureAction::Failed(_) => Err(Failure::Binding),
                PhyTemperatureAction::SampleCode => {
                    transition
                        .advance(PhyTemperatureCompletion::CodeSampled {
                            value: io.temperature_code(),
                        })
                        .map_err(|_| Failure::Binding)?;
                    Ok(Step::Runnable)
                }
                action => {
                    let binding =
                        PhyTemperatureI2cBinding::new(action).map_err(|_| Failure::Binding)?;
                    self.edge = Some(Edge::new(Command::Temperature(binding), now, true)?);
                    Ok(Step::Runnable)
                }
            },
            Work::PbusClear(transition) => {
                let completion = match transition.action() {
                    PhyPbusClearAction::Complete(outcome) => {
                        return Ok(Step::Complete(Outcome::PbusClear(outcome)));
                    }
                    PhyPbusClearAction::ConfigureDebugMode => {
                        io.clear_pbus_control(false)?;
                        PhyPbusClearCompletion::DebugModeConfigured
                    }
                    PhyPbusClearAction::ForceTest(transaction) => {
                        self.edge = Some(Edge::new(
                            Command::Pbus(PhyPbusHardwareBinding::new(transaction)),
                            now,
                            false,
                        )?);
                        return Ok(Step::Runnable);
                    }
                    PhyPbusClearAction::ConfigureWorkMode => {
                        let settled = io.clear_pbus_control(true)?;
                        // The complete conditional settle/pulse tail is one
                        // atom; consume its model steps without another I/O.
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
                Ok(Step::Runnable)
            }
            _ => Err(Failure::Binding),
        }
    }
}

enum EdgeStep {
    Checkpoint(Step),
    Complete,
    Timeout,
}

impl Edge {
    fn new(command: Command, now: Instant, delayed: bool) -> Result<Self, Failure> {
        Ok(Self {
            command,
            deadline: deadline(now, EDGE_MICROS)?,
            delayed,
        })
    }

    fn advance(&mut self, io: &mut dyn StepIo, now: Instant) -> Result<EdgeStep, Failure> {
        let transaction = match &mut self.command {
            Command::I2c(transaction) => Some(transaction),
            Command::Temperature(binding) => Some(binding.transaction_mut()),
            Command::Pbus(_) => None,
        };
        if let Some(transaction) = transaction {
            if matches!(transaction.action(), PhyColdI2cAction::Complete(_)) {
                return Ok(EdgeStep::Complete);
            }
            // Terminal recognition must not lose a previously accepted read
            // to the scheduler delay after its forced Pending boundary.
            if now > self.deadline {
                return Ok(EdgeStep::Timeout);
            }
            match transaction.action() {
                PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                    if io.start_i2c(transaction)? {
                        self.deadline = deadline(now, EDGE_MICROS)?;
                        return if self.delayed {
                            self.wait(now)
                        } else {
                            Ok(EdgeStep::Checkpoint(Step::Runnable))
                        };
                    }
                }
                PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                    if io.sample_i2c(transaction)? == PhyColdI2cObservation::EdgeConsumed {
                        if !matches!(transaction.action(), PhyColdI2cAction::Complete(_)) {
                            self.deadline = deadline(now, EDGE_MICROS)?;
                        }
                        return Ok(EdgeStep::Checkpoint(Step::Runnable));
                    }
                }
                PhyColdI2cAction::Complete(_) => return Ok(EdgeStep::Complete),
            }
        } else if let Command::Pbus(binding) = &mut self.command {
            if matches!(
                binding.action(),
                crate::analog::pbus::PhyPbusHardwareAction::Complete(_)
            ) {
                return Ok(EdgeStep::Complete);
            }
            if now > self.deadline {
                return Ok(EdgeStep::Timeout);
            }
            match binding.action() {
                crate::analog::pbus::PhyPbusHardwareAction::Start(_) => {
                    io.start_pbus(binding)?;
                    self.deadline = deadline(now, EDGE_MICROS)?;
                    return Ok(EdgeStep::Checkpoint(Step::Runnable));
                }
                crate::analog::pbus::PhyPbusHardwareAction::AwaitCompletionEdge(_) => {
                    if io.sample_pbus(binding)? == PhyPbusHardwareObservation::EdgeConsumed {
                        return Ok(EdgeStep::Checkpoint(Step::Runnable));
                    }
                }
                crate::analog::pbus::PhyPbusHardwareAction::Complete(_) => {
                    return Ok(EdgeStep::Complete);
                }
            }
        }
        self.wait(now)
    }

    fn wait(&self, now: Instant) -> Result<EdgeStep, Failure> {
        if now == self.deadline {
            return Ok(EdgeStep::Timeout);
        }
        Ok(EdgeStep::Checkpoint(Step::WaitAt(
            now.checked_add(RETRY)
                .ok_or(Failure::DeadlineOverflow)?
                .min(self.deadline),
        )))
    }
}

fn deadline(now: Instant, micros: u64) -> Result<Instant, Failure> {
    now.checked_add(Duration::from_micros(micros))
        .ok_or(Failure::DeadlineOverflow)
}

/// One wait position for both voluntary boundaries and not-ready backoff.
/// Even a timer ready on its first poll must schedule another executor poll.
async fn gate<T: Timer + ?Sized>(timer: &T, at: Instant, sink: &RefCell<&mut dyn Sink>) {
    let mut wait = pin!(timer.wait_until(at));
    let mut first = true;
    poll_fn(|context| {
        let result = wait.as_mut().poll(context);
        if first {
            first = false;
            if result.is_ready() {
                context.waker().wake_by_ref();
            }
            return Poll::Pending;
        }
        if result.is_ready() && sink.borrow().enabled() {
            sink.borrow_mut().observe(EngineEvent::TimerResumed {
                deadline: at.as_micros(),
                resumed: timer.now().as_micros(),
            });
        }
        result
    })
    .await
}

pub(crate) async fn drive<T: Timer + ?Sized>(
    job: &mut Job<'_>,
    io: &mut dyn StepIo,
    timer: &T,
    sink: &mut dyn Sink,
) -> Result<Outcome, Failure> {
    let sink = RefCell::new(sink);
    let mut future = pin!(async {
        loop {
            let at = match job.advance_one(io, timer.now()) {
                Step::Runnable => timer.now(),
                Step::WaitAt(at) => at,
                Step::Complete(outcome) => return Ok(outcome),
                Step::Failure(error) => return Err(error),
            };
            gate(timer, at, &sink).await;
        }
    });
    poll_fn(|context| {
        let enabled = sink.borrow().enabled();
        let started = enabled.then(|| timer.now().as_micros());
        let result = future.as_mut().poll(context);
        if let Some(started) = started {
            sink.borrow_mut().observe(EngineEvent::Poll {
                started,
                finished: timer.now().as_micros(),
                ready: result.is_ready(),
            });
        }
        result
    })
    .await
}

#[cfg(test)]
mod tests;
