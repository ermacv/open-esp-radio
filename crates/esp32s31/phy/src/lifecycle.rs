//! Whole-PHY power lifecycle recovered from the current ESP32-S31 library.
//!
//! Physical RF close is independent of protocol-client bookkeeping. A caller
//! may execute it only after the last client has been released and every
//! protocol hardware engine has crossed its stopped boundary.

use crate::analog::i2c::{PhyI2cAddress, analog_registers};

/// One complete child call or finite hardware edge in current-vendor
/// `phy_wakeup_init`.
///
/// Runtime values such as channel, CBW, TX capacitance and retained
/// calibration parameters remain in [`crate::state::PhyState`]. This enum
/// records only the parent ordering; each child keeps its own typed request
/// and completion contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfWakeOperation {
    SetBasebandMode { mode: u8 },
    ForceTxRxOff { enabled: bool },
    SetHardwareFrequencyControl { enabled: bool },
    ResetI2cMaster,
    OpenFrontendBasebandClocks,
    SetBbpllCalibration { enabled: bool },
    ConfigureBiasRegisters,
    PowerTemperatureSensor,
    ReadXtalFrequency,
    OpenI2cPower,
    ClearPbusRegisters,
    ConfigureI2cClock,
    ResetFrontendTxRx,
    ConfigureAdcRate,
    ConfigureI2cMasterRegisters,
    ConfigureFrequencyRegisters,
    ConfigureFrontendRegisters,
    UpdateFrontendRegisters,
    ConfigurePowerDetectorRegisters,
    ConfigureI2cStageTwo,
    PublishRetainedFrequencyI2c,
    RestoreChannelFrequency,
    RestorePbusBoundaries,
    ConfigurePhyRegisters,
    UpdatePhyRegisters,
    UpdateAgcRegisters,
    RestoreChannelRegisters,
    RestoreTxCapacitance,
    RestoreBasebandChannelWidth,
    EnableAgc,
    WaitFrequencyReady,
    ResetClockGenerator,
}

/// Exact target-visible operation order of current S31 retained wake.
///
/// The two vendor-private retained flags are represented by the terminal
/// owner transition rather than synthetic hardware actions. The conditional
/// `phy_wifi_enable_set(0)` tail is unreachable for this transition because
/// [`crate::RegisteredPhyRfClosed`] proves that the protocol-client set is
/// empty.
pub const PHY_RF_WAKE_OPERATIONS: [PhyRfWakeOperation; 36] = [
    PhyRfWakeOperation::SetBasebandMode { mode: 2 },
    PhyRfWakeOperation::ForceTxRxOff { enabled: true },
    PhyRfWakeOperation::SetHardwareFrequencyControl { enabled: false },
    PhyRfWakeOperation::ResetI2cMaster,
    PhyRfWakeOperation::OpenFrontendBasebandClocks,
    PhyRfWakeOperation::SetBbpllCalibration { enabled: true },
    PhyRfWakeOperation::ConfigureBiasRegisters,
    PhyRfWakeOperation::PowerTemperatureSensor,
    PhyRfWakeOperation::ReadXtalFrequency,
    PhyRfWakeOperation::OpenI2cPower,
    PhyRfWakeOperation::ClearPbusRegisters,
    PhyRfWakeOperation::ConfigureI2cClock,
    PhyRfWakeOperation::ResetFrontendTxRx,
    PhyRfWakeOperation::ConfigureAdcRate,
    PhyRfWakeOperation::ConfigureI2cMasterRegisters,
    PhyRfWakeOperation::ConfigureFrequencyRegisters,
    PhyRfWakeOperation::ConfigureFrontendRegisters,
    PhyRfWakeOperation::UpdateFrontendRegisters,
    PhyRfWakeOperation::ConfigurePowerDetectorRegisters,
    PhyRfWakeOperation::ConfigureI2cStageTwo,
    PhyRfWakeOperation::PublishRetainedFrequencyI2c,
    PhyRfWakeOperation::RestoreChannelFrequency,
    PhyRfWakeOperation::RestorePbusBoundaries,
    PhyRfWakeOperation::ConfigurePhyRegisters,
    PhyRfWakeOperation::UpdatePhyRegisters,
    PhyRfWakeOperation::UpdateAgcRegisters,
    PhyRfWakeOperation::RestoreChannelRegisters,
    PhyRfWakeOperation::RestoreTxCapacitance,
    PhyRfWakeOperation::RestoreBasebandChannelWidth,
    PhyRfWakeOperation::EnableAgc,
    PhyRfWakeOperation::WaitFrequencyReady,
    PhyRfWakeOperation::ResetClockGenerator,
    PhyRfWakeOperation::SetHardwareFrequencyControl { enabled: true },
    PhyRfWakeOperation::SetBbpllCalibration { enabled: false },
    PhyRfWakeOperation::ForceTxRxOff { enabled: false },
    PhyRfWakeOperation::SetBasebandMode { mode: 0 },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRfWakeOutcome;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfWakeAction {
    Execute(PhyRfWakeOperation),
    Complete(PhyRfWakeOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRfWakeCompletion {
    operation: PhyRfWakeOperation,
}

impl PhyRfWakeCompletion {
    pub const fn executed(operation: PhyRfWakeOperation) -> Self {
        Self { operation }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfWakeTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

/// Source-owned parent ordering for one retained RF wake transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRfWakeTransition {
    index: u8,
}

impl PhyRfWakeTransition {
    pub const fn new() -> Self {
        Self { index: 0 }
    }

    pub const fn action(self) -> PhyRfWakeAction {
        if self.index as usize == PHY_RF_WAKE_OPERATIONS.len() {
            PhyRfWakeAction::Complete(PhyRfWakeOutcome)
        } else {
            PhyRfWakeAction::Execute(PHY_RF_WAKE_OPERATIONS[self.index as usize])
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRfWakeCompletion,
    ) -> Result<(), PhyRfWakeTransitionError> {
        let PhyRfWakeAction::Execute(operation) = self.action() else {
            return Err(PhyRfWakeTransitionError::AlreadyComplete);
        };
        if completion.operation != operation {
            return Err(PhyRfWakeTransitionError::WrongCompletion);
        }
        self.index += 1;
        Ok(())
    }
}

impl Default for PhyRfWakeTransition {
    fn default() -> Self {
        Self::new()
    }
}

/// One ordered edge in complete current-vendor `phy_close_rf`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PhyRfCloseOperation {
    EnterCritical,
    DisableHardwareFrequencyControl,
    SettleOneMicrosecond,
    DisableAgc,
    ClearBasebandControl,
    WriteI2c { address: PhyI2cAddress, value: u8 },
    PowerOffRfCircuits,
    ClearImmediateClockPower,
    CloseFrontendBasebandClocks,
    EnableBbpllCalibration,
    ExitCritical,
}

/// Exact default-profile close graph after the vendor's pre-close temperature
/// observation. The S31 critical-section callbacks are empty functions, but
/// their boundaries remain explicit so a future implementation cannot move
/// physical work across them accidentally.
pub(crate) const PHY_RF_CLOSE_OPERATIONS: [PhyRfCloseOperation; 13] = [
    PhyRfCloseOperation::EnterCritical,
    PhyRfCloseOperation::DisableHardwareFrequencyControl,
    PhyRfCloseOperation::DisableAgc,
    PhyRfCloseOperation::ClearBasebandControl,
    PhyRfCloseOperation::SettleOneMicrosecond,
    PhyRfCloseOperation::WriteI2c {
        address: analog_registers::RF_CLOSE_CONTROL,
        value: 0x07,
    },
    PhyRfCloseOperation::PowerOffRfCircuits,
    PhyRfCloseOperation::ClearImmediateClockPower,
    PhyRfCloseOperation::CloseFrontendBasebandClocks,
    PhyRfCloseOperation::EnableBbpllCalibration,
    PhyRfCloseOperation::WriteI2c {
        address: analog_registers::RF_CLOSE_RETENTION_ZERO,
        value: 0x77,
    },
    PhyRfCloseOperation::WriteI2c {
        address: analog_registers::RF_CLOSE_RETENTION_ONE,
        value: 0x77,
    },
    PhyRfCloseOperation::ExitCritical,
];

/// Drive the complete close graph through one target-owned operation port.
///
/// The caller supplies the hardware binding. Returning an error stops before
/// every later edge, so the outer owner can retain the exact poisoned
/// frontier instead of inventing cleanup after an ambiguous write.
pub(crate) fn drive_rf_close<E>(
    mut execute: impl FnMut(PhyRfCloseOperation) -> Result<(), E>,
) -> Result<(), E> {
    for operation in PHY_RF_CLOSE_OPERATIONS {
        execute(operation)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_wake_parent_has_a_safe_envelope_and_terminal_completion() {
        let mut transition = PhyRfWakeTransition::new();
        let mut trace = std::vec::Vec::new();
        while let PhyRfWakeAction::Execute(operation) = transition.action() {
            trace.push(operation);
            transition
                .advance(PhyRfWakeCompletion::executed(operation))
                .unwrap();
        }
        assert_eq!(trace.len(), 36);
        assert_eq!(
            &trace[..3],
            &[
                PhyRfWakeOperation::SetBasebandMode { mode: 2 },
                PhyRfWakeOperation::ForceTxRxOff { enabled: true },
                PhyRfWakeOperation::SetHardwareFrequencyControl { enabled: false },
            ]
        );
        assert_eq!(
            &trace[32..],
            &[
                PhyRfWakeOperation::SetHardwareFrequencyControl { enabled: true },
                PhyRfWakeOperation::SetBbpllCalibration { enabled: false },
                PhyRfWakeOperation::ForceTxRxOff { enabled: false },
                PhyRfWakeOperation::SetBasebandMode { mode: 0 },
            ]
        );
        assert_eq!(
            transition.action(),
            PhyRfWakeAction::Complete(PhyRfWakeOutcome)
        );
        assert_eq!(
            transition.advance(PhyRfWakeCompletion::executed(
                PhyRfWakeOperation::SetBasebandMode { mode: 0 }
            )),
            Err(PhyRfWakeTransitionError::AlreadyComplete)
        );
    }

    #[test]
    fn retained_wake_parent_rejects_out_of_order_completion_without_progress() {
        let mut transition = PhyRfWakeTransition::new();
        assert_eq!(
            transition.advance(PhyRfWakeCompletion::executed(
                PhyRfWakeOperation::ResetI2cMaster
            )),
            Err(PhyRfWakeTransitionError::WrongCompletion)
        );
        assert_eq!(
            transition.action(),
            PhyRfWakeAction::Execute(PhyRfWakeOperation::SetBasebandMode { mode: 2 })
        );
    }

    #[test]
    fn close_driver_observes_the_complete_critical_section() {
        let mut trace = std::vec::Vec::new();
        drive_rf_close::<core::convert::Infallible>(|operation| {
            trace.push(operation);
            Ok(())
        })
        .unwrap();

        assert_eq!(trace.len(), PHY_RF_CLOSE_OPERATIONS.len());
        assert_eq!(trace.first(), Some(&PhyRfCloseOperation::EnterCritical));
        assert_eq!(trace.last(), Some(&PhyRfCloseOperation::ExitCritical));
        assert_eq!(
            trace
                .iter()
                .filter(|operation| matches!(operation, PhyRfCloseOperation::SettleOneMicrosecond))
                .count(),
            1
        );
        assert_eq!(
            trace
                .iter()
                .filter(|operation| matches!(operation, PhyRfCloseOperation::WriteI2c { .. }))
                .count(),
            3
        );
    }

    #[test]
    fn close_driver_stops_at_the_first_failed_hardware_edge() {
        let mut trace = std::vec::Vec::new();
        let result = drive_rf_close(|operation| {
            trace.push(operation);
            if operation == PhyRfCloseOperation::DisableAgc {
                Err("agc-write-failed")
            } else {
                Ok(())
            }
        });

        assert_eq!(result, Err("agc-write-failed"));
        assert_eq!(trace.last(), Some(&PhyRfCloseOperation::DisableAgc));
        assert!(!trace.contains(&PhyRfCloseOperation::ClearBasebandControl));
    }
}
