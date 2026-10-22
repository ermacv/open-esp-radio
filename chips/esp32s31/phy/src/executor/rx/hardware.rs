//! Typed physical leaves, shared by production and validation owners.
#[cfg(target_arch = "riscv32")]
use super::*;

#[cfg(target_arch = "riscv32")]
impl<R: oer_esp32s31_hal::owner::SharedPhyContext> Hardware for R {
    #[inline]
    fn control<D: PhyShortDelay>(&mut self, command: ControlCommand) -> Result<(), RxError> {
        use oer_esp32s31_hal::phy::{agc, baseband, pbus, rx_dco};
        match command {
            ControlCommand::PrepareRoot | ControlCommand::PrepareCalibration => {
                rx_dco::prepare_control_restore(self).map_err(|_| RxError::Hardware)?
            }
            ControlCommand::RestoreRoot | ControlCommand::RestoreCalibration => {
                rx_dco::restore_control(self).map_err(|_| RxError::Hardware)?
            }
            ControlCommand::Registers(enabled) => {
                crate::hardware::configure_phy_rx_gain_dc_registers(self, enabled)
            }
            ControlCommand::Debug => pbus::configure_debug_mode(self),
            ControlCommand::RxClock(enabled) => pbus::configure_rx_clock(self, enabled),
            ControlCommand::TxClock(enabled) => pbus::configure_tx_clock(self, enabled),
            ControlCommand::WorkMode => {
                if pbus::configure_work_mode(self) {
                    settle::<D>(1)?;
                    agc::configure_pbus_work_mode_pulse(self);
                    settle::<D>(2)?;
                    agc::clear_pbus_work_mode_pulse(self);
                }
            }
            ControlCommand::Limits(last) => agc::configure_rx_gain_limits(self, last),
            ControlCommand::IqCorrection => baseband::enable_iq_correction(self),
        }
        Ok(())
    }
    #[inline]
    fn pbus<C: Clock>(&mut self, clock: &C, command: PhyPbusForceTest) -> Result<bool, RxError> {
        Ok(crate::target_executor::force_pbus_direct(
            clock, self, command,
        ))
    }
    #[inline]
    fn read_pbus(&mut self, selector: u8, path: u8) -> Result<u32, RxError> {
        oer_esp32s31_hal::phy::pbus::read_result(self, selector, path)
            .map(u32::from)
            .ok_or(RxError::Hardware)
    }
    #[inline]
    fn entry(&mut self, command: PhyGainMemoryEntry) -> Result<(), RxError> {
        oer_esp32s31_hal::phy::memory::program_gain_memory_entry(self, command);
        Ok(())
    }
    #[inline]
    fn estimator<D: PhyShortDelay>(
        &mut self,
        command: PhyDcIqAction,
    ) -> Result<PhyDcIqCompletion, RxError> {
        use crate::calibration::estimator::PhyDcIqExternalBinding as B;
        match B::lower(command).map_err(|_| RxError::WrongCompletion)? {
            B::Mmio(binding) => binding.capture_target::<D>(self).map_err(port_error),
            B::Readiness(binding) => binding.capture_target::<D>(self).map_err(port_error),
            B::Timer(binding) => {
                settle::<D>(binding.micros())?;
                Ok(binding.into_completion())
            }
        }
    }
    #[inline]
    fn frequency<D: PhyShortDelay, C: Clock>(
        &mut self,
        _clock: &C,
        command: RfpllFrequencyAction,
    ) -> Result<RfpllFrequencyCompletion, RxError> {
        use crate::analog::rfpll::RfpllFrequencyExternalBinding as B;
        match B::lower(command).map_err(|_| RxError::WrongCompletion)? {
            B::Mmio(binding) => Ok(binding.execute_target(self)),
            B::Timer(binding) => {
                settle::<D>(binding.micros())?;
                Ok(binding.into_completion())
            }
            B::I2c(_) => Err(RxError::WrongCompletion),
        }
    }
    #[inline]
    fn direct_i2c<C: Clock>(
        &mut self,
        clock: &C,
        transaction: &mut PhyColdI2cTransaction,
    ) -> Result<(), RxError> {
        let completed = super::super::wait::i2c::complete(
            clock,
            transaction,
            PhyColdI2cTransaction::action,
            |transaction, action| {
                let start = matches!(
                    action,
                    PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. }
                );
                if !matches!(action, PhyColdI2cAction::Complete(_)) {
                    self.i2c(transaction, start)?;
                }
                Ok(())
            },
        )?;
        if completed {
            Ok(())
        } else {
            Err(RxError::TimedOut)
        }
    }
    #[inline]
    fn i2c(
        &mut self,
        transaction: &mut PhyColdI2cTransaction,
        start: bool,
    ) -> Result<PhyColdI2cObservation, RxError> {
        if start {
            match transaction.start_target(self) {
                Ok(()) => Ok(PhyColdI2cObservation::EdgeConsumed),
                Err(crate::calibration::cold::PhyColdI2cError::BusyAtStart) => {
                    Ok(PhyColdI2cObservation::StillPending)
                }
                Err(_) => Err(RxError::WrongCompletion),
            }
        } else {
            transaction
                .observe_target_edge(self)
                .map_err(|_| RxError::WrongCompletion)
        }
    }
}

#[cfg(target_arch = "riscv32")]
fn settle<D: PhyShortDelay>(micros: u32) -> Result<(), RxError> {
    if micros != 0 && micros <= D::MAX_MICROS && D::settle_micros(micros) {
        Ok(())
    } else {
        Err(RxError::Capability)
    }
}

#[cfg(target_arch = "riscv32")]
fn port_error(error: crate::target_executor::PhyTargetPortError) -> RxError {
    use crate::target_executor::PhyTargetPortError as E;
    match error {
        E::HardwareEdgeTimedOut => RxError::TimedOut,
        E::HardwareCapabilityUnavailable => RxError::Capability,
        E::RfOperationLimit => RxError::OperationLimit,
        E::UnexpectedBinding => RxError::WrongCompletion,
        _ => RxError::Hardware,
    }
}
