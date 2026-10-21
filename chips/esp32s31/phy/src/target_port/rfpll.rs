//! Current-archive RFPLL child execution through existing typed I2C bindings.
//!
//! These entries require an already exclusive PHY borrow. They do not stop
//! radio traffic, acquire a coex grant, or commit a tracking temperature.
//! `maintain` includes the current frequency-control entry and restoration;
//! `search` and `correct` are children requiring that boundary from the caller.
//! Runtime execution polls I2C directly. The P1 engine yields after a complete
//! capacitor write and measures the required 5-us minimum before status; the ROM
//! supplies only any remaining settle. Memory entries yield after read/write.
//! The entry settle remains 2 µs; direct I2C keeps its named 20-µs settle.

#![allow(
    private_bounds,
    reason = "probe entries use the closed transport engine routes"
)]

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
    target_executor::PhyTargetPortError,
    tracking::rfpll::{
        self,
        search::{Action, Completion, Status},
    },
};

fn i2c<D: PhyShortDelay, C: oer_time::Clock + ?Sized>(
    clock: &C,
    registers: &mut (impl SharedPhyAccess + crate::executor::step::Registers),
    action: I2cAction,
) -> Result<u8, PhyTargetPortError> {
    let binding =
        RfpllFrequencyI2cBinding::new(action).map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    let completion = registers
        .rfpll_atom::<D, _>(
            clock,
            crate::analog::rfpll::RfpllFrequencyExternalBinding::I2c(binding),
        )
        .map_err(super::step::port_error)?;
    match completion {
        I2cCompletion::ByteRead { value, .. } | I2cCompletion::MaskedRead { value, .. } => {
            Ok(value)
        }
        I2cCompletion::ByteWrite { .. } | I2cCompletion::MaskedWrite { .. } => Ok(0),
        _ => Err(PhyTargetPortError::UnexpectedBinding),
    }
}

fn complete<D: PhyShortDelay, C: oer_time::Clock + ?Sized>(
    clock: &C,
    registers: &mut (impl SharedPhyAccess + crate::executor::step::Registers),
    action: Action,
) -> Result<Completion, PhyTargetPortError> {
    match action {
        Action::ReadInitialCap => {
            let low = i2c::<D, _>(
                clock,
                registers,
                I2cAction::ReadByte {
                    address: analog_registers::RFPLL_CALIBRATED_CAPACITOR_LOW,
                },
            )?;
            let high = i2c::<D, _>(
                clock,
                registers,
                I2cAction::ReadMasked {
                    field: analog_registers::RFPLL_CALIBRATED_CAPACITOR_HIGH,
                },
            )?;
            Ok(Completion::InitialCap(
                u16::from(low) | (u16::from(high) << 8),
            ))
        }
        Action::EnableSearch => {
            i2c::<D, _>(
                clock,
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
            i2c::<D, _>(
                clock,
                registers,
                I2cAction::WriteByte {
                    address: analog_registers::RFPLL_CAPACITOR_LOW,
                    value: programmed.low_byte(),
                },
            )?;
            i2c::<D, _>(
                clock,
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
            let value = i2c::<D, _>(
                clock,
                registers,
                I2cAction::ReadMasked {
                    field: analog_registers::RFPLL_CAPACITOR_SEARCH_STATUS,
                },
            )?;
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
#[cfg(any(test, oer_probe))]
pub async fn search<D: PhyShortDelay, C: oer_time::Timer>(
    clock: &C,
    registers: &mut (impl SharedPhyAccess + crate::executor::step::Registers),
) -> Result<rfpll::search::Outcome, PhyTargetPortError> {
    use crate::executor::step::{Job, drive, rfpll::Cursor};
    let mut search = rfpll::search::Search::new();
    {
        let mut job = Job::rfpll(Cursor::Search(&mut search), clock.now())
            .map_err(super::step::port_error)?;
        drive::<D, _, _>(&mut job, registers, clock, None, None)
            .await
            .map_err(super::step::port_error)?;
    }
    match search.action() {
        Action::Complete(outcome) => Ok(outcome),
        _ => Err(PhyTargetPortError::UnexpectedBinding),
    }
}

/// Search, update every frequency-memory entry when the measured delta is
/// nonzero, and restore the current channel index before returning success.
#[cfg(any(test, oer_probe))]
pub async fn correct<D: PhyShortDelay, C: oer_time::Timer>(
    clock: &C,
    registers: &mut (impl SharedPhyAccess + crate::executor::step::Registers),
    current_channel: u16,
) -> Result<rfpll::Outcome, PhyTargetPortError> {
    use crate::executor::step::{Job, drive, rfpll::Cursor};
    let mut correction = rfpll::Correction::new(current_channel);
    {
        let mut job = Job::rfpll(Cursor::Correction(&mut correction), clock.now())
            .map_err(super::step::port_error)?;
        drive::<D, _, _>(&mut job, registers, clock, None, None)
            .await
            .map_err(super::step::port_error)?;
    }
    match correction.action() {
        rfpll::Action::Complete(outcome) => Ok(outcome),
        _ => Err(PhyTargetPortError::UnexpectedBinding),
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
#[cfg(any(test, oer_probe))]
pub async fn maintain<D: PhyShortDelay, C: oer_time::Timer>(
    clock: &C,
    registers: &mut (impl SharedPhyAccess + crate::executor::step::Registers),
    current_channel: u16,
) -> Result<rfpll::Outcome, PhyTargetPortError> {
    let outcome = track::<D, _>(
        clock,
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

#[inline(always)]
pub(crate) fn complete_correction<D: PhyShortDelay, C: oer_time::Clock + ?Sized>(
    clock: &C,
    registers: &mut (impl SharedPhyAccess + crate::executor::step::Registers),
    action: rfpll::Action,
) -> Result<rfpll::Completion, PhyTargetPortError> {
    match action {
        rfpll::Action::Search(action) => Ok(rfpll::Completion::Search(complete::<D, _>(
            clock, registers, action,
        )?)),
        rfpll::Action::Memory(action) => {
            let binding = PhyFrequencyCapMemoryExternalBinding::lower(action)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            Ok(rfpll::Completion::Memory(binding.execute_target(registers)))
        }
        rfpll::Action::Complete(_) => Err(PhyTargetPortError::UnexpectedBinding),
    }
}

/// Synchronous entry/restoration around the sole correction await. It never
/// executes a correction action or constructs an always-ready child future.
pub(super) fn envelope<D: PhyShortDelay>(
    child: &mut rfpll::thermal::Transition,
    registers: &mut impl SharedPhyAccess,
    grant: &mut impl super::PhyGrantProtectPort,
    observer: &mut impl super::PhyTargetObserver,
) -> Result<(), PhyTargetPortError> {
    use oer_esp32s31_hal::phy::frequency;
    use rfpll::thermal::{Action, Completion};
    loop {
        let action = child.action();
        let completion = match action {
            Action::Correct(_) | Action::Complete(_) => return Ok(()),
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
            Action::RestoreHardwareControl => {
                frequency::set_baseband_mode(registers, 0);
                Completion::HardwareControlRestored
            }
        };
        if let Some((operation, event)) = crate::tracking::observation::rfpll_hold_edge(action) {
            observer.tracking_operation(operation, event);
        }
        child
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
}

/// Execute an admitted thermal transaction, retaining exclusive access across
/// all waits. Success is available only after restoration (or a hardware-free
/// skip). Error or cancellation requires the caller to contain the failed epoch.
#[cfg(any(test, oer_probe))]
pub async fn track<D: PhyShortDelay, C: oer_time::Timer>(
    clock: &C,
    registers: &mut (impl SharedPhyAccess + crate::executor::step::Registers),
    request: rfpll::thermal::Request,
) -> Result<rfpll::thermal::Outcome, PhyTargetPortError> {
    let mut child = rfpll::thermal::Transition::new(request);
    envelope::<D>(
        &mut child,
        registers,
        &mut super::WeakPhyGrantProtect,
        &mut super::NoopPhyTargetObserver,
    )?;
    if let Some(correction) = child.correction_mut() {
        {
            use crate::executor::step::{Job, drive, rfpll::Cursor};
            let mut job = Job::rfpll(Cursor::Correction(correction), clock.now())
                .map_err(super::step::port_error)?;
            drive::<D, _, _>(&mut job, registers, clock, None, None)
                .await
                .map_err(super::step::port_error)?;
        }
        child
            .finish_correction()
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        envelope::<D>(
            &mut child,
            registers,
            &mut super::WeakPhyGrantProtect,
            &mut super::NoopPhyTargetObserver,
        )?;
    }
    match child.action() {
        rfpll::thermal::Action::Complete(outcome) => Ok(outcome),
        _ => Err(PhyTargetPortError::UnexpectedBinding),
    }
}

/// Program the synthesizer directly (`RfpllFrequencyTransition::new`, the
/// `phy_set_rfpll_freq` path of crystal-duty, IQ and Bluetooth calibration)
/// through the production target executor. The outer error is an executor
/// failure; the inner one is the transition's own fail-closed outcome.
#[cfg(any(test, oer_probe))]
pub async fn program<D: PhyShortDelay>(
    timer: &impl oer_time::Timer,
    registers: &mut (impl SharedPhyAccess + crate::executor::step::Registers),
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
            .complete_rfpll(binding, registers)
            .await?;
        transition
            .advance(completion)
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
    }
}
