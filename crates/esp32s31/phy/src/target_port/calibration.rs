//! Complete calibration children on the admitted physical PHY partition.
//!
//! These executors borrow the existing maintenance capability. They neither
//! acquire radio access nor publish parent calibration state. A returned child
//! completion must still be accepted by its parent transition.

use super::{
    CHANNEL_READY_SAMPLE_LIMIT, PhyTargetObserver, PhyTargetPortError, RF_OPERATION_LIMIT,
};
use crate::{
    executor::wait::PhyShortDelay,
    tracking::calibration::{
        PhyCalibrationChannelTransition, PhyCalibrationDcodeTransition,
        PhyCalibrationPbusClearTransition, PhyCalibrationRxGainTransition,
        PhyCalibrationTrackingCompletion,
    },
};
use oer_esp32s31_hal::owner::{SharedPhyAccess, SharedPhyContext};

/// Run the bounded PBus-clear child, including readiness and work-mode settling.
/// Once polled, keep the physical maintenance borrow until terminal completion;
/// an error does not establish a safe state for resuming radio traffic.
pub fn clear_pbus<D: PhyShortDelay, O: PhyTargetObserver>(
    mut child: PhyCalibrationPbusClearTransition,
    registers: &mut impl SharedPhyContext,
    _observer: &mut O,
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    use crate::analog::pbus::{PhyPbusClearAction as Action, PhyPbusClearCompletion as Completion};
    use oer_esp32s31_hal::phy::{agc, pbus};

    // Keep the admitted child and its exact command/settle order. This small
    // runtime child does not need the cold-init action/completion envelopes.
    for _ in 0..RF_OPERATION_LIMIT {
        child = match child.commit() {
            Ok(completion) => return Ok(completion),
            Err(child) => child,
        };
        let completion = match child.action() {
            Action::ConfigureDebugMode => {
                pbus::configure_debug_mode(registers);
                Completion::DebugModeConfigured
            }
            Action::ForceTest(transaction) => {
                if !crate::target_executor::force_pbus_direct(registers, transaction) {
                    return Err(PhyTargetPortError::HardwareEdgeTimedOut);
                }
                Completion::ForceTestCompleted(transaction)
            }
            Action::ConfigureWorkMode => Completion::WorkModeConfigured {
                settle_required: pbus::configure_work_mode(registers),
            },
            Action::DelayMicros(micros) => {
                short_settle::<D>(micros)?;
                Completion::DelayElapsed
            }
            Action::ConfigureWorkModePulse => {
                agc::configure_pbus_work_mode_pulse(registers);
                Completion::WorkModePulseConfigured
            }
            Action::ClearWorkModePulse => {
                agc::clear_pbus_work_mode_pulse(registers);
                Completion::WorkModePulseCleared
            }
            Action::Complete(_) => return Err(PhyTargetPortError::UnexpectedBinding),
        };
        child
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

/// Run the complete D-code child, retaining its nested PLL and I2C waits.
/// The caller retains the same maintenance and cancellation obligations as PBus clearing.
pub fn dcode<D: PhyShortDelay, P>(
    mut child: PhyCalibrationDcodeTransition,
    _platform: &mut P,
    registers: &mut impl SharedPhyAccess,
    mut observe: impl FnMut(bool),
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    use crate::analog::dcode::{PhyDcodeAction, PhyDcodeExternalBinding};

    for _ in 0..RF_OPERATION_LIMIT {
        // Consume ownership only at the terminal boundary; moving the whole
        // nested PLL state through a failed commit is unnecessary per action.
        let action = child.action();
        if matches!(
            action,
            PhyDcodeAction::Complete(_) | PhyDcodeAction::Failed(_)
        ) {
            return child
                .commit()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
        }
        let binding = PhyDcodeExternalBinding::lower(action)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        let completion = complete_dcode_direct::<D>(binding, registers)?;
        if let crate::analog::dcode::PhyDcodeCompletion::Rfpll(
            crate::analog::rfpll::RfpllFrequencyCompletion::MaskedRead {
                field: crate::analog::i2c::analog_registers::RFPLL_LOCK_STATUS,
                value,
            },
        ) = completion
        {
            observe(value != 0);
        }
        child
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

fn complete_dcode_direct<D: PhyShortDelay>(
    binding: crate::analog::dcode::PhyDcodeExternalBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::analog::dcode::PhyDcodeCompletion, PhyTargetPortError> {
    use crate::analog::dcode::{PhyDcodeCompletion, PhyDcodeExternalBinding};
    match binding {
        PhyDcodeExternalBinding::Rfpll(binding) => Ok(PhyDcodeCompletion::Rfpll(
            complete_rfpll_hot::<D>(binding, registers)?,
        )),
        PhyDcodeExternalBinding::I2c(mut binding) => {
            use crate::analog::i2c::PhyI2cError;
            use crate::calibration::cold::PhyColdI2cAction;
            // One shared transition budget, including the terminal check,
            // spans read and read/modify/write just as in the cold binding.
            let completed = crate::executor::wait::poll::bounded(|| {
                match binding.action() {
                    PhyColdI2cAction::StartRead { address } => {
                        match crate::analog::i2c::try_start_read(registers, address) {
                            Ok(()) => binding
                                .read_started()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                            Err(PhyI2cError::Busy) => {}
                        }
                    }
                    PhyColdI2cAction::StartWrite { address, value } => {
                        match crate::analog::i2c::try_start_write(registers, address, value) {
                            Ok(()) => binding
                                .write_started()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                            Err(PhyI2cError::Busy) => {}
                        }
                    }
                    PhyColdI2cAction::AwaitReadCompletionEdge { address } => {
                        binding
                            .observe_read_result(crate::analog::i2c::try_finish_read(
                                registers, address,
                            ))
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                    }
                    PhyColdI2cAction::AwaitWriteCompletionEdge { address } => {
                        binding
                            .observe_write_result(crate::analog::i2c::try_finish_write(
                                registers, address,
                            ))
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
    }
}

/// Run RX-DC calibration and publish both gain banks using the same admitted owner.
pub fn rx_gain<D: PhyShortDelay, P>(
    mut child: PhyCalibrationRxGainTransition,
    platform: &mut P,
    registers: &mut impl SharedPhyContext,
    observe: impl FnMut(crate::tracking::observation::Operation, crate::tracking::observation::Event),
    summarize: impl FnMut(crate::tracking::observation::RxGainExecution),
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    rx_gain_init::<D, _>(
        child.transition_mut(),
        platform,
        registers,
        observe,
        summarize,
    )?;
    child
        .commit()
        .map_err(|_| PhyTargetPortError::UnexpectedBinding)
}

/// Execute an RX-gain root through its terminal edge. Its caller owns state
/// publication; success here may still contain a typed calibration failure.
#[inline(never)]
pub fn rx_gain_init<D: PhyShortDelay, P>(
    child: &mut crate::rx::gain::PhyRxGainInitTransition,
    _platform: &mut P,
    registers: &mut impl SharedPhyContext,
    mut observe: impl FnMut(
        crate::tracking::observation::Operation,
        crate::tracking::observation::Event,
    ),
    mut summarize: impl FnMut(crate::tracking::observation::RxGainExecution),
) -> Result<(), PhyTargetPortError> {
    use crate::tracking::observation::{Event, Operation};
    let mut execution = crate::tracking::observation::RxGainExecution::default();
    let mut budget = crate::target_executor::DirectOperationBudget::new(RF_OPERATION_LIMIT);
    let result = (|| {
        for _ in 0..4 {
            if child.terminal().is_some() {
                return Ok(());
            }
            if let Some(parameters) = child.direct_dc_parameters() {
                observe(Operation::RxGainControlPhase, Event::Started);
                let prepared = oer_esp32s31_hal::phy::rx_dco::prepare_control_restore(registers)
                    .map_err(|_| PhyTargetPortError::HardwareInvariant);
                observe(Operation::RxGainControlPhase, terminal_event(&prepared));
                prepared?;

                observe(Operation::RxGainDcPhase, Event::Started);
                let outcome =
                    rx_gain_dc_direct::<D>(parameters, registers, &mut budget, &mut execution);
                observe(Operation::RxGainDcPhase, terminal_event(&outcome));
                let outcome = outcome?;
                execution.quality = outcome.as_ref().ok().map(|outcome| outcome.quality);

                observe(Operation::RxGainControlPhase, Event::Started);
                let restored = oer_esp32s31_hal::phy::rx_dco::restore_control(registers)
                    .map_err(|_| PhyTargetPortError::HardwareInvariant);
                observe(Operation::RxGainControlPhase, terminal_event(&restored));
                restored?;
                child
                    .commit_direct_dc(outcome)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                continue;
            }

            if child.phase() == Some(crate::rx::gain::Phase::Publish) {
                observe(Operation::RxGainPublishPhase, Event::Started);
                let published = child.execute_publish_target_direct::<D>(registers);
                observe(Operation::RxGainPublishPhase, terminal_event(&published));
                if !published? {
                    return Err(PhyTargetPortError::UnexpectedBinding);
                }
                continue;
            }

            if child.phase() == Some(crate::rx::gain::Phase::Control) {
                observe(Operation::RxGainControlPhase, Event::Started);
                if !child.execute_tail_target_direct(registers) {
                    observe(Operation::RxGainControlPhase, Event::Failed);
                    return Err(PhyTargetPortError::UnexpectedBinding);
                }
                observe(Operation::RxGainControlPhase, Event::Completed);
                continue;
            }
            return Err(PhyTargetPortError::UnexpectedBinding);
        }
        Err(PhyTargetPortError::RfOperationLimit)
    })();
    if result.is_ok() {
        summarize(execution);
    }
    result
}

enum DirectRxGainDcError {
    Target(PhyTargetPortError),
    Calibration(crate::rx::gain_calibration::PhyRxGainDcFailure),
}

impl From<PhyTargetPortError> for DirectRxGainDcError {
    fn from(error: PhyTargetPortError) -> Self {
        Self::Target(error)
    }
}

impl From<crate::rx::gain_calibration::PhyRxGainDcFailure> for DirectRxGainDcError {
    fn from(error: crate::rx::gain_calibration::PhyRxGainDcFailure) -> Self {
        Self::Calibration(error)
    }
}

fn force_rx_gain_pbus(
    registers: &mut impl SharedPhyAccess,
    bank: crate::rx::gain_calibration::PhyRxGainDcBank,
    transaction: crate::analog::pbus::PhyPbusForceTest,
) -> Result<(), crate::rx::gain_calibration::PhyRxGainDcFailure> {
    if crate::target_executor::force_pbus_direct(registers, transaction) {
        Ok(())
    } else {
        Err(crate::rx::gain_calibration::PhyRxGainDcFailure::Pbus { bank, transaction })
    }
}

fn rx_gain_rfpll_direct<D: PhyShortDelay>(
    crystal_selector: u8,
    registers: &mut impl SharedPhyAccess,
) -> Result<(), DirectRxGainDcError> {
    use crate::analog::rfpll::{
        RfpllFrequencyAction, RfpllFrequencyExternalBinding, RfpllFrequencyRequest,
        RfpllFrequencyTransition,
    };
    let mut transition = RfpllFrequencyTransition::channel(RfpllFrequencyRequest {
        crystal_selector,
        frequency_code: 0x9b4,
        offset: 0,
    });
    loop {
        match transition.action() {
            RfpllFrequencyAction::Complete(_) => return Ok(()),
            RfpllFrequencyAction::Failed(failure) => {
                return Err(crate::rx::gain_calibration::PhyRxGainDcFailure::Rfpll(failure).into());
            }
            action => {
                let binding = RfpllFrequencyExternalBinding::lower(action)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                let completion = complete_rfpll_hot::<D>(binding, registers)?;
                transition
                    .advance(completion)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            }
        }
    }
}

fn rx_gain_one_step_direct<D: PhyShortDelay>(
    request: crate::rx::gain_calibration::PhyRxDcCalibrationRequest,
    registers: &mut impl SharedPhyContext,
    budget: &mut crate::target_executor::DirectOperationBudget,
    execution: &mut crate::tracking::observation::RxGainExecution,
) -> Result<crate::rx::gain_calibration::PhyRxDcCalibrationOutcome, DirectRxGainDcError> {
    execution.outer_operations += 1;
    let mut transition = crate::rx::gain_calibration::PhyRxDcCalibrationTransition::new(request);
    let stats = transition.execute_target_direct::<D>(registers, budget)?;
    execution.minimum_searches += stats.minimum_searches;
    execution.minimum_operations += stats.minimum_operations;
    execution.settle_1us += stats.settle_1us;
    execution.settle_10us += stats.settle_10us;
    match transition.terminal() {
        Some(Ok(outcome)) => Ok(outcome),
        Some(Err(failure)) => {
            Err(crate::rx::gain_calibration::PhyRxGainDcFailure::Calibration(failure).into())
        }
        None => Err(PhyTargetPortError::UnexpectedBinding.into()),
    }
}

fn rx_gain_cleanup_direct<D: PhyShortDelay>(
    registers: &mut impl SharedPhyContext,
) -> Result<(), PhyTargetPortError> {
    oer_esp32s31_hal::phy::pbus::configure_rx_clock(registers, false);
    oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, false);
    for index in 0..3 {
        let _ = crate::target_executor::force_pbus_direct(
            registers,
            crate::rx::gain_calibration::rx_off(index),
        );
    }
    if oer_esp32s31_hal::phy::pbus::configure_work_mode(registers) {
        short_settle::<D>(1)?;
        oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
        short_settle::<D>(2)?;
        oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
    }
    crate::hardware::configure_phy_rx_gain_dc_registers(registers, false);
    Ok(())
}

#[inline(never)]
fn rx_gain_dc_direct<D: PhyShortDelay>(
    parameters: crate::rx::gain_calibration::PhyRxGainDcParameters,
    registers: &mut impl SharedPhyContext,
    budget: &mut crate::target_executor::DirectOperationBudget,
    execution: &mut crate::tracking::observation::RxGainExecution,
) -> Result<
    Result<
        crate::rx::gain_calibration::PhyRxGainDcOutcome,
        crate::rx::gain_calibration::PhyRxGainDcFailure,
    >,
    PhyTargetPortError,
> {
    use crate::{
        analog::{i2c::analog_registers, pbus::PhyPbusForceTest},
        rx::gain_calibration::{
            FINE_CODES, PhyRxDcCalibrationRequest, PhyRxDcCalibrationStage, PhyRxGainDcBank,
            PhyRxGainDcOutcome, SHARED_CALIBRATION_GAIN, WIFI_CALIBRATION_GAIN, fine_code, rx_on,
            set_rx_gain_transaction, shared_mixer_dgain_transaction,
        },
    };

    let mut outcome = PhyRxGainDcOutcome {
        quality: crate::rx::gain_calibration::PhyRxGainDcQuality::EMPTY,
        wifi_index_dc: [[0; 2]; 8],
        wifi_fine_dc: [[0; 2]; FINE_CODES],
        shared_index_dc: [[0; 2]; 11],
    };
    let run = (|| -> Result<(), DirectRxGainDcError> {
        crate::hardware::configure_phy_rx_gain_dc_registers(registers, true);
        rx_gain_rfpll_direct::<D>(parameters.crystal_selector, registers)?;
        oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
        for index in 0..7 {
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Shared,
                rx_on(index, parameters.pbus_rx_path_value),
            )?;
        }
        oer_esp32s31_hal::phy::pbus::configure_rx_clock(registers, true);
        oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, true);
        let shared_control = oer_esp32s31_hal::phy::pbus::read_result(registers, 1, 1)
            .ok_or(PhyTargetPortError::HardwareInvariant)?
            | 2;
        force_rx_gain_pbus(
            registers,
            PhyRxGainDcBank::Shared,
            PhyPbusForceTest::new(1, 1, shared_control),
        )?;
        crate::target_executor::write_i2c_field_direct(
            registers,
            analog_registers::SHARED_RX_GAIN_CALIBRATION_ENABLE,
            0,
        )?;
        for index in 0..SHARED_CALIBRATION_GAIN.len() as u8 {
            let previous = if index == 0 {
                [0x100; 2]
            } else {
                outcome.shared_index_dc[0]
            };
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Shared,
                PhyPbusForceTest::new(2, 1, 0x100),
            )?;
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Shared,
                PhyPbusForceTest::new(3, 1, 0x100),
            )?;
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Shared,
                PhyPbusForceTest::new(2, 2, 0x100),
            )?;
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Shared,
                PhyPbusForceTest::new(3, 2, 0x100),
            )?;
            for transaction in 0..3 {
                force_rx_gain_pbus(
                    registers,
                    PhyRxGainDcBank::Shared,
                    set_rx_gain_transaction(
                        u32::from(SHARED_CALIBRATION_GAIN[index as usize]) << 12,
                        parameters.pbus_rx_path_value,
                        transaction,
                    ),
                )?;
            }
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Shared,
                shared_mixer_dgain_transaction(index, parameters.pbus_rx_path_value),
            )?;
            let calibrated = rx_gain_one_step_direct::<D>(
                PhyRxDcCalibrationRequest {
                    shared_radio: true,
                    stage: PhyRxDcCalibrationStage::Baseband,
                    control: 0x800,
                    initial: previous,
                    gain_index: index,
                    rx_saturation_detected: parameters.rx_saturation_detected,
                },
                registers,
                budget,
                execution,
            )?;
            outcome.quality.record_shared(index, calibrated.converged);
            outcome.shared_index_dc[index as usize] = calibrated.configuration;
        }
        crate::target_executor::write_i2c_field_direct(
            registers,
            analog_registers::SHARED_RX_GAIN_CALIBRATION_ENABLE,
            1,
        )?;
        rx_gain_cleanup_direct::<D>(registers)?;

        crate::hardware::configure_phy_rx_gain_dc_registers(registers, true);
        rx_gain_rfpll_direct::<D>(parameters.crystal_selector, registers)?;
        oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
        for index in 0..7 {
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Wifi,
                rx_on(index, parameters.pbus_rx_path_value),
            )?;
        }
        oer_esp32s31_hal::phy::pbus::configure_rx_clock(registers, true);
        oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, true);
        for index in 0..WIFI_CALIBRATION_GAIN.len() as u8 {
            let previous = if index == 0 {
                [0x100; 2]
            } else {
                outcome.wifi_index_dc[0]
            };
            let baseband = if index == 0 {
                [0x100; 2]
            } else {
                outcome.wifi_fine_dc[0]
            };
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Wifi,
                PhyPbusForceTest::new(2, 1, 0x100),
            )?;
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Wifi,
                PhyPbusForceTest::new(3, 1, 0x100),
            )?;
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Wifi,
                PhyPbusForceTest::new(2, 2, baseband[0]),
            )?;
            force_rx_gain_pbus(
                registers,
                PhyRxGainDcBank::Wifi,
                PhyPbusForceTest::new(3, 2, baseband[1]),
            )?;
            for transaction in 0..3 {
                force_rx_gain_pbus(
                    registers,
                    PhyRxGainDcBank::Wifi,
                    set_rx_gain_transaction(
                        u32::from(WIFI_CALIBRATION_GAIN[index as usize]) << 12,
                        parameters.pbus_rx_path_value,
                        transaction,
                    ),
                )?;
            }
            let calibrated = rx_gain_one_step_direct::<D>(
                PhyRxDcCalibrationRequest {
                    shared_radio: false,
                    stage: PhyRxDcCalibrationStage::Baseband,
                    control: 0x800,
                    initial: previous,
                    gain_index: index,
                    rx_saturation_detected: parameters.rx_saturation_detected,
                },
                registers,
                budget,
                execution,
            )?;
            outcome
                .quality
                .record_wifi_baseband(index, calibrated.converged);
            outcome.wifi_index_dc[index as usize] = calibrated.configuration;
            if index == 0 {
                // `phy_rxdc_fine_cal`: one radio search per fine code, each
                // starting from the previous search's pair.
                let mut fine_current = [0x100; 2];
                for fine in 0..FINE_CODES as u8 {
                    force_rx_gain_pbus(
                        registers,
                        PhyRxGainDcBank::Wifi,
                        PhyPbusForceTest::new(1, 2, fine_code(fine)),
                    )?;
                    let calibrated = rx_gain_one_step_direct::<D>(
                        PhyRxDcCalibrationRequest {
                            shared_radio: false,
                            stage: PhyRxDcCalibrationStage::Radio,
                            control: 0x800,
                            initial: fine_current,
                            gain_index: 0,
                            rx_saturation_detected: parameters.rx_saturation_detected,
                        },
                        registers,
                        budget,
                        execution,
                    )?;
                    outcome.quality.record_wifi_fine(fine, calibrated.converged);
                    fine_current = calibrated.configuration;
                    outcome.wifi_fine_dc[fine as usize] = fine_current;
                }
            }
        }
        Ok(())
    })();

    match run {
        Ok(()) => {
            rx_gain_cleanup_direct::<D>(registers)?;
            Ok(Ok(outcome))
        }
        Err(DirectRxGainDcError::Target(error)) => Err(error),
        Err(DirectRxGainDcError::Calibration(failure)) => {
            rx_gain_cleanup_direct::<D>(registers)?;
            Ok(Err(failure))
        }
    }
}

fn short_settle<D: PhyShortDelay>(micros: u32) -> Result<(), PhyTargetPortError> {
    if micros != 0 && micros <= D::MAX_MICROS && D::settle_micros(micros) {
        Ok(())
    } else {
        Err(PhyTargetPortError::HardwareCapabilityUnavailable)
    }
}

fn complete_rfpll_hot<D: PhyShortDelay>(
    binding: crate::analog::rfpll::RfpllFrequencyExternalBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::analog::rfpll::RfpllFrequencyCompletion, PhyTargetPortError> {
    use crate::analog::rfpll::{
        RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyExternalBinding,
    };
    match binding {
        RfpllFrequencyExternalBinding::Mmio(binding) => match binding.action() {
            RfpllFrequencyAction::ReadChannelReady { samples }
                if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
            {
                Ok(RfpllFrequencyCompletion::ChannelReadyTimedOut)
            }
            _ => Ok(binding.execute_target(registers)),
        },
        RfpllFrequencyExternalBinding::I2c(binding) => {
            crate::target_executor::complete_rfpll_i2c_direct(binding, registers)
        }
        RfpllFrequencyExternalBinding::Timer(binding) => {
            short_settle::<D>(binding.micros())?;
            Ok(binding.into_completion())
        }
    }
}

fn terminal_event<T, E>(result: &Result<T, E>) -> crate::tracking::observation::Event {
    use crate::tracking::observation::Event;
    if result.is_ok() {
        Event::Completed
    } else {
        Event::Failed
    }
}

/// Execute the complete TX-DC/PWDET root as one vendor-shaped blocking
/// transaction, including measurement and restoration. The caller retains
/// physical access until terminal completion. Returning a target error or a
/// typed failed action does not establish a safe RF state.
#[inline(never)]
pub fn tx_dc_pwdet_init<D: PhyShortDelay, O: PhyTargetObserver>(
    child: &mut crate::tx::dc_power_detector::PhyTxDcPwdetTransition,
    registers: &mut impl SharedPhyContext,
    observer: &core::cell::RefCell<&mut O>,
    clock: &impl oer_time::Clock,
) -> Result<(), PhyTargetPortError> {
    child.execute_target_direct(
        registers,
        |scope, micros| tx_settle::<D, O>(observer, clock, scope, micros),
        |ready| observer.borrow_mut().tx_sar_ready(ready),
    )
}

fn tx_settle<D: PhyShortDelay, O: PhyTargetObserver>(
    observer: &core::cell::RefCell<&mut O>,
    clock: &impl oer_time::Clock,
    scope: crate::executor::wait::tx::Scope,
    micros: u32,
) -> Result<(), PhyTargetPortError> {
    use crate::executor::wait::{Event, Kind};
    let started = O::OBSERVE_DELAYS.then(|| clock.now());
    if O::OBSERVE_DELAYS {
        observer.borrow_mut().tx_wait(
            scope,
            Kind::Settle,
            Event::Started {
                requested_micros: u64::from(micros),
            },
        );
    }
    let result = short_settle::<D>(micros);
    // A failed settle reports no completion, which leaves the evidence
    // incomplete.
    if let (Ok(()), Some(started)) = (&result, started) {
        let elapsed_micros = clock.now().saturating_duration_since(started).as_micros();
        observer.borrow_mut().tx_wait(
            scope,
            Kind::Settle,
            Event::Completed {
                elapsed_micros,
                lateness_micros: elapsed_micros.saturating_sub(u64::from(micros)),
            },
        );
    }
    result
}

/// Restore the operating channel without acquiring or releasing physical access.
pub fn channel<D: PhyShortDelay, P, O: PhyTargetObserver>(
    mut child: PhyCalibrationChannelTransition,
    platform: &mut P,
    registers: &mut impl SharedPhyAccess,
    observer: &mut O,
) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
    for _ in 0..RF_OPERATION_LIMIT {
        child = match child.commit() {
            Ok(completion) => return Ok(completion),
            Err(child) => child,
        };
        let binding = child
            .lower_external()
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        let completion =
            complete_channel_direct::<D, _, _>(binding, platform, registers, observer)?;
        child
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
    Err(PhyTargetPortError::RfOperationLimit)
}

fn complete_channel_direct<D: PhyShortDelay, P, O: PhyTargetObserver>(
    binding: crate::channel::PhyChipChannelExternalBinding,
    platform: &mut P,
    registers: &mut impl SharedPhyAccess,
    observer: &mut O,
) -> Result<crate::channel::PhyChipChannelCompletion, PhyTargetPortError> {
    use crate::channel::{
        PhyChipChannelAction, PhyChipChannelCompletion, PhyChipChannelExternalBinding,
    };
    match binding {
        PhyChipChannelExternalBinding::Mmio(binding) => match binding.action() {
            PhyChipChannelAction::AwaitFrequencyReadyEdge { samples, .. }
                if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
            {
                observer.channel_frequency_ready_timed_out(samples);
                Ok(PhyChipChannelCompletion::FrequencyReadyTimedOut)
            }
            _ => Ok(binding.execute_target(platform, registers)),
        },
        PhyChipChannelExternalBinding::Temperature(binding) => Ok(
            PhyChipChannelCompletion::Temperature(complete_temperature_direct(binding, registers)?),
        ),
        PhyChipChannelExternalBinding::Timer(binding) => {
            short_settle::<D>(binding.micros())?;
            Ok(binding.into_completion())
        }
        PhyChipChannelExternalBinding::I2c(binding) => {
            complete_channel_i2c_direct(binding, registers)
        }
        PhyChipChannelExternalBinding::TxGain(binding) => {
            let request = binding.request();
            let completion = binding.execute();
            if let PhyChipChannelCompletion::TxGainCalculated { image, .. } = completion {
                observer.channel_tx_gain(request, image);
            }
            Ok(completion)
        }
    }
}

fn complete_temperature_direct(
    binding: crate::analog::temperature::PhyTemperatureExternalBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::analog::temperature::PhyTemperatureCompletion, PhyTargetPortError> {
    use crate::analog::temperature::PhyTemperatureExternalBinding;
    match binding {
        PhyTemperatureExternalBinding::I2c(mut binding) => {
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
        PhyTemperatureExternalBinding::Sample(binding) => Ok(binding.execute_target(registers)),
    }
}

fn complete_channel_i2c_direct(
    mut binding: crate::channel::PhyChipChannelI2cBinding,
    registers: &mut impl SharedPhyAccess,
) -> Result<crate::channel::PhyChipChannelCompletion, PhyTargetPortError> {
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
        return Ok(binding.into_timeout_completion());
    }
    binding
        .into_completion()
        .map_err(|_| PhyTargetPortError::UnexpectedBinding)
}
