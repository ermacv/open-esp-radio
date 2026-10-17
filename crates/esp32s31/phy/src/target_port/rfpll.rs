//! Current-archive RFPLL child execution through existing typed I2C bindings.
//!
//! These entries require an already exclusive PHY borrow. They do not stop
//! radio traffic, acquire a coex grant, or commit a tracking temperature.
//! `maintain` includes the current frequency-control entry and restoration;
//! `search` and `correct` are children requiring that boundary from the caller.
//! Runtime execution polls I2C directly and retains the required 2/5-us ROM
//! settles. These futures do not suspend inside the admitted RFPLL transaction;
//! finite edge limits are observation bounds, not wall-clock guarantees.

use oer_esp32s31_hal::owner::SharedPhyAccess;

use crate::{
    analog::{
        frequency::PhyFrequencyCapMemoryExternalBinding,
        i2c::analog_registers,
        rfpll::{
            RfpllFrequencyAction as I2cAction, RfpllFrequencyCompletion as I2cCompletion,
            RfpllFrequencyI2cBinding,
        },
    },
    executor::wait::PhyShortDelay,
    target_executor::{PhyTargetPortError, complete_rfpll_i2c_direct},
    tracking::rfpll::{
        self,
        search::{Action, Completion, Status},
    },
};

fn i2c(
    registers: &mut impl SharedPhyAccess,
    action: I2cAction,
) -> Result<I2cCompletion, PhyTargetPortError> {
    let binding =
        RfpllFrequencyI2cBinding::new(action).map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    complete_rfpll_i2c_direct(binding, registers)
}

fn complete<D: PhyShortDelay>(
    registers: &mut impl SharedPhyAccess,
    action: Action,
) -> Result<Completion, PhyTargetPortError> {
    match action {
        Action::ReadInitialCap => {
            let I2cCompletion::ByteRead { value: low, .. } = i2c(
                registers,
                I2cAction::ReadByte {
                    address: analog_registers::RFPLL_CALIBRATED_CAPACITOR_LOW,
                },
            )?
            else {
                return Err(PhyTargetPortError::UnexpectedBinding);
            };
            let I2cCompletion::MaskedRead { value: high, .. } = i2c(
                registers,
                I2cAction::ReadMasked {
                    field: analog_registers::RFPLL_CALIBRATED_CAPACITOR_HIGH,
                },
            )?
            else {
                return Err(PhyTargetPortError::UnexpectedBinding);
            };
            Ok(Completion::InitialCap(
                u16::from(low) | (u16::from(high) << 8),
            ))
        }
        Action::EnableSearch => {
            i2c(
                registers,
                I2cAction::WriteMasked {
                    field: analog_registers::RFPLL_CAPACITOR_SEARCH_ENABLE,
                    value: 1,
                },
            )?;
            Ok(Completion::SearchEnabled)
        }
        Action::WriteCap(requested) => {
            // Complete ROM phy_write_pll_cap clamps a negative signed input;
            // the search bounds every other input to a nine-bit code.
            let programmed = crate::analog::rfpll::RfpllCapacitorCode::from_request(requested)
                .ok_or(PhyTargetPortError::HardwareInvariant)?;
            i2c(
                registers,
                I2cAction::WriteByte {
                    address: analog_registers::RFPLL_CAPACITOR_LOW,
                    value: programmed.low_byte(),
                },
            )?;
            i2c(
                registers,
                I2cAction::WriteMasked {
                    field: analog_registers::RFPLL_CAPACITOR_HIGH,
                    value: programmed.high_bit(),
                },
            )?;
            Ok(Completion::CapWritten(requested))
        }
        Action::DelayMicros(micros) => {
            if !D::settle_micros(micros) {
                return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
            }
            Ok(Completion::DelayElapsed(micros))
        }
        Action::ReadStatus => {
            let I2cCompletion::MaskedRead { value, .. } = i2c(
                registers,
                I2cAction::ReadMasked {
                    field: analog_registers::RFPLL_CAPACITOR_SEARCH_STATUS,
                },
            )?
            else {
                return Err(PhyTargetPortError::UnexpectedBinding);
            };
            let status = match value {
                0 => Status::Accepted,
                1 => Status::Increase,
                2 => Status::Decrease,
                3 => Status::Other,
                _ => return Err(PhyTargetPortError::UnexpectedBinding),
            };
            Ok(Completion::Status(status))
        }
        Action::Complete(_) => Err(PhyTargetPortError::UnexpectedBinding),
    }
}

/// Run the bounded search while retaining the caller's exclusive PHY borrow.
/// A hardware failure leaves that borrow with the caller for fault containment;
/// this entry does not imply that normal RF operation may resume.
#[cfg(any(test, feature = "validation-probes"))]
pub async fn search<D: PhyShortDelay>(
    registers: &mut impl SharedPhyAccess,
) -> Result<rfpll::search::Outcome, PhyTargetPortError> {
    let mut search = rfpll::search::Search::new();
    loop {
        let action = search.action();
        if let Action::Complete(outcome) = action {
            return Ok(outcome);
        }
        search
            .advance(complete::<D>(registers, action)?)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
}

/// Search, update every frequency-memory entry when the measured delta is
/// nonzero, and restore the current channel index before returning success.
#[cfg(any(test, feature = "validation-probes"))]
pub async fn correct<D: PhyShortDelay>(
    registers: &mut impl SharedPhyAccess,
    current_channel: u16,
) -> Result<rfpll::Outcome, PhyTargetPortError> {
    let mut correction = rfpll::Correction::new(current_channel);
    loop {
        if let rfpll::Action::Complete(outcome) = correction.action() {
            return Ok(outcome);
        }
        let completion = complete_correction::<D>(registers, correction.action())?;
        correction
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
}

/// Execute the current frequency-control envelope and measured correction.
///
/// The caller must already have stopped all RF clients and excluded hardware
/// channel changes. Once started this future must run to its terminal result;
/// a cancelled or failed transaction does not authorize normal RF operation.
/// In particular, failure does not blindly re-enable hardware frequency changes.
/// The caller retains the physical owner for fault containment.
///
/// Current `phy_dis_hw_set_freq_new` selects baseband mode two, delays two
/// microseconds and observes the SDM counter and I2C-number word. Its paired
/// enable selects mode zero. These are different from the ROM's frequency-
/// disable bit; the existing typed baseband-mode accessor implements the new
/// operations without duplicating the physical register's identity.
#[cfg(any(test, feature = "validation-probes"))]
pub async fn maintain<D: PhyShortDelay>(
    registers: &mut impl SharedPhyAccess,
    current_channel: u16,
) -> Result<rfpll::Outcome, PhyTargetPortError> {
    let outcome = track::<D>(
        registers,
        rfpll::thermal::Request {
            current_temperature: 0,
            reference_temperature: 0,
            current_channel,
            threshold_override: Some(0),
        },
    )
    .await?;
    outcome
        .correction
        .ok_or(PhyTargetPortError::UnexpectedBinding)
}

fn complete_correction<D: PhyShortDelay>(
    registers: &mut impl SharedPhyAccess,
    action: rfpll::Action,
) -> Result<rfpll::Completion, PhyTargetPortError> {
    match action {
        rfpll::Action::Search(action) => {
            Ok(rfpll::Completion::Search(complete::<D>(registers, action)?))
        }
        rfpll::Action::Memory(action) => {
            let binding = PhyFrequencyCapMemoryExternalBinding::lower(action)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            Ok(rfpll::Completion::Memory(binding.execute_target(registers)))
        }
        rfpll::Action::Complete(_) => Err(PhyTargetPortError::UnexpectedBinding),
    }
}

pub(super) async fn complete_thermal<D: PhyShortDelay>(
    registers: &mut impl SharedPhyAccess,
    grant: &mut impl super::PhyGrantProtectPort,
    action: rfpll::thermal::Action,
) -> Result<rfpll::thermal::Completion, PhyTargetPortError> {
    use oer_esp32s31_hal::phy::frequency;
    use rfpll::thermal::{Action, Completion};
    Ok(match action {
        Action::SetGrantProtect { enabled } => {
            grant.set_grant_protect(enabled)?;
            Completion::GrantProtectSet { enabled }
        }
        Action::SelectSoftwareControl => {
            frequency::set_baseband_mode(registers, 2);
            Completion::SoftwareControlSelected
        }
        Action::Settle => {
            if !D::settle_micros(2) {
                return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
            }
            Completion::Settled
        }
        Action::ObserveBoundary => {
            frequency::observe_software_frequency_boundary(registers);
            Completion::BoundaryObserved
        }
        Action::Correct(action) => {
            Completion::Correction(complete_correction::<D>(registers, action)?)
        }
        Action::RestoreHardwareControl => {
            frequency::set_baseband_mode(registers, 0);
            Completion::HardwareControlRestored
        }
        Action::Complete(_) => return Err(PhyTargetPortError::UnexpectedBinding),
    })
}

/// Execute an admitted thermal transaction, retaining exclusive access across
/// all waits. Success is available only after restoration (or a hardware-free
/// skip). Error or cancellation requires the caller to contain the failed epoch.
#[cfg(any(test, feature = "validation-probes"))]
pub async fn track<D: PhyShortDelay>(
    registers: &mut impl SharedPhyAccess,
    request: rfpll::thermal::Request,
) -> Result<rfpll::thermal::Outcome, PhyTargetPortError> {
    let mut child = rfpll::thermal::Transition::new(request);
    loop {
        if let rfpll::thermal::Action::Complete(outcome) = child.action() {
            return Ok(outcome);
        }
        child
            .advance(
                complete_thermal::<D>(registers, &mut super::WeakPhyGrantProtect, child.action())
                    .await?,
            )
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
}

/// Program the synthesizer directly (`RfpllFrequencyTransition::new`, the
/// `phy_set_rfpll_freq` path of crystal-duty, IQ and Bluetooth calibration)
/// through the production target executor. The outer error is an executor
/// failure; the inner one is the transition's own fail-closed outcome.
#[cfg(any(test, feature = "validation-probes"))]
pub async fn program<D: PhyShortDelay>(
    timer: &impl oer_time::Timer,
    registers: &mut impl SharedPhyAccess,
    request: crate::analog::rfpll::RfpllFrequencyRequest,
) -> Result<
    Result<
        crate::analog::rfpll::RfpllFrequencyOutcome,
        crate::analog::rfpll::RfpllFrequencyFailure,
    >,
    PhyTargetPortError,
> {
    use crate::analog::rfpll::{RfpllFrequencyExternalBinding, RfpllFrequencyTransition};
    let mut transition = RfpllFrequencyTransition::new(request);
    loop {
        let action = transition.action();
        match action {
            I2cAction::Complete(outcome) => return Ok(Ok(outcome)),
            I2cAction::Failed(failure) => return Ok(Err(failure)),
            _ => {}
        }
        let binding = RfpllFrequencyExternalBinding::lower(action)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        let completion = super::TargetCompleter::<D, _>::new(timer)
            .complete_rfpll(binding, &mut (), registers)
            .await?;
        transition
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
}
