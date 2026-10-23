//! Direct typed RX leaves and one retained poll loop, without per-leaf futures.
mod hardware;
use super::wait::PhyShortDelay;
use crate::{
    analog::{
        pbus::PhyPbusForceTest,
        rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion},
    },
    calibration::{
        baseband::PhyGainMemoryEntry,
        cold::{PhyColdI2cAction, PhyColdI2cObservation, PhyColdI2cTransaction},
        estimator::{PhyDcIqAction, PhyDcIqCompletion},
    },
    rx::program::{ControlCommand, RxError, RxPhase, RxProgram, RxStep},
    tracking::observation::{EngineTiming, Event, Operation, RxGainExecution},
};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use oer_cooperate::CooperativeTimer as Timer;
use oer_time::{Clock, Duration, Instant};

pub(crate) trait Hardware {
    fn control<D: PhyShortDelay>(&mut self, command: ControlCommand) -> Result<(), RxError>;
    fn pbus<C: Clock>(&mut self, clock: &C, command: PhyPbusForceTest) -> Result<bool, RxError>;
    fn read_pbus(&mut self, selector: u8, path: u8) -> Result<u32, RxError>;
    fn entry(&mut self, command: PhyGainMemoryEntry) -> Result<(), RxError>;
    fn estimator<D: PhyShortDelay>(
        &mut self,
        command: PhyDcIqAction,
    ) -> Result<PhyDcIqCompletion, RxError>;
    fn frequency<D: PhyShortDelay, C: Clock>(
        &mut self,
        clock: &C,
        command: RfpllFrequencyAction,
    ) -> Result<RfpllFrequencyCompletion, RxError>;
    fn direct_i2c<C: Clock>(
        &mut self,
        clock: &C,
        transaction: &mut PhyColdI2cTransaction,
    ) -> Result<(), RxError>;
    fn i2c(
        &mut self,
        transaction: &mut PhyColdI2cTransaction,
        start: bool,
    ) -> Result<PhyColdI2cObservation, RxError>;
}

enum Atom {
    Complete,
    Runnable,
    Wait,
}

/// All results, including hardware errors, go through their borrowed token
/// before this function returns. No await or early ? separates publication
/// from recording the program's cleanup obligation.
#[inline(never)]
fn atom<D: PhyShortDelay, R: Hardware, C: Clock>(
    program: &mut RxProgram,
    registers: &mut R,
    clock: &C,
    execution: &mut RxGainExecution,
    measured: bool,
    tag: u8,
    #[cfg(feature = "radio-health")] observe: &mut impl FnMut(Operation, Event),
) -> Result<Atom, RxError> {
    let begun = measured.then(|| clock.now());
    let command = program.step()?;
    let lowered = measured.then(|| clock.now());
    macro_rules! complete {
        ($step:expr,$result:expr) => {{
            let hardware = measured.then(|| clock.now());
            let result = $step.complete($result);
            if let (Some(begun), Some(lowered), Some(hardware)) = (begun, lowered, hardware) {
                record_parts(execution, tag, [begun, lowered, hardware, clock.now()]);
            }
            result
        }};
    }
    let result = match command {
        RxStep::Complete => return Ok(Atom::Complete),
        RxStep::Pbus(mut step) => {
            let command = step.command();
            step.prepare_publication();
            let result = registers.pbus(clock, command);
            complete!(step, result)
        }
        RxStep::ReadPbus(step) => {
            let (selector, path) = step.command();
            let result = registers.read_pbus(selector, path);
            complete!(step, result)
        }
        RxStep::Control(mut step) => {
            let command = step.command();
            step.prepare_publication();
            let result = registers.control::<D>(command);
            complete!(step, result)
        }
        RxStep::GainEntry(step) => {
            let command = step.command();
            let result = registers.entry(command);
            complete!(step, result)
        }
        RxStep::Settle(step) => {
            let micros = step.micros();
            if micros == 10 {
                execution.settle_10us += 1;
            }
            let result = if micros != 0 && micros <= D::MAX_MICROS && D::settle_micros(micros) {
                Ok(())
            } else {
                Err(RxError::Capability)
            };
            complete!(step, result)
        }
        RxStep::Frequency(step) => {
            let command = step.command();
            let result = registers.frequency::<D, _>(clock, command);
            complete!(step, result)
        }
        RxStep::Estimator(step) => {
            let command = step.command();
            #[cfg(feature = "radio-health")]
            let health_context = step.health_context()?;
            if let PhyDcIqAction::Configure(request) = command {
                if execution.minimum_operations
                    > 100_000 - u32::from(crate::HARDWARE_EDGE_LIMIT) - 8
                {
                    return complete!(step, Err(RxError::OperationLimit)).map(|()| Atom::Complete);
                }
                execution.minimum_operations += 4;
                execution.settle_1us += 1;
                if request.iteration == 0 {
                    execution.minimum_searches += 1;
                }
            }
            #[cfg(feature = "radio-health")]
            let health_started = clock.now();
            let result = registers.estimator::<D>(command);
            #[cfg(feature = "radio-health")]
            let observation = result.ok().and_then(|completion| {
                crate::tracking::observation::health::DcIq::observed(
                    health_started.as_micros(),
                    clock.now().as_micros(),
                    completion,
                    health_context,
                )
            });
            let wait = matches!(
                result,
                Ok(PhyDcIqCompletion::CapturePending { .. }
                    | PhyDcIqCompletion::ReadinessObserved {
                        snapshot: crate::calibration::estimator::PhyDcIqReadinessSnapshot {
                            ready: false,
                            ..
                        },
                        ..
                    })
            );
            match result {
                Ok(PhyDcIqCompletion::CaptureFinished { accumulators, .. }) => {
                    execution.minimum_operations += 3 + u32::from(accumulators.is_some()) * 2;
                    execution.settle_1us += 1;
                }
                Ok(
                    PhyDcIqCompletion::ReadinessObserved { .. }
                    | PhyDcIqCompletion::CapturePending { .. },
                ) => execution.minimum_operations += 1,
                _ => {}
            }
            let completed = complete!(step, result);
            #[cfg(feature = "radio-health")]
            if let Some(observation) = observation {
                observe(Operation::RxGainDcPhase, Event::DcIq(observation));
            }
            if wait {
                completed?;
                return Ok(Atom::Wait);
            }
            completed
        }
        RxStep::I2c(mut step) => {
            step.prepare_publication();
            let result = match step.transaction_mut() {
                Ok(transaction) => registers.direct_i2c(clock, transaction),
                Err(error) => Err(error),
            };
            complete!(step, result)
        }
    };
    result?;
    Ok(Atom::Runnable)
}

#[inline(never)]
fn record_parts(execution: &mut RxGainExecution, tag: u8, at: [Instant; 4]) {
    let [a, b, c, d] = at;
    for ((maximum, action), (from, to)) in execution
        .maximum_parts_micros
        .iter_mut()
        .zip(execution.maximum_parts_actions.iter_mut())
        .zip([(a, b), (b, c), (c, d)])
    {
        let elapsed = to
            .checked_duration_since(from)
            .and_then(|d| u32::try_from(d.as_micros()).ok())
            .unwrap_or(u32::MAX);
        if elapsed > *maximum {
            *maximum = elapsed;
            *action = tag;
        }
    }
}

pin_project_lite::pin_project! {
    struct Driver<'a,R,T,F,M,D,O> {
        program:&'a mut RxProgram,registers:&'a mut R,timer:&'a T,
        execution:&'a mut RxGainExecution,timing:Option<&'a mut EngineTiming>,
        observe:O,phase:Option<RxPhase>,seen:u64,
        #[pin] wait:Option<F>,wait_at:Instant,wait_ready:bool,make_wait:M,
        entered:bool,finished:bool,
        delay:core::marker::PhantomData<D>,
    }
    impl<'a,R,T,F,M,D,O> PinnedDrop for Driver<'a,R,T,F,M,D,O> {
        fn drop(this:Pin<&mut Self>) {
            let this=this.project();
            if *this.entered && !*this.finished {this.program.abandon();}
        }
    }
}

fn operation(phase: RxPhase) -> Operation {
    match phase {
        RxPhase::Control => Operation::RxGainControlPhase,
        RxPhase::Dc => Operation::RxGainDcPhase,
        RxPhase::Publish => Operation::RxGainPublishPhase,
    }
}

impl<
    'a,
    R: Hardware,
    T: Timer,
    F: Future<Output = ()>,
    M: Fn(&'a T, Instant) -> F,
    D: PhyShortDelay,
    O: FnMut(Operation, Event),
> Future for Driver<'a, R, T, F, M, D, O>
{
    type Output = Result<(), RxError>;
    #[inline(never)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        *this.entered = true;
        let clock = &super::step::rfpll::OutlinedClock(*this.timer);
        let started = clock.now();
        let mut resumed = None;
        let mut ready = None;
        let mut tag = 0;
        let mut i2c_tag = None;
        let result = loop {
            if let Some(wait) = this.wait.as_mut().as_pin_mut() {
                if !*this.wait_ready && wait.poll(cx).is_pending() {
                    break Poll::Pending;
                }
                *this.wait_ready = false;
                resumed = Some((*this.wait_at, clock.now()));
                this.wait.as_mut().set(None);
            }
            let phase = this.program.phase();
            if phase != *this.phase {
                if let Some(old) = this.phase.take() {
                    (this.observe)(operation(old), Event::Completed);
                }
                if let Some(new) = phase {
                    (this.observe)(operation(new), Event::Started);
                }
                *this.phase = phase;
            }
            let action = this.program.action_tag();
            let bit = 1_u64 << action;
            tag = action | if *this.seen & bit == 0 { 0x80 } else { 0 };
            *this.seen |= bit;
            if action == 11 {
                i2c_tag = Some(tag);
            }
            if action == 16 {
                this.execution.outer_operations += 1;
            }
            let at = clock.now();
            let step = atom::<D, _, _>(
                this.program,
                *this.registers,
                clock,
                this.execution,
                this.timing.is_some(),
                tag,
                #[cfg(feature = "radio-health")]
                &mut this.observe,
            );
            #[cfg(feature = "radio-health")]
            if let Some(correction) = this.program.take_health_correction() {
                (this.observe)(Operation::RxGainDcPhase, Event::RxDcCorrection(correction));
            }
            let finished = clock.now();
            ready = Some(finished.saturating_duration_since(at));
            // Phase edges are observed immediately, before any voluntary
            // yield, while the same RF/control owner is still borrowed.
            let phase = this.program.phase();
            if phase != *this.phase {
                if let Some(old) = this.phase.take() {
                    (this.observe)(operation(old), Event::Completed);
                }
                if let Some(new) = phase {
                    (this.observe)(operation(new), Event::Started);
                }
                *this.phase = phase;
            }
            match step {
                Err(error) => {
                    if let Some(old) = this.phase.take() {
                        (this.observe)(operation(old), Event::Failed);
                    }
                    break Poll::Ready(Err(error));
                }
                Ok(Atom::Complete) => break Poll::Ready(Ok(())),
                Ok(Atom::Wait) => {
                    let Some(at) = clock.now().checked_add(Duration::from_micros(1)) else {
                        break Poll::Ready(Err(RxError::Hardware));
                    };
                    *this.wait_at = at;
                    this.wait
                        .as_mut()
                        .set(Some((this.make_wait)(*this.timer, at)));
                    // Register the timer in this owning poll; a ready
                    // timer resumes only in a later poll.
                    let Some(wait) = this.wait.as_mut().as_pin_mut() else {
                        break Poll::Ready(Err(RxError::WrongCompletion));
                    };
                    if wait.poll(cx).is_ready() {
                        *this.wait_ready = true;
                        cx.waker().wake_by_ref();
                    }
                    break Poll::Pending;
                }
                Ok(Atom::Runnable) => {
                    if !this.timer.should_yield()
                        && clock.now().saturating_duration_since(started).as_micros()
                            < u64::from(oer_cooperate::POLL_ADMISSION_MICROS)
                    {
                        continue;
                    }
                }
            }
            cx.waker().wake_by_ref();
            break Poll::Pending;
        };
        if let Some(timing) = this.timing.as_deref_mut() {
            let before = timing.total_micros;
            timing.poll_finished(clock, started, result.is_pending(), resumed, ready, None);
            if let Some(tag) = i2c_tag {
                let elapsed = timing.total_micros.checked_sub(before).unwrap_or_else(|| {
                    timing.invalid = true;
                    u32::MAX
                });
                let (maximum, count) = if tag & 0x80 == 0 {
                    (
                        &mut this.execution.i2c_poll_micros[1][1],
                        &mut this.execution.i2c_samples[1],
                    )
                } else {
                    (
                        &mut this.execution.i2c_poll_micros[1][0],
                        &mut this.execution.i2c_samples[0],
                    )
                };
                *maximum = (*maximum).max(elapsed);
                *count += 1;
            }
            this.execution.observe_engine(timing, tag);
        }
        if let Poll::Ready(Err(error)) = &result {
            this.program.fail_stop(*error);
        }
        *this.finished = result.is_ready();
        result
    }
}

pub(crate) fn drive<
    'a,
    D: PhyShortDelay + 'a,
    R: Hardware,
    T: Timer,
    O: FnMut(Operation, Event) + 'a,
>(
    program: &'a mut RxProgram,
    registers: &'a mut R,
    timer: &'a T,
    execution: &'a mut RxGainExecution,
    timing: Option<&'a mut EngineTiming>,
    observe: O,
) -> impl Future<Output = Result<(), RxError>> + 'a {
    Driver {
        program,
        registers,
        timer,
        execution,
        timing,
        observe,
        phase: None,
        seen: 0,
        wait: None,
        wait_at: Instant::EPOCH,
        wait_ready: false,
        make_wait: |timer: &'a T, at| timer.wait_until(at),
        entered: false,
        finished: false,
        delay: core::marker::PhantomData::<D>,
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn port_error(error: RxError) -> crate::target_executor::PhyTargetPortError {
    use crate::target_executor::PhyTargetPortError as E;
    match error {
        RxError::WrongCompletion => E::UnexpectedBinding,
        RxError::Hardware => E::HardwareInvariant,
        RxError::TimedOut => E::HardwareEdgeTimedOut,
        RxError::Capability => E::HardwareCapabilityUnavailable,
        RxError::OperationLimit => E::RfOperationLimit,
    }
}

#[cfg(test)]
mod tests;
