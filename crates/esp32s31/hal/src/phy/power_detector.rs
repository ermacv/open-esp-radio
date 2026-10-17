//! Owned ESP32-S31 PHY power-detector register leaves.
//!
//! These methods own only finite MMIO. Delays, readiness repetition, PBus
//! transactions, calibration policy, and cleanup sequencing remain explicit
//! in the PHY state machines.

#[cfg(target_arch = "riscv32")]
use crate::{
    owner::{SharedPhyAccess, phy_parts_mut},
    phy::restore::{TxDcPwdetLifecycleError, TxDcPwdetPrepareError, TxDcPwdetRestoreError},
    phy_pac_mut,
};

/// Apply the complete power-detector register initialization leaf.
///
/// Complete rev0 ROM `phy_pwdet_reg_init` at `0x2f82_634a`, size `0x5c`,
/// performs six finite stores through the PAC's table-zero, table-one,
/// control, reference, and auxiliary-mode identities.
#[cfg(target_arch = "riscv32")]
pub fn initialize_registers(
    registers: &mut impl SharedPhyAccess,
) -> Result<(), TxDcPwdetLifecycleError> {
    let (phy, restore) = phy_parts_mut(registers);
    if restore.txdc_pending() {
        return Err(TxDcPwdetLifecycleError::RestorePending);
    }
    phy.initialize_power_detector_registers();
    crate::phy::power_detector::platform::select_initialization_mode(registers);
    Ok(())
}

/// Configure and enable the background TX power-control path.
///
/// Complete rev0 ROM `phy_tx_pwctrl_bg_init` at `0x2f82_67f6`, size `0x1e`,
/// calls complete `phy_en_pwdet` (`0x2f82_63da`, size `0x26`) and
/// `phy_pwdet_sar2_init` (`0x2f82_63a6`, size `0x34`), then sets bit 16 at
/// `POWER_DETECTOR_CONTROL`. All eight fresh-read/full-write operations are
/// preserved.
#[cfg(target_arch = "riscv32")]
pub fn configure_background(registers: &mut impl SharedPhyAccess) {
    configure_enabled(registers);
    phy_pac_mut(registers).enable_power_detector_background_control();
}

/// Configure the power-detector/SAR path without enabling background control.
///
/// Complete rev0 ROM `phy_en_pwdet` at `0x2f82_63da`, size `0x26`, and its
/// complete `phy_pwdet_sar2_init` callee at `0x2f82_63a6`, size `0x34`,
/// supply the eight ordered operations.
#[cfg(target_arch = "riscv32")]
pub fn configure_enabled(registers: &mut impl SharedPhyAccess) {
    phy_pac_mut(registers).configure_power_detector_enabled();
    crate::phy::power_detector::platform::select_enabled_mode(registers);
}

/// Select the auxiliary power-detector calibration mode.
///
/// Complete rev0 ROM `phy_txcal_debuge_mode_` at `0x2f82_44fe`, size `0x56`,
/// replaces the PAC's three-bit auxiliary-mode field with mode two after
/// enabling PWDET.
#[cfg(target_arch = "riscv32")]
pub fn configure_calibration_mode(registers: &mut impl SharedPhyAccess) {
    crate::phy::power_detector::platform::select_calibration_mode(registers);
}

/// Prepare the two TX-DC power-detector fields for calibration.
///
/// Pinned esp-phy-lib `phy_txdc_cal_pwdet_init`, size
/// `0x232`, first reads `POWER_DETECTOR_TABLE_1` and
/// `POWER_DETECTOR_CONTROL`, then replaces the low table byte with `0xf0` and
/// the calibration field with `0x78`. The route restore slot retains the
/// original fields and rejects an overlapping calibration before any MMIO.
#[cfg(target_arch = "riscv32")]
pub fn prepare_txdc_calibration(
    registers: &mut impl SharedPhyAccess,
) -> Result<(), TxDcPwdetPrepareError> {
    let (phy, restore) = phy_parts_mut(registers);
    restore.prepare_txdc_with(
        phy,
        |phy| phy.capture_txdc_power_detector(),
        |phy| phy.apply_txdc_power_detector_calibration(),
    )
}

/// Select the TX-DC PWDET SAR mode after the initial PBus setup.
///
/// Pinned esp-phy-lib `phy_txdc_cal_pwdet_init`, size
/// `0x232`, replaces the two-bit PAC SAR-mode field with one at this point.
#[cfg(target_arch = "riscv32")]
pub fn configure_txdc_sar(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.configure_txdc_power_detector_sar();
}

/// Restore the saved TX-DC fields and select the final SAR mode.
///
/// The cleanup-enabled tail of the pinned esp-phy-lib
/// `phy_txdc_cal_pwdet_init`, size `0x232`, restores
/// the table-one low byte and control calibration field, then sets the
/// two-bit SAR-mode field. A caller without a pending restore operation is
/// rejected before any MMIO.
#[cfg(target_arch = "riscv32")]
pub fn restore_txdc_calibration(
    registers: &mut impl SharedPhyAccess,
) -> Result<(), TxDcPwdetRestoreError> {
    let (phy, restore) = phy_parts_mut(registers);
    restore.restore_txdc_with(|fields| phy.restore_txdc_power_detector(fields))
}

/// Publish one power-detector reference word.
///
/// Complete rev0 ROM `phy_get_tone_sar_dout_` at `0x2f82_66da`, size `0x40`,
/// and its complete callers write the evidenced `0`, `0x5555`, or `0xaaaa`
/// values to `POWER_DETECTOR_REFERENCE`; PWDET SAR initialization additionally
/// uses `0x016a`. This finite leaf performs the full 32-bit zero-extended
/// store.
#[cfg(target_arch = "riscv32")]
pub fn write_reference(registers: &mut impl SharedPhyAccess, value: u16) {
    let registers = phy_pac_mut(registers);
    registers.write_power_detector_reference(value);
}

/// Pulse the power-detector SAR trigger.
///
/// Complete rev0 ROM `phy_get_tone_sar_dout_` at `0x2f82_66da`, size `0x40`,
/// clears then sets the PAC `SAR_TRIGGER` bit through two fresh reads, with the
/// one-microsecond delay remaining in the caller.
#[cfg(target_arch = "riscv32")]
pub fn trigger_sar(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.trigger_power_detector_sar();
}

/// Read the power-detector readiness state through its SVD field.
///
/// Complete rev0 ROM `phy_get_tone_sar_dout_` at `0x2f82_66da`, size `0x40`,
/// samples `POWER_DETECTOR_SAR_CONTROL_STATUS`; repetition and deadline
/// ownership remain in the Rust async state machine.
#[cfg(target_arch = "riscv32")]
pub fn sample_ready(registers: &mut impl SharedPhyAccess) -> bool {
    let registers = phy_pac_mut(registers);
    registers.power_detector_ready()
}

/// Read one power-detector SAR sample through its SVD field.
///
/// Complete rev0 ROM `phy_get_tone_sar_dout_` at `0x2f82_66da`, size `0x40`,
/// consumes the upper thirteen-bit sample from `POWER_DETECTOR_SAR_RESULT`.
#[cfg(target_arch = "riscv32")]
pub fn sample_sar(registers: &mut impl SharedPhyAccess) -> u16 {
    let registers = phy_pac_mut(registers);
    registers.power_detector_sar_sample()
}

pub mod platform;
