//! Target execution for whole-PHY close and retained-wake transactions.

use super::*;
use crate::lifecycle::{PhyRfCloseOperation, drive_rf_close};

/// Why the current-vendor temperature observation that immediately precedes
/// RF close failed.
///
/// Failure precedes every shutdown mutation, so the caller may retain and
/// retry its powered-idle owner. The observation updates only source-owned
/// temperature state and acquisition provenance.
pub(crate) enum PhyRfCloseTemperatureFailure {
    /// The transition rejected state after every issued hardware transaction
    /// had completed, so the powered-idle owner remains usable.
    Recoverable(PhyTargetPortError),
    /// An I2C command may still be active or partially observed. Reusing the
    /// physical epoch without reset would invent a completion boundary.
    HardwareAmbiguous(PhyTargetPortError),
}

const fn preclose_i2c_failure(error: PhyTargetPortError) -> PhyRfCloseTemperatureFailure {
    PhyRfCloseTemperatureFailure::HardwareAmbiguous(error)
}

pub(super) async fn observe_temperature_with_hal<P, D: PhyShortDelay>(
    timer: &impl oer_time::Timer,
    platform: &mut P,
    registers: &mut impl SharedPhyAccess,
    state: &mut PhyState,
) -> Result<(), PhyRfCloseTemperatureFailure> {
    let started = Some(timer.now().as_micros());
    let mut transition = PhyTemperatureTransition::new();
    for _ in 0..RF_OPERATION_LIMIT {
        match transition.action() {
            PhyTemperatureAction::Complete(outcome) => {
                state.apply_observed_temperature_outcome(
                    outcome,
                    started,
                    Some(timer.now().as_micros()),
                );
                return Ok(());
            }
            PhyTemperatureAction::Failed(_) => {
                return Err(PhyRfCloseTemperatureFailure::Recoverable(
                    PhyTargetPortError::HardwareInvariant,
                ));
            }
            action => {
                let binding = PhyTemperatureExternalBinding::lower(action).map_err(|_| {
                    PhyRfCloseTemperatureFailure::Recoverable(PhyTargetPortError::UnexpectedBinding)
                })?;
                let completion = TargetCompleter::<D, _>::new(timer)
                    .complete_temperature(binding, platform, registers)
                    .await
                    .map_err(preclose_i2c_failure)?;
                transition.advance(completion).map_err(|_| {
                    PhyRfCloseTemperatureFailure::Recoverable(PhyTargetPortError::UnexpectedBinding)
                })?;
            }
        }
    }
    Err(PhyRfCloseTemperatureFailure::Recoverable(
        PhyTargetPortError::RfOperationLimit,
    ))
}

/// Execute the exact finite current-vendor RF-close graph after preflight.
///
/// The first operation crosses the point of no recovery. Any returned error
/// means the powered hardware epoch is ambiguous and must remain poisoned.
pub(super) fn execute_rf_close_with_hal<D: PhyShortDelay>(
    registers: &mut impl SharedPhyAccess,
) -> Result<(), PhyTargetPortError> {
    drive_rf_close(|operation| {
        match operation {
            // The current ESP32-S31 library installs empty critical-section
            // callbacks. Preserve the boundaries in the graph without
            // inventing a CPU interrupt lock as an RF ownership proof.
            PhyRfCloseOperation::EnterCritical | PhyRfCloseOperation::ExitCritical => {}
            PhyRfCloseOperation::DisableHardwareFrequencyControl => {
                oer_esp32s31_hal::phy::frequency::set_hardware_control(registers, false);
            }
            PhyRfCloseOperation::SettleOneMicrosecond => {
                if !D::settle_micros(1) {
                    return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
                }
            }
            PhyRfCloseOperation::DisableAgc => {
                oer_esp32s31_hal::phy::agc::set_enabled(registers, false);
            }
            PhyRfCloseOperation::ClearBasebandControl => {
                oer_esp32s31_hal::phy::clock::clear_rf_baseband_control(registers);
            }
            PhyRfCloseOperation::WriteI2c { address, value } => {
                write_i2c_direct(registers, address, value)?;
            }
            PhyRfCloseOperation::PowerOffRfCircuits => {
                oer_esp32s31_hal::phy::analog_i2c::power_off_rf_circuits(registers);
            }
            PhyRfCloseOperation::ClearImmediateClockPower => {
                oer_esp32s31_hal::phy::clock::clear_rf_immediate_clock_power(registers);
            }
            PhyRfCloseOperation::CloseFrontendBasebandClocks => {
                oer_esp32s31_hal::phy::clock::close_frontend_baseband(registers);
            }
            PhyRfCloseOperation::EnableBbpllCalibration => {
                oer_esp32s31_hal::phy::i2c::configure_bbpll_calibration(registers, true);
            }
        }
        Ok(())
    })
}

#[cfg(target_arch = "riscv32")]
async fn drive_wake_i2c_configuration<D: PhyShortDelay>(
    timer: &impl oer_time::Timer,
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    operation: oer_esp32s31_hal::phy::i2c::PhyI2cConfigurationOperation,
) -> Result<(), PhyTargetPortError> {
    use oer_esp32s31_hal::phy::i2c::{
        PhyI2cConfigurationAction, PhyI2cConfigurationError, PhyI2cConfigurationObservation,
        PhyI2cConfigurationTransaction,
    };

    let mut transaction = PhyI2cConfigurationTransaction::new(operation);
    for _ in 0..RF_OPERATION_LIMIT {
        match transaction.action() {
            PhyI2cConfigurationAction::StartCommand => {
                match oer_esp32s31_hal::phy::i2c::start_configuration(&mut transaction, registers) {
                    Ok(()) => {}
                    Err(PhyI2cConfigurationError::BusyAtStart) => {
                        delay::<D, _>(timer, Kind::BusBusy, 1).await;
                    }
                    Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                }
            }
            PhyI2cConfigurationAction::AwaitCompletionEdge => {
                delay::<D, _>(timer, Kind::Completion, 1).await;
                match oer_esp32s31_hal::phy::i2c::observe_configuration(&mut transaction, registers)
                {
                    Ok(PhyI2cConfigurationObservation::StillPending)
                    | Ok(PhyI2cConfigurationObservation::EdgeConsumed) => {}
                    Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                }
            }
            PhyI2cConfigurationAction::Complete => return Ok(()),
        }
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn reset_wake_i2c_master(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
) -> Result<(), PhyTargetPortError> {
    use oer_esp32s31_hal::phy::i2c::{PhyI2cHost, pulse_master_reset, sample_master_reset_busy};

    for host in [PhyI2cHost::Host0, PhyI2cHost::Host1] {
        if !sample_master_reset_busy(registers, host) {
            continue;
        }
        pulse_master_reset(registers, host);
        let idle = crate::executor::wait::poll::bounded::<core::convert::Infallible>(|| {
            Ok(!sample_master_reset_busy(registers, host))
        })
        .unwrap_or_else(|never| match never {});
        if !idle {
            return Err(PhyTargetPortError::HardwareEdgeTimedOut);
        }
    }
    Ok(())
}

#[cfg(target_arch = "riscv32")]
fn force_wake_txrx<D: PhyShortDelay>(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    enabled: bool,
) -> Result<(), PhyTargetPortError> {
    for phase in 0..2 {
        oer_esp32s31_hal::phy::pbus::configure_force_txrx(registers, enabled, phase);
        if !D::settle_micros(1) {
            return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
        }
    }
    Ok(())
}

#[cfg(target_arch = "riscv32")]
fn open_wake_i2c_power(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
) -> Result<(), PhyTargetPortError> {
    use crate::analog::i2c::{
        OpenI2cXpdAction, OpenI2cXpdCompletion, OpenI2cXpdOutcome, OpenI2cXpdTransition,
    };
    let mut transition = OpenI2cXpdTransition::new(false);
    for _ in 0..RF_OPERATION_LIMIT {
        let completion = match transition.action() {
            OpenI2cXpdAction::ConfigurePowerAndPulse => {
                crate::analog::i2c::configure_open_i2c_power_and_pulse(registers);
                OpenI2cXpdCompletion::PowerAndPulseConfigured {
                    started_at_cycle: oer_esp32s31_hal::phy::prelude::sample_sdm_deadline_counter(
                        registers,
                    ),
                }
            }
            OpenI2cXpdAction::CheckSdmDeadline {
                started_at_cycle,
                maximum_cycles,
            } => {
                let current =
                    oer_esp32s31_hal::phy::prelude::sample_sdm_deadline_counter(registers);
                OpenI2cXpdCompletion::DeadlineObserved {
                    expired: current.wrapping_sub(started_at_cycle) > maximum_cycles,
                }
            }
            OpenI2cXpdAction::ReadSdmSample { address } => OpenI2cXpdCompletion::SdmSample(
                crate::target_executor::read_i2c_direct(registers, address)?,
            ),
            OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::Stable) => return Ok(()),
            OpenI2cXpdAction::Complete(OpenI2cXpdOutcome::TimedOut) => {
                return Err(PhyTargetPortError::HardwareEdgeTimedOut);
            }
            OpenI2cXpdAction::ConfigurePreDelay | OpenI2cXpdAction::DelayMicros(_) => {
                return Err(PhyTargetPortError::UnexpectedBinding);
            }
        };
        transition
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

#[cfg(target_arch = "riscv32")]
fn clear_wake_pbus<D: PhyShortDelay>(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
) -> Result<(), PhyTargetPortError> {
    use crate::analog::pbus::{
        PhyPbusClearAction, PhyPbusClearCompletion, PhyPbusClearOutcome, PhyPbusClearTransition,
    };
    let mut transition = PhyPbusClearTransition::new();
    for _ in 0..RF_OPERATION_LIMIT {
        let completion = match transition.action() {
            PhyPbusClearAction::ConfigureDebugMode => {
                oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
                PhyPbusClearCompletion::DebugModeConfigured
            }
            PhyPbusClearAction::ForceTest(transaction) => {
                if !crate::target_executor::force_pbus_direct(registers, transaction) {
                    return Err(PhyTargetPortError::HardwareEdgeTimedOut);
                }
                PhyPbusClearCompletion::ForceTestCompleted(transaction)
            }
            PhyPbusClearAction::ConfigureWorkMode => {
                let settle_required = oer_esp32s31_hal::phy::pbus::configure_work_mode(registers);
                PhyPbusClearCompletion::WorkModeConfigured { settle_required }
            }
            PhyPbusClearAction::DelayMicros(micros) => {
                if !D::settle_micros(micros) {
                    return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
                }
                PhyPbusClearCompletion::DelayElapsed
            }
            PhyPbusClearAction::ConfigureWorkModePulse => {
                oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
                PhyPbusClearCompletion::WorkModePulseConfigured
            }
            PhyPbusClearAction::ClearWorkModePulse => {
                oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
                PhyPbusClearCompletion::WorkModePulseCleared
            }
            PhyPbusClearAction::Complete(PhyPbusClearOutcome::Cleared) => return Ok(()),
            PhyPbusClearAction::Complete(PhyPbusClearOutcome::ForceTestTimedOut(_)) => {
                return Err(PhyTargetPortError::HardwareEdgeTimedOut);
            }
        };
        transition
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

#[cfg(target_arch = "riscv32")]
fn publish_wake_frequency_i2c(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    front_end_parameter_bit: bool,
) -> Result<(), PhyTargetPortError> {
    use crate::analog::frequency::{
        PhyFrequencyI2cAction, PhyFrequencyI2cCompletion, PhyFrequencyI2cRequest,
        PhyFrequencyI2cTransition,
    };
    let mut transition = PhyFrequencyI2cTransition::new(PhyFrequencyI2cRequest::retained_wake(
        front_end_parameter_bit,
    ));
    for _ in 0..RF_OPERATION_LIMIT {
        let completion = match transition.action() {
            PhyFrequencyI2cAction::WriteMasked { field, value } => {
                crate::target_executor::write_i2c_field_direct(registers, field, value)?;
                PhyFrequencyI2cCompletion::MaskedWrite { field }
            }
            PhyFrequencyI2cAction::ReadByte { address } => {
                let value = crate::target_executor::read_i2c_direct(registers, address)?;
                PhyFrequencyI2cCompletion::ByteRead { address, value }
            }
            PhyFrequencyI2cAction::ConfigureNumberAddresses(addresses) => {
                oer_esp32s31_hal::phy::frequency::configure_i2c_number_addresses(
                    registers, addresses,
                );
                PhyFrequencyI2cCompletion::NumberAddressesConfigured(addresses)
            }
            PhyFrequencyI2cAction::Complete(_) => return Ok(()),
            PhyFrequencyI2cAction::WriteMemory { .. } => {
                return Err(PhyTargetPortError::UnexpectedBinding);
            }
        };
        transition
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

#[cfg(target_arch = "riscv32")]
fn restore_wake_pbus_boundaries(
    registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
) -> Result<(), PhyTargetPortError> {
    use crate::analog::pbus::memory::{
        PhyPbusBoundaryRestoreAction, PhyPbusBoundaryRestoreMmioBinding,
        PhyPbusBoundaryRestoreTransition,
    };
    let mut transition = PhyPbusBoundaryRestoreTransition::new();
    for _ in 0..RF_OPERATION_LIMIT {
        let action = transition.action();
        if matches!(action, PhyPbusBoundaryRestoreAction::Complete(_)) {
            return Ok(());
        }
        let completion = PhyPbusBoundaryRestoreMmioBinding::new(action)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
            .execute_target(registers)
            .map_err(|_| PhyTargetPortError::HardwareInvariant)?;
        transition
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

/// Execute current-vendor retained RF wake against the existing registered
/// calibration state, through any route's shared-PHY borrow.
///
/// The parent transition enforces exact child order. Every child is finite;
/// completion is published only after frequency control, BBPLL, force-TX/RX
/// and baseband mode have all returned to their operational values.
#[cfg(target_arch = "riscv32")]
pub(super) async fn execute_rf_wake_with_hal<D: PhyShortDelay>(
    timer: &impl oer_time::Timer,
    registers: &mut impl PhyInitializationAccess,
    state: &PhyState,
) -> Result<(), PhyTargetPortError> {
    use crate::lifecycle::{PhyRfWakeAction, PhyRfWakeCompletion, PhyRfWakeOperation};
    use oer_esp32s31_hal::phy::i2c::{PhyAdcRate, PhyI2cConfigurationOperation};

    let init_parameters = state.rf_init_parameter_snapshot();
    let channel_parameters = state.channel_parameters();
    let channel = state.current_wifi_channel();
    let cbw = state.current_wifi_channel_bandwidth();
    let frequency_index = crate::analog::frequency::phy_frequency_channel_index(channel);
    let frequency_mhz = crate::channel::channel_to_frequency(channel);
    let tx_cap = crate::channel::retained_tx_cap_value(channel, channel_parameters.tx_capacitance);
    let register_parameters = state.register_init_parameters();
    let frequency_control = state.channel_frequency_control();
    let mut transition = crate::lifecycle::PhyRfWakeTransition::new();

    for _ in 0..RF_OPERATION_LIMIT {
        let PhyRfWakeAction::Execute(operation) = transition.action() else {
            return Ok(());
        };
        {
            match operation {
                PhyRfWakeOperation::SetBasebandMode { mode } => {
                    oer_esp32s31_hal::phy::frequency::set_baseband_mode(registers, mode);
                }
                PhyRfWakeOperation::ForceTxRxOff { enabled } => {
                    force_wake_txrx::<D>(registers, enabled)?;
                }
                PhyRfWakeOperation::SetHardwareFrequencyControl { enabled } => {
                    oer_esp32s31_hal::phy::frequency::set_hardware_control(registers, enabled);
                }
                PhyRfWakeOperation::ResetI2cMaster => reset_wake_i2c_master(registers)?,
                PhyRfWakeOperation::OpenFrontendBasebandClocks => {
                    registers.open_frontend_baseband_internal_clocks();
                    oer_esp32s31_hal::phy::analog_i2c::enable_frontend_baseband_power(registers);
                }
                PhyRfWakeOperation::SetBbpllCalibration { enabled } => {
                    oer_esp32s31_hal::phy::i2c::configure_bbpll_calibration(registers, enabled);
                }
                PhyRfWakeOperation::ConfigureBiasRegisters => {
                    drive_wake_i2c_configuration::<D>(
                        timer,
                        registers,
                        PhyI2cConfigurationOperation::BiasRegisters,
                    )
                    .await?;
                }
                PhyRfWakeOperation::PowerTemperatureSensor => {
                    oer_esp32s31_hal::phy::temperature::power_up(registers);
                }
                PhyRfWakeOperation::ReadXtalFrequency => {
                    oer_esp32s31_hal::phy::prelude::configure_fixed_xtal_40mhz(registers);
                }
                PhyRfWakeOperation::OpenI2cPower => open_wake_i2c_power(registers)?,
                PhyRfWakeOperation::ClearPbusRegisters => clear_wake_pbus::<D>(registers)?,
                PhyRfWakeOperation::ConfigureI2cClock => {
                    oer_esp32s31_hal::phy::i2c::configure_clock_selection(registers);
                }
                PhyRfWakeOperation::ResetFrontendTxRx => {
                    oer_esp32s31_hal::phy::baseband::reset_frontend_txrx(registers);
                }
                PhyRfWakeOperation::ConfigureAdcRate => {
                    drive_wake_i2c_configuration::<D>(
                        timer,
                        registers,
                        PhyI2cConfigurationOperation::ConfigureAdcRate(PhyAdcRate::High),
                    )
                    .await?;
                    crate::hardware::configure_phy_adc_rate(registers, PhyAdcRate::High);
                }
                PhyRfWakeOperation::ConfigureI2cMasterRegisters => {
                    oer_esp32s31_hal::phy::i2c::configure_master_registers(registers);
                }
                PhyRfWakeOperation::ConfigureFrequencyRegisters => {
                    oer_esp32s31_hal::phy::frequency::initialize_registers(
                        registers,
                        frequency_control.frequency_register_parameter_override,
                    );
                }
                PhyRfWakeOperation::ConfigureFrontendRegisters => {
                    crate::hardware::configure_phy_front_end_registers(registers);
                }
                PhyRfWakeOperation::UpdateFrontendRegisters => {
                    crate::hardware::configure_phy_front_end_update(registers);
                }
                PhyRfWakeOperation::ConfigurePowerDetectorRegisters => {
                    oer_esp32s31_hal::phy::power_detector::initialize_registers(registers)
                        .map_err(|_| PhyTargetPortError::HardwareInvariant)?;
                }
                PhyRfWakeOperation::ConfigureI2cStageTwo => {
                    crate::analog::i2c::configure_i2c_initialization_stage_two(
                        registers,
                        init_parameters,
                        u32::from(HARDWARE_EDGE_LIMIT),
                    )
                    .map_err(|_| PhyTargetPortError::HardwareEdgeTimedOut)?;
                }
                PhyRfWakeOperation::PublishRetainedFrequencyI2c => {
                    publish_wake_frequency_i2c(
                        registers,
                        frequency_control.front_end_parameter_bit,
                    )?;
                }
                PhyRfWakeOperation::RestoreChannelFrequency => {
                    oer_esp32s31_hal::phy::frequency::start_channel_switch(
                        registers,
                        frequency_index,
                    );
                    if !D::settle_micros(1) {
                        return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
                    }
                    oer_esp32s31_hal::phy::frequency::clear_channel_switch(registers);
                }
                PhyRfWakeOperation::RestorePbusBoundaries => {
                    restore_wake_pbus_boundaries(registers)?;
                }
                PhyRfWakeOperation::ConfigurePhyRegisters => {
                    crate::hardware::configure_phy_registers(registers, register_parameters);
                }
                PhyRfWakeOperation::UpdatePhyRegisters => {
                    oer_esp32s31_hal::phy::agc::update_post_initialization(registers);
                }
                PhyRfWakeOperation::UpdateAgcRegisters => {
                    oer_esp32s31_hal::phy::agc::update_baseband_registers(registers);
                }
                PhyRfWakeOperation::RestoreChannelRegisters => {
                    oer_esp32s31_hal::phy::frequency::configure_bss_cbw(registers, cbw);
                    oer_esp32s31_hal::phy::agc::configure_rx_compensation(registers);
                    write_i2c_direct(
                        registers,
                        crate::analog::i2c::analog_registers::TX_CAPACITOR_BANKS,
                        tx_cap,
                    )?;
                    oer_esp32s31_hal::phy::frequency::configure_nrx_frequency(
                        registers,
                        u32::from(frequency_mhz),
                    );
                }
                PhyRfWakeOperation::RestoreTxCapacitance => {
                    write_i2c_direct(
                        registers,
                        crate::analog::i2c::analog_registers::TX_CAPACITOR_BANKS,
                        tx_cap,
                    )?;
                }
                PhyRfWakeOperation::RestoreBasebandChannelWidth => {
                    oer_esp32s31_hal::phy::frequency::configure_channel_cbw(
                        registers,
                        u32::from(cbw),
                    );
                }
                PhyRfWakeOperation::EnableAgc => {
                    oer_esp32s31_hal::phy::agc::set_enabled(registers, true);
                }
                PhyRfWakeOperation::WaitFrequencyReady => {
                    let ready =
                        crate::executor::wait::poll::bounded::<core::convert::Infallible>(|| {
                            Ok(oer_esp32s31_hal::phy::frequency::sample_frequency_ready(
                                registers,
                            ))
                        })
                        .unwrap_or_else(|never| match never {});
                    if !ready {
                        return Err(PhyTargetPortError::HardwareEdgeTimedOut);
                    }
                }
                PhyRfWakeOperation::ResetClockGenerator => {
                    crate::target_executor::reset_clock_generator_direct(registers)?;
                }
            }
        }
        transition
            .advance(PhyRfWakeCompletion::executed(operation))
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn preclose_i2c_failure_never_returns_a_reusable_epoch() {
        let failure = preclose_i2c_failure(PhyTargetPortError::HardwareEdgeTimedOut);
        assert!(matches!(
            failure,
            PhyRfCloseTemperatureFailure::HardwareAmbiguous(
                PhyTargetPortError::HardwareEdgeTimedOut
            )
        ));
    }
}
