//! Owned ESP32-S31 PHY power-detector register leaves.
//!
//! These methods own only finite MMIO. Delays, readiness repetition, PBus
//! transactions, calibration policy, and cleanup sequencing remain explicit
//! in the PHY state machines.

#[cfg(target_arch = "riscv32")]
use crate::{
    owner::{SharedPhyAccess, phy_parts_mut},
    phy::{
        baseband::ops,
        restore::{TxDcPwdetLifecycleError, TxDcPwdetPrepareError, TxDcPwdetRestoreError},
    },
    phy_blocks_mut,
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
    crate::phy::sequence::initialize_power_detector(phy_parts_mut(registers))
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
    crate::phy::sequence::configure_background_power_detector(phy_blocks_mut(registers));
}

/// Configure the power-detector/SAR path without enabling background control.
///
/// Complete rev0 ROM `phy_en_pwdet` at `0x2f82_63da`, size `0x26`, and its
/// complete `phy_pwdet_sar2_init` callee at `0x2f82_63a6`, size `0x34`,
/// supply the eight ordered operations.
#[cfg(target_arch = "riscv32")]
pub fn configure_enabled(registers: &mut impl SharedPhyAccess) {
    crate::phy::sequence::enable_power_detector(phy_blocks_mut(registers));
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
    crate::phy::sequence::prepare_txdc(phy_parts_mut(registers))
}

/// Select the TX-DC PWDET SAR mode after the initial PBus setup.
///
/// Pinned esp-phy-lib `phy_txdc_cal_pwdet_init`, size
/// `0x232`, replaces the two-bit PAC SAR-mode field with one at this point.
#[cfg(target_arch = "riscv32")]
pub fn configure_txdc_sar(registers: &mut impl SharedPhyAccess) {
    ops::configure_txdc_power_detector_sar(
        &mut phy_blocks_mut(registers).phy_baseband_config_oracle,
    );
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
    crate::phy::sequence::restore_txdc(phy_parts_mut(registers))
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
    ops::write_power_detector_reference(
        &mut phy_blocks_mut(registers).phy_baseband_config_oracle,
        value,
    );
}

/// Pulse the power-detector SAR trigger.
///
/// Rev0 ROM `phy_pwdet_tone_start` at `0x2f82_643c` clears then sets
/// `SAR_TRIGGER` at +0x28/+0x34 through fresh reads and requests a 2 µs
/// post-trigger wait at +0x38. Its caller `phy_get_tone_sar_dout_` invokes
/// tone-start once per sample; the wait remains in the Rust caller.
/// SOURCE: ermacv/oer-notes research/opaque-blobray-phy.md, SAR/PWDET.
#[cfg(target_arch = "riscv32")]
pub fn trigger_sar(registers: &mut impl SharedPhyAccess) {
    ops::trigger_power_detector_sar(&mut phy_blocks_mut(registers).phy_baseband_config_oracle);
}

/// Read the power-detector readiness state through its SVD field.
///
/// Rev0 ROM `phy_pwdet_tone_start+0x42` and `phy_pwdet_wait_idle+0x6`
/// sample the three-bit field and compare it with 7. The wrapper
/// `phy_get_tone_sar_dout_` calls tone-start; repetition and deadline
/// ownership remain in the Rust state machine.
#[cfg(target_arch = "riscv32")]
pub fn sample_ready(registers: &mut impl SharedPhyAccess) -> bool {
    ops::power_detector_ready(&phy_blocks_mut(registers).phy_baseband_config_oracle)
}

/// Read one power-detector SAR sample through its SVD field.
///
/// Complete rev0 ROM `phy_get_tone_sar_dout_` at `0x2f82_66da`, size `0x40`,
/// consumes the upper thirteen-bit sample from `POWER_DETECTOR_SAR_RESULT`.
#[cfg(target_arch = "riscv32")]
pub fn sample_sar(registers: &mut impl SharedPhyAccess) -> u16 {
    ops::power_detector_sar_sample(&phy_blocks_mut(registers).phy_baseband_config_oracle)
}

pub mod platform;
