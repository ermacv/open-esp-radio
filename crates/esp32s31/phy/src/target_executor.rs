//! ESP32-S31 target executors for finite PHY hardware edges.
//!
//! The recovered PHY transitions describe what must happen, while this module
//! owns the common polling contract for the target. Time is the caller's
//! [`oer_time::Timer`], so neither Embassy nor an RTOS becomes a dependency of
//! the PHY crate; short settles are a [`PhyShortDelay`].

use crate::executor::wait::{Kind, PhyShortDelay, delay};
use oer_time::Timer;

use core::future::Future;

use crate::{
    HARDWARE_EDGE_LIMIT,
    analog::{
        dcode::{PhyDcodeCompletion, PhyDcodeI2cBinding},
        i2c::MaskedI2cWriteBinding,
        pbus::PhyPbusHardwareObservation,
        rfpll::{RfpllFrequencyCompletion, RfpllFrequencyI2cBinding},
        temperature::{PhyTemperatureCompletion, PhyTemperatureI2cBinding},
    },
    calibration::{
        bluetooth::{
            PhyBluetoothI2cAction, PhyBluetoothI2cBinding, PhyBluetoothPbusBinding,
            PhyBluetoothTxPowerCompletion,
        },
        cold::{
            PhyColdI2cAction, PhyColdI2cConfigurationBinding, PhyColdI2cError,
            PhyColdI2cObservation,
        },
        registration::{PhyRegisterCompletion, PhyRegisterFinalI2cBinding},
    },
    channel::{PhyChipChannelCompletion, PhyChipChannelI2cBinding},
    rx::{
        dc_offset::{PhyRxDcoCompletion, PhyRxDcoPbusBinding},
        gain::{PhyRxGainPublishCompletion, PhyRxGainPublishPbusBinding},
        gain_calibration::{
            PhyRxDcCalibrationCompletion, PhyRxDcCalibrationPbusBinding, PhyRxGainDcCompletion,
            PhyRxGainDcPbusBinding,
        },
        iq::{
            PhyRxIqAdjustedTxCompletion, PhyRxIqAdjustedTxI2cBinding, PhyRxIqGainCompletion,
            PhyRxIqGainI2cBinding, PhyRxIqGainPbusBinding, PhyRxIqInitCompletion,
            PhyRxIqInitI2cBinding, PhyRxIqInitPbusBinding,
        },
        saturation::{PhyRxSaturationCompletion, PhyRxSaturationPbusBinding},
    },
    tx::{
        calibration::{
            PhyTxCalibrationEnvironmentCompletion, PhyTxCalibrationEnvironmentPbusBinding,
        },
        dc_power_detector::{
            PhyTxDcPwdetCompletion, PhyTxDcPwdetPbusBinding, PhyTxDcPwdetSearchCompletion,
            PhyTxDcPwdetSearchPbusBinding,
        },
        iq::{
            PhyTxIqCalibrationCompletion, PhyTxIqInitCompletion, PhyTxIqInitI2cBinding,
            PhyTxIqPbusBinding,
        },
        power::{PhyTxPowerCompletion, PhyTxPowerI2cBinding},
    },
};

use oer_esp32s31_hal::owner::SharedPhyAccess;

/// Execute one complete rev0 PBus force command with the vendor's direct
/// status polling shape. The finite observation ceiling is OER's fail-closed
/// extension; no executor edge exists inside this transaction.
#[cfg(target_arch = "riscv32")]
#[inline]
pub(crate) fn force_pbus_direct(
    registers: &mut impl SharedPhyAccess,
    transaction: crate::analog::pbus::PhyPbusForceTest,
) -> bool {
    oer_esp32s31_hal::phy::pbus::start_force_test(
        registers,
        transaction.selector(),
        transaction.path(),
        transaction.value(),
    );
    crate::executor::wait::poll::bounded::<core::convert::Infallible>(|| {
        Ok(oer_esp32s31_hal::phy::pbus::try_finish_force_test(registers).is_ok())
    })
    .unwrap_or_else(|never| match never {})
}

/// Complete one byte read through the PHY analog-I2C host without creating a
/// transition or an async completion for each busy observation.
#[cfg(target_arch = "riscv32")]
#[inline]
pub(crate) fn read_i2c_direct(
    registers: &mut impl SharedPhyAccess,
    address: oer_esp32s31_hal::phy::i2c::PhyI2cAddress,
) -> Result<u8, PhyTargetPortError> {
    let started = crate::executor::wait::poll::bounded::<core::convert::Infallible>(|| {
        Ok(crate::analog::i2c::try_start_read(registers, address).is_ok())
    })
    .unwrap_or_else(|never| match never {});
    if !started {
        return Err(PhyTargetPortError::HardwareEdgeTimedOut);
    }
    for _ in 0..HARDWARE_EDGE_LIMIT {
        match crate::analog::i2c::try_finish_read(registers, address) {
            Ok(value) => return Ok(value),
            Err(crate::analog::i2c::PhyI2cError::Busy) => {}
        }
    }
    Err(PhyTargetPortError::HardwareEdgeTimedOut)
}

/// Complete one byte write through the PHY analog-I2C host using the same
/// direct busy polling as the vendor leaf.
#[cfg(target_arch = "riscv32")]
#[inline]
pub(crate) fn write_i2c_direct(
    registers: &mut impl SharedPhyAccess,
    address: oer_esp32s31_hal::phy::i2c::PhyI2cAddress,
    value: u8,
) -> Result<(), PhyTargetPortError> {
    let started = crate::executor::wait::poll::bounded::<core::convert::Infallible>(|| {
        Ok(crate::analog::i2c::try_start_write(registers, address, value).is_ok())
    })
    .unwrap_or_else(|never| match never {});
    if !started {
        return Err(PhyTargetPortError::HardwareEdgeTimedOut);
    }
    let completed = crate::executor::wait::poll::bounded::<core::convert::Infallible>(|| {
        Ok(crate::analog::i2c::try_finish_write(registers, address).is_ok())
    })
    .unwrap_or_else(|never| match never {});
    if completed {
        Ok(())
    } else {
        Err(PhyTargetPortError::HardwareEdgeTimedOut)
    }
}

/// Read, replace and write one reviewed analog-I2C field as one blocking
/// vendor-shaped operation.
#[cfg(target_arch = "riscv32")]
#[inline]
pub(crate) fn write_i2c_field_direct(
    registers: &mut impl SharedPhyAccess,
    field: oer_esp32s31_hal::phy::i2c::PhyI2cField,
    value: u8,
) -> Result<(), PhyTargetPortError> {
    let current = read_i2c_direct(registers, field.address())?;
    write_i2c_direct(registers, field.address(), field.replace(current, value))
}

/// Execute the four masked analog-I²C writes of complete ROM
/// `phy_reset_ckgen` with direct finite polling.
#[cfg(target_arch = "riscv32")]
pub(crate) fn reset_clock_generator_direct(
    registers: &mut impl SharedPhyAccess,
) -> Result<(), PhyTargetPortError> {
    use oer_esp32s31_hal::phy::i2c::analog_registers;

    for (field, value) in [
        (analog_registers::RFPLL_DCODE_0_SOURCE_SELECT, 0),
        (analog_registers::RFPLL_DCODE_1_SOURCE_SELECT, 0),
        (analog_registers::RFPLL_DCODE_CKGEN_RESET, 0),
        (analog_registers::RFPLL_DCODE_CKGEN_RESET, 1),
    ] {
        write_i2c_field_direct(registers, field, value)?;
    }
    Ok(())
}

/// Finite allowance shared by nested work in one direct PHY transaction.
///
/// The budget counts typed hardware operations reported by a completed child;
/// it is not a wall-clock deadline. A child receives only the remaining
/// allowance and must leave hardware untouched when that allowance cannot fit
/// one complete operation envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DirectOperationBudget {
    remaining: u32,
}

impl DirectOperationBudget {
    pub(crate) const fn new(limit: u32) -> Self {
        Self { remaining: limit }
    }

    pub(crate) const fn remaining(&self) -> u32 {
        self.remaining
    }

    pub(crate) fn consume(&mut self, operations: u32) -> bool {
        let Some(remaining) = self.remaining.checked_sub(operations) else {
            return false;
        };
        self.remaining = remaining;
        true
    }
}

impl PhyShortDelay for oer_esp32s31_hal::phy::delay::RomShortDelay {
    const MAX_MICROS: u32 = oer_esp32s31_hal::phy::delay::RomShortDelay::MAX_MICROS;

    fn settle_micros(micros: u32) -> bool {
        oer_esp32s31_hal::phy::delay::RomShortDelay::settle_micros(micros)
    }
}

/// Failure while completing a finite target PHY hardware operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTargetPortError {
    HardwareEdgeTimedOut,
    HardwareCapabilityUnavailable,
    HardwareInvariant,
    RfOperationLimit,
    UnexpectedBinding,
    /// A registration held apart from its hardware no longer describes the
    /// borrowed PHY partition; the operation touched no hardware.
    RegistrationEpochMismatch,
}

/// Complete the registration tail's bounded read-only PHY-I2C transaction.
///
/// This leaf is deliberately distinct from the general I2C executor: the
/// recovered registration tail permits only a read, and exhausting the
/// deadline produces a typed transition completion rather than an executor
/// error. Applications must not reinterpret a write action or select another
/// polling bound at this boundary.
pub async fn complete_final_i2c<D: PhyShortDelay>(
    timer: &impl Timer,
    mut binding: PhyRegisterFinalI2cBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<PhyRegisterCompletion, PhyTargetPortError> {
    for _ in 0..HARDWARE_EDGE_LIMIT {
        match binding.action() {
            PhyColdI2cAction::StartRead { .. } => match binding.start_target(registers) {
                Ok(()) => {}
                Err(PhyColdI2cError::BusyAtStart) => delay::<D, _>(timer, Kind::BusBusy, 1).await,
                Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
            },
            PhyColdI2cAction::AwaitReadCompletionEdge { .. } => {
                delay::<D, _>(timer, Kind::Completion, 1).await;
                match binding
                    .observe_target_edge(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                {
                    PhyColdI2cObservation::EdgeConsumed | PhyColdI2cObservation::StillPending => {}
                }
            }
            PhyColdI2cAction::Complete(_) => {
                return binding
                    .into_completion()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding);
            }
            PhyColdI2cAction::StartWrite { .. }
            | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                return Err(PhyTargetPortError::UnexpectedBinding);
            }
        }
    }
    Ok(binding.into_deadline_completion())
}

macro_rules! define_i2c_executor {
    ($function:ident, $binding:ty, $completion:ty) => {
        pub async fn $function<F: Future<Output = ()>>(
            mut binding: $binding,
            registers: &mut impl SharedPhyAccess,
            mut delay: impl FnMut(crate::executor::wait::Kind, u64) -> F,
        ) -> Result<$completion, PhyTargetPortError> {
            for _ in 0..HARDWARE_EDGE_LIMIT {
                match binding.action() {
                    PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => {
                        match binding.start_target(registers) {
                            Ok(()) => {}
                            Err(PhyColdI2cError::BusyAtStart) => {
                                delay(crate::executor::wait::Kind::BusBusy, 1).await
                            }
                            Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                        }
                    }
                    PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                    | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                        delay(crate::executor::wait::Kind::Completion, 1).await;
                        match binding
                            .observe_target_edge(registers)
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                        {
                            PhyColdI2cObservation::EdgeConsumed
                            | PhyColdI2cObservation::StillPending => {}
                        }
                    }
                    PhyColdI2cAction::Complete(_) => {
                        return binding
                            .into_completion()
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                    }
                }
            }
            Err(PhyTargetPortError::HardwareEdgeTimedOut)
        }
    };
}

/// RX gain I2C command loop, using the immediate status observations in rev0
/// phy_chip_i2c_readReg_org / phy_chip_i2c_writeReg. No settle is specified
/// between command publication and status reads. Preserve the old total edge
/// budget (including publication and terminal recognition), not a new deadline.
pub fn complete_rx_gain_i2c(
    mut binding: MaskedI2cWriteBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<oer_radio_analog::Completion<crate::analog::i2c::PhyI2cAddress>, PhyTargetPortError> {
    let completed = crate::executor::wait::poll::bounded(|| {
        match binding.action() {
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
            PhyColdI2cAction::Complete(_) => return Ok(true),
        }
        Ok(false)
    })?;
    if !completed {
        return Err(PhyTargetPortError::HardwareEdgeTimedOut);
    }
    binding
        .into_completion()
        .map_err(|_| PhyTargetPortError::UnexpectedBinding)
}

/// Complete an admitted RFPLL I2C command with the ROM's immediate status
/// polling. Publication, retries and terminal recognition share the existing
/// finite edge budget. Bus status reads have no mandatory settling interval;
/// analog settling remains an explicit responsibility of the caller.
pub(crate) fn complete_rfpll_i2c_direct(
    mut binding: crate::analog::rfpll::RfpllFrequencyI2cBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::analog::rfpll::RfpllFrequencyCompletion, PhyTargetPortError> {
    use crate::calibration::cold::{PhyColdI2cAction, PhyColdI2cError};
    let completed = crate::executor::wait::poll::bounded(|| {
        match binding.action() {
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
            PhyColdI2cAction::Complete(_) => return Ok(true),
        }
        Ok(false)
    })?;
    if !completed {
        return Err(PhyTargetPortError::HardwareEdgeTimedOut);
    }
    binding
        .into_completion()
        .map_err(|_| PhyTargetPortError::UnexpectedBinding)
}

macro_rules! define_pbus_executor {
    ($function:ident, $binding:ty, $completion:ty) => {
        pub async fn $function<D: PhyShortDelay>(
            timer: &impl Timer,
            mut binding: $binding,
            registers: &mut impl SharedPhyAccess,
        ) -> Result<$completion, PhyTargetPortError> {
            let mut started = false;
            for _ in 0..HARDWARE_EDGE_LIMIT {
                if binding.start_target(registers).is_ok() {
                    started = true;
                    break;
                }
                delay::<D, _>(timer, Kind::BusBusy, 1).await;
            }
            if !started {
                return Err(PhyTargetPortError::HardwareEdgeTimedOut);
            }
            for _ in 0..HARDWARE_EDGE_LIMIT {
                delay::<D, _>(timer, Kind::Completion, 1).await;
                match binding
                    .observe_target_edge(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                {
                    PhyPbusHardwareObservation::EdgeConsumed => {
                        return binding
                            .into_completion()
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                    }
                    PhyPbusHardwareObservation::StillPending => {}
                }
            }
            Err(PhyTargetPortError::HardwareEdgeTimedOut)
        }
    };
}

// These bindings model a PBus timeout as an ordinary completion consumed by
// their parent transition. Keep the finite wait and the timeout conversion in
// the driver: an application must not be able to silently select a different
// polling bound or turn the recovered fallback path into an executor error.
macro_rules! define_timeout_pbus_executor {
    ($function:ident, $binding:ty, $completion:ty) => {
        pub async fn $function<F: Future<Output = ()>>(
            mut binding: $binding,
            registers: &mut impl SharedPhyAccess,
            mut delay: impl FnMut(crate::executor::wait::Kind, u64) -> F,
        ) -> Result<$completion, PhyTargetPortError> {
            let mut started = false;
            for _ in 0..HARDWARE_EDGE_LIMIT {
                if binding.start_target(registers).is_ok() {
                    started = true;
                    break;
                }
                delay(crate::executor::wait::Kind::BusBusy, 1).await;
            }
            if !started {
                return Ok(binding.into_timeout_completion());
            }

            for _ in 0..HARDWARE_EDGE_LIMIT {
                delay(crate::executor::wait::Kind::Completion, 1).await;
                match binding
                    .observe_target_edge(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                {
                    PhyPbusHardwareObservation::EdgeConsumed => {
                        return binding
                            .into_completion()
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                    }
                    PhyPbusHardwareObservation::StillPending => {}
                }
            }
            Ok(binding.into_timeout_completion())
        }
    };
}

// RX gain uses the direct rev0 phy_pbus_force_test completion loop
// (command 0x2f82424e, status 0x2f824256). There is no intervening settle.
// Keep the same finite per-phase attempt bound and parent timeout completion.
macro_rules! define_direct_timeout_pbus_executor {
    ($function:ident, $binding:ty, $completion:ty) => {
        pub fn $function(
            mut binding: $binding,
            registers: &mut impl SharedPhyAccess,
        ) -> Result<$completion, PhyTargetPortError> {
            use crate::executor::wait::poll::bounded;
            let started = bounded::<core::convert::Infallible>(|| {
                Ok(binding.start_target(registers).is_ok())
            })
            .unwrap_or_else(|never| match never {});
            if !started {
                return Ok(binding.into_timeout_completion());
            }
            let completed = bounded(|| {
                binding
                    .observe_target_edge(registers)
                    .map(|edge| edge == PhyPbusHardwareObservation::EdgeConsumed)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)
            })?;
            if !completed {
                return Ok(binding.into_timeout_completion());
            }
            binding
                .into_completion()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)
        }
    };
}

define_i2c_executor!(
    complete_rfpll_i2c,
    RfpllFrequencyI2cBinding,
    RfpllFrequencyCompletion
);
define_i2c_executor!(
    complete_masked_i2c,
    MaskedI2cWriteBinding,
    oer_radio_analog::Completion<crate::analog::i2c::PhyI2cAddress>
);
define_i2c_executor!(
    complete_temperature_i2c,
    PhyTemperatureI2cBinding,
    PhyTemperatureCompletion
);
define_i2c_executor!(
    complete_tx_power_i2c,
    PhyTxPowerI2cBinding,
    PhyTxPowerCompletion
);
pub async fn complete_bluetooth_i2c<D: PhyShortDelay>(
    timer: &impl Timer,
    mut binding: PhyBluetoothI2cBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<PhyBluetoothTxPowerCompletion, PhyTargetPortError> {
    for _ in 0..HARDWARE_EDGE_LIMIT {
        match binding.action() {
            PhyBluetoothI2cAction::StartCommand => match binding.start_target(registers) {
                Ok(()) => {}
                Err(PhyColdI2cError::BusyAtStart) => delay::<D, _>(timer, Kind::BusBusy, 1).await,
                Err(_) => return Err(PhyTargetPortError::HardwareInvariant),
            },
            PhyBluetoothI2cAction::AwaitCompletionEdge => {
                delay::<D, _>(timer, Kind::Completion, 1).await;
                match binding
                    .observe_target_edge(registers)
                    .map_err(|_| PhyTargetPortError::HardwareInvariant)?
                {
                    PhyColdI2cObservation::EdgeConsumed | PhyColdI2cObservation::StillPending => {}
                }
            }
            PhyBluetoothI2cAction::Complete => {
                return binding
                    .into_completion()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding);
            }
        }
    }
    Err(PhyTargetPortError::HardwareEdgeTimedOut)
}

pub async fn complete_i2c_configuration<D: PhyShortDelay>(
    timer: &impl Timer,
    mut binding: PhyColdI2cConfigurationBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::analog::i2c::PhyRfInitPrefixCompletion, PhyTargetPortError> {
    for _ in 0..HARDWARE_EDGE_LIMIT {
        match binding.action() {
            oer_esp32s31_hal::phy::i2c::PhyI2cConfigurationAction::StartCommand => {
                match binding.start_target(registers) {
                    Ok(()) => {}
                    Err(PhyColdI2cError::BusyAtStart) => {
                        delay::<D, _>(timer, Kind::BusBusy, 1).await
                    }
                    Err(_) => return Err(PhyTargetPortError::HardwareInvariant),
                }
            }
            oer_esp32s31_hal::phy::i2c::PhyI2cConfigurationAction::AwaitCompletionEdge => {
                delay::<D, _>(timer, Kind::Completion, 1).await;
                match binding
                    .observe_target_edge(registers)
                    .map_err(|_| PhyTargetPortError::HardwareInvariant)?
                {
                    PhyColdI2cObservation::EdgeConsumed | PhyColdI2cObservation::StillPending => {}
                }
            }
            oer_esp32s31_hal::phy::i2c::PhyI2cConfigurationAction::Complete => {
                return binding
                    .into_completion()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding);
            }
        }
    }
    Err(PhyTargetPortError::HardwareEdgeTimedOut)
}
define_i2c_executor!(complete_dcode_i2c, PhyDcodeI2cBinding, PhyDcodeCompletion);
define_i2c_executor!(
    complete_txiq_init_i2c,
    PhyTxIqInitI2cBinding,
    PhyTxIqInitCompletion
);
define_i2c_executor!(
    complete_rxiq_adjusted_tx_i2c,
    PhyRxIqAdjustedTxI2cBinding,
    PhyRxIqAdjustedTxCompletion
);
define_i2c_executor!(
    complete_rxiq_gain_i2c,
    PhyRxIqGainI2cBinding,
    PhyRxIqGainCompletion
);
define_i2c_executor!(
    complete_rxiq_init_i2c,
    PhyRxIqInitI2cBinding,
    PhyRxIqInitCompletion
);
define_i2c_executor!(
    complete_channel_i2c,
    PhyChipChannelI2cBinding,
    PhyChipChannelCompletion
);
define_pbus_executor!(
    complete_tx_calibration_environment_pbus,
    PhyTxCalibrationEnvironmentPbusBinding,
    PhyTxCalibrationEnvironmentCompletion
);
define_timeout_pbus_executor!(
    complete_bluetooth_pbus,
    PhyBluetoothPbusBinding,
    PhyBluetoothTxPowerCompletion
);
define_timeout_pbus_executor!(
    complete_tx_dc_pwdet_search_pbus,
    PhyTxDcPwdetSearchPbusBinding,
    PhyTxDcPwdetSearchCompletion
);
define_timeout_pbus_executor!(
    complete_tx_dc_pwdet_pbus,
    PhyTxDcPwdetPbusBinding,
    PhyTxDcPwdetCompletion
);
define_timeout_pbus_executor!(
    complete_txiq_pbus,
    PhyTxIqPbusBinding,
    PhyTxIqCalibrationCompletion
);
define_timeout_pbus_executor!(
    complete_rx_dco_pbus,
    PhyRxDcoPbusBinding,
    PhyRxDcoCompletion
);
define_timeout_pbus_executor!(
    complete_rxiq_gain_pbus,
    PhyRxIqGainPbusBinding,
    PhyRxIqGainCompletion
);
define_timeout_pbus_executor!(
    complete_rxiq_init_pbus,
    PhyRxIqInitPbusBinding,
    PhyRxIqInitCompletion
);
define_timeout_pbus_executor!(
    complete_rx_saturation_pbus,
    PhyRxSaturationPbusBinding,
    PhyRxSaturationCompletion
);
define_direct_timeout_pbus_executor!(
    complete_rx_dc_calibration_pbus,
    PhyRxDcCalibrationPbusBinding,
    PhyRxDcCalibrationCompletion
);
define_direct_timeout_pbus_executor!(
    complete_rx_gain_dc_pbus,
    PhyRxGainDcPbusBinding,
    PhyRxGainDcCompletion
);
define_direct_timeout_pbus_executor!(
    complete_rx_gain_publish_pbus,
    PhyRxGainPublishPbusBinding,
    PhyRxGainPublishCompletion
);

#[cfg(test)]
mod direct_operation_budget_tests {
    use super::DirectOperationBudget;

    #[test]
    fn shared_budget_rejects_overrun_without_mutation() {
        let mut budget = DirectOperationBudget::new(10);

        assert_eq!(budget.remaining(), 10);
        assert!(budget.consume(4));
        assert_eq!(budget.remaining(), 6);
        assert!(!budget.consume(7));
        assert_eq!(budget.remaining(), 6);
        assert!(budget.consume(6));
        assert_eq!(budget.remaining(), 0);
    }
}
