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
    fn advance_power<D: super::super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        _child: &mut crate::tracking::parameters::PhyParamTrackingTxPowerTransition<'_>,
        _gain: &mut super::power::Cursor,
        _action: crate::tracking::power::PhyTxPowerTrackingAction,
        _clock: &C,
        _measured: bool,
    ) -> Step {
        Step::Failure(Failure::Binding)
    }
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
    fn advance_power<D: super::super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        child: &mut crate::tracking::parameters::PhyParamTrackingTxPowerTransition<'_>,
        gain: &mut super::power::Cursor,
        action: crate::tracking::power::PhyTxPowerTrackingAction,
        clock: &C,
        measured: bool,
    ) -> Step {
        super::power::advance::<D, _>(&mut Io::Shared(self), child, gain, action, clock, measured)
            .unwrap_or_else(Step::Failure)
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
    fn advance_power<D: super::super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        child: &mut crate::tracking::parameters::PhyParamTrackingTxPowerTransition<'_>,
        gain: &mut super::power::Cursor,
        action: crate::tracking::power::PhyTxPowerTrackingAction,
        clock: &C,
        measured: bool,
    ) -> Step {
        super::power::advance::<D, _>(
            &mut Io::Validation(self),
            child,
            gain,
            action,
            clock,
            measured,
        )
        .unwrap_or_else(Step::Failure)
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
    /// Keep channel's large command/completion values out of the shared dispatcher.
    #[inline(never)]
    pub(super) fn channel<D: super::super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        clock: &C,
        binding: &mut Option<crate::channel::PhyChipChannelExternalBinding>,
        completion: &mut Option<crate::channel::PhyChipChannelCompletion>,
        gain: &mut super::channel_gain::Cursor,
    ) -> Result<bool, Failure> {
        if let Some(crate::channel::PhyChipChannelExternalBinding::Mmio(command)) = binding.as_ref()
            && let Some(image) = command.tx_gain_image()
        {
            if gain.advance(self, image)? {
                *binding = None;
                *completion = Some(crate::channel::PhyChipChannelCompletion::TxGainPublished);
            }
            return Ok(false);
        }
        let binding = binding.take().ok_or(Failure::Binding)?;
        #[cfg(target_arch = "riscv32")]
        fn apply<D: super::super::wait::PhyShortDelay, C: oer_time::Clock>(
            clock: &C,
            registers: &mut (impl oer_esp32s31_hal::owner::SharedPhyAccess + Registers),
            binding: crate::channel::PhyChipChannelExternalBinding,
        ) -> Result<crate::channel::PhyChipChannelCompletion, Failure> {
            crate::target_port::calibration::complete_channel_direct::<D, _, _, _>(
                clock,
                binding,
                &mut (),
                registers,
                &mut crate::target_port::NoopPhyTargetObserver,
            )
            .map_err(Failure::Target)
        }
        #[cfg(not(target_arch = "riscv32"))]
        let _ = (clock, binding);
        let result = match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => apply::<D, _>(clock, *registers, binding),
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => apply::<D, _>(clock, *registers, binding),
            #[cfg(test)]
            Self::Test(..) => Err(Failure::Binding),
        }?;
        let waiting = matches!(
            result,
            crate::channel::PhyChipChannelCompletion::FrequencyReadyObserved { ready: false }
        );
        *completion = Some(result);
        Ok(waiting)
    }
    #[inline(never)]
    pub(super) fn rx_gain<D: super::super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        clock: &C,
        binding: &mut Option<crate::rx::gain::PhyRxGainInitExternalBinding>,
        completion: &mut Option<crate::rx::gain::PhyRxGainInitCompletion>,
    ) -> Result<bool, Failure> {
        let binding = binding.take().ok_or(Failure::Binding)?;
        #[cfg(not(target_arch = "riscv32"))]
        let _ = clock;
        let result = match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => {
                super::rx_gain::execute::<D, _>(clock, *registers, binding).map_err(Failure::Target)
            }
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => {
                super::rx_gain::execute::<D, _>(clock, *registers, binding).map_err(Failure::Target)
            }
            #[cfg(test)]
            Self::Test(io, _) => io.rx_gain::<D>(binding),
        }?;
        let waiting = super::rx_gain::waiting(result);
        *completion = Some(result);
        Ok(waiting)
    }
    pub(super) fn frequency<D: super::super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        clock: &C,
        binding: crate::analog::rfpll::RfpllFrequencyExternalBinding,
    ) -> Result<crate::analog::rfpll::RfpllFrequencyCompletion, Failure> {
        #[cfg(not(any(target_arch = "riscv32", test)))]
        let _ = (clock, binding);
        #[cfg(test)]
        if let crate::analog::rfpll::RfpllFrequencyExternalBinding::I2c(mut binding) = binding {
            super::rfpll::direct_i2c(self, clock, binding.transaction_mut(), None, false)?;
            return binding.into_completion().map_err(|_| Failure::Binding);
        }
        match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(_) => self.rfpll_frequency::<D, _>(clock, binding),
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(_) => self.rfpll_frequency::<D, _>(clock, binding),
            #[cfg(test)]
            Self::Test(io, _) => io.frequency::<D>(binding),
        }
    }
    pub(super) fn gain_base(
        &mut self,
        action: crate::tracking::power::PhyTxPowerTrackingAction,
    ) -> Result<u8, Failure> {
        #[cfg(not(test))]
        let _ = action;
        match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => Ok(
                oer_esp32s31_hal::phy::memory::read_table_memory_base_index(*registers),
            ),
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => Ok(
                oer_esp32s31_hal::phy::memory::read_table_memory_base_index(*registers),
            ),
            #[cfg(test)]
            Self::Test(io, _) => io.power_gain_base(action),
        }
    }

    pub(super) fn channel_gain_base(&mut self) -> Result<u8, Failure> {
        match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => Ok(
                oer_esp32s31_hal::phy::memory::read_table_memory_base_index(*registers),
            ),
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => Ok(
                oer_esp32s31_hal::phy::memory::read_table_memory_base_index(*registers),
            ),
            #[cfg(test)]
            Self::Test(io, _) => io.channel_gain_base(),
        }
    }

    pub(super) fn gain_entry(
        &mut self,
        entry: oer_esp32s31_hal::types::PhyGainMemoryEntry,
    ) -> Result<(), Failure> {
        match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => {
                oer_esp32s31_hal::phy::memory::program_gain_memory_entry(*registers, entry);
                Ok(())
            }
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => {
                oer_esp32s31_hal::phy::memory::program_gain_memory_entry(*registers, entry);
                Ok(())
            }
            #[cfg(test)]
            Self::Test(io, _) => io.power_gain_entry(entry),
        }
    }

    pub(super) fn power(
        &mut self,
        binding: crate::tracking::power::ControlBinding,
    ) -> Result<crate::tracking::power::PhyTxPowerTrackingCompletion, Failure> {
        match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => Ok(binding.execute_target(*registers)),
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => Ok(binding.execute_target(*registers)),
            #[cfg(test)]
            Self::Test(io, _) => io.power(binding),
        }
    }

    #[cfg(target_arch = "riscv32")]
    #[inline(always)]
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
            Binding::I2c(mut binding) => {
                super::rfpll::direct_i2c(self, clock, binding.transaction_mut(), None, false)?;
                binding.into_completion().map_err(|_| Failure::Binding)
            }
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
    /// Execute at most one bus command, retaining partial helper/transaction
    /// state across the waker checkpoint. Memory entries remain whole.
    #[inline(never)]
    pub(super) fn advance_rfpll<D: super::super::wait::PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        clock: &C,
        run: &mut super::rfpll::Run<'_>,
        settle_at: &mut Instant,
        observed: bool,
    ) -> Result<Step, Failure> {
        use crate::{
            analog::{
                frequency::PhyFrequencyCapMemoryAction as Memory,
                i2c::{fields, registers},
                rfpll::RfpllCapacitorCode,
            },
            calibration::cold::{
                PhyColdI2cAction as I2c, PhyColdI2cOutcome as Done, PhyColdI2cRequest as Request,
            },
            tracking::{
                observation::{RfpllCommand as Kind, RfpllCommandSample},
                rfpll::{Action, Completion, search},
            },
        };
        let mut action = run.cursor.action();
        if let Action::Search(search::Action::DelayMicros(micros)) = action {
            let remaining = settle_at
                .saturating_duration_since(super::rfpll::clock_now(clock))
                .as_micros();
            if remaining > u64::from(micros) {
                return Err(Failure::Binding);
            }
            if remaining != 0 {
                if !D::settle_micros(remaining as u32) {
                    return Err(Failure::Binding);
                }
                #[cfg(test)]
                if let Self::Test(io, _) = self {
                    io.rfpll::<D>(Action::Search(search::Action::DelayMicros(
                        remaining as u32,
                    )))?;
                }
            }
            run.cursor
                .advance(Completion::Search(search::Completion::DelayElapsed(micros)))?;
            action = run.cursor.action();
        }
        let sample;
        let completion = match action {
            Action::Search(search_action) => {
                if matches!(search_action, search::Action::Complete(_)) {
                    return Ok(Step::Complete);
                }
                if run.transaction.is_none() {
                    let request = match search_action {
                        search::Action::ReadInitialCap if run.phase == 0 => {
                            Request::read_byte(registers::RFPLL_CALIBRATED_CAPACITOR_LOW)
                        }
                        search::Action::ReadInitialCap => {
                            Request::read_field(fields::RFPLL_CALIBRATED_CAPACITOR_HIGH)
                        }
                        search::Action::EnableSearch => {
                            Request::write_field(fields::RFPLL_CAPACITOR_SEARCH_ENABLE, 1)
                        }
                        search::Action::WriteCap(requested) => {
                            let code = RfpllCapacitorCode::from_request(requested)
                                .ok_or(Failure::Binding)?;
                            if run.phase == 0 {
                                Request::write_byte(registers::RFPLL_CAPACITOR_LOW, code.low_byte())
                            } else {
                                Request::write_field(fields::RFPLL_CAPACITOR_HIGH, code.high_bit())
                            }
                        }
                        search::Action::ReadStatus => {
                            Request::read_field(fields::RFPLL_CAPACITOR_SEARCH_STATUS)
                        }
                        _ => return Err(Failure::Binding),
                    };
                    run.transaction = Some(PhyColdI2cTransaction::new(request));
                }
                let transaction = run.transaction.as_mut().ok_or(Failure::Binding)?;
                let writing = matches!(transaction.action(), I2c::StartWrite { .. });
                let kind = match search_action {
                    search::Action::ReadInitialCap if run.phase == 0 => Kind::InitialLow,
                    search::Action::ReadInitialCap => Kind::InitialHigh,
                    search::Action::EnableSearch if writing => Kind::EnableWrite,
                    search::Action::EnableSearch => Kind::EnableRead,
                    search::Action::WriteCap(_) if run.phase == 0 => Kind::CapLow,
                    search::Action::WriteCap(_) if writing => Kind::CapHighWrite,
                    search::Action::WriteCap(_) => Kind::CapHighRead,
                    search::Action::ReadStatus => Kind::Status,
                    _ => return Err(Failure::Binding),
                };
                #[cfg(test)]
                if let Self::Test(io, _) = self {
                    io.prepare_rfpll_command(kind);
                }
                sample = super::rfpll::direct_i2c(self, clock, transaction, Some(kind), observed)?;
                let outcome = match transaction.action() {
                    I2c::Complete(outcome) => outcome,
                    // A masked read owns its captured byte until the next poll
                    // publishes the write through this same typed transaction.
                    I2c::StartWrite { .. } => return Ok(Step::Rfpll(sample)),
                    _ => return Err(Failure::Binding),
                };
                run.transaction = None;
                let done = match (search_action, outcome) {
                    (search::Action::ReadInitialCap, Done::Read { value, .. })
                        if run.phase == 0 =>
                    {
                        run.low = value;
                        run.phase = 1;
                        return Ok(Step::Rfpll(sample));
                    }
                    (search::Action::ReadInitialCap, Done::Read { value, .. }) => {
                        search::Completion::InitialCap(u16::from(run.low) | (u16::from(value) << 8))
                    }
                    (search::Action::EnableSearch, Done::Written { .. }) => {
                        search::Completion::SearchEnabled
                    }
                    (search::Action::WriteCap(_), Done::Written { .. }) if run.phase == 0 => {
                        run.phase = 1;
                        return Ok(Step::Rfpll(sample));
                    }
                    (search::Action::WriteCap(requested), Done::Written { .. }) => {
                        search::Completion::CapWritten(requested)
                    }
                    (search::Action::ReadStatus, Done::Read { value, .. }) => {
                        search::Completion::Status(match value {
                            0 => search::Status::Accepted,
                            1 => search::Status::Increase,
                            2 => search::Status::Decrease,
                            3 => search::Status::Other,
                            _ => return Err(Failure::Binding),
                        })
                    }
                    _ => return Err(Failure::Binding),
                };
                #[cfg(test)]
                if let Self::Test(io, _) = self {
                    io.rfpll::<D>(action)?;
                }
                Completion::Search(done)
            }
            Action::Memory(memory) => {
                let kind = if matches!(memory, Memory::RestoreChannelIndex { .. }) {
                    Kind::Restore
                } else {
                    Kind::Memory
                };
                let started = observed.then(|| super::rfpll::clock_now(clock));
                let mut done = self.rfpll_memory(memory)?;
                run.cursor.advance(Completion::Memory(done))?;
                if matches!(memory, Memory::ReadMemory { .. }) {
                    let Action::Memory(next @ Memory::WriteMemory { .. }) = run.cursor.action()
                    else {
                        return Err(Failure::Binding);
                    };
                    done = self.rfpll_memory(next)?;
                    run.cursor.advance(Completion::Memory(done))?;
                }
                return Ok(Step::Rfpll(RfpllCommandSample::elapsed(
                    kind,
                    started,
                    &super::rfpll::OutlinedClock(clock),
                )));
            }
            Action::Complete(_) => return Ok(Step::Complete),
        };
        // The minimum starts only after all writes and their completion edges.
        let written_at = matches!(action, Action::Search(search::Action::WriteCap(_)))
            .then(|| super::rfpll::clock_now(clock));
        run.cursor.advance(completion)?;
        run.phase = 0;
        if let (Some(at), Action::Search(search::Action::DelayMicros(micros))) =
            (written_at, run.cursor.action())
        {
            *settle_at = at
                .checked_add(oer_time::Duration::from_micros(u64::from(micros)))
                .ok_or(Failure::DeadlineOverflow)?;
        }
        Ok(Step::Rfpll(sample))
    }

    fn rfpll_memory(
        &mut self,
        action: crate::analog::frequency::PhyFrequencyCapMemoryAction,
    ) -> Result<crate::analog::frequency::PhyFrequencyCapMemoryCompletion, Failure> {
        match self {
            #[cfg(target_arch = "riscv32")]
            Self::Shared(registers) => {
                crate::target_port::rfpll::complete_memory(*registers, action)
                    .map_err(Failure::Target)
            }
            #[cfg(all(target_arch = "riscv32", oer_probe))]
            Self::Validation(registers) => {
                crate::target_port::rfpll::complete_memory(*registers, action)
                    .map_err(Failure::Target)
            }
            #[cfg(test)]
            Self::Test(io, _) => {
                let crate::tracking::rfpll::Completion::Memory(done) = io
                    .rfpll::<super::tests::TestDelay>(crate::tracking::rfpll::Action::Memory(
                        action,
                    ))?
                else {
                    return Err(Failure::Binding);
                };
                Ok(done)
            }
        }
    }

    fn i2c_status(&mut self) -> u8 {
        #[cfg(target_arch = "riscv32")]
        fn sample(registers: &impl oer_esp32s31_hal::owner::SharedPhyAccess) -> u8 {
            use oer_esp32s31_hal::phy::i2c::{Host, sample_master_reset_busy};
            u8::from(sample_master_reset_busy(registers, Host::Host0))
                | (u8::from(sample_master_reset_busy(registers, Host::Host1)) << 1)
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
