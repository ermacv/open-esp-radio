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
    matches!(
        completion,
        PhyRxGainInitCompletion::Dc(PhyRxGainDcCompletion::Calibration(
            PhyRxDcCalibrationCompletion::Minimum(PhyRxDcMinimumCompletion::DcIq(
                PhyDcIqCompletion::ReadinessObserved {
                    snapshot: PhyDcIqReadinessSnapshot { ready: false, .. },
                    ..
                } | PhyDcIqCompletion::CapturePending { .. }
            ))
        ))
    )
}

pub(crate) async fn complete<
    D: crate::executor::wait::PhyShortDelay,
    R: super::Registers,
    T: oer_time::Timer,
>(
    binding: crate::rx::gain::PhyRxGainInitExternalBinding,
    registers: &mut R,
    timer: &T,
) -> Result<PhyRxGainInitCompletion, Failure> {
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
}

pub(super) struct Run<'a> {
    pub(super) transition: &'a mut PhyRxGainInitTransition,
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
        if self.transition.phase() != Some(self.phase) {
            return Ok(Step::Complete);
        }
        let started = self.measured.then(|| clock.now());
        if matches!(
            self.transition.action(),
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
        self.observe(self.transition.action());
        let binding =
            crate::rx::gain::PhyRxGainInitExternalBinding::lower(self.transition.action())
                .map_err(|_| Failure::Binding)?;
        let lowered = self.measured.then(|| clock.now());
        let completion = io.rx_gain::<D, _>(clock, binding)?;
        let completed = self.measured.then(|| clock.now());
        if let PhyRxGainInitCompletion::Dc(PhyRxGainDcCompletion::Calibration(
            PhyRxDcCalibrationCompletion::Minimum(PhyRxDcMinimumCompletion::DcIq(completion)),
        )) = completion
        {
            match completion {
                PhyDcIqCompletion::CaptureFinished { accumulators, .. } => {
                    self.execution.minimum_operations += 3 + u32::from(accumulators.is_some()) * 2;
                    self.execution.settle_1us += 1;
                }
                PhyDcIqCompletion::ReadinessObserved { .. }
                | PhyDcIqCompletion::CapturePending { .. } => {
                    self.execution.minimum_operations += 1
                }
                _ => {}
            }
        }
        let waiting = waiting(completion);
        self.transition
            .advance(completion)
            .map_err(|_| Failure::Binding)?;
        if let (Some(started), Some(lowered), Some(completed)) = (started, lowered, completed) {
            for (index, (from, to)) in [
                (started, lowered),
                (lowered, completed),
                (completed, clock.now()),
            ]
            .into_iter()
            .enumerate()
            {
                let elapsed = to
                    .checked_duration_since(from)
                    .and_then(|d| u32::try_from(d.as_micros()).ok())
                    .unwrap_or(u32::MAX);
                self.execution.maximum_parts_micros[index] =
                    self.execution.maximum_parts_micros[index].max(elapsed);
            }
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

    fn observe(&mut self, action: PhyRxGainInitAction) {
        if let PhyRxGainInitAction::Dc(PhyRxGainDcAction::Calibration(action)) = action {
            match action {
                PhyRxDcCalibrationAction::PrepareControlRestore => {
                    self.execution.outer_operations += 1
                }
                PhyRxDcCalibrationAction::DelayMicros { micros: 10, .. } => {
                    self.execution.settle_10us += 1
                }
                PhyRxDcCalibrationAction::Minimum(PhyRxDcMinimumAction::DcIq(action)) => {
                    if let PhyDcIqAction::Configure(request) = action {
                        self.execution.minimum_operations += 4;
                        self.execution.settle_1us += 1;
                        if request.iteration == 0 {
                            self.execution.minimum_searches += 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
#[inline(never)]
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
fn calibration<D: crate::executor::wait::PhyShortDelay, C: Clock>(
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
