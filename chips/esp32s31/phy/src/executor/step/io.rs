//! Closed hardware routes; all dispatch is static or an enum match.
use super::{Failure, Job, Step};
use crate::{
    analog::pbus::{PhyPbusHardwareBinding, PhyPbusHardwareObservation},
    calibration::cold::{PhyColdI2cError, PhyColdI2cObservation, PhyColdI2cTransaction},
};
use oer_time::Instant;

pub(crate) trait Registers {
    fn advance(&mut self, job: &mut Job<'_>, now: Instant) -> Step;
}
pub(super) enum Io<'a, 'owner> {
    #[cfg(target_arch = "riscv32")]
    Shared(
        &'a mut oer_esp32s31_hal::owner::SharedPhyHal<
            'owner,
            oer_esp32s31_hal::owner::route::Shared,
        >,
    ),
    #[cfg(all(target_arch = "riscv32", oer_probe))]
    Validation(&'a mut oer_esp32s31_hal::owner::ValidationSharedPhy<'owner>),
    #[cfg(test)]
    Test(
        &'a mut super::tests::Io,
        core::marker::PhantomData<&'owner ()>,
    ),
}
#[cfg(target_arch = "riscv32")]
impl Registers
    for oer_esp32s31_hal::owner::SharedPhyHal<'_, oer_esp32s31_hal::owner::route::Shared>
{
    fn advance(&mut self, job: &mut Job<'_>, now: Instant) -> Step {
        job.advance_one(Io::Shared(self), now)
    }
}
#[cfg(all(target_arch = "riscv32", oer_probe))]
impl Registers for oer_esp32s31_hal::owner::ValidationSharedPhy<'_> {
    fn advance(&mut self, job: &mut Job<'_>, now: Instant) -> Step {
        job.advance_one(Io::Validation(self), now)
    }
}

impl Io<'_, '_> {
    #[inline(never)]
    pub(super) fn i2c(
        &mut self,
        transaction: &mut PhyColdI2cTransaction,
        start: bool,
    ) -> Result<PhyColdI2cObservation, Failure> {
        #[cfg(target_arch = "riscv32")]
        fn apply(
            registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
            transaction: &mut PhyColdI2cTransaction,
            start: bool,
        ) -> Result<PhyColdI2cObservation, Failure> {
            if start {
                match transaction.start_target(registers) {
                    Ok(()) => Ok(PhyColdI2cObservation::EdgeConsumed),
                    Err(PhyColdI2cError::BusyAtStart) => Ok(PhyColdI2cObservation::StillPending),
                    Err(_) => Err(Failure::Binding),
                }
            } else {
                transaction
                    .observe_target_edge(registers)
                    .map_err(|_| Failure::Binding)
            }
        }
        match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => apply(*registers, transaction, start),
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => apply(*registers, transaction, start),
            #[cfg(test)]
            Self::Test(io, _) => {
                if start {
                    io.start_i2c(transaction).map(|started| {
                        if started {
                            PhyColdI2cObservation::EdgeConsumed
                        } else {
                            PhyColdI2cObservation::StillPending
                        }
                    })
                } else {
                    io.sample_i2c(transaction)
                }
            }
        }
    }
    pub(super) fn pbus(
        &mut self,
        binding: &mut PhyPbusHardwareBinding,
        start: bool,
    ) -> Result<PhyPbusHardwareObservation, Failure> {
        #[cfg(target_arch = "riscv32")]
        fn apply(
            registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
            binding: &mut PhyPbusHardwareBinding,
            start: bool,
        ) -> Result<PhyPbusHardwareObservation, Failure> {
            if start {
                binding
                    .start_target(registers)
                    .map_err(|_| Failure::Binding)?;
                Ok(PhyPbusHardwareObservation::EdgeConsumed)
            } else {
                binding
                    .observe_target_edge(registers)
                    .map_err(|_| Failure::Binding)
            }
        }
        match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => apply(*registers, binding, start),
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => apply(*registers, binding, start),
            #[cfg(test)]
            Self::Test(io, _) => {
                if start {
                    io.start_pbus(binding)
                        .map(|()| PhyPbusHardwareObservation::EdgeConsumed)
                } else {
                    io.sample_pbus(binding)
                }
            }
        }
    }
    pub(super) fn temperature_code(&mut self) -> u8 {
        match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => oer_esp32s31_hal::phy::temperature::read_code(*registers),
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => {
                oer_esp32s31_hal::phy::temperature::read_code(*registers)
            }
            #[cfg(test)]
            Self::Test(io, _) => io.temperature_code(),
        }
    }
    pub(super) fn pbus_control(&mut self, work_mode: bool) -> Result<bool, Failure> {
        #[cfg(target_arch = "riscv32")]
        fn apply(
            registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
            work_mode: bool,
        ) -> Result<bool, Failure> {
            use oer_esp32s31_hal::phy::{agc, delay::RomShortDelay, pbus};
            if !work_mode {
                pbus::configure_debug_mode(registers);
                return Ok(false);
            }
            let settled = pbus::configure_work_mode(registers);
            if settled {
                if !RomShortDelay::settle_micros(1) {
                    return Err(Failure::Binding);
                }
                agc::configure_pbus_work_mode_pulse(registers);
                if !RomShortDelay::settle_micros(2) {
                    return Err(Failure::Binding);
                }
                agc::clear_pbus_work_mode_pulse(registers);
            }
            Ok(settled)
        }
        match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => apply(*registers, work_mode),
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => apply(*registers, work_mode),
            #[cfg(test)]
            Self::Test(io, _) => io.clear_pbus_control(work_mode),
        }
    }
}
