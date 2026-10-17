//! Non-blocking ESP32-S31 PHY-I2C command encoding and RC-calibration plan.
//!
//! The rev0 ROM PHY-I2C leaves busy-wait on bit 25 of the host command
//! register. This module deliberately does not reproduce those loops. It
//! separates command publication from completion observation so an outer
//! Rust async owner can arrange a wakeup and inspect the register once.
//!
//! Reference: qualified ESP32-S31 rev0 ROM image.
//! The relevant complete ROM bodies are `phy_chip_i2c_readReg_org` at
//! `0x2f82_9ffa`, `phy_chip_i2c_writeReg` at `0x2f82_a30e`, and
//! `phy_get_rc_dout` at `0x2f82_61ac`. ESP32-S31 `libphy.a[phy_i2c.o]`
//! supplies the target-specific host configuration and read-mask callbacks
//! installed around those ROM leaves. Neither oracle is linked into the
//! firmware.

/// Required pinned `libphy.a` vendor-ABI no-op leaf; the body is one `ret`.
#[inline]
#[cfg(feature = "validation-probes")]
pub const fn phy_get_i2c_data() {}

/// Complete pinned target hook; the ESP32-S31 archive body is one `ret`.
#[inline]
#[cfg(feature = "validation-probes")]
pub const fn phy_i2c_enter_critical() {}

/// Complete pinned target hook; the ESP32-S31 archive body is one `ret`.
#[inline]
#[cfg(feature = "validation-probes")]
pub const fn phy_i2c_exit_critical() {}

/// Initialize the six-byte master-memory descriptor exactly as the pinned
/// archive leaf.
#[inline]
#[cfg(feature = "validation-probes")]
pub fn phy_i2c_master_mem_cfg(configuration: &mut [u8; 6]) {
    configuration[0] = 0;
    configuration[1] = 0;
    configuration[3] = 1;
    configuration[4] = 0x2c;
    configuration[2] = 1;
    configuration[5] = 1;
}

/// Initialize the command-memory descriptor and its two-word mode value.
#[inline]
#[cfg(feature = "validation-probes")]
pub fn phy_i2c_master_command_mem_cfg(configuration: &mut [u8; 8], mode: &mut u32) {
    configuration[3] = 1;
    configuration[4] = 1;
    configuration[5] = 1;
    configuration[7] = 1;
    configuration[0] = 0;
    configuration[1] = 0;
    configuration[2] = 0;
    configuration[6] = 0x2c;
    *mode = 2;
}

use crate::analog::{
    crystal_duty::{
        XtalDutyCalibrationAction, XtalDutyCalibrationCompletion, XtalDutyCalibrationOutcome,
        XtalDutyCalibrationParameters, XtalDutyCalibrationTransition,
    },
    frequency::{
        PhyChannelFrequencyInitAction, PhyChannelFrequencyInitCompletion,
        PhyChannelFrequencyInitControl, PhyChannelFrequencyInitFailure,
        PhyChannelFrequencyInitOutcome, PhyChannelFrequencyInitRequest,
        PhyChannelFrequencyInitTransition,
    },
    pbus::{
        PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusClearOutcome, PhyPbusClearTransition,
        PhyPbusForceTest,
    },
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::owner::SharedPhyAccess;

use oer_esp32s31_hal::phy::i2c::PhyAdcRate;
#[cfg(all(feature = "validation-probes", target_arch = "riscv32"))]
pub(crate) use oer_esp32s31_hal::phy::i2c::PhyI2cBlock;

pub(crate) use oer_esp32s31_hal::phy::i2c::{PhyI2cAddress, PhyI2cField, analog_registers};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::phy::{analog_i2c, i2c as hal_phy_i2c};

const PHY_I2C_SDM_STABLE_VALUE: u8 = 0x5b;
const PHY_I2C_SDM_DEADLINE_CYCLES: u32 = 9_999;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cError {
    Busy,
}

/// Program the complete PHY-I2C command RAM from Rust-owned cold state.
///
/// The shared-PHY capability is borrowed from the active protocol lifecycle,
/// making exclusive ownership explicit for the complete finite 45-store
/// transaction without depending on a Wi-Fi radio owner.
///
/// Basis: complete
/// `libphy.a[phy_i2c.o]::phy_i2c_master_cmd_mem_init`; destinations come from
/// the SVD-generated 45-element command-RAM array.
#[cfg(target_arch = "riscv32")]
pub fn configure_i2c_master_command_memory(
    registers: &mut impl SharedPhyAccess,
    parameter: PhyRfInitParameterSnapshot,
) {
    hal_phy_i2c::configure_command_memory(registers, parameter.pac_command_memory_inputs());
}

/// Execute the current-vendor 22-pair retained-wake analog initialization.
#[cfg(target_arch = "riscv32")]
pub fn configure_i2c_initialization_stage_two(
    registers: &mut impl SharedPhyAccess,
    parameter: PhyRfInitParameterSnapshot,
    maximum_observations: u32,
) -> Result<(), hal_phy_i2c::PhyI2cInitializationStageTwoError> {
    hal_phy_i2c::configure_initialization_stage_two(
        registers,
        parameter.pac_command_memory_inputs(),
        maximum_observations,
    )
}

/// Publish one complete-register PHY-I2C read without waiting for completion.
///
/// Unlike the ROM read leaf, this function also rejects an already-busy host
/// before publishing the command. This is a deliberate fail-fast ownership
/// check, not a claim that the ROM performed the same pre-command check.
///
/// The caller must keep borrowing the same shared-PHY register owner until
/// [`try_finish_read`] succeeds.
#[cfg(target_arch = "riscv32")]
pub(crate) fn try_start_read(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    address: PhyI2cAddress,
) -> Result<(), PhyI2cError> {
    hal_phy_i2c::try_start_read(registers, address).map_err(|_| PhyI2cError::Busy)
}

/// Observe one previously published PHY-I2C read exactly once.
///
/// The caller may invoke this once after an independently delivered hardware
/// or timer completion edge. `Busy` is then an incomplete/timeout result; it
/// must not be converted into a self-waking retry loop. This function never
/// loops, delays, or schedules itself.
///
/// `address` must name the in-flight command started by [`try_start_read`]
/// under the same borrowed radio ownership.
#[cfg(target_arch = "riscv32")]
pub(crate) fn try_finish_read(
    registers: &impl oer_esp32s31_hal::owner::SharedPhyAccess,
    address: PhyI2cAddress,
) -> Result<u8, PhyI2cError> {
    hal_phy_i2c::try_finish_read(registers, address).map_err(|_| PhyI2cError::Busy)
}

/// Publish one complete-register PHY-I2C write after observing the
/// pre-command busy state once. It never waits or loops on that state and
/// leaves post-command completion to [`try_finish_write`].
///
/// The caller must keep borrowing the same shared-PHY register owner until
/// [`try_finish_write`] succeeds.
#[cfg(target_arch = "riscv32")]
pub(crate) fn try_start_write(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    address: PhyI2cAddress,
    value: u8,
) -> Result<(), PhyI2cError> {
    hal_phy_i2c::try_start_write(registers, address, value).map_err(|_| PhyI2cError::Busy)
}

/// Observe one previously published PHY-I2C write exactly once.
///
/// The caller may invoke this once after an independently delivered hardware
/// or timer completion edge. `Busy` is an incomplete/timeout result and must
/// not be converted into a self-waking retry loop.
///
/// `address` must name the in-flight command started by [`try_start_write`]
/// under the same borrowed radio ownership.
#[cfg(target_arch = "riscv32")]
pub(crate) fn try_finish_write(
    registers: &impl oer_esp32s31_hal::owner::SharedPhyAccess,
    address: PhyI2cAddress,
) -> Result<(), PhyI2cError> {
    hal_phy_i2c::try_finish_write(registers, address).map_err(|_| PhyI2cError::Busy)
}

/// Execute the finite register prefix which precedes the vendor
/// `ets_delay_us(100)` call in `phy_open_i2c_xpd_new(true)`.
///
/// This leaf deliberately stops before the delay and delegates to the
/// shared-PHY HAL. Basis: complete pinned `libphy.a[phy_reg.o]` sequence at
/// offsets `0x2e..0x4e`; field identities remain private to the custom PAC.
#[cfg(target_arch = "riscv32")]
pub fn configure_open_i2c_pre_delay(registers: &mut impl SharedPhyAccess) {
    analog_i2c::prepare_open_i2c_pre_delay(registers);
}

/// Execute the finite common register suffix of `phy_open_i2c_xpd_new`.
///
/// The conditional PMU reset edge is preserved by the owned HAL. Basis:
/// complete pinned `libphy.a[phy_reg.o]::phy_open_i2c_xpd_new`; PMU field
/// identities remain private to the custom PAC.
#[cfg(target_arch = "riscv32")]
pub fn configure_open_i2c_power_and_pulse(registers: &mut impl SharedPhyAccess) {
    analog_i2c::complete_open_i2c_power_and_reset(registers);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenI2cXpdOutcome {
    Stable,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenI2cXpdAction {
    ConfigurePreDelay,
    DelayMicros(u32),
    ConfigurePowerAndPulse,
    CheckSdmDeadline {
        started_at_cycle: u32,
        maximum_cycles: u32,
    },
    ReadSdmSample {
        address: PhyI2cAddress,
    },
    Complete(OpenI2cXpdOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenI2cXpdCompletion {
    PreDelayConfigured,
    DelayElapsed,
    PowerAndPulseConfigured { started_at_cycle: u32 },
    DeadlineObserved { expired: bool },
    SdmSample(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenI2cXpdTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OpenI2cXpdStep {
    PreDelayConfiguration,
    Delay,
    PowerAndPulseConfiguration,
    DeadlineCheck { started_at_cycle: u32 },
    SdmSample { started_at_cycle: u32 },
    Complete(OpenI2cXpdOutcome),
}

/// Event-driven replacement plan for `phy_open_i2c_xpd_new` and ROM
/// `phy_wait_i2c_sdm_stable`.
///
/// The vendor path contains one synchronous 100-microsecond delay and then a
/// cycle-counter/I2C polling loop. Here the delay, deadline observation and
/// every I2C sample are explicit completions delivered by the outer async
/// radio owner. A mismatching SDM value returns to `CheckSdmDeadline`; it does
/// not self-wake or read again from `poll`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenI2cXpdTransition {
    step: OpenI2cXpdStep,
    samples: u16,
}

impl OpenI2cXpdTransition {
    pub const fn new(with_pre_delay: bool) -> Self {
        Self {
            step: if with_pre_delay {
                OpenI2cXpdStep::PreDelayConfiguration
            } else {
                OpenI2cXpdStep::PowerAndPulseConfiguration
            },
            samples: 0,
        }
    }

    pub const fn action(self) -> OpenI2cXpdAction {
        const SDM_SAMPLE: PhyI2cAddress = analog_registers::RFPLL_SDM_UPDATE_ENABLE.address();

        match self.step {
            OpenI2cXpdStep::PreDelayConfiguration => OpenI2cXpdAction::ConfigurePreDelay,
            OpenI2cXpdStep::Delay => OpenI2cXpdAction::DelayMicros(100),
            OpenI2cXpdStep::PowerAndPulseConfiguration => OpenI2cXpdAction::ConfigurePowerAndPulse,
            OpenI2cXpdStep::DeadlineCheck { started_at_cycle } => {
                OpenI2cXpdAction::CheckSdmDeadline {
                    started_at_cycle,
                    maximum_cycles: PHY_I2C_SDM_DEADLINE_CYCLES,
                }
            }
            OpenI2cXpdStep::SdmSample { .. } => OpenI2cXpdAction::ReadSdmSample {
                address: SDM_SAMPLE,
            },
            OpenI2cXpdStep::Complete(outcome) => OpenI2cXpdAction::Complete(outcome),
        }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn samples(self) -> u16 {
        self.samples
    }

    pub fn advance(
        &mut self,
        completion: OpenI2cXpdCompletion,
    ) -> Result<(), OpenI2cXpdTransitionError> {
        self.step = match (self.step, completion) {
            (OpenI2cXpdStep::PreDelayConfiguration, OpenI2cXpdCompletion::PreDelayConfigured) => {
                OpenI2cXpdStep::Delay
            }
            (OpenI2cXpdStep::Delay, OpenI2cXpdCompletion::DelayElapsed) => {
                OpenI2cXpdStep::PowerAndPulseConfiguration
            }
            (
                OpenI2cXpdStep::PowerAndPulseConfiguration,
                OpenI2cXpdCompletion::PowerAndPulseConfigured { started_at_cycle },
            ) => OpenI2cXpdStep::DeadlineCheck { started_at_cycle },
            (
                OpenI2cXpdStep::DeadlineCheck { .. },
                OpenI2cXpdCompletion::DeadlineObserved { expired: true },
            ) => OpenI2cXpdStep::Complete(OpenI2cXpdOutcome::TimedOut),
            (
                OpenI2cXpdStep::DeadlineCheck { started_at_cycle },
                OpenI2cXpdCompletion::DeadlineObserved { expired: false },
            ) => OpenI2cXpdStep::SdmSample { started_at_cycle },
            (
                OpenI2cXpdStep::SdmSample { started_at_cycle },
                OpenI2cXpdCompletion::SdmSample(value),
            ) => {
                self.samples = self.samples.saturating_add(1);
                if value == PHY_I2C_SDM_STABLE_VALUE {
                    OpenI2cXpdStep::Complete(OpenI2cXpdOutcome::Stable)
                } else {
                    OpenI2cXpdStep::DeadlineCheck { started_at_cycle }
                }
            }
            (OpenI2cXpdStep::Complete(_), _) => {
                return Err(OpenI2cXpdTransitionError::AlreadyComplete);
            }
            _ => return Err(OpenI2cXpdTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdcRateAction {
    ConfigureI2c { rate: PhyAdcRate },
    ConfigureMmio { rate: PhyAdcRate },
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdcRateCompletion {
    I2cConfigured,
    MmioConfigured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdcRateTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdcRateStep {
    ConfigureI2c,
    ConfigureMmio,
    Complete,
}

/// Event-driven replacement for complete rev0 ROM `phy_adc_rate_set`.
///
/// The PAC owns the analog-register RMW and the finite two-write MMIO suffix.
/// This transition retains only their semantic order and never polls or
/// repeats either operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdcRateTransition {
    step: AdcRateStep,
    rate: PhyAdcRate,
}

impl AdcRateTransition {
    pub const fn new(rate: PhyAdcRate) -> Self {
        Self {
            step: AdcRateStep::ConfigureI2c,
            rate,
        }
    }

    pub const fn action(self) -> AdcRateAction {
        match self.step {
            AdcRateStep::ConfigureI2c => AdcRateAction::ConfigureI2c { rate: self.rate },
            AdcRateStep::ConfigureMmio => AdcRateAction::ConfigureMmio { rate: self.rate },
            AdcRateStep::Complete => AdcRateAction::Complete,
        }
    }

    pub fn advance(&mut self, completion: AdcRateCompletion) -> Result<(), AdcRateTransitionError> {
        self.step = match (self.step, completion) {
            (AdcRateStep::ConfigureI2c, AdcRateCompletion::I2cConfigured) => {
                AdcRateStep::ConfigureMmio
            }
            (AdcRateStep::ConfigureMmio, AdcRateCompletion::MmioConfigured) => {
                AdcRateStep::Complete
            }
            (AdcRateStep::Complete, _) => return Err(AdcRateTransitionError::AlreadyComplete),
            _ => return Err(AdcRateTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

/// One owned replacement for ROM `phy_i2c_writeReg_Mask`: the portable
/// [`oer_radio_analog::FieldWriteTransition`] over this chip's analog
/// identities. The current byte crosses the async read edge as a completion
/// value, is transformed in Rust, and is then owned until the separately
/// completed write.
///
/// Every production value fits its field; a wider value would leak into the
/// neighboring bits of the vendor leaf and is a programming error here.
pub const fn field_write(
    field: PhyI2cField,
    value: u8,
) -> oer_radio_analog::FieldWriteTransition<PhyI2cAddress> {
    let (msb, lsb) = field.bit_range();
    let Some(field) = oer_radio_analog::AnalogField::new(field.address(), msb, lsb) else {
        panic!("a reviewed analog field lies within its byte");
    };
    match oer_radio_analog::FieldWriteTransition::new(field, value) {
        Ok(write) => write,
        Err(_) => panic!("a production value fits its analog field"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaskedI2cWriteBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

/// Non-cloneable lowering for one explicit read or write emitted by
/// [`oer_radio_analog::FieldWriteTransition<crate::analog::i2c::PhyI2cAddress>`].
#[derive(Debug, Eq, PartialEq)]
pub struct MaskedI2cWriteBinding {
    outer_action: oer_radio_analog::Action<crate::analog::i2c::PhyI2cAddress, ()>,
    transaction: crate::calibration::cold::PhyColdI2cTransaction,
}

impl MaskedI2cWriteBinding {
    pub fn new(
        action: oer_radio_analog::Action<crate::analog::i2c::PhyI2cAddress, ()>,
    ) -> Result<Self, MaskedI2cWriteBindingError> {
        let request = match action {
            oer_radio_analog::Action::Read { address } => {
                crate::calibration::cold::PhyColdI2cRequest::read_byte(address)
            }
            oer_radio_analog::Action::Write { address, value } => {
                crate::calibration::cold::PhyColdI2cRequest::write_byte(address, value)
            }
            oer_radio_analog::Action::Complete(()) => {
                return Err(MaskedI2cWriteBindingError::UnsupportedAction);
            }
        };
        Ok(Self {
            outer_action: action,
            transaction: crate::calibration::cold::PhyColdI2cTransaction::new(request),
        })
    }

    pub const fn action(&self) -> crate::calibration::cold::PhyColdI2cAction {
        self.transaction.action()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn read_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.read_started()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn write_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.write_started()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn observe_read_result(
        &mut self,
        result: Result<u8, PhyI2cError>,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_read_result(result)
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn observe_write_result(
        &mut self,
        result: Result<(), PhyI2cError>,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_write_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &P,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_target_edge(platform)
    }

    pub fn into_completion(
        self,
    ) -> Result<
        oer_radio_analog::Completion<crate::analog::i2c::PhyI2cAddress>,
        MaskedI2cWriteBindingError,
    > {
        match (self.outer_action, self.transaction.action()) {
            (
                oer_radio_analog::Action::Read { address },
                crate::calibration::cold::PhyColdI2cAction::Complete(
                    crate::calibration::cold::PhyColdI2cOutcome::Read {
                        address: completed_address,
                        value,
                    },
                ),
            ) if completed_address == address => {
                Ok(oer_radio_analog::Completion::Read { address, value })
            }
            (
                oer_radio_analog::Action::Write { address, .. },
                crate::calibration::cold::PhyColdI2cAction::Complete(
                    crate::calibration::cold::PhyColdI2cOutcome::Written {
                        address: completed_address,
                    },
                ),
            ) if completed_address == address => {
                Ok(oer_radio_analog::Completion::Written { address })
            }
            (_, crate::calibration::cold::PhyColdI2cAction::Complete(_)) => {
                Err(MaskedI2cWriteBindingError::UnexpectedOutcome)
            }
            _ => Err(MaskedI2cWriteBindingError::IncompleteTransaction),
        }
    }
}

/// Five explicit parameter bytes consumed by ROM `phy_filter_dcap_set`.
///
/// Offset-based names are deliberate: the electrical meaning of these
/// vendor parameter fields has not yet been established. Bytes `0xe9` and
/// `0xea` are the two primary RC-derived codes of
/// the RC-calibration state, for divisors 20 and 40; `phy_filter_dcap_set` writes the first to
/// block-0x67 registers 0x04, 0x05, 0x0c and 0x0d and the second to 0x06,
/// 0x07, 0x0e and 0x0f.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FilterDcapParameters {
    parameter_e9: u8,
    parameter_ea: u8,
    parameter_ed: u8,
    parameter_ee: u8,
    parameter_f0: u8,
}

impl FilterDcapParameters {
    pub const fn new(
        parameter_e9: u8,
        parameter_ea: u8,
        parameter_ed: u8,
        parameter_ee: u8,
        parameter_f0: u8,
    ) -> Self {
        Self {
            parameter_e9,
            parameter_ea,
            parameter_ed,
            parameter_ee,
            parameter_f0,
        }
    }

    pub(crate) const fn pac_inputs(self) -> oer_esp32s31_hal::phy::i2c::PhyFilterDcapInputs {
        oer_esp32s31_hal::phy::i2c::PhyFilterDcapInputs::new(
            self.parameter_e9,
            self.parameter_ea,
            self.parameter_ed,
            self.parameter_ee,
            self.parameter_f0,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterDcapAction {
    Configure(FilterDcapParameters),
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterDcapCompletion {
    Configured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterDcapTransitionError {
    AlreadyComplete,
}

/// Semantic owner of the complete ROM `phy_filter_dcap_set` operation.
///
/// The finite analog-register transaction is owned by the PAC. This parent
/// stores only a five-byte parameter snapshot and never exposes register
/// geometry or reads a shared parameter image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FilterDcapTransition {
    parameter: FilterDcapParameters,
    complete: bool,
}

impl FilterDcapTransition {
    pub const fn new(parameter: FilterDcapParameters) -> Self {
        Self {
            parameter,
            complete: false,
        }
    }

    pub const fn parameters(self) -> FilterDcapParameters {
        self.parameter
    }

    pub const fn action(self) -> FilterDcapAction {
        if self.complete {
            FilterDcapAction::Complete
        } else {
            FilterDcapAction::Configure(self.parameter)
        }
    }

    pub fn advance(
        &mut self,
        _completion: FilterDcapCompletion,
    ) -> Result<(), FilterDcapTransitionError> {
        if self.complete {
            Err(FilterDcapTransitionError::AlreadyComplete)
        } else {
            self.complete = true;
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRfInitParameterSnapshot {
    filter_dcap: FilterDcapParameters,
    parameter_18e: u8,
}

impl PhyRfInitParameterSnapshot {
    pub const fn new(filter_dcap: FilterDcapParameters, parameter_18e: u8) -> Self {
        Self {
            filter_dcap,
            parameter_18e,
        }
    }

    pub const fn filter_dcap(self) -> FilterDcapParameters {
        self.filter_dcap
    }

    pub const fn parameter_18e(self) -> u8 {
        self.parameter_18e
    }

    pub const fn with_parameter_18e(self, parameter_18e: u8) -> Self {
        Self {
            filter_dcap: self.filter_dcap,
            parameter_18e,
        }
    }

    pub(crate) const fn pac_initialization_stage_one_inputs(
        self,
    ) -> oer_esp32s31_hal::phy::i2c::PhyI2cInitializationStageOneInputs {
        oer_esp32s31_hal::phy::i2c::PhyI2cInitializationStageOneInputs::new(
            self.parameter_18e,
            self.filter_dcap.parameter_ee,
        )
    }

    #[cfg(target_arch = "riscv32")]
    const fn pac_command_memory_inputs(self) -> hal_phy_i2c::PhyI2cCommandMemoryInputs {
        hal_phy_i2c::PhyI2cCommandMemoryInputs::new(
            self.parameter_18e,
            self.filter_dcap.parameter_e9,
            self.filter_dcap.parameter_ea,
            self.filter_dcap.parameter_ed,
            self.filter_dcap.parameter_ee,
            self.filter_dcap.parameter_f0,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cInit1Action {
    Configure(PhyRfInitParameterSnapshot),
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cInit1Completion {
    Configured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cInit1TransitionError {
    AlreadyComplete,
}

/// Semantic owner of complete `libphy.a[phy_i2c.o]::phy_i2c_init1`.
///
/// The PAC owns the finite write plan. This transition receives the two
/// dynamic vendor facts through an owned snapshot without exposing any analog
/// register geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2cInit1Transition {
    parameter: PhyRfInitParameterSnapshot,
    complete: bool,
}

impl I2cInit1Transition {
    pub const fn new(parameter: PhyRfInitParameterSnapshot) -> Self {
        Self {
            parameter,
            complete: false,
        }
    }

    pub const fn action(self) -> I2cInit1Action {
        if self.complete {
            I2cInit1Action::Complete
        } else {
            I2cInit1Action::Configure(self.parameter)
        }
    }

    pub fn advance(
        &mut self,
        _completion: I2cInit1Completion,
    ) -> Result<(), I2cInit1TransitionError> {
        if self.complete {
            Err(I2cInit1TransitionError::AlreadyComplete)
        } else {
            self.complete = true;
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllChargePumpOutcome {
    pub parameter_18e: u8,
    pub lock_observed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllChargePumpAction {
    WriteMasked { field: PhyI2cField, value: u8 },
    DelayMicros(u32),
    ReadMasked { field: PhyI2cField },
    ReadByte { address: PhyI2cAddress },
    Complete(RfpllChargePumpOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllChargePumpCompletion {
    Write,
    Delay,
    ReadMasked(u8),
    ReadByte { address: PhyI2cAddress, value: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfpllChargePumpTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RfpllChargePumpStep {
    InitialWrite(u8),
    Delay { attempt: u8 },
    LockRead { attempt: u8 },
    CapRead { lock_observed: bool },
    EnableAdjustedValue { value: u8, lock_observed: bool },
    WriteAdjustedValue { value: u8, lock_observed: bool },
    FinalRead { lock_observed: bool },
    Complete(RfpllChargePumpOutcome),
}

/// Event-driven replacement for complete ROM `phy_rfpll_chgp_cal`.
///
/// The ROM body performs as many as 100 synchronous 20-microsecond
/// delay/read iterations and prints on the final miss. Rust exposes every
/// delay and I2C observation as an external completion. The non-blocking
/// result retains `lock_observed` instead of invoking `ets_printf`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllChargePumpTransition {
    step: RfpllChargePumpStep,
}

impl RfpllChargePumpTransition {
    pub const fn new() -> Self {
        Self {
            step: RfpllChargePumpStep::InitialWrite(0),
        }
    }

    const fn initial_write(index: u8) -> RfpllChargePumpAction {
        let (field, value) = match index {
            0 => (analog_registers::RFPLL_CHARGE_PUMP_CALIBRATION_ENABLE, 0),
            1 => (analog_registers::RFPLL_CHARGE_PUMP_CALIBRATION_PULSE, 0),
            _ => (analog_registers::RFPLL_CHARGE_PUMP_CALIBRATION_PULSE, 1),
        };
        RfpllChargePumpAction::WriteMasked { field, value }
    }

    pub const fn action(self) -> RfpllChargePumpAction {
        match self.step {
            RfpllChargePumpStep::InitialWrite(index) => Self::initial_write(index),
            RfpllChargePumpStep::Delay { .. } => RfpllChargePumpAction::DelayMicros(20),
            RfpllChargePumpStep::LockRead { .. } => RfpllChargePumpAction::ReadMasked {
                field: analog_registers::RFPLL_CHARGE_PUMP_LOCK_STATUS,
            },
            RfpllChargePumpStep::CapRead { .. } => RfpllChargePumpAction::ReadMasked {
                field: analog_registers::RFPLL_CHARGE_PUMP_RESULT,
            },
            RfpllChargePumpStep::EnableAdjustedValue { .. } => RfpllChargePumpAction::WriteMasked {
                field: analog_registers::RFPLL_CHARGE_PUMP_CALIBRATION_ENABLE,
                value: 1,
            },
            RfpllChargePumpStep::WriteAdjustedValue { value, .. } => {
                RfpllChargePumpAction::WriteMasked {
                    field: analog_registers::RFPLL_CHARGE_PUMP_VALUE,
                    value,
                }
            }
            RfpllChargePumpStep::FinalRead { .. } => RfpllChargePumpAction::ReadByte {
                address: analog_registers::RFPLL_CHARGE_PUMP_VALUE.address(),
            },
            RfpllChargePumpStep::Complete(outcome) => RfpllChargePumpAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: RfpllChargePumpCompletion,
    ) -> Result<(), RfpllChargePumpTransitionError> {
        self.step = match (self.step, completion) {
            (RfpllChargePumpStep::InitialWrite(index), RfpllChargePumpCompletion::Write) => {
                if index == 2 {
                    RfpllChargePumpStep::Delay { attempt: 0 }
                } else {
                    RfpllChargePumpStep::InitialWrite(index + 1)
                }
            }
            (RfpllChargePumpStep::Delay { attempt }, RfpllChargePumpCompletion::Delay) => {
                RfpllChargePumpStep::LockRead { attempt }
            }
            (
                RfpllChargePumpStep::LockRead { attempt },
                RfpllChargePumpCompletion::ReadMasked(value),
            ) => {
                if value != 0 {
                    RfpllChargePumpStep::CapRead {
                        lock_observed: true,
                    }
                } else if attempt == 99 {
                    RfpllChargePumpStep::CapRead {
                        lock_observed: false,
                    }
                } else {
                    RfpllChargePumpStep::Delay {
                        attempt: attempt + 1,
                    }
                }
            }
            (
                RfpllChargePumpStep::CapRead { lock_observed },
                RfpllChargePumpCompletion::ReadMasked(value),
            ) => {
                let adjusted = ((u16::from(value) * 7) / 6 + 9).min(0x1f) as u8;
                RfpllChargePumpStep::EnableAdjustedValue {
                    value: adjusted,
                    lock_observed,
                }
            }
            (
                RfpllChargePumpStep::EnableAdjustedValue {
                    value,
                    lock_observed,
                },
                RfpllChargePumpCompletion::Write,
            ) => RfpllChargePumpStep::WriteAdjustedValue {
                value,
                lock_observed,
            },
            (
                RfpllChargePumpStep::WriteAdjustedValue { lock_observed, .. },
                RfpllChargePumpCompletion::Write,
            ) => RfpllChargePumpStep::FinalRead { lock_observed },
            (
                RfpllChargePumpStep::FinalRead { lock_observed },
                RfpllChargePumpCompletion::ReadByte { address, value },
            ) if address == analog_registers::RFPLL_CHARGE_PUMP_VALUE.address() => {
                RfpllChargePumpStep::Complete(RfpllChargePumpOutcome {
                    parameter_18e: value,
                    lock_observed,
                })
            }
            (RfpllChargePumpStep::Complete(_), _) => {
                return Err(RfpllChargePumpTransitionError::AlreadyComplete);
            }
            _ => return Err(RfpllChargePumpTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl Default for RfpllChargePumpTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfInitPrefixOutcome {
    ChannelFrequencyInitialized {
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
        sar2_reinitialized: bool,
        xtal_duty: XtalDutyCalibrationOutcome,
        channel_frequency: PhyChannelFrequencyInitOutcome,
    },
    ChannelFrequencyInitializationFailed(PhyChannelFrequencyInitFailure),
    XtalDutyCalibrationFailed(crate::analog::crystal_duty::XtalDutyFailure),
    SdmTimedOut,
    PbusForceTestTimedOut(PhyPbusForceTest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfInitPrefixAction {
    ConfigureFeBbClock,
    ConfigureBbpllCalibration {
        enabled: bool,
    },
    ConfigureBiasRegisters,
    OpenI2cXpd(OpenI2cXpdAction),
    PbusClear(PhyPbusClearAction),
    ConfigureI2cClockSelection,
    ConfigureI2cBbpll,
    AdcRate(AdcRateAction),
    ConfigureI2cMasterRegisters,
    ConfigurePowerDetectorRegisters,
    ConfigureFrontEndRegisters,
    ConfigureTemperatureSensorRead,
    ConfigureTxPowerControlBackground,
    ConfigureRcCalibrationSettings,
    InspectRcCalibrationState,
    RcCalibration(RcCalibrationAction),
    CaptureFilterDcapParameters,
    FilterDcap(FilterDcapAction),
    ReadParameter18e {
        address: PhyI2cAddress,
    },
    I2cInit1(I2cInit1Action),
    RfpllChargePump(RfpllChargePumpAction),
    ConfigureI2cMasterCommandMemory {
        parameter: PhyRfInitParameterSnapshot,
    },
    ReadMasked69 {
        field: PhyI2cField,
    },
    ConfigureSar2,
    CaptureXtalDutyParameters,
    XtalDuty(XtalDutyCalibrationAction),
    ConfigureFrontEndRegisterUpdate,
    CaptureChannelFrequencyControl,
    ChannelFrequency(PhyChannelFrequencyInitAction),
    DelayMicros(u32),
    Complete(PhyRfInitPrefixOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfInitPrefixCompletion {
    FeBbClockConfigured,
    BbpllCalibrationConfigured,
    BiasRegistersConfigured,
    OpenI2cXpd(OpenI2cXpdCompletion),
    PbusClear(PhyPbusClearCompletion),
    I2cClockSelectionConfigured,
    I2cBbpllConfigured,
    AdcRate(AdcRateCompletion),
    I2cMasterRegistersConfigured,
    PowerDetectorRegistersConfigured,
    FrontEndRegistersConfigured,
    TemperatureSensorReadConfigured,
    TxPowerControlBackgroundConfigured,
    RcCalibrationSettingsConfigured,
    RcCalibrationStateInspected { already_complete: bool },
    RcCalibration(RcCalibrationCompletion),
    FilterDcapParametersCaptured(FilterDcapParameters),
    FilterDcap(FilterDcapCompletion),
    Parameter18eRead { address: PhyI2cAddress, value: u8 },
    I2cInit1(I2cInit1Completion),
    RfpllChargePump(RfpllChargePumpCompletion),
    I2cMasterCommandMemoryConfigured,
    Masked69Read(u8),
    Sar2Configured,
    XtalDutyParametersCaptured(XtalDutyCalibrationParameters),
    XtalDuty(XtalDutyCalibrationCompletion),
    FrontEndRegisterUpdateConfigured,
    ChannelFrequencyControlCaptured(PhyChannelFrequencyInitControl),
    ChannelFrequency(PhyChannelFrequencyInitCompletion),
    DelayElapsed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfInitPrefixTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyRfInitPrefixStep {
    FeBbClock,
    BbpllCalibration,
    BiasRegisters,
    OpenI2cXpd(OpenI2cXpdTransition),
    PostI2cDelay,
    PbusClear(PhyPbusClearTransition),
    I2cClockSelection,
    I2cBbpll,
    AdcRate(AdcRateTransition),
    I2cMasterRegisters,
    PowerDetectorRegisters,
    FrontEndRegisters,
    TemperatureSensorRead,
    TxPowerControlBackground,
    RcCalibrationSettings,
    RcCalibrationState,
    RcCalibration(RcCalibrationTransition),
    FilterDcapParameters,
    FilterDcap(FilterDcapTransition),
    Parameter18eRead {
        filter_dcap: FilterDcapParameters,
    },
    I2cInit1 {
        transition: I2cInit1Transition,
        parameter: PhyRfInitParameterSnapshot,
    },
    RfpllChargePump {
        transition: RfpllChargePumpTransition,
        parameter: PhyRfInitParameterSnapshot,
    },
    I2cMasterCommandMemory {
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
    },
    Masked69Read {
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
    },
    Sar2Configuration {
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
    },
    XtalDutyParameters {
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
        sar2_reinitialized: bool,
    },
    XtalDuty {
        transition: XtalDutyCalibrationTransition,
        xtal_parameters: XtalDutyCalibrationParameters,
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
        sar2_reinitialized: bool,
    },
    FrontEndRegisterUpdate {
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
        sar2_reinitialized: bool,
    },
    ChannelFrequencyControl {
        xtal_parameters: XtalDutyCalibrationParameters,
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
        sar2_reinitialized: bool,
        xtal_duty: XtalDutyCalibrationOutcome,
    },
    ChannelFrequency {
        transition: PhyChannelFrequencyInitTransition,
        parameter: PhyRfInitParameterSnapshot,
        rfpll_lock_observed: bool,
        sar2_reinitialized: bool,
        xtal_duty: XtalDutyCalibrationOutcome,
    },
    Complete(PhyRfInitPrefixOutcome),
}

/// Event-driven composition of operations one through twenty-five in the complete
/// pinned `libphy.a[phy_init.o]::phy_rf_init` body.
///
/// The finite MMIO and PAC-planned PHY-I2C commands are semantic actions. Every SDM
/// sample requires an external PHY-I2C completion. The 100- and 10-microsecond
/// intervals are separate executor timer edges. No transition is caused by
/// polling this value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRfInitPrefixTransition {
    step: PhyRfInitPrefixStep,
}

impl PhyRfInitPrefixTransition {
    pub const fn new() -> Self {
        Self {
            step: PhyRfInitPrefixStep::FeBbClock,
        }
    }

    pub const fn action(self) -> PhyRfInitPrefixAction {
        match self.step {
            PhyRfInitPrefixStep::FeBbClock => PhyRfInitPrefixAction::ConfigureFeBbClock,
            PhyRfInitPrefixStep::BbpllCalibration => {
                PhyRfInitPrefixAction::ConfigureBbpllCalibration { enabled: true }
            }
            PhyRfInitPrefixStep::BiasRegisters => PhyRfInitPrefixAction::ConfigureBiasRegisters,
            PhyRfInitPrefixStep::OpenI2cXpd(transition) => match transition.action() {
                OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::Stable) => {
                    PhyRfInitPrefixAction::DelayMicros(10)
                }
                OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::TimedOut) => {
                    PhyRfInitPrefixAction::Complete(PhyRfInitPrefixOutcome::SdmTimedOut)
                }
                action => PhyRfInitPrefixAction::OpenI2cXpd(action),
            },
            PhyRfInitPrefixStep::PostI2cDelay => PhyRfInitPrefixAction::DelayMicros(10),
            PhyRfInitPrefixStep::PbusClear(transition) => match transition.action() {
                PhyPbusClearAction::Complete(PhyPbusClearOutcome::Cleared) => {
                    PhyRfInitPrefixAction::ConfigureI2cClockSelection
                }
                PhyPbusClearAction::Complete(PhyPbusClearOutcome::ForceTestTimedOut(
                    transaction,
                )) => PhyRfInitPrefixAction::Complete(
                    PhyRfInitPrefixOutcome::PbusForceTestTimedOut(transaction),
                ),
                action => PhyRfInitPrefixAction::PbusClear(action),
            },
            PhyRfInitPrefixStep::I2cClockSelection => {
                PhyRfInitPrefixAction::ConfigureI2cClockSelection
            }
            PhyRfInitPrefixStep::I2cBbpll => PhyRfInitPrefixAction::ConfigureI2cBbpll,
            PhyRfInitPrefixStep::AdcRate(transition) => match transition.action() {
                AdcRateAction::Complete => PhyRfInitPrefixAction::ConfigureI2cMasterRegisters,
                action => PhyRfInitPrefixAction::AdcRate(action),
            },
            PhyRfInitPrefixStep::I2cMasterRegisters => {
                PhyRfInitPrefixAction::ConfigureI2cMasterRegisters
            }
            PhyRfInitPrefixStep::PowerDetectorRegisters => {
                PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters
            }
            PhyRfInitPrefixStep::FrontEndRegisters => {
                PhyRfInitPrefixAction::ConfigureFrontEndRegisters
            }
            PhyRfInitPrefixStep::TemperatureSensorRead => {
                PhyRfInitPrefixAction::ConfigureTemperatureSensorRead
            }
            PhyRfInitPrefixStep::TxPowerControlBackground => {
                PhyRfInitPrefixAction::ConfigureTxPowerControlBackground
            }
            PhyRfInitPrefixStep::RcCalibrationSettings => {
                PhyRfInitPrefixAction::ConfigureRcCalibrationSettings
            }
            PhyRfInitPrefixStep::RcCalibrationState => {
                PhyRfInitPrefixAction::InspectRcCalibrationState
            }
            PhyRfInitPrefixStep::RcCalibration(transition) => match transition.action() {
                RcCalibrationAction::Complete => PhyRfInitPrefixAction::CaptureFilterDcapParameters,
                action => PhyRfInitPrefixAction::RcCalibration(action),
            },
            PhyRfInitPrefixStep::FilterDcapParameters => {
                PhyRfInitPrefixAction::CaptureFilterDcapParameters
            }
            PhyRfInitPrefixStep::FilterDcap(transition) => match transition.action() {
                FilterDcapAction::Complete => PhyRfInitPrefixAction::ReadParameter18e {
                    address: analog_registers::RFPLL_CHARGE_PUMP_VALUE.address(),
                },
                action => PhyRfInitPrefixAction::FilterDcap(action),
            },
            PhyRfInitPrefixStep::Parameter18eRead { .. } => {
                PhyRfInitPrefixAction::ReadParameter18e {
                    address: analog_registers::RFPLL_CHARGE_PUMP_VALUE.address(),
                }
            }
            PhyRfInitPrefixStep::I2cInit1 {
                transition,
                parameter: _,
            } => match transition.action() {
                I2cInit1Action::Complete => PhyRfInitPrefixAction::RfpllChargePump(
                    RfpllChargePumpTransition::new().action(),
                ),
                action => PhyRfInitPrefixAction::I2cInit1(action),
            },
            PhyRfInitPrefixStep::RfpllChargePump {
                transition,
                parameter,
            } => match transition.action() {
                RfpllChargePumpAction::Complete(outcome) => {
                    PhyRfInitPrefixAction::ConfigureI2cMasterCommandMemory {
                        parameter: parameter.with_parameter_18e(outcome.parameter_18e),
                    }
                }
                action => PhyRfInitPrefixAction::RfpllChargePump(action),
            },
            PhyRfInitPrefixStep::I2cMasterCommandMemory { parameter, .. } => {
                PhyRfInitPrefixAction::ConfigureI2cMasterCommandMemory { parameter }
            }
            PhyRfInitPrefixStep::Masked69Read { .. } => PhyRfInitPrefixAction::ReadMasked69 {
                field: analog_registers::TEMPERATURE_SENSOR_SAR2_STATUS,
            },
            PhyRfInitPrefixStep::Sar2Configuration { .. } => PhyRfInitPrefixAction::ConfigureSar2,
            PhyRfInitPrefixStep::XtalDutyParameters { .. } => {
                PhyRfInitPrefixAction::CaptureXtalDutyParameters
            }
            PhyRfInitPrefixStep::XtalDuty { transition, .. } => match transition.action() {
                XtalDutyCalibrationAction::Complete(_) => {
                    PhyRfInitPrefixAction::CaptureChannelFrequencyControl
                }
                XtalDutyCalibrationAction::Failed(failure) => PhyRfInitPrefixAction::Complete(
                    PhyRfInitPrefixOutcome::XtalDutyCalibrationFailed(failure),
                ),
                action => PhyRfInitPrefixAction::XtalDuty(action),
            },
            PhyRfInitPrefixStep::FrontEndRegisterUpdate { .. } => {
                PhyRfInitPrefixAction::ConfigureFrontEndRegisterUpdate
            }
            PhyRfInitPrefixStep::ChannelFrequencyControl { .. } => {
                PhyRfInitPrefixAction::CaptureChannelFrequencyControl
            }
            PhyRfInitPrefixStep::ChannelFrequency { transition, .. } => {
                PhyRfInitPrefixAction::ChannelFrequency(transition.action())
            }
            PhyRfInitPrefixStep::Complete(outcome) => PhyRfInitPrefixAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRfInitPrefixCompletion,
    ) -> Result<(), PhyRfInitPrefixTransitionError> {
        self.step = match (self.step, completion) {
            (PhyRfInitPrefixStep::FeBbClock, PhyRfInitPrefixCompletion::FeBbClockConfigured) => {
                PhyRfInitPrefixStep::BbpllCalibration
            }
            (
                PhyRfInitPrefixStep::BbpllCalibration,
                PhyRfInitPrefixCompletion::BbpllCalibrationConfigured,
            ) => PhyRfInitPrefixStep::BiasRegisters,
            (
                PhyRfInitPrefixStep::BiasRegisters,
                PhyRfInitPrefixCompletion::BiasRegistersConfigured,
            ) => PhyRfInitPrefixStep::OpenI2cXpd(OpenI2cXpdTransition::new(true)),
            (
                PhyRfInitPrefixStep::OpenI2cXpd(mut transition),
                PhyRfInitPrefixCompletion::OpenI2cXpd(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                match transition.action() {
                    OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::Stable) => {
                        PhyRfInitPrefixStep::PostI2cDelay
                    }
                    OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::TimedOut) => {
                        PhyRfInitPrefixStep::Complete(PhyRfInitPrefixOutcome::SdmTimedOut)
                    }
                    _ => PhyRfInitPrefixStep::OpenI2cXpd(transition),
                }
            }
            (PhyRfInitPrefixStep::PostI2cDelay, PhyRfInitPrefixCompletion::DelayElapsed) => {
                PhyRfInitPrefixStep::PbusClear(PhyPbusClearTransition::new())
            }
            (
                PhyRfInitPrefixStep::PbusClear(mut transition),
                PhyRfInitPrefixCompletion::PbusClear(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyPbusClearAction::Complete(PhyPbusClearOutcome::Cleared) => {
                        PhyRfInitPrefixStep::I2cClockSelection
                    }
                    PhyPbusClearAction::Complete(PhyPbusClearOutcome::ForceTestTimedOut(
                        transaction,
                    )) => PhyRfInitPrefixStep::Complete(
                        PhyRfInitPrefixOutcome::PbusForceTestTimedOut(transaction),
                    ),
                    _ => PhyRfInitPrefixStep::PbusClear(transition),
                }
            }
            (
                PhyRfInitPrefixStep::I2cClockSelection,
                PhyRfInitPrefixCompletion::I2cClockSelectionConfigured,
            ) => PhyRfInitPrefixStep::I2cBbpll,
            (PhyRfInitPrefixStep::I2cBbpll, PhyRfInitPrefixCompletion::I2cBbpllConfigured) => {
                PhyRfInitPrefixStep::AdcRate(AdcRateTransition::new(PhyAdcRate::High))
            }
            (
                PhyRfInitPrefixStep::AdcRate(mut transition),
                PhyRfInitPrefixCompletion::AdcRate(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                if transition.action() == AdcRateAction::Complete {
                    PhyRfInitPrefixStep::I2cMasterRegisters
                } else {
                    PhyRfInitPrefixStep::AdcRate(transition)
                }
            }
            (
                PhyRfInitPrefixStep::I2cMasterRegisters,
                PhyRfInitPrefixCompletion::I2cMasterRegistersConfigured,
            ) => PhyRfInitPrefixStep::PowerDetectorRegisters,
            (
                PhyRfInitPrefixStep::PowerDetectorRegisters,
                PhyRfInitPrefixCompletion::PowerDetectorRegistersConfigured,
            ) => PhyRfInitPrefixStep::FrontEndRegisters,
            (
                PhyRfInitPrefixStep::FrontEndRegisters,
                PhyRfInitPrefixCompletion::FrontEndRegistersConfigured,
            ) => PhyRfInitPrefixStep::TemperatureSensorRead,
            (
                PhyRfInitPrefixStep::TemperatureSensorRead,
                PhyRfInitPrefixCompletion::TemperatureSensorReadConfigured,
            ) => PhyRfInitPrefixStep::TxPowerControlBackground,
            (
                PhyRfInitPrefixStep::TxPowerControlBackground,
                PhyRfInitPrefixCompletion::TxPowerControlBackgroundConfigured,
            ) => PhyRfInitPrefixStep::RcCalibrationSettings,
            (
                PhyRfInitPrefixStep::RcCalibrationSettings,
                PhyRfInitPrefixCompletion::RcCalibrationSettingsConfigured,
            ) => PhyRfInitPrefixStep::RcCalibrationState,
            (
                PhyRfInitPrefixStep::RcCalibrationState,
                PhyRfInitPrefixCompletion::RcCalibrationStateInspected { already_complete },
            ) => {
                if already_complete {
                    PhyRfInitPrefixStep::FilterDcapParameters
                } else {
                    PhyRfInitPrefixStep::RcCalibration(RcCalibrationTransition::new())
                }
            }
            (
                PhyRfInitPrefixStep::RcCalibration(mut transition),
                PhyRfInitPrefixCompletion::RcCalibration(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                if transition.action() == RcCalibrationAction::Complete {
                    PhyRfInitPrefixStep::FilterDcapParameters
                } else {
                    PhyRfInitPrefixStep::RcCalibration(transition)
                }
            }
            (
                PhyRfInitPrefixStep::FilterDcapParameters,
                PhyRfInitPrefixCompletion::FilterDcapParametersCaptured(parameter),
            ) => PhyRfInitPrefixStep::FilterDcap(FilterDcapTransition::new(parameter)),
            (
                PhyRfInitPrefixStep::FilterDcap(mut transition),
                PhyRfInitPrefixCompletion::FilterDcap(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                if transition.action() == FilterDcapAction::Complete {
                    PhyRfInitPrefixStep::Parameter18eRead {
                        filter_dcap: transition.parameters(),
                    }
                } else {
                    PhyRfInitPrefixStep::FilterDcap(transition)
                }
            }
            (
                PhyRfInitPrefixStep::Parameter18eRead { filter_dcap },
                PhyRfInitPrefixCompletion::Parameter18eRead { address, value },
            ) if address == analog_registers::RFPLL_CHARGE_PUMP_VALUE.address() => {
                let parameter = PhyRfInitParameterSnapshot::new(filter_dcap, value);
                PhyRfInitPrefixStep::I2cInit1 {
                    transition: I2cInit1Transition::new(parameter),
                    parameter,
                }
            }
            (
                PhyRfInitPrefixStep::I2cInit1 {
                    mut transition,
                    parameter,
                },
                PhyRfInitPrefixCompletion::I2cInit1(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                if transition.action() == I2cInit1Action::Complete {
                    PhyRfInitPrefixStep::RfpllChargePump {
                        transition: RfpllChargePumpTransition::new(),
                        parameter,
                    }
                } else {
                    PhyRfInitPrefixStep::I2cInit1 {
                        transition,
                        parameter,
                    }
                }
            }
            (
                PhyRfInitPrefixStep::RfpllChargePump {
                    mut transition,
                    parameter,
                },
                PhyRfInitPrefixCompletion::RfpllChargePump(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                match transition.action() {
                    RfpllChargePumpAction::Complete(outcome) => {
                        PhyRfInitPrefixStep::I2cMasterCommandMemory {
                            parameter: parameter.with_parameter_18e(outcome.parameter_18e),
                            rfpll_lock_observed: outcome.lock_observed,
                        }
                    }
                    _ => PhyRfInitPrefixStep::RfpllChargePump {
                        transition,
                        parameter,
                    },
                }
            }
            (
                PhyRfInitPrefixStep::I2cMasterCommandMemory {
                    parameter,
                    rfpll_lock_observed,
                },
                PhyRfInitPrefixCompletion::I2cMasterCommandMemoryConfigured,
            ) => PhyRfInitPrefixStep::Masked69Read {
                parameter,
                rfpll_lock_observed,
            },
            (
                PhyRfInitPrefixStep::Masked69Read {
                    parameter,
                    rfpll_lock_observed,
                },
                PhyRfInitPrefixCompletion::Masked69Read(value),
            ) => {
                if value == 0 {
                    PhyRfInitPrefixStep::Sar2Configuration {
                        parameter,
                        rfpll_lock_observed,
                    }
                } else {
                    PhyRfInitPrefixStep::FrontEndRegisterUpdate {
                        parameter,
                        rfpll_lock_observed,
                        sar2_reinitialized: false,
                    }
                }
            }
            (
                PhyRfInitPrefixStep::Sar2Configuration {
                    parameter,
                    rfpll_lock_observed,
                },
                PhyRfInitPrefixCompletion::Sar2Configured,
            ) => PhyRfInitPrefixStep::FrontEndRegisterUpdate {
                parameter,
                rfpll_lock_observed,
                sar2_reinitialized: true,
            },
            (
                PhyRfInitPrefixStep::XtalDutyParameters {
                    parameter,
                    rfpll_lock_observed,
                    sar2_reinitialized,
                },
                PhyRfInitPrefixCompletion::XtalDutyParametersCaptured(xtal_parameters),
            ) => PhyRfInitPrefixStep::XtalDuty {
                transition: XtalDutyCalibrationTransition::new(xtal_parameters),
                xtal_parameters,
                parameter,
                rfpll_lock_observed,
                sar2_reinitialized,
            },
            (
                PhyRfInitPrefixStep::XtalDuty {
                    mut transition,
                    xtal_parameters,
                    parameter,
                    rfpll_lock_observed,
                    sar2_reinitialized,
                },
                PhyRfInitPrefixCompletion::XtalDuty(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                match transition.action() {
                    XtalDutyCalibrationAction::Complete(xtal_duty) => {
                        PhyRfInitPrefixStep::ChannelFrequencyControl {
                            xtal_parameters,
                            parameter,
                            rfpll_lock_observed,
                            sar2_reinitialized,
                            xtal_duty,
                        }
                    }
                    XtalDutyCalibrationAction::Failed(failure) => PhyRfInitPrefixStep::Complete(
                        PhyRfInitPrefixOutcome::XtalDutyCalibrationFailed(failure),
                    ),
                    _ => PhyRfInitPrefixStep::XtalDuty {
                        transition,
                        xtal_parameters,
                        parameter,
                        rfpll_lock_observed,
                        sar2_reinitialized,
                    },
                }
            }
            (
                PhyRfInitPrefixStep::FrontEndRegisterUpdate {
                    parameter,
                    rfpll_lock_observed,
                    sar2_reinitialized,
                },
                PhyRfInitPrefixCompletion::FrontEndRegisterUpdateConfigured,
            ) => PhyRfInitPrefixStep::XtalDutyParameters {
                parameter,
                rfpll_lock_observed,
                sar2_reinitialized,
            },
            (
                PhyRfInitPrefixStep::ChannelFrequencyControl {
                    xtal_parameters,
                    parameter,
                    rfpll_lock_observed,
                    sar2_reinitialized,
                    xtal_duty,
                },
                PhyRfInitPrefixCompletion::ChannelFrequencyControlCaptured(control),
            ) => PhyRfInitPrefixStep::ChannelFrequency {
                transition: PhyChannelFrequencyInitTransition::new(
                    PhyChannelFrequencyInitRequest {
                        frequency_register_parameter_override: control
                            .frequency_register_parameter_override,
                        frequency_table_initialized: control.frequency_table_initialized,
                        crystal_selector: xtal_parameters.rf_frequency_offset_base,
                        middle_xtal_duty: xtal_duty.low_frequency.best_candidate,
                        outer_xtal_duty: xtal_duty.high_frequency.best_candidate,
                        front_end_parameter_bit: control.front_end_parameter_bit,
                    },
                ),
                parameter,
                rfpll_lock_observed,
                sar2_reinitialized,
                xtal_duty,
            },
            (
                PhyRfInitPrefixStep::ChannelFrequency {
                    mut transition,
                    parameter,
                    rfpll_lock_observed,
                    sar2_reinitialized,
                    xtal_duty,
                },
                PhyRfInitPrefixCompletion::ChannelFrequency(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyRfInitPrefixTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyChannelFrequencyInitAction::Complete(channel_frequency) => {
                        PhyRfInitPrefixStep::Complete(
                            PhyRfInitPrefixOutcome::ChannelFrequencyInitialized {
                                parameter,
                                rfpll_lock_observed,
                                sar2_reinitialized,
                                xtal_duty,
                                channel_frequency,
                            },
                        )
                    }
                    PhyChannelFrequencyInitAction::Failed(failure) => {
                        PhyRfInitPrefixStep::Complete(
                            PhyRfInitPrefixOutcome::ChannelFrequencyInitializationFailed(failure),
                        )
                    }
                    _ => PhyRfInitPrefixStep::ChannelFrequency {
                        transition,
                        parameter,
                        rfpll_lock_observed,
                        sar2_reinitialized,
                        xtal_duty,
                    },
                }
            }
            (PhyRfInitPrefixStep::Complete(_), _) => {
                return Err(PhyRfInitPrefixTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRfInitPrefixTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl Default for PhyRfInitPrefixTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RcCalibrationAction {
    WriteMasked { field: PhyI2cField, value: u8 },
    DelayMicros(u32),
    ReadMasked { field: PhyI2cField },
    ApplyResult(u8),
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RcCalibrationCompletion {
    Write,
    Delay,
    Read(u8),
    Applied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RcCalibrationTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

/// Exact finite action plan recovered from ROM `phy_get_rc_dout`.
///
/// The owner executes each I2C action through a non-blocking transaction and
/// implements `DelayMicros(100)` with its Rust async timer. No action advances
/// merely because the future was polled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RcCalibrationTransition {
    step: u8,
    result: u8,
}

impl RcCalibrationTransition {
    pub const fn new() -> Self {
        Self { step: 0, result: 0 }
    }

    pub const fn action(self) -> RcCalibrationAction {
        match self.step {
            0 => RcCalibrationAction::WriteMasked {
                field: analog_registers::RC_CALIBRATION_DOUT_PATH_ENABLE,
                value: 1,
            },
            1 => RcCalibrationAction::WriteMasked {
                field: analog_registers::RC_CALIBRATION_ENABLE,
                // ROM `phy_get_rc_dout` asserts the RC-calibration enable
                // before pulsing bit 1. Leaving this clear makes the result
                // register stay at zero and poisons every derived RX filter
                // code in PHY-I2C block 0x67.
                value: 1,
            },
            2 => RcCalibrationAction::WriteMasked {
                field: analog_registers::RC_CALIBRATION_PULSE,
                value: 0,
            },
            3 => RcCalibrationAction::WriteMasked {
                field: analog_registers::RC_CALIBRATION_PULSE,
                value: 1,
            },
            4 => RcCalibrationAction::DelayMicros(100),
            5 => RcCalibrationAction::ReadMasked {
                field: analog_registers::RC_CALIBRATION_RESULT,
            },
            6 => RcCalibrationAction::WriteMasked {
                field: analog_registers::RC_CALIBRATION_DOUT_PATH_ENABLE,
                value: 0,
            },
            7 => RcCalibrationAction::WriteMasked {
                field: analog_registers::RC_CALIBRATION_ENABLE,
                value: 0,
            },
            8 => RcCalibrationAction::ApplyResult(self.result),
            _ => RcCalibrationAction::Complete,
        }
    }

    pub fn advance(
        &mut self,
        completion: RcCalibrationCompletion,
    ) -> Result<(), RcCalibrationTransitionError> {
        let matches = matches!(
            (self.action(), completion),
            (
                RcCalibrationAction::WriteMasked { .. },
                RcCalibrationCompletion::Write
            ) | (
                RcCalibrationAction::DelayMicros(_),
                RcCalibrationCompletion::Delay
            ) | (
                RcCalibrationAction::ReadMasked { .. },
                RcCalibrationCompletion::Read(_)
            ) | (
                RcCalibrationAction::ApplyResult(_),
                RcCalibrationCompletion::Applied
            )
        );
        if !matches {
            return if self.action() == RcCalibrationAction::Complete {
                Err(RcCalibrationTransitionError::AlreadyComplete)
            } else {
                Err(RcCalibrationTransitionError::WrongCompletion)
            };
        }
        if let RcCalibrationCompletion::Read(value) = completion {
            self.result = value;
        }
        self.step += 1;
        Ok(())
    }
}

impl Default for RcCalibrationTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
