//! RX calibration retains its existing arithmetic and coefficient owner.
use super::{Failure, Step, io::Io};
use crate::{
    calibration::estimator::{PhyDcIqAction, PhyDcIqCompletion, PhyDcIqReadinessSnapshot},
    rx::{
        dc_offset::{PhyRxDcMinimumAction, PhyRxDcMinimumCompletion},
        gain::{Phase, PhyRxGainInitAction, PhyRxGainInitCompletion, PhyRxGainInitTransition},
        gain_calibration::{
            PhyRxDcCalibrationAction, PhyRxDcCalibrationCompletion, PhyRxGainDcAction,
            PhyRxGainDcCompletion,
        },
    },
    tracking::observation::RxGainExecution,
};
use oer_time::{Clock, Instant};

pub(super) fn waiting(completion: PhyRxGainInitCompletion) -> bool {
    match completion {
        PhyRxGainInitCompletion::Dc(PhyRxGainDcCompletion::Calibration(completion)) => {
            calibration_waiting(completion)
        }
        _ => false,
    }
}

fn calibration_waiting(completion: PhyRxDcCalibrationCompletion) -> bool {
    matches!(
        completion,
        PhyRxDcCalibrationCompletion::Minimum(PhyRxDcMinimumCompletion::DcIq(
            PhyDcIqCompletion::ReadinessObserved {
                snapshot: PhyDcIqReadinessSnapshot { ready: false, .. },
                ..
            } | PhyDcIqCompletion::CapturePending { .. }
        ))
    )
}

pin_project_lite::pin_project! {
    struct Complete<F> { #[pin] inner: F }
}
impl<F: core::future::Future<Output = Result<PhyRxGainInitCompletion, Failure>>>
    core::future::Future for Complete<F>
{
    type Output = Result<PhyRxGainInitCompletion, Failure>;
    #[inline(never)]
    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        self.project().inner.poll(cx)
    }
}

pub(crate) fn complete<
    'a,
    D: crate::executor::wait::PhyShortDelay + 'a,
    R: super::Registers,
    T: oer_time::Timer,
>(
    binding: crate::rx::gain::PhyRxGainInitExternalBinding,
    registers: &'a mut R,
    timer: &'a T,
) -> impl core::future::Future<Output = Result<PhyRxGainInitCompletion, Failure>> + 'a {
    Complete {
        inner: async move {
            use crate::{
                analog::rfpll::RfpllFrequencyExternalBinding as Frequency,
                rx::gain::PhyRxGainInitExternalBinding as Binding,
                rx::gain_calibration::PhyRxGainDcExternalBinding as Dc,
            };
            // Cold RFPLL retains its counted, timer-delivered I2C policy.
            if let Binding::Dc(Dc::Rfpll(Frequency::I2c(mut binding))) = binding {
                let mut job = super::Job::counted_i2c(binding.transaction_mut(), timer.now())?;
                super::drive::<D, _, _>(&mut job, registers, timer, None, None, None).await?;
                return binding
                    .into_completion()
                    .map(|done| PhyRxGainInitCompletion::Dc(PhyRxGainDcCompletion::Rfpll(done)))
                    .map_err(|_| Failure::Binding);
            }
            let mut binding = Some(binding);
            let mut completion = None;
            let mut job = super::Job::rx_gain_atom(&mut binding, &mut completion, timer.now())?;
            super::drive::<D, _, _>(&mut job, registers, timer, None, None, None).await?;
            completion.ok_or(Failure::Binding)
        },
    }
}

/// Command and result storage belongs to the retained RX owner, not a poll frame.
#[derive(Default)]
pub(crate) struct Scratch {
    pub(super) binding: Option<crate::rx::gain::PhyRxGainInitExternalBinding>,
    pub(super) completion: Option<PhyRxGainInitCompletion>,
    seen_actions: u64,
    lowering: Option<PhyRxGainInitAction>,
    poll_total_micros: u32,
    poll_sample: u8,
    part_times: [u64; 3],
}

impl Scratch {
    // Actions occupy bits 0..33; the high byte retains the current identity.
    // This diagnostic packing leaves the shared Run/Work transport unchanged.
    pub(super) fn observe_action(&mut self, action: u8) -> u8 {
        let bit = 1_u64 << action;
        let tag = action
            | if self.seen_actions & bit == 0 {
                0x80
            } else {
                0
            };
        let next = ((self.seen_actions | bit) & 0x00ff_ffff_ffff_ffff) | (u64::from(tag) << 56);
        if next != self.seen_actions {
            self.seen_actions = next;
        }
        tag
    }

    pub(super) fn action(&self) -> u8 {
        (self.seen_actions >> 56) as u8
    }

    fn set_action(&mut self, tag: u8) {
        self.seen_actions = (self.seen_actions & 0x00ff_ffff_ffff_ffff) | (u64::from(tag) << 56);
    }

    pub(super) fn start_poll_timing(&mut self, total: u32) {
        self.poll_total_micros = total;
        self.poll_sample = 0;
    }

    /// Called after Driver's final timestamp; no diagnostic work is charged
    /// to the I2C transaction or excluded from the engine's poll duration.
    pub(super) fn observe_poll(
        &mut self,
        execution: &mut RxGainExecution,
        timing: &mut crate::tracking::observation::EngineTiming,
    ) {
        let elapsed = timing.total_micros.checked_sub(self.poll_total_micros);
        self.poll_total_micros = timing.total_micros;
        let tag = core::mem::take(&mut self.poll_sample);
        let stage = match tag & 0x7f {
            34 => 0,
            11 => 1,
            _ => return,
        };
        let elapsed = elapsed.unwrap_or_else(|| {
            timing.invalid = true;
            u32::MAX
        });
        let repeat = usize::from(tag & 0x80 == 0);
        execution.i2c_poll_micros[stage][repeat] =
            execution.i2c_poll_micros[stage][repeat].max(elapsed);
        if stage == 1 {
            execution.i2c_samples[repeat] += 1;
        }
    }

    #[inline(never)]
    fn record_parts(&self, execution: &mut RxGainExecution, tag: u8, finished: Instant) {
        Run::record_part(
            execution,
            0,
            tag,
            Instant::from_micros(self.part_times[0]),
            Instant::from_micros(self.part_times[1]),
        );
        Run::record_part(
            execution,
            1,
            tag,
            Instant::from_micros(self.part_times[1]),
            Instant::from_micros(self.part_times[2]),
        );
        Run::record_part(
            execution,
            2,
            tag,
            Instant::from_micros(self.part_times[2]),
            finished,
        );
    }
}

pub(super) struct Run<'a> {
    pub(super) transition: &'a mut PhyRxGainInitTransition,
    pub(super) scratch: &'a mut Scratch,
    pub(super) execution: &'a mut RxGainExecution,
    pub(super) phase: Phase,
    pub(super) measured: bool,
}

impl Run<'_> {
    #[inline(never)]
    pub(super) fn advance<D: crate::executor::wait::PhyShortDelay, C: Clock>(
        &mut self,
        io: &mut Io<'_, '_>,
        clock: &C,
        now: Instant,
    ) -> Result<Step, Failure> {
        self.scratch.poll_sample = 0;
        if self.transition.phase() != Some(self.phase) {
            return Ok(Step::Complete);
        }
        let stamp = super::rfpll::OutlinedClock(clock);
        if self.measured {
            self.scratch.part_times[0] = stamp.now().as_micros();
        }
        let prepared = self.scratch.binding.is_some();
        if !prepared {
            if let Some(action) = self.transition.calibration_action() {
                return self.advance_calibration::<D, _>(io, clock, now, action);
            }
        }
        let mut tag = 0;
        if !prepared {
            self.scratch.lowering = Some(self.transition.action());
            if self.measured {
                let action = crate::tracking::observation::calibration::rx(
                    self.scratch.lowering.as_ref().ok_or(Failure::Binding)?,
                );
                tag = self.scratch.observe_action(action);
            }
            let action = self.scratch.lowering.as_ref().ok_or(Failure::Binding)?;
            if matches!(
                action,
                PhyRxGainInitAction::Dc(PhyRxGainDcAction::Calibration(
                    PhyRxDcCalibrationAction::Minimum(PhyRxDcMinimumAction::DcIq(
                        PhyDcIqAction::Configure(_)
                    ))
                ))
            ) && self.execution.minimum_operations
                > 100_000 - u32::from(crate::HARDWARE_EDGE_LIMIT) - 8
            {
                #[cfg(target_arch = "riscv32")]
                return Err(Failure::Target(
                    crate::target_executor::PhyTargetPortError::RfOperationLimit,
                ));
                #[cfg(not(target_arch = "riscv32"))]
                return Err(Failure::Binding);
            }
            let split_i2c = matches!(action, PhyRxGainInitAction::Dc(PhyRxGainDcAction::I2c(_)));
            Self::observe(self.execution, action);
            self.scratch.binding = Some(
                crate::rx::gain::PhyRxGainInitExternalBinding::lower(*action)
                    .map_err(|_| Failure::Binding)?,
            );
            // lower() only constructs a binding (gain::PhyRxGainInitExternalBinding
            // and gain_calibration::PhyRxGainDcExternalBinding). No I2C command has
            // started: the borrowed RF/bus owner can yield before the whole helper.
            if split_i2c {
                if self.measured {
                    tag = 34 | (tag & 0x80);
                    self.scratch.set_action(tag);
                    self.scratch.poll_sample = tag;
                    Self::record_part(
                        self.execution,
                        0,
                        tag,
                        Instant::from_micros(self.scratch.part_times[0]),
                        stamp.now(),
                    );
                }
                return Ok(Step::Runnable);
            }
        } else if self.measured {
            tag = 11 | (self.scratch.action() & 0x80);
            self.scratch.set_action(tag);
            self.scratch.poll_sample = tag;
        }
        if self.measured {
            self.scratch.part_times[1] = stamp.now().as_micros();
        }
        let waiting = io.rx_gain::<D, _>(
            clock,
            &mut self.scratch.binding,
            &mut self.scratch.completion,
        )?;
        let completion = self.scratch.completion.take().ok_or(Failure::Binding)?;
        if self.measured {
            self.scratch.part_times[2] = stamp.now().as_micros();
        }
        if let PhyRxGainInitCompletion::Dc(PhyRxGainDcCompletion::Calibration(completion)) =
            completion
        {
            Self::observe_completion(self.execution, completion);
        }
        self.transition
            .advance(completion)
            .map_err(|_| Failure::Binding)?;
        if self.measured {
            let finished = stamp.now();
            self.scratch.record_parts(self.execution, tag, finished);
        }
        if waiting {
            Ok(Step::WaitAt(
                now.checked_add(super::RETRY)
                    .ok_or(Failure::DeadlineOverflow)?,
            ))
        } else {
            Ok(Step::Runnable)
        }
    }

    /// The root keeps its child and coefficient bank in place. Only the live
    /// calibration command crosses this typed adapter; validation and terminal
    /// handoff still use the same child transition and DC publication routine.
    #[inline(always)]
    fn advance_calibration<D: crate::executor::wait::PhyShortDelay, C: Clock>(
        &mut self,
        io: &mut Io<'_, '_>,
        clock: &C,
        now: Instant,
        action: PhyRxDcCalibrationAction,
    ) -> Result<Step, Failure> {
        if matches!(
            action,
            PhyRxDcCalibrationAction::Minimum(PhyRxDcMinimumAction::DcIq(
                PhyDcIqAction::Configure(_)
            ))
        ) && self.execution.minimum_operations
            > 100_000 - u32::from(crate::HARDWARE_EDGE_LIMIT) - 8
        {
            #[cfg(target_arch = "riscv32")]
            return Err(Failure::Target(
                crate::target_executor::PhyTargetPortError::RfOperationLimit,
            ));
            #[cfg(not(target_arch = "riscv32"))]
            return Err(Failure::Binding);
        }
        let tag = if self.measured {
            self.scratch
                .observe_action(crate::tracking::observation::calibration::rx_calibration(
                    &action,
                ))
        } else {
            0
        };
        Self::observe_calibration(self.execution, &action);
        let binding = crate::rx::gain_calibration::PhyRxDcCalibrationExternalBinding::lower(action)
            .map_err(|_| Failure::Binding)?;
        let stamp = super::rfpll::OutlinedClock(clock);
        if self.measured {
            self.scratch.part_times[1] = stamp.now().as_micros();
        }
        let completion = io.rx_calibration::<D, _>(clock, binding)?;
        if self.measured {
            self.scratch.part_times[2] = stamp.now().as_micros();
        }
        Self::observe_completion(self.execution, completion);
        self.transition
            .advance_calibration(completion)
            .map_err(|_| Failure::Binding)?;
        if self.measured {
            self.scratch.record_parts(self.execution, tag, stamp.now());
        }
        if calibration_waiting(completion) {
            Ok(Step::WaitAt(
                now.checked_add(super::RETRY)
                    .ok_or(Failure::DeadlineOverflow)?,
            ))
        } else {
            Ok(Step::Runnable)
        }
    }

    fn observe_completion(
        execution: &mut RxGainExecution,
        completion: PhyRxDcCalibrationCompletion,
    ) {
        if let PhyRxDcCalibrationCompletion::Minimum(PhyRxDcMinimumCompletion::DcIq(completion)) =
            completion
        {
            match completion {
                PhyDcIqCompletion::CaptureFinished { accumulators, .. } => {
                    execution.minimum_operations += 3 + u32::from(accumulators.is_some()) * 2;
                    execution.settle_1us += 1;
                }
                PhyDcIqCompletion::ReadinessObserved { .. }
                | PhyDcIqCompletion::CapturePending { .. } => execution.minimum_operations += 1,
                _ => {}
            }
        }
    }

    #[inline(never)]
    fn record_part(
        execution: &mut RxGainExecution,
        index: usize,
        tag: u8,
        from: Instant,
        to: Instant,
    ) {
        let elapsed = to
            .checked_duration_since(from)
            .and_then(|d| u32::try_from(d.as_micros()).ok())
            .unwrap_or(u32::MAX);
        if elapsed > execution.maximum_parts_micros[index] {
            execution.maximum_parts_micros[index] = elapsed;
            execution.maximum_parts_actions[index] = tag;
        }
    }

    fn observe(execution: &mut RxGainExecution, action: &PhyRxGainInitAction) {
        if let PhyRxGainInitAction::Dc(PhyRxGainDcAction::Calibration(action)) = action {
            Self::observe_calibration(execution, action);
        }
    }

    fn observe_calibration(execution: &mut RxGainExecution, action: &PhyRxDcCalibrationAction) {
        match action {
            PhyRxDcCalibrationAction::PrepareControlRestore => execution.outer_operations += 1,
            PhyRxDcCalibrationAction::DelayMicros { micros: 10, .. } => execution.settle_10us += 1,
            PhyRxDcCalibrationAction::Minimum(PhyRxDcMinimumAction::DcIq(action)) => {
                if let PhyDcIqAction::Configure(request) = action {
                    execution.minimum_operations += 4;
                    execution.settle_1us += 1;
                    if request.iteration == 0 {
                        execution.minimum_searches += 1;
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
pub(super) fn execute<D: crate::executor::wait::PhyShortDelay, C: Clock>(
    clock: &C,
    registers: &mut (impl oer_esp32s31_hal::owner::SharedPhyContext + super::Registers),
    binding: crate::rx::gain::PhyRxGainInitExternalBinding,
) -> Result<PhyRxGainInitCompletion, crate::target_executor::PhyTargetPortError> {
    use crate::rx::gain::PhyRxGainInitExternalBinding as Binding;
    match binding {
        Binding::Mmio(binding) => binding
            .execute_target(registers)
            .map_err(|_| crate::target_executor::PhyTargetPortError::HardwareInvariant),
        Binding::Dc(binding) => {
            dc::<D, _>(clock, registers, binding).map(PhyRxGainInitCompletion::Dc)
        }
        Binding::Publish(binding) => {
            use crate::rx::gain::PhyRxGainPublishExternalBinding as Publish;
            Ok(PhyRxGainInitCompletion::Publish(match binding {
                Publish::Mmio(binding) => match binding.execute_target(registers) {
                    crate::rx::gain::PhyRxGainPublishCompletion::PbusWorkModeConfigured {
                        bank,
                        settle_required,
                    } => {
                        work_mode_tail::<D>(registers, settle_required)?;
                        crate::rx::gain::PhyRxGainPublishCompletion::PbusWorkModeRestored { bank }
                    }
                    completion => completion,
                },
                Publish::Pbus(binding) => crate::target_executor::complete_rx_gain_publish_pbus(
                    binding, registers, clock,
                )?,
                Publish::Timer(binding) => {
                    settle::<D>(binding.micros())?;
                    binding.into_completion()
                }
            }))
        }
    }
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
fn dc<D: crate::executor::wait::PhyShortDelay, C: Clock>(
    clock: &C,
    registers: &mut (impl oer_esp32s31_hal::owner::SharedPhyContext + super::Registers),
    binding: crate::rx::gain_calibration::PhyRxGainDcExternalBinding,
) -> Result<PhyRxGainDcCompletion, crate::target_executor::PhyTargetPortError> {
    use crate::rx::gain_calibration::PhyRxGainDcExternalBinding as Binding;
    Ok(match binding {
        Binding::Mmio(binding) => match binding.execute_target(registers) {
            PhyRxGainDcCompletion::PbusWorkModeConfigured { settle_required } => {
                work_mode_tail::<D>(registers, settle_required)?;
                PhyRxGainDcCompletion::PbusWorkModeRestored
            }
            completion => completion,
        },
        Binding::Rfpll(binding) => PhyRxGainDcCompletion::Rfpll(
            registers
                .rfpll_atom::<D, _>(clock, binding)
                .map_err(crate::target_port::engine_error)?,
        ),
        Binding::Pbus(binding) => {
            crate::target_executor::complete_rx_gain_dc_pbus(binding, registers, clock)?
        }
        Binding::I2c(binding) => PhyRxGainDcCompletion::I2c(
            crate::target_executor::complete_rx_gain_i2c(binding, registers, clock)?,
        ),
        Binding::Timer(binding) => {
            settle::<D>(binding.micros())?;
            binding.into_completion()
        }
        Binding::Calibration(binding) => {
            PhyRxGainDcCompletion::Calibration(calibration::<D, _>(clock, registers, binding)?)
        }
    })
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
pub(super) fn calibration<D: crate::executor::wait::PhyShortDelay, C: Clock>(
    clock: &C,
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    binding: crate::rx::gain_calibration::PhyRxDcCalibrationExternalBinding,
) -> Result<PhyRxDcCalibrationCompletion, crate::target_executor::PhyTargetPortError> {
    use crate::{
        calibration::estimator::PhyDcIqExternalBinding as Estimate,
        rx::{
            dc_offset::PhyRxDcMinimumExternalBinding as Minimum,
            gain_calibration::PhyRxDcCalibrationExternalBinding as Binding,
        },
    };
    Ok(match binding {
        Binding::Mmio(binding) => binding
            .execute_target(registers)
            .map_err(|_| crate::target_executor::PhyTargetPortError::HardwareInvariant)?,
        Binding::Pbus(binding) => {
            crate::target_executor::complete_rx_dc_calibration_pbus(binding, registers, clock)?
        }
        Binding::Timer(binding) => {
            settle::<D>(binding.micros())?;
            binding.into_completion()
        }
        Binding::Minimum(Minimum::DcIq(binding)) => {
            PhyRxDcCalibrationCompletion::Minimum(PhyRxDcMinimumCompletion::DcIq(match binding {
                Estimate::Mmio(binding) => binding.capture_target::<D>(registers)?,
                Estimate::Timer(binding) => {
                    settle::<D>(binding.micros())?;
                    binding.into_completion()
                }
                Estimate::Readiness(binding) => binding.capture_target::<D>(registers)?,
            }))
        }
    })
}

#[cfg(target_arch = "riscv32")]
fn settle<D: crate::executor::wait::PhyShortDelay>(
    micros: u32,
) -> Result<(), crate::target_executor::PhyTargetPortError> {
    D::settle_micros(micros)
        .then_some(())
        .ok_or(crate::target_executor::PhyTargetPortError::HardwareCapabilityUnavailable)
}

#[cfg(target_arch = "riscv32")]
fn work_mode_tail<D: crate::executor::wait::PhyShortDelay>(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    settle_required: bool,
) -> Result<(), crate::target_executor::PhyTargetPortError> {
    if settle_required {
        settle::<D>(1)?;
        oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
        settle::<D>(2)?;
        oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
    }
    Ok(())
}
