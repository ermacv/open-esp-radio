//! Closed hardware routes; all dispatch is static or an enum match.
use super::{Failure, Job, Step};
use crate::{
    analog::pbus::{PhyPbusHardwareBinding, PhyPbusHardwareObservation},
    calibration::cold::{PhyColdI2cError, PhyColdI2cObservation, PhyColdI2cTransaction},
};
use oer_time::Instant;

pub(crate) trait Registers {
    fn advance<D: super::super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        job: &mut Job<'_>,
        clock: &C,
        now: Instant,
    ) -> Step;
    #[cfg(target_arch = "riscv32")]
    fn rfpll_atom<D: super::super::wait::PhyShortDelay, C: oer_time::Clock + ?Sized>(
        &mut self,
        clock: &C,
        binding: crate::analog::rfpll::RfpllFrequencyExternalBinding,
    ) -> Result<crate::analog::rfpll::RfpllFrequencyCompletion, Failure>;
    fn i2c_status(&mut self) -> u8;
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
    fn advance<D: super::super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        job: &mut Job<'_>,
        clock: &C,
        now: Instant,
    ) -> Step {
        job.advance_one::<D, _>(Io::Shared(self), clock, now)
    }
    fn rfpll_atom<D: super::super::wait::PhyShortDelay, C: oer_time::Clock + ?Sized>(
        &mut self,
        clock: &C,
        binding: crate::analog::rfpll::RfpllFrequencyExternalBinding,
    ) -> Result<crate::analog::rfpll::RfpllFrequencyCompletion, Failure> {
        Io::Shared(self).rfpll_frequency::<D, _>(clock, binding)
    }
    fn i2c_status(&mut self) -> u8 {
        Io::Shared(self).i2c_status()
    }
}
#[cfg(all(target_arch = "riscv32", oer_probe))]
impl Registers for oer_esp32s31_hal::owner::ValidationSharedPhy<'_> {
    fn advance<D: super::super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        job: &mut Job<'_>,
        clock: &C,
        now: Instant,
    ) -> Step {
        job.advance_one::<D, _>(Io::Validation(self), clock, now)
    }
    fn rfpll_atom<D: super::super::wait::PhyShortDelay, C: oer_time::Clock + ?Sized>(
        &mut self,
        clock: &C,
        binding: crate::analog::rfpll::RfpllFrequencyExternalBinding,
    ) -> Result<crate::analog::rfpll::RfpllFrequencyCompletion, Failure> {
        Io::Validation(self).rfpll_frequency::<D, _>(clock, binding)
    }
    fn i2c_status(&mut self) -> u8 {
        Io::Validation(self).i2c_status()
    }
}

impl Io<'_, '_> {
    #[cfg(target_arch = "riscv32")]
    #[inline(never)]
    fn rfpll_frequency<D: super::super::wait::PhyShortDelay, C: oer_time::Clock + ?Sized>(
        &mut self,
        clock: &C,
        binding: crate::analog::rfpll::RfpllFrequencyExternalBinding,
    ) -> Result<crate::analog::rfpll::RfpllFrequencyCompletion, Failure> {
        use crate::analog::rfpll::{
            RfpllFrequencyAction as Action, RfpllFrequencyCompletion as Completion,
            RfpllFrequencyExternalBinding as Binding,
        };
        match binding {
            Binding::I2c(binding) => super::rfpll::direct_i2c(self, clock, binding),
            Binding::Timer(binding) => {
                if !D::settle_micros(binding.micros()) {
                    return Err(Failure::Target(
                        crate::target_executor::PhyTargetPortError::HardwareCapabilityUnavailable,
                    ));
                }
                Ok(binding.into_completion())
            }
            Binding::Mmio(binding) => {
                if matches!(binding.action(), Action::ReadChannelReady { samples } if samples >= crate::target_port::CHANNEL_READY_SAMPLE_LIMIT)
                {
                    return Ok(Completion::ChannelReadyTimedOut);
                }
                Ok(match self {
                    Self::Shared(registers) => binding.execute_target(*registers),
                    #[cfg(oer_probe)]
                    Self::Validation(registers) => binding.execute_target(*registers),
                    #[cfg(test)]
                    Self::Test(..) => return Err(Failure::Binding),
                })
            }
        }
    }
    /// Keep aggregate model actions/results in the synchronous atom adapter,
    /// rather than retaining their stack slots throughout a checkpoint loop.
    #[inline(never)]
    pub(super) fn advance_rfpll<D: super::super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        _clock: &C,
        cursor: &mut super::rfpll::Cursor<'_>,
    ) -> Result<(), Failure> {
        let action = cursor.action();
        let completion = match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => {
                crate::target_port::rfpll::complete_correction::<D, _>(_clock, *registers, action)
                    .map_err(Failure::Target)
            }
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => {
                crate::target_port::rfpll::complete_correction::<D, _>(_clock, *registers, action)
                    .map_err(Failure::Target)
            }
            #[cfg(test)]
            Self::Test(io, _) => io.rfpll::<D>(action),
        }?;
        cursor.advance(completion)
    }

    fn i2c_status(&mut self) -> u8 {
        #[cfg(target_arch = "riscv32")]
        fn sample(registers: &impl oer_esp32s31_hal::owner::SharedPhyAccess) -> u8 {
            use oer_esp32s31_hal::phy::i2c::{PhyI2cHost, sample_master_reset_busy};
            u8::from(sample_master_reset_busy(registers, PhyI2cHost::Host0))
                | (u8::from(sample_master_reset_busy(registers, PhyI2cHost::Host1)) << 1)
        }
        match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => sample(*registers),
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => sample(*registers),
            #[cfg(test)]
            Self::Test(io, _) => Registers::i2c_status(*io),
        }
    }
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
