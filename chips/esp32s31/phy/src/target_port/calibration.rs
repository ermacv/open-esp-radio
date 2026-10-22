//! Complete calibration children on the admitted physical PHY partition.
//!
//! These executors borrow the existing maintenance capability. They neither
//! acquire radio access nor publish parent calibration state. A returned child
//! completion must still be accepted by its parent transition.

use super::{
    CHANNEL_READY_SAMPLE_LIMIT, PhyTargetObserver, PhyTargetPortError, RF_OPERATION_LIMIT,
};
use crate::{
    executor::wait::PhyShortDelay,
    tracking::calibration::{
        PhyCalibrationChannelTransition, PhyCalibrationDcodeTransition,
        PhyCalibrationPbusClearTransition, PhyCalibrationRxGainTransition,
        PhyCalibrationTrackingCompletion,
    },
};
use oer_esp32s31_hal::owner::{SharedPhyAccess, SharedPhyContext};

/// Run the bounded PBus-clear child, including readiness and work-mode settling.
/// Once polled, keep the physical maintenance borrow until terminal completion;
/// an error does not establish a safe state for resuming radio traffic.
pub fn clear_pbus<D: PhyShortDelay, O: PhyTargetObserver, C: oer_time::Clock + ?Sized>(
    clock: &C,
    mut child: PhyCalibrationPbusClearTransition,
    registers: &mut impl SharedPhyContext,
    _observer: &mut O,
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    use crate::analog::pbus::{PhyPbusClearAction as Action, PhyPbusClearCompletion as Completion};
    use oer_esp32s31_hal::phy::{agc, pbus};

    // Keep the admitted child and its exact command/settle order. This small
    // runtime child does not need the cold-init action/completion envelopes.
    for _ in 0..RF_OPERATION_LIMIT {
        child = match child.commit() {
            Ok(completion) => return Ok(completion),
            Err(child) => child,
        };
        let completion = match child.action() {
            Action::ConfigureDebugMode => {
                pbus::configure_debug_mode(registers);
                Completion::DebugModeConfigured
            }
            Action::ForceTest(transaction) => {
                if crate::target_executor::force_pbus_direct(clock, registers, transaction) {
                    Completion::ForceTestCompleted(transaction)
                } else {
                    Completion::ForceTestTimedOut(transaction)
                }
            }
            Action::ConfigureWorkMode => Completion::WorkModeConfigured {
                settle_required: pbus::configure_work_mode(registers),
            },
            Action::DelayMicros(micros) => {
                short_settle::<D>(micros)?;
                Completion::DelayElapsed
            }
            Action::ConfigureWorkModePulse => {
                agc::configure_pbus_work_mode_pulse(registers);
                Completion::WorkModePulseConfigured
            }
            Action::ClearWorkModePulse => {
                agc::clear_pbus_work_mode_pulse(registers);
                Completion::WorkModePulseCleared
            }
            Action::Complete(_) => return Err(PhyTargetPortError::UnexpectedBinding),
        };
        child
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

pin_project_lite::pin_project! {
    struct Dcode<F> { #[pin] inner: F }
}
impl<F: core::future::Future<Output = Result<Option<bool>, PhyTargetPortError>>>
    core::future::Future for Dcode<F>
{
    type Output = Result<Option<bool>, PhyTargetPortError>;
    #[inline(never)]
    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        self.project().inner.poll(cx)
    }
}

/// Run the borrowed D-code cursor through its terminal edge. The caller keeps
/// the affine child and commits it only after this future has completed.
/// Hardware ownership and cancellation obligations remain with that caller.
pub fn dcode<'a, D: PhyShortDelay + 'a, P, T: oer_time::Timer>(
    clock: &'a T,
    child: &'a mut PhyCalibrationDcodeTransition,
    _platform: &mut P,
    registers: &'a mut (impl SharedPhyAccess + crate::executor::step::Registers),
    mut observe: impl FnMut(bool) + 'a,
    mut timing: Option<&'a mut crate::tracking::observation::EngineTiming>,
) -> impl core::future::Future<Output = Result<(), PhyTargetPortError>> + 'a {
    async move {
        for _ in 0..RF_OPERATION_LIMIT {
            if let Some(locked) =
                dcode_poll::<D, _, _>(clock, child, registers, timing.as_deref_mut()).await?
            {
                observe(locked);
            } else {
                return Ok(());
            }
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }
}

// The poll boundary has no observer or platform type parameter. Diagnostic
// callbacks stay in the thin outer adapter; the retained job is shared.
fn dcode_poll<
    'a,
    D: PhyShortDelay + 'a,
    R: crate::executor::step::Registers,
    T: oer_time::Timer,
>(
    clock: &'a T,
    child: &'a mut PhyCalibrationDcodeTransition,
    registers: &'a mut R,
    timing: Option<&'a mut crate::tracking::observation::EngineTiming>,
) -> impl core::future::Future<Output = Result<Option<bool>, PhyTargetPortError>> + 'a {
    Dcode {
        inner: async move {
            use crate::executor::step::{Job, drive};
            if matches!(
                child.action(),
                crate::analog::dcode::PhyDcodeAction::Complete(_)
                    | crate::analog::dcode::PhyDcodeAction::Failed(_)
            ) {
                return Ok(None);
            }
            let mut lock = None;
            let mut job = Job::dcode(child.transition_mut(), &mut lock, clock.now())
                .map_err(super::step::port_error)?;
            drive::<D, _, _>(&mut job, registers, clock, timing, None, None)
                .await
                .map_err(super::step::port_error)?;
            Ok(lock)
        },
    }
}

#[inline(never)]
pub(super) fn observe_dcode_result<O: PhyTargetObserver>(
    observer: &mut O,
    child: &PhyCalibrationDcodeTransition,
    result: &Result<(), PhyTargetPortError>,
) {
    observer.dcode_result(format_args!(
        "action={:?} result={:?}",
        child.action(),
        result
    ));
}

/// Run RX-DC calibration and publish both gain banks using the admitted owner.
pub async fn rx_gain<D: PhyShortDelay, P, T: oer_time::Timer>(
    clock: &T,
    child: &mut PhyCalibrationRxGainTransition,
    platform: &mut P,
    registers: &mut (impl SharedPhyContext + crate::executor::step::Registers),
    observe: impl FnMut(crate::tracking::observation::Operation, crate::tracking::observation::Event),
    summarize: impl FnMut(crate::tracking::observation::RxGainExecution),
    timing: Option<&mut crate::tracking::observation::EngineTiming>,
) -> Result<(), PhyTargetPortError> {
    rx_gain_init::<D, _, _>(
        clock,
        child.transition_mut(),
        platform,
        registers,
        observe,
        summarize,
        timing,
    )
    .await
}

/// Advance the retained coefficient owner; only its caller publishes the result.
pub async fn rx_gain_init<D: PhyShortDelay, P, T: oer_time::Timer>(
    clock: &T,
    child: &mut crate::rx::gain::PhyRxGainInitTransition,
    _platform: &mut P,
    registers: &mut (impl SharedPhyContext + crate::executor::step::Registers),
    mut observe: impl FnMut(
        crate::tracking::observation::Operation,
        crate::tracking::observation::Event,
    ),
    mut summarize: impl FnMut(crate::tracking::observation::RxGainExecution),
    mut timing: Option<&mut crate::tracking::observation::EngineTiming>,
) -> Result<(), PhyTargetPortError> {
    use crate::{
        rx::gain::Phase,
        tracking::observation::{Event, Operation},
    };
    let mut execution = crate::tracking::observation::RxGainExecution::default();
    for _ in 0..RF_OPERATION_LIMIT {
        let Some(phase) = child.phase() else {
            execution.quality = child
                .terminal()
                .and_then(Result::ok)
                .and_then(|outcome| outcome.dc)
                .map(|outcome| outcome.quality);
            summarize(execution);
            return Ok(());
        };
        let operation = match phase {
            Phase::Control => Operation::RxGainControlPhase,
            Phase::Dc => Operation::RxGainDcPhase,
            Phase::Publish => Operation::RxGainPublishPhase,
        };
        observe(operation, Event::Started);
        let result = rx_gain_phase::<D, _, _>(
            clock,
            child,
            registers,
            &mut execution,
            timing.as_deref_mut(),
        )
        .await;
        observe(operation, terminal_event(&result));
        result?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

pin_project_lite::pin_project! {
    struct RxGain<F> { #[pin] inner: F }
}
impl<F: core::future::Future<Output = Result<(), PhyTargetPortError>>> core::future::Future
    for RxGain<F>
{
    type Output = Result<(), PhyTargetPortError>;
    #[inline(never)]
    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        self.project().inner.poll(cx)
    }
}
fn rx_gain_phase<
    'a,
    D: PhyShortDelay + 'a,
    R: crate::executor::step::Registers,
    T: oer_time::Timer,
>(
    clock: &'a T,
    child: &'a mut crate::rx::gain::PhyRxGainInitTransition,
    registers: &'a mut R,
    execution: &'a mut crate::tracking::observation::RxGainExecution,
    timing: Option<&'a mut crate::tracking::observation::EngineTiming>,
) -> impl core::future::Future<Output = Result<(), PhyTargetPortError>> + 'a {
    RxGain {
        inner: async move {
            use crate::executor::step::{Job, drive_calibration};
            let mut scratch = crate::executor::step::rx_gain::Scratch::default();
            let mut job = Job::rx_gain(
                child,
                &mut scratch,
                execution,
                clock.now(),
                timing.is_some(),
            )
            .map_err(super::step::port_error)?;
            drive_calibration::<D, _, _>(&mut job, registers, clock, timing)
                .await
                .map_err(super::step::port_error)
        },
    }
}

fn short_settle<D: PhyShortDelay>(micros: u32) -> Result<(), PhyTargetPortError> {
    if micros != 0 && micros <= D::MAX_MICROS && D::settle_micros(micros) {
        Ok(())
    } else {
        Err(PhyTargetPortError::HardwareCapabilityUnavailable)
    }
}

fn terminal_event<T, E>(result: &Result<T, E>) -> crate::tracking::observation::Event {
    use crate::tracking::observation::Event;
    if result.is_ok() {
        Event::Completed
    } else {
        Event::Failed
    }
}

/// Execute the complete TX-DC/PWDET root as one admitted transaction,
/// including measurement and restoration. The caller retains physical access
/// across SAR timer waits until terminal completion. Returning a target error or a
/// typed failed action does not establish a safe RF state.
#[inline(never)]
pub async fn tx_dc_pwdet_init<D: PhyShortDelay, O: PhyTargetObserver>(
    child: &mut crate::tx::dc_power_detector::PhyTxDcPwdetTransition,
    registers: &mut impl SharedPhyContext,
    observer: &core::cell::RefCell<&mut O>,
    clock: &impl oer_time::Timer,
) -> Result<(), PhyTargetPortError> {
    child
        .execute_target_direct(
            clock,
            registers,
            |scope, micros| tx_settle::<D, O>(observer, clock, scope, micros),
            |ready| observer.borrow_mut().tx_sar_ready(ready),
            |elapsed, observations, ready| {
                observer
                    .borrow_mut()
                    .tx_sar_wait(elapsed, observations, ready)
            },
        )
        .await
}

fn tx_settle<D: PhyShortDelay, O: PhyTargetObserver>(
    observer: &core::cell::RefCell<&mut O>,
    clock: &impl oer_time::Clock,
    scope: crate::executor::wait::tx::Scope,
    micros: u32,
) -> Result<(), PhyTargetPortError> {
    use crate::executor::wait::{Event, Kind};
    let started = O::OBSERVE_DELAYS.then(|| clock.now());
    if O::OBSERVE_DELAYS {
        observer.borrow_mut().tx_wait(
            scope,
            Kind::Settle,
            Event::Started {
                requested_micros: u64::from(micros),
            },
        );
    }
    let result = short_settle::<D>(micros);
    // A failed settle reports no completion, which leaves the evidence
    // incomplete.
    if let (Ok(()), Some(started)) = (&result, started) {
        let elapsed_micros = clock.now().saturating_duration_since(started).as_micros();
        observer.borrow_mut().tx_wait(
            scope,
            Kind::Settle,
            Event::Completed {
                elapsed_micros,
                lateness_micros: elapsed_micros.saturating_sub(u64::from(micros)),
            },
        );
    }
    result
}

/// Restore the operating channel without acquiring or releasing physical access.
pin_project_lite::pin_project! { struct Channel<F> { #[pin] inner: F } }
impl<F: core::future::Future<Output = Result<(), PhyTargetPortError>>> core::future::Future
    for Channel<F>
{
    type Output = Result<(), PhyTargetPortError>;
    #[inline(never)]
    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        self.project().inner.poll(cx)
    }
}

pub fn channel<'a, D: PhyShortDelay + 'a, P: 'a, O: PhyTargetObserver + 'a, T: oer_time::Timer>(
    clock: &'a T,
    child: &'a mut PhyCalibrationChannelTransition,
    _platform: &'a mut P,
    registers: &'a mut (impl SharedPhyAccess + crate::executor::step::Registers),
    observer: &'a mut O,
) -> impl core::future::Future<Output = Result<(), PhyTargetPortError>> + 'a {
    Channel {
        inner: async move {
            use crate::{
                channel::PhyChipChannelCompletion as Done,
                executor::step::{Job, drive},
                tracking::observation::{EngineTiming, Operation},
            };
            let mut timing = EngineTiming::default();
            for _ in 0..RF_OPERATION_LIMIT {
                if child.is_terminal() {
                    if O::OBSERVE_ENGINE_POLLS {
                        observer.engine_completed(Operation::ChannelRestore, timing);
                    }
                    return Ok(());
                }
                let action = if O::OBSERVE_ENGINE_POLLS {
                    channel_action_tag(child)
                } else {
                    0
                };
                let mut atom_timing = EngineTiming::default();
                let mut binding = Some(
                    child
                        .lower_external()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                );
                let mut completion = None;
                let mut job = Job::channel(&mut binding, &mut completion, clock.now())
                    .map_err(super::step::port_error)?;
                let result = drive::<D, _, _>(
                    &mut job,
                    registers,
                    clock,
                    O::OBSERVE_ENGINE_POLLS.then_some(&mut atom_timing),
                    None,
                    None,
                )
                .await;
                if O::OBSERVE_ENGINE_POLLS {
                    timing.merge(&atom_timing);
                    observer.channel_action_completed(action, &atom_timing);
                }
                result.map_err(super::step::port_error)?;
                let completion = completion.ok_or(PhyTargetPortError::UnexpectedBinding)?;
                match completion {
                    Done::FrequencyReadyTimedOut => {
                        let crate::channel::PhyChipChannelAction::AwaitFrequencyReadyEdge {
                            samples,
                            ..
                        } = child.action()
                        else {
                            return Err(PhyTargetPortError::UnexpectedBinding);
                        };
                        observer.channel_frequency_ready_timed_out(samples);
                    }
                    Done::TxGainCalculated { request, image } => {
                        observer.channel_tx_gain(request, image)
                    }
                    _ => {}
                }
                child
                    .advance(completion)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            }
            Err(PhyTargetPortError::RfOperationLimit)
        },
    }
}

#[inline(never)]
fn channel_action_tag(child: &PhyCalibrationChannelTransition) -> u8 {
    crate::tracking::observation::calibration::channel(&child.action())
}

pub(crate) fn complete_channel_direct<
    D: PhyShortDelay,
    P,
    O: PhyTargetObserver,
    C: oer_time::Clock + ?Sized,
>(
    clock: &C,
    binding: crate::channel::PhyChipChannelExternalBinding,
    platform: &mut P,
    registers: &mut (impl SharedPhyAccess + crate::executor::step::Registers),
    observer: &mut O,
) -> Result<crate::channel::PhyChipChannelCompletion, PhyTargetPortError> {
    use crate::channel::{
        PhyChipChannelAction, PhyChipChannelCompletion, PhyChipChannelExternalBinding,
    };
    match binding {
        PhyChipChannelExternalBinding::Mmio(binding) => match binding.action() {
            PhyChipChannelAction::AwaitFrequencyReadyEdge { samples, .. }
                if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
            {
                observer.channel_frequency_ready_timed_out(samples);
                Ok(PhyChipChannelCompletion::FrequencyReadyTimedOut)
            }
            _ => Ok(binding.execute_target(platform, registers)),
        },
        PhyChipChannelExternalBinding::Temperature(binding) => {
            Ok(PhyChipChannelCompletion::Temperature(
                complete_temperature_direct(clock, binding, registers)?,
            ))
        }
        PhyChipChannelExternalBinding::Timer(binding) => {
            short_settle::<D>(binding.micros())?;
            Ok(binding.into_completion())
        }
        PhyChipChannelExternalBinding::I2c(binding) => {
            complete_channel_i2c_direct(clock, binding, registers)
        }
        PhyChipChannelExternalBinding::TxGain(binding) => {
            let request = binding.request();
            let completion = binding.execute();
            if let PhyChipChannelCompletion::TxGainCalculated { image, .. } = completion {
                observer.channel_tx_gain(request, image);
            }
            Ok(completion)
        }
    }
}

fn complete_temperature_direct<C: oer_time::Clock + ?Sized>(
    clock: &C,
    binding: crate::analog::temperature::PhyTemperatureExternalBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::analog::temperature::PhyTemperatureCompletion, PhyTargetPortError> {
    use crate::analog::temperature::PhyTemperatureExternalBinding;
    match binding {
        PhyTemperatureExternalBinding::I2c(mut binding) => {
            use crate::calibration::cold::{PhyColdI2cAction, PhyColdI2cError};
            let completed = crate::executor::wait::i2c::complete(
                clock,
                &mut binding,
                crate::analog::temperature::PhyTemperatureI2cBinding::action,
                |binding, action| {
                    match action {
                        PhyColdI2cAction::StartRead { .. }
                        | PhyColdI2cAction::StartWrite { .. } => {
                            match binding.start_target(registers) {
                                Ok(()) | Err(PhyColdI2cError::BusyAtStart) => {}
                                Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                            }
                        }
                        PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                        | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                            binding
                                .observe_target_edge(registers)
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                        }
                        PhyColdI2cAction::Complete(_) => {}
                    }
                    Ok(())
                },
            )?;
            if !completed {
                return Err(PhyTargetPortError::HardwareEdgeTimedOut);
            }
            binding
                .into_completion()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)
        }
        PhyTemperatureExternalBinding::Sample(binding) => Ok(binding.execute_target(registers)),
    }
}

fn complete_channel_i2c_direct<C: oer_time::Clock + ?Sized>(
    clock: &C,
    mut binding: crate::channel::PhyChipChannelI2cBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::channel::PhyChipChannelCompletion, PhyTargetPortError> {
    use crate::calibration::cold::{PhyColdI2cAction, PhyColdI2cError};
    let completed = crate::executor::wait::i2c::complete(
        clock,
        &mut binding,
        crate::channel::PhyChipChannelI2cBinding::action,
        |binding, action| {
            match action {
                PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                    match binding.start_target(registers) {
                        Ok(()) | Err(PhyColdI2cError::BusyAtStart) => {}
                        Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                    }
                }
                PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                    binding
                        .observe_target_edge(registers)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                }
                PhyColdI2cAction::Complete(_) => {}
            }
            Ok(())
        },
    )?;
    if !completed {
        return Ok(binding.into_timeout_completion());
    }
    binding
        .into_completion()
        .map_err(|_| PhyTargetPortError::UnexpectedBinding)
}
