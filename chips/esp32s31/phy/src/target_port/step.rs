//! Thin typed I/O and observer adapters for the shared step dispatcher.
use super::PhyTargetObserver;
use crate::{
    analog::pbus::{PhyPbusHardwareBinding, PhyPbusHardwareObservation},
    calibration::cold::{PhyColdI2cError, PhyColdI2cObservation, PhyColdI2cTransaction},
    executor::{
        step::{Failure, Sink, StepIo},
        wait::PhyShortDelay,
    },
    target_executor::PhyTargetPortError,
    tracking::observation::EngineEvent,
};
use core::marker::PhantomData;
use oer_esp32s31_hal::owner::SharedPhyContext;

pub(super) struct TargetIo<'a, R, D> {
    registers: &'a mut R,
    short: PhantomData<D>,
}
impl<'a, R, D> TargetIo<'a, R, D> {
    pub(super) fn new(registers: &'a mut R) -> Self {
        Self {
            registers,
            short: PhantomData,
        }
    }
}
impl<R: SharedPhyContext, D: PhyShortDelay> StepIo for TargetIo<'_, R, D> {
    fn start_i2c(&mut self, transaction: &mut PhyColdI2cTransaction) -> Result<bool, Failure> {
        match transaction.start_target(self.registers) {
            Ok(()) => Ok(true),
            Err(PhyColdI2cError::BusyAtStart) => Ok(false),
            Err(_) => Err(Failure::Binding),
        }
    }
    fn sample_i2c(
        &mut self,
        transaction: &mut PhyColdI2cTransaction,
    ) -> Result<PhyColdI2cObservation, Failure> {
        transaction
            .observe_target_edge(self.registers)
            .map_err(|_| Failure::Binding)
    }
    fn start_pbus(&mut self, binding: &mut PhyPbusHardwareBinding) -> Result<(), Failure> {
        binding
            .start_target(self.registers)
            .map_err(|_| Failure::Binding)
    }
    fn sample_pbus(
        &mut self,
        binding: &mut PhyPbusHardwareBinding,
    ) -> Result<PhyPbusHardwareObservation, Failure> {
        binding
            .observe_target_edge(self.registers)
            .map_err(|_| Failure::Binding)
    }
    fn temperature_code(&mut self) -> u8 {
        oer_esp32s31_hal::phy::temperature::read_code(self.registers)
    }
    fn clear_pbus_control(&mut self, work_mode: bool) -> Result<bool, Failure> {
        if work_mode {
            use oer_esp32s31_hal::phy::{agc, pbus};
            let settled = pbus::configure_work_mode(self.registers);
            if settled {
                if !D::settle_micros(1) {
                    return Err(Failure::Binding);
                }
                agc::configure_pbus_work_mode_pulse(self.registers);
                if !D::settle_micros(2) {
                    return Err(Failure::Binding);
                }
                agc::clear_pbus_work_mode_pulse(self.registers);
            }
            Ok(settled)
        } else {
            oer_esp32s31_hal::phy::pbus::configure_debug_mode(self.registers);
            Ok(false)
        }
    }
}

pub(super) struct ObserverSink<'a, O>(pub(super) &'a mut O);
impl<O: PhyTargetObserver> Sink for ObserverSink<'_, O> {
    fn enabled(&self) -> bool {
        O::OBSERVE_ENGINE_POLLS
    }
    fn observe(&mut self, event: EngineEvent) {
        self.0.engine_event(event);
    }
}
pub(super) fn port_error(error: Failure) -> PhyTargetPortError {
    match error {
        Failure::DeadlineOverflow => PhyTargetPortError::DeadlineOverflow,
        Failure::TimedOut => PhyTargetPortError::HardwareEdgeTimedOut,
        Failure::Binding => PhyTargetPortError::UnexpectedBinding,
    }
}
