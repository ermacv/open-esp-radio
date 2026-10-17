//! Shared PHY operations used by the ESP32-S31 Bluetooth calibration graph.
//!
//! Bluetooth-specific state and table publication remain explicitly named;
//! they are not folded into the Wi-Fi channel layer. The first recovered
//! leaves are the complete rev0 ROM conversions used by
//! `phy_bt_tx_gain_init`.

/// Convert the three Bluetooth gain indices to their baseband encodings.
///
/// Basis: complete rev0 ROM `phy_bt_index_to_bb` at `0x2f82_6b1a`, size
/// `0x1c`. All values outside the two nonzero indices map to zero.
#[inline]
pub const fn bluetooth_gain_index_to_baseband(index: u32) -> u32 {
    match index {
        1 => 0x80,
        2 => 0x100,
        _ => 0,
    }
}

/// Convert a Bluetooth baseband gain encoding back to its table index.
///
/// Basis: complete rev0 ROM `phy_bt_bb_to_index` at `0x2f82_6b36`, size
/// `0x1c`. Noncanonical encodings map to index zero exactly as in ROM.
#[inline]
#[cfg(any(test, feature = "validation-probes"))]
pub const fn bluetooth_baseband_to_gain_index(baseband: u32) -> u32 {
    match baseband {
        0x80 => 1,
        0x100 => 2,
        _ => 0,
    }
}

// ESP32-S31 BT/154 gain profile from `phy_bt_get_tx_tab_new` in esp-phy-lib
// 20f1db053a0e6cb9f1c09d255c43bf42483041d0, libphy.a SHA-256
// 116c7c3a55e2ddfd7937edad2667b5d64717f1967519a0201ca6354246f0b1a2.
// phy_tx_gain.o .rodata offsets 0/0x24/0x48: 18 little-endian halfwords each.
// LOW/MID encode RF/baseband settings; HIGH contains signed gain references.
// Physical gain units are not established. This profile retains subtractive
// attenuation and is independent of the current Wi-Fi profile and adjustment.
const BLUETOOTH_TX_GAIN_TABLE_LOW: [u16; 18] = [
    0x003f, 0x002f, 0x001f, 0x0016, 0x000f, 0x000e, 0x000d, 0x000d, 0x000c, 0x000b, 0x0005, 0x0004,
    0x0003, 0x0002, 0x0001, 0x0001, 0x0000, 0x0000,
];
const BLUETOOTH_TX_GAIN_TABLE_MID: [u16; 18] = [
    0x0100, 0x0100, 0x0100, 0x0100, 0x0100, 0x0100, 0x0100, 0x0080, 0x0080, 0x0080, 0x0080, 0x0080,
    0x0080, 0x0080, 0x0080, 0x0000, 0x0080, 0x0000,
];
const BLUETOOTH_TX_GAIN_TABLE_HIGH: [u16; 18] = [
    0x002f, 0x0027, 0x001a, 0x000c, 0x0005, 0x0000, 0xfffb, 0xfff3, 0xffed, 0xffe5, 0xffda, 0xffd4,
    0xffcc, 0xffc2, 0xffb3, 0xffab, 0xff9b, 0xff93,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxGainParameters {
    pub seed: [u32; 6],
    pub config: u16,
    pub calibration_curve: [u8; 3],
    pub correction: i8,
    pub base: u8,
    pub attenuation: u8,
}

/// Canonical 16-entry Bluetooth gain table before its PAC-backed publication.
///
/// The three arrays correspond to semantic gain components, not to a C stack
/// layout. Keeping them separate lets the Rust implementation reuse the
/// shared hardware encoder without exposing the vendor's temporary buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxGainImage {
    pub seed: [u32; 6],
    pub output_32: [u8; 16],
    pub output_64: [u16; 16],
    pub output_72: [u16; 16],
    pub config: u16,
}

const fn select_bluetooth_tx_gain_index(mut index: u8, target: i16) -> u8 {
    let mut iteration = 0;
    while iteration != BLUETOOTH_TX_GAIN_TABLE_HIGH.len() {
        let current = BLUETOOTH_TX_GAIN_TABLE_HIGH[index as usize] as i16;
        if target < current {
            if index as usize == BLUETOOTH_TX_GAIN_TABLE_HIGH.len() - 1 {
                break;
            }
            index += 1;
            if target >= BLUETOOTH_TX_GAIN_TABLE_HIGH[index as usize] as i16 {
                break;
            }
        } else {
            if index == 0 {
                break;
            }
            if target < BLUETOOTH_TX_GAIN_TABLE_HIGH[index as usize - 1] as i16 {
                break;
            }
            index -= 1;
        }
        iteration += 1;
    }
    index
}

/// Pure Rust translation of archive `phy_bt_get_tx_tab_new` and complete ROM
/// children `phy_bt_get_tx_gain`, `phy_get_data_sat` and
/// `phy_get_tx_gain_value`.
///
/// The debug-print argument used by the vendor caller is zero and is omitted.
/// Arithmetic retains the vendor's signed 8/16-bit domains explicitly.
pub const fn calculate_bluetooth_tx_gain(
    parameters: PhyBluetoothTxGainParameters,
) -> PhyBluetoothTxGainImage {
    let mut image = PhyBluetoothTxGainImage {
        seed: parameters.seed,
        output_32: [0; 16],
        output_64: [0; 16],
        output_72: [0; 16],
        config: parameters.config,
    };
    let base_delta = parameters.base.wrapping_sub(parameters.attenuation) as i8 as i16;
    let interpolation = parameters.calibration_curve[1] as i8 as i16;
    let mut driver = -96_i16;
    let mut table_index = 0_u8;
    let mut output_index = 0;
    while output_index != 16 {
        let target = base_delta
            .wrapping_sub(parameters.correction as i16)
            .wrapping_add(crate::calibration::math::saturate_signed(driver as i32, 80, -72) as i16);
        table_index = select_bluetooth_tx_gain_index(table_index, target);
        let table_index = table_index as usize;
        image.output_72[output_index] = BLUETOOTH_TX_GAIN_TABLE_LOW[table_index];
        image.output_64[output_index] = BLUETOOTH_TX_GAIN_TABLE_MID[table_index];
        let residual = target
            .wrapping_sub(BLUETOOTH_TX_GAIN_TABLE_HIGH[table_index] as i16)
            .wrapping_sub(interpolation);
        image.output_32[output_index] =
            crate::calibration::math::saturate_signed(residual as i32, 24, -60) as u8;
        driver = driver.wrapping_add(12);
        output_index += 1;
    }
    image
}

/// Direct PAC-backed publication edge for one canonical Bluetooth gain image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxGainPublication {
    image: PhyBluetoothTxGainImage,
}

impl PhyBluetoothTxGainPublication {
    pub const fn new(image: PhyBluetoothTxGainImage) -> Self {
        Self { image }
    }

    pub const fn image(self) -> PhyBluetoothTxGainImage {
        self.image
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(self, registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess) {
        crate::hardware::publish_bluetooth_tx_gain_memory(registers, self.image);
    }
}

/// One edge of the Bluetooth gain child.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxGainChildAction {
    ForceTxRx(crate::analog::pbus::PhyForceTxRxAction),
    Publish(PhyBluetoothTxGainPublication),
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxGainChildCompletion {
    ForceTxRx(crate::analog::pbus::PhyForceTxRxCompletion),
    Published(PhyBluetoothTxGainPublication),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyBluetoothTxGainChildStep {
    Enter(crate::analog::pbus::PhyForceTxRxTransition),
    Publish,
    Leave(crate::analog::pbus::PhyForceTxRxTransition),
    Complete,
}

/// Archive `phy_bt_set_tx_gain_new` after its gain calculation: gain-memory
/// publication bracketed by a `phy_force_txrx_off_new` pair entered at
/// `depth`. The grant callbacks around the pair are empty on this target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxGainChild {
    publication: PhyBluetoothTxGainPublication,
    depth: crate::analog::pbus::PhyForceTxRxDepth,
    step: PhyBluetoothTxGainChildStep,
}

impl PhyBluetoothTxGainChild {
    pub const fn new(
        publication: PhyBluetoothTxGainPublication,
        depth: crate::analog::pbus::PhyForceTxRxDepth,
    ) -> Self {
        Self {
            publication,
            depth,
            step: match depth.enter() {
                Some(child) => PhyBluetoothTxGainChildStep::Enter(child),
                None => PhyBluetoothTxGainChildStep::Publish,
            },
        }
    }

    pub const fn publication(&self) -> PhyBluetoothTxGainPublication {
        self.publication
    }

    pub const fn action(&self) -> PhyBluetoothTxGainChildAction {
        match self.step {
            PhyBluetoothTxGainChildStep::Enter(child)
            | PhyBluetoothTxGainChildStep::Leave(child) => {
                PhyBluetoothTxGainChildAction::ForceTxRx(child.action())
            }
            PhyBluetoothTxGainChildStep::Publish => {
                PhyBluetoothTxGainChildAction::Publish(self.publication)
            }
            PhyBluetoothTxGainChildStep::Complete => PhyBluetoothTxGainChildAction::Complete,
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyBluetoothTxGainChildCompletion,
    ) -> Result<(), PhyBluetoothTxGainInitTransitionError> {
        use crate::analog::pbus::PhyForceTxRxAction;
        use PhyBluetoothTxGainChildStep as Step;
        self.step = match (self.step, completion) {
            (Step::Enter(mut child), PhyBluetoothTxGainChildCompletion::ForceTxRx(completion)) => {
                child
                    .advance(completion)
                    .map_err(|_| PhyBluetoothTxGainInitTransitionError::WrongCompletion)?;
                match child.action() {
                    PhyForceTxRxAction::Complete { .. } => Step::Publish,
                    _ => Step::Enter(child),
                }
            }
            (Step::Publish, PhyBluetoothTxGainChildCompletion::Published(published))
                if published == self.publication =>
            {
                match self.depth.leave() {
                    Some(child) => Step::Leave(child),
                    None => Step::Complete,
                }
            }
            (Step::Leave(mut child), PhyBluetoothTxGainChildCompletion::ForceTxRx(completion)) => {
                child
                    .advance(completion)
                    .map_err(|_| PhyBluetoothTxGainInitTransitionError::WrongCompletion)?;
                match child.action() {
                    PhyForceTxRxAction::Complete { .. } => Step::Complete,
                    _ => Step::Leave(child),
                }
            }
            (Step::Complete, _) => {
                return Err(PhyBluetoothTxGainInitTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyBluetoothTxGainInitTransitionError::WrongCompletion),
        };
        Ok(())
    }

    /// Execute the complete child as one blocking target transaction.
    ///
    /// Production steps this child inside the TX-gain initialization graph;
    /// only the standalone vendor comparison executes it alone.
    #[cfg(all(target_arch = "riscv32", feature = "validation-probes"))]
    pub fn execute_target<D: crate::executor::wait::PhyShortDelay>(
        mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<(), crate::target_executor::PhyTargetPortError> {
        use crate::{
            analog::pbus::PhyForceTxRxExternalBinding, target_executor::PhyTargetPortError,
        };
        loop {
            let completion = match self.action() {
                PhyBluetoothTxGainChildAction::Complete => return Ok(()),
                PhyBluetoothTxGainChildAction::Publish(publication) => {
                    publication.execute_target(registers);
                    PhyBluetoothTxGainChildCompletion::Published(publication)
                }
                PhyBluetoothTxGainChildAction::ForceTxRx(action) => {
                    let binding = PhyForceTxRxExternalBinding::lower(action)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                    PhyBluetoothTxGainChildCompletion::ForceTxRx(match binding {
                        PhyForceTxRxExternalBinding::Mmio(binding) => {
                            binding.execute_target(registers)
                        }
                        PhyForceTxRxExternalBinding::Timer(binding) => {
                            if !D::settle_micros(binding.micros()) {
                                return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
                            }
                            binding.into_completion()
                        }
                    })
                }
            };
            self.advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
    }
}

/// Rust-owned implementation of archive `phy_bt_txdc_cal_new`.
///
/// The common comparator search remains in `phy_txdc`; this wrapper gives the
/// Bluetooth graph an explicit type instead of disguising it as Wi-Fi state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxDcTransition {
    inner: crate::tx::dc_offset::PhyTxDcTransition,
}

/// Bluetooth-owned use of the shared power-detector TX-DC calibration.
///
/// This keeps archive `phy_txdc_cal_pwdet_init(1, 0, 1)` distinct from the
/// Wi-Fi invocation while reusing its protocol-independent search engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxDcPwdetTransition {
    inner: crate::tx::dc_power_detector::PhyTxDcPwdetTransition,
}

/// Owned inputs of archive `phy_bt_tx_pwctrl_init`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxPowerParameters {
    pub calibration: crate::tx::power::PhyTxPowerParameters,
    pub pbus_power_path_value: u8,
    pub pbus_tx_path_value: u8,
    pub dco: [u16; 4],
    pub tone_selector: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxPowerOutcome {
    pub calibration: crate::tx::power::PhyTxPowerOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxPowerFailure {
    PbusTimedOut(crate::analog::pbus::PhyPbusForceTest),
    Calibration(crate::tx::power::PhyTxPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxPowerAction {
    I2cControl(oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlOperation),
    Prepare(crate::tx::calibration::PhyTxCalibrationEnvironmentAction),
    ForcePbus(crate::analog::pbus::PhyPbusForceTest),
    ReadPbus { selector: u8, path: u8 },
    Calibration(crate::tx::power::PhyTxPowerAction),
    Cleanup(crate::tx::calibration::PhyTxCalibrationEnvironmentAction),
    Complete(PhyBluetoothTxPowerOutcome),
    Failed(PhyBluetoothTxPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxPowerCompletion {
    I2cControl(oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlCompletion),
    Prepare(crate::tx::calibration::PhyTxCalibrationEnvironmentCompletion),
    PbusCompleted(crate::analog::pbus::PhyPbusForceTest),
    PbusTimedOut(crate::analog::pbus::PhyPbusForceTest),
    PbusRead { selector: u8, path: u8, value: u16 },
    Calibration(crate::tx::power::PhyTxPowerCompletion),
    Cleanup(crate::tx::calibration::PhyTxCalibrationEnvironmentCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxPowerTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyBluetoothTxPowerTerminal {
    Complete(crate::tx::power::PhyTxPowerOutcome),
    Failed(PhyBluetoothTxPowerFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyBluetoothTxPowerStep {
    PrepareI2cControlRestore,
    ConfigureI2cControl,
    Prepare(crate::tx::calibration::PhyTxCalibrationEnvironmentTransition),
    ForcePowerPath,
    ForceRxPath,
    ReadForcedPath,
    RestoreForcedPath {
        value: u16,
    },
    ForceTxPath,
    ProgramDco(u8),
    Calibration(crate::tx::power::PhyTxPowerTransition),
    RestoreI2cControl(PhyBluetoothTxPowerTerminal),
    Cleanup {
        terminal: PhyBluetoothTxPowerTerminal,
        transition: crate::tx::calibration::PhyTxCalibrationEnvironmentTransition,
    },
    Complete(PhyBluetoothTxPowerOutcome),
    Failed(PhyBluetoothTxPowerFailure),
}

const fn bluetooth_dco_transaction(
    dco: [u16; 4],
    index: u8,
) -> crate::analog::pbus::PhyPbusForceTest {
    match index {
        0 => crate::analog::pbus::PhyPbusForceTest::new(2, 1, dco[0]),
        1 => crate::analog::pbus::PhyPbusForceTest::new(3, 1, dco[1]),
        2 => crate::analog::pbus::PhyPbusForceTest::new(2, 2, dco[2]),
        _ => crate::analog::pbus::PhyPbusForceTest::new(3, 2, dco[3]),
    }
}

/// Complete Rust-owned state machine for archive `phy_bt_tx_pwctrl_init`.
///
/// The vendor's blocking PHY-I2C and PBus calls are represented as externally
/// completed actions. Temporary analog configuration is restored on both the
/// success path and every finite Rust failure path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxPowerTransition {
    parameters: PhyBluetoothTxPowerParameters,
    step: PhyBluetoothTxPowerStep,
}

impl PhyBluetoothTxPowerTransition {
    pub const fn new(parameters: PhyBluetoothTxPowerParameters) -> Self {
        let step = if parameters.calibration.already_calibrated {
            PhyBluetoothTxPowerStep::Complete(PhyBluetoothTxPowerOutcome {
                calibration: crate::tx::power::PhyTxPowerOutcome {
                    reference_codes: parameters.calibration.reference_codes,
                    power_curve: [0; 3],
                    point_corrections: [0; 3],
                    power_adjustment: 0,
                    final_attenuation: parameters.calibration.initial_attenuation,
                    current_channel: 0,
                    calibration_performed: false,
                },
            })
        } else {
            PhyBluetoothTxPowerStep::PrepareI2cControlRestore
        };
        Self { parameters, step }
    }

    fn cleanup(&mut self, terminal: PhyBluetoothTxPowerTerminal) {
        self.step = PhyBluetoothTxPowerStep::RestoreI2cControl(terminal);
    }

    pub const fn action(self) -> PhyBluetoothTxPowerAction {
        match self.step {
            PhyBluetoothTxPowerStep::PrepareI2cControlRestore => {
                PhyBluetoothTxPowerAction::I2cControl(
                    oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlOperation::PrepareRestore,
                )
            }
            PhyBluetoothTxPowerStep::ConfigureI2cControl => PhyBluetoothTxPowerAction::I2cControl(
                oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlOperation::ConfigureCalibration,
            ),
            PhyBluetoothTxPowerStep::Prepare(transition) => {
                PhyBluetoothTxPowerAction::Prepare(transition.action())
            }
            PhyBluetoothTxPowerStep::ForcePowerPath => {
                PhyBluetoothTxPowerAction::ForcePbus(crate::analog::pbus::PhyPbusForceTest::new(
                    5,
                    1,
                    self.parameters.pbus_power_path_value as u16 + 0x1c0,
                ))
            }
            PhyBluetoothTxPowerStep::ForceRxPath => PhyBluetoothTxPowerAction::ForcePbus(
                crate::analog::pbus::PhyPbusForceTest::new(1, 2, 0),
            ),
            PhyBluetoothTxPowerStep::ReadForcedPath => PhyBluetoothTxPowerAction::ReadPbus {
                selector: 1,
                path: 1,
            },
            PhyBluetoothTxPowerStep::RestoreForcedPath { value } => {
                PhyBluetoothTxPowerAction::ForcePbus(crate::analog::pbus::PhyPbusForceTest::new(
                    1,
                    1,
                    value | 2,
                ))
            }
            PhyBluetoothTxPowerStep::ForceTxPath => {
                PhyBluetoothTxPowerAction::ForcePbus(crate::analog::pbus::PhyPbusForceTest::new(
                    4,
                    2,
                    (self.parameters.pbus_tx_path_value as u16) << 3,
                ))
            }
            PhyBluetoothTxPowerStep::ProgramDco(index) => PhyBluetoothTxPowerAction::ForcePbus(
                bluetooth_dco_transaction(self.parameters.dco, index),
            ),
            PhyBluetoothTxPowerStep::Calibration(transition) => {
                PhyBluetoothTxPowerAction::Calibration(transition.action())
            }
            PhyBluetoothTxPowerStep::RestoreI2cControl(_) => PhyBluetoothTxPowerAction::I2cControl(
                oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlOperation::Restore,
            ),
            PhyBluetoothTxPowerStep::Cleanup { transition, .. } => {
                PhyBluetoothTxPowerAction::Cleanup(transition.action())
            }
            PhyBluetoothTxPowerStep::Complete(outcome) => {
                PhyBluetoothTxPowerAction::Complete(outcome)
            }
            PhyBluetoothTxPowerStep::Failed(failure) => PhyBluetoothTxPowerAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyBluetoothTxPowerCompletion,
    ) -> Result<(), PhyBluetoothTxPowerTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyBluetoothTxPowerStep::PrepareI2cControlRestore,
                PhyBluetoothTxPowerCompletion::I2cControl(
                    oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlCompletion::RestorePrepared,
                ),
            ) => PhyBluetoothTxPowerStep::ConfigureI2cControl,
            (
                PhyBluetoothTxPowerStep::ConfigureI2cControl,
                PhyBluetoothTxPowerCompletion::I2cControl(
                    oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlCompletion::CalibrationConfigured,
                ),
            ) => PhyBluetoothTxPowerStep::Prepare(
                crate::tx::calibration::PhyTxCalibrationEnvironmentTransition::enter(
                    self.parameters.calibration.environment,
                ),
            ),
            (
                PhyBluetoothTxPowerStep::Prepare(mut transition),
                PhyBluetoothTxPowerCompletion::Prepare(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyBluetoothTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    crate::tx::calibration::PhyTxCalibrationEnvironmentAction::Complete(
                        crate::tx::calibration::PhyTxCalibrationEnvironment::Debug,
                    ) => PhyBluetoothTxPowerStep::ForcePowerPath,
                    crate::tx::calibration::PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        self.cleanup(PhyBluetoothTxPowerTerminal::Failed(
                            PhyBluetoothTxPowerFailure::Calibration(
                                crate::tx::power::PhyTxPowerFailure::Environment(failure),
                            ),
                        ));
                        return Ok(());
                    }
                    _ => PhyBluetoothTxPowerStep::Prepare(transition),
                }
            }
            (PhyBluetoothTxPowerStep::ForcePowerPath, completion) => self.force_next(
                completion,
                crate::analog::pbus::PhyPbusForceTest::new(
                    5,
                    1,
                    self.parameters.pbus_power_path_value as u16 + 0x1c0,
                ),
                PhyBluetoothTxPowerStep::ForceRxPath,
            )?,
            (PhyBluetoothTxPowerStep::ForceRxPath, completion) => self.force_next(
                completion,
                crate::analog::pbus::PhyPbusForceTest::new(1, 2, 0),
                PhyBluetoothTxPowerStep::ReadForcedPath,
            )?,
            (
                PhyBluetoothTxPowerStep::ReadForcedPath,
                PhyBluetoothTxPowerCompletion::PbusRead {
                    selector: 1,
                    path: 1,
                    value,
                },
            ) => PhyBluetoothTxPowerStep::RestoreForcedPath { value },
            (PhyBluetoothTxPowerStep::RestoreForcedPath { value }, completion) => self.force_next(
                completion,
                crate::analog::pbus::PhyPbusForceTest::new(1, 1, value | 2),
                PhyBluetoothTxPowerStep::ForceTxPath,
            )?,
            (PhyBluetoothTxPowerStep::ForceTxPath, completion) => self.force_next(
                completion,
                crate::analog::pbus::PhyPbusForceTest::new(
                    4,
                    2,
                    (self.parameters.pbus_tx_path_value as u16) << 3,
                ),
                PhyBluetoothTxPowerStep::ProgramDco(0),
            )?,
            (PhyBluetoothTxPowerStep::ProgramDco(index), completion) => self.force_next(
                completion,
                bluetooth_dco_transaction(self.parameters.dco, index),
                if index == 3 {
                    PhyBluetoothTxPowerStep::Calibration(
                        crate::tx::power::PhyTxPowerTransition::new_bluetooth(
                            self.parameters.calibration,
                            self.parameters.tone_selector,
                        ),
                    )
                } else {
                    PhyBluetoothTxPowerStep::ProgramDco(index + 1)
                },
            )?,
            (
                PhyBluetoothTxPowerStep::Calibration(mut transition),
                PhyBluetoothTxPowerCompletion::Calibration(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyBluetoothTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    crate::tx::power::PhyTxPowerAction::Complete(outcome) => {
                        self.cleanup(PhyBluetoothTxPowerTerminal::Complete(outcome));
                        return Ok(());
                    }
                    crate::tx::power::PhyTxPowerAction::Failed(failure) => {
                        self.cleanup(PhyBluetoothTxPowerTerminal::Failed(
                            PhyBluetoothTxPowerFailure::Calibration(failure),
                        ));
                        return Ok(());
                    }
                    _ => PhyBluetoothTxPowerStep::Calibration(transition),
                }
            }
            (
                PhyBluetoothTxPowerStep::RestoreI2cControl(terminal),
                PhyBluetoothTxPowerCompletion::I2cControl(
                    oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlCompletion::Restored,
                ),
            ) => PhyBluetoothTxPowerStep::Cleanup {
                terminal,
                transition: crate::tx::calibration::PhyTxCalibrationEnvironmentTransition::exit(
                    self.parameters.calibration.environment,
                ),
            },
            (
                PhyBluetoothTxPowerStep::Cleanup {
                    terminal,
                    mut transition,
                },
                PhyBluetoothTxPowerCompletion::Cleanup(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyBluetoothTxPowerTransitionError::WrongCompletion)?;
                match transition.action() {
                    crate::tx::calibration::PhyTxCalibrationEnvironmentAction::Complete(
                        crate::tx::calibration::PhyTxCalibrationEnvironment::Work,
                    ) => match terminal {
                        PhyBluetoothTxPowerTerminal::Complete(calibration) => {
                            PhyBluetoothTxPowerStep::Complete(PhyBluetoothTxPowerOutcome {
                                calibration,
                            })
                        }
                        PhyBluetoothTxPowerTerminal::Failed(failure) => {
                            PhyBluetoothTxPowerStep::Failed(failure)
                        }
                    },
                    crate::tx::calibration::PhyTxCalibrationEnvironmentAction::Failed(failure) => {
                        PhyBluetoothTxPowerStep::Failed(PhyBluetoothTxPowerFailure::Calibration(
                            crate::tx::power::PhyTxPowerFailure::Environment(failure),
                        ))
                    }
                    _ => PhyBluetoothTxPowerStep::Cleanup {
                        terminal,
                        transition,
                    },
                }
            }
            (PhyBluetoothTxPowerStep::Complete(_) | PhyBluetoothTxPowerStep::Failed(_), _) => {
                return Err(PhyBluetoothTxPowerTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyBluetoothTxPowerTransitionError::WrongCompletion),
        };
        Ok(())
    }

    fn force_next(
        &mut self,
        completion: PhyBluetoothTxPowerCompletion,
        expected: crate::analog::pbus::PhyPbusForceTest,
        next: PhyBluetoothTxPowerStep,
    ) -> Result<PhyBluetoothTxPowerStep, PhyBluetoothTxPowerTransitionError> {
        match completion {
            PhyBluetoothTxPowerCompletion::PbusCompleted(transaction)
                if transaction == expected =>
            {
                Ok(next)
            }
            PhyBluetoothTxPowerCompletion::PbusTimedOut(transaction) if transaction == expected => {
                self.cleanup(PhyBluetoothTxPowerTerminal::Failed(
                    PhyBluetoothTxPowerFailure::PbusTimedOut(transaction),
                ));
                Ok(self.step)
            }
            _ => Err(PhyBluetoothTxPowerTransitionError::WrongCompletion),
        }
    }
}

impl PhyBluetoothTxDcTransition {
    pub const fn new(
        parameters: crate::tx::dc_offset::PhyTxDcParameters,
        tx_path_value: u8,
    ) -> Self {
        Self {
            inner: crate::tx::dc_offset::PhyTxDcTransition::new_bluetooth(
                parameters,
                tx_path_value,
            ),
        }
    }

    pub const fn action(self) -> crate::tx::dc_offset::PhyTxDcAction {
        self.inner.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::tx::dc_offset::PhyTxDcCompletion,
    ) -> Result<(), crate::tx::dc_offset::PhyTxDcTransitionError> {
        self.inner.advance(completion)
    }
}

impl PhyBluetoothTxDcPwdetTransition {
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn transition_mut(
        &mut self,
    ) -> &mut crate::tx::dc_power_detector::PhyTxDcPwdetTransition {
        &mut self.inner
    }

    pub const fn new(
        parameters: crate::tx::dc_power_detector::PhyTxDcPwdetParameters,
        tx_path_value: u8,
    ) -> Self {
        Self {
            inner: crate::tx::dc_power_detector::PhyTxDcPwdetTransition::new_bluetooth(
                parameters,
                tx_path_value,
            ),
        }
    }

    pub const fn action(&self) -> crate::tx::dc_power_detector::PhyTxDcPwdetAction {
        self.inner.action()
    }

    pub fn advance(
        &mut self,
        completion: crate::tx::dc_power_detector::PhyTxDcPwdetCompletion,
    ) -> Result<(), crate::tx::dc_power_detector::PhyTxDcPwdetTransitionError> {
        self.inner.advance(completion)
    }
}

/// Complete semantic inputs of archive `phy_bt_tx_gain_init`.
///
/// The values are grouped by the Rust transitions which consume them; no
/// field represents an offset in the former vendor parameter image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxGainInitParameters {
    pub crystal_selector: u8,
    pub capacitance: [u8; 6],
    pub tx_dc_calibrated: bool,
    pub tx_dc: crate::tx::dc_offset::PhyTxDcParameters,
    pub tx_path_value: u8,
    pub tx_power: PhyBluetoothTxPowerParameters,
    pub tx_dc_pwdet: crate::tx::dc_power_detector::PhyTxDcPwdetParameters,
    pub gain: PhyBluetoothTxGainParameters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxGainInitOutcome {
    pub tx_dc_calibrated: bool,
    pub dco: [[u16; 4]; 3],
    pub tx_power: PhyBluetoothTxPowerOutcome,
    pub gain: PhyBluetoothTxGainImage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxGainInitFailure {
    Rfpll(crate::analog::rfpll::RfpllFrequencyFailure),
    TxDc(crate::tx::dc_offset::PhyTxDcFailure),
    TxPower(PhyBluetoothTxPowerFailure),
    TxDcPwdet(crate::tx::dc_power_detector::PhyTxDcPwdetFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxGainInitAction {
    Rfpll(crate::analog::rfpll::RfpllFrequencyAction),
    TxCap(crate::tx::power::PhyTxPowerAction),
    TxDc(crate::tx::dc_offset::PhyTxDcAction),
    TxPower(PhyBluetoothTxPowerAction),
    TxDcPwdet(crate::tx::dc_power_detector::PhyTxDcPwdetAction),
    /// One edge of the gain child's `phy_force_txrx_off_new` pair.
    ForceTxRx(crate::analog::pbus::PhyForceTxRxAction),
    Publish(PhyBluetoothTxGainPublication),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxGainInitCompletion {
    Rfpll(crate::analog::rfpll::RfpllFrequencyCompletion),
    TxCap(crate::tx::power::PhyTxPowerCompletion),
    TxDc(crate::tx::dc_offset::PhyTxDcCompletion),
    TxPower(PhyBluetoothTxPowerCompletion),
    TxDcPwdet(crate::tx::dc_power_detector::PhyTxDcPwdetCompletion),
    ForceTxRx(crate::analog::pbus::PhyForceTxRxCompletion),
    Published(PhyBluetoothTxGainPublication),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxGainInitLocalStep {
    StateAdvanced,
    External(PhyBluetoothTxGainInitAction),
    Complete(PhyBluetoothTxGainInitOutcome),
    Failed(PhyBluetoothTxGainInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxGainInitTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyBluetoothTxGainInitStep {
    Rfpll(crate::analog::rfpll::RfpllFrequencyTransition),
    TxCap,
    TxDc(PhyBluetoothTxDcTransition),
    TxPower(PhyBluetoothTxPowerTransition),
    TxDcPwdet(PhyBluetoothTxDcPwdetTransition),
    Gain(PhyBluetoothTxGainChild),
    Complete(PhyBluetoothTxGainInitOutcome),
    Failed(PhyBluetoothTxGainInitFailure),
}

/// Exact source-owned parent for archive `phy_bt_tx_gain_init`.
///
/// Vendor flag checks remain semantic: retained TX-DC and TX-power results
/// skip their respective expensive children, while RFPLL selection,
/// channel-six TX-cap publication, shared PWDET adjustment and gain-table
/// publication still run in the recovered parent order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxGainInitTransition {
    parameters: PhyBluetoothTxGainInitParameters,
    /// Force-TX/RX nesting count held by the caller of the gain child.
    depth: crate::analog::pbus::PhyForceTxRxDepth,
    step: PhyBluetoothTxGainInitStep,
    dco: [[u16; 4]; 3],
    power: Option<PhyBluetoothTxPowerOutcome>,
    gain: PhyBluetoothTxGainParameters,
}

impl PhyBluetoothTxGainInitTransition {
    /// `depth` is the force-TX/RX nesting count held when the vendor
    /// `phy_bt_set_tx_gain_new` child brackets its publication.
    pub const fn new(
        parameters: PhyBluetoothTxGainInitParameters,
        depth: crate::analog::pbus::PhyForceTxRxDepth,
    ) -> Self {
        Self {
            depth,
            step: PhyBluetoothTxGainInitStep::Rfpll(
                crate::analog::rfpll::RfpllFrequencyTransition::new(
                    crate::analog::rfpll::RfpllFrequencyRequest {
                        crystal_selector: parameters.crystal_selector,
                        frequency_code: 0x985,
                        offset: 0,
                    },
                ),
            ),
            dco: parameters.tx_dc_pwdet.dco,
            power: None,
            gain: parameters.gain,
            parameters,
        }
    }

    const fn packed_seed(rows: [[u16; 4]; 3]) -> [u32; 6] {
        let mut seed = [0; 6];
        let mut index = 0;
        while index != seed.len() {
            let first = rows[index / 2][(index % 2) * 2];
            let second = rows[index / 2][(index % 2) * 2 + 1];
            seed[index] = first as u32 | ((second as u32) << 16);
            index += 1;
        }
        seed
    }

    fn tx_power_transition(&self) -> PhyBluetoothTxPowerTransition {
        let mut parameters = self.parameters.tx_power;
        parameters.dco = self.dco[0];
        PhyBluetoothTxPowerTransition::new(parameters)
    }

    fn tx_dc_pwdet_transition(&self) -> PhyBluetoothTxDcPwdetTransition {
        let mut parameters = self.parameters.tx_dc_pwdet;
        parameters.dco = self.dco;
        PhyBluetoothTxDcPwdetTransition::new(parameters, self.parameters.tx_path_value)
    }

    pub fn step_local(
        &mut self,
    ) -> Result<PhyBluetoothTxGainInitLocalStep, PhyBluetoothTxGainInitTransitionError> {
        let local = match self.step {
            PhyBluetoothTxGainInitStep::Rfpll(transition) => match transition.action() {
                crate::analog::rfpll::RfpllFrequencyAction::Complete(_) => {
                    self.step = PhyBluetoothTxGainInitStep::TxCap;
                    PhyBluetoothTxGainInitLocalStep::StateAdvanced
                }
                crate::analog::rfpll::RfpllFrequencyAction::Failed(failure) => {
                    self.step = PhyBluetoothTxGainInitStep::Failed(
                        PhyBluetoothTxGainInitFailure::Rfpll(failure),
                    );
                    PhyBluetoothTxGainInitLocalStep::StateAdvanced
                }
                action => PhyBluetoothTxGainInitLocalStep::External(
                    PhyBluetoothTxGainInitAction::Rfpll(action),
                ),
            },
            PhyBluetoothTxGainInitStep::TxCap => {
                PhyBluetoothTxGainInitLocalStep::External(PhyBluetoothTxGainInitAction::TxCap(
                    crate::tx::power::bluetooth_tx_cap_action(self.parameters.capacitance),
                ))
            }
            PhyBluetoothTxGainInitStep::TxDc(transition) => match transition.action() {
                crate::tx::dc_offset::PhyTxDcAction::Complete(outcome) => {
                    let mut row = 0;
                    while row != self.dco.len() {
                        self.dco[row] = outcome.dco[row];
                        row += 1;
                    }
                    self.step = PhyBluetoothTxGainInitStep::TxPower(self.tx_power_transition());
                    PhyBluetoothTxGainInitLocalStep::StateAdvanced
                }
                crate::tx::dc_offset::PhyTxDcAction::Failed(failure) => {
                    self.step = PhyBluetoothTxGainInitStep::Failed(
                        PhyBluetoothTxGainInitFailure::TxDc(failure),
                    );
                    PhyBluetoothTxGainInitLocalStep::StateAdvanced
                }
                action => PhyBluetoothTxGainInitLocalStep::External(
                    PhyBluetoothTxGainInitAction::TxDc(action),
                ),
            },
            PhyBluetoothTxGainInitStep::TxPower(transition) => match transition.action() {
                PhyBluetoothTxPowerAction::Complete(outcome) => {
                    if outcome.calibration.calibration_performed {
                        self.gain.calibration_curve = [
                            outcome.calibration.power_curve[0] as u8,
                            outcome.calibration.power_curve[1] as u8,
                            outcome.calibration.power_curve[2] as u8,
                        ];
                        self.gain.correction = outcome.calibration.power_adjustment;
                    }
                    self.power = Some(outcome);
                    self.step =
                        PhyBluetoothTxGainInitStep::TxDcPwdet(self.tx_dc_pwdet_transition());
                    PhyBluetoothTxGainInitLocalStep::StateAdvanced
                }
                PhyBluetoothTxPowerAction::Failed(failure) => {
                    self.step = PhyBluetoothTxGainInitStep::Failed(
                        PhyBluetoothTxGainInitFailure::TxPower(failure),
                    );
                    PhyBluetoothTxGainInitLocalStep::StateAdvanced
                }
                action => PhyBluetoothTxGainInitLocalStep::External(
                    PhyBluetoothTxGainInitAction::TxPower(action),
                ),
            },
            PhyBluetoothTxGainInitStep::TxDcPwdet(transition) => match transition.action() {
                crate::tx::dc_power_detector::PhyTxDcPwdetAction::Complete(outcome) => {
                    self.dco = outcome.dco;
                    self.gain.seed = Self::packed_seed(self.dco);
                    let publication =
                        PhyBluetoothTxGainPublication::new(calculate_bluetooth_tx_gain(self.gain));
                    self.step = PhyBluetoothTxGainInitStep::Gain(PhyBluetoothTxGainChild::new(
                        publication,
                        self.depth,
                    ));
                    PhyBluetoothTxGainInitLocalStep::StateAdvanced
                }
                crate::tx::dc_power_detector::PhyTxDcPwdetAction::Failed(failure) => {
                    self.step = PhyBluetoothTxGainInitStep::Failed(
                        PhyBluetoothTxGainInitFailure::TxDcPwdet(failure),
                    );
                    PhyBluetoothTxGainInitLocalStep::StateAdvanced
                }
                action => PhyBluetoothTxGainInitLocalStep::External(
                    PhyBluetoothTxGainInitAction::TxDcPwdet(action),
                ),
            },
            PhyBluetoothTxGainInitStep::Gain(child) => match child.action() {
                PhyBluetoothTxGainChildAction::Complete => {
                    let power = self
                        .power
                        .ok_or(PhyBluetoothTxGainInitTransitionError::WrongCompletion)?;
                    self.step =
                        PhyBluetoothTxGainInitStep::Complete(PhyBluetoothTxGainInitOutcome {
                            tx_dc_calibrated: true,
                            dco: self.dco,
                            tx_power: power,
                            gain: child.publication().image(),
                        });
                    PhyBluetoothTxGainInitLocalStep::StateAdvanced
                }
                PhyBluetoothTxGainChildAction::Publish(publication) => {
                    PhyBluetoothTxGainInitLocalStep::External(
                        PhyBluetoothTxGainInitAction::Publish(publication),
                    )
                }
                PhyBluetoothTxGainChildAction::ForceTxRx(action) => {
                    PhyBluetoothTxGainInitLocalStep::External(
                        PhyBluetoothTxGainInitAction::ForceTxRx(action),
                    )
                }
            },
            PhyBluetoothTxGainInitStep::Complete(outcome) => {
                PhyBluetoothTxGainInitLocalStep::Complete(outcome)
            }
            PhyBluetoothTxGainInitStep::Failed(failure) => {
                PhyBluetoothTxGainInitLocalStep::Failed(failure)
            }
        };
        Ok(local)
    }

    pub fn advance_external(
        &mut self,
        completion: PhyBluetoothTxGainInitCompletion,
    ) -> Result<(), PhyBluetoothTxGainInitTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyBluetoothTxGainInitStep::Rfpll(mut transition),
                PhyBluetoothTxGainInitCompletion::Rfpll(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyBluetoothTxGainInitTransitionError::WrongCompletion)?;
                PhyBluetoothTxGainInitStep::Rfpll(transition)
            }
            (
                PhyBluetoothTxGainInitStep::TxCap,
                PhyBluetoothTxGainInitCompletion::TxCap(
                    crate::tx::power::PhyTxPowerCompletion::I2cWritten { address, value },
                ),
            ) if crate::tx::power::bluetooth_tx_cap_action(self.parameters.capacitance)
                == crate::tx::power::PhyTxPowerAction::WriteI2c { address, value } =>
            {
                if self.parameters.tx_dc_calibrated {
                    PhyBluetoothTxGainInitStep::TxPower(self.tx_power_transition())
                } else {
                    PhyBluetoothTxGainInitStep::TxDc(PhyBluetoothTxDcTransition::new(
                        self.parameters.tx_dc,
                        self.parameters.tx_path_value,
                    ))
                }
            }
            (
                PhyBluetoothTxGainInitStep::TxDc(mut transition),
                PhyBluetoothTxGainInitCompletion::TxDc(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyBluetoothTxGainInitTransitionError::WrongCompletion)?;
                PhyBluetoothTxGainInitStep::TxDc(transition)
            }
            (
                PhyBluetoothTxGainInitStep::TxPower(mut transition),
                PhyBluetoothTxGainInitCompletion::TxPower(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyBluetoothTxGainInitTransitionError::WrongCompletion)?;
                PhyBluetoothTxGainInitStep::TxPower(transition)
            }
            (
                PhyBluetoothTxGainInitStep::TxDcPwdet(mut transition),
                PhyBluetoothTxGainInitCompletion::TxDcPwdet(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyBluetoothTxGainInitTransitionError::WrongCompletion)?;
                PhyBluetoothTxGainInitStep::TxDcPwdet(transition)
            }
            (
                PhyBluetoothTxGainInitStep::Gain(mut child),
                PhyBluetoothTxGainInitCompletion::Published(published),
            ) => {
                child.advance(PhyBluetoothTxGainChildCompletion::Published(published))?;
                PhyBluetoothTxGainInitStep::Gain(child)
            }
            (
                PhyBluetoothTxGainInitStep::Gain(mut child),
                PhyBluetoothTxGainInitCompletion::ForceTxRx(completion),
            ) => {
                child.advance(PhyBluetoothTxGainChildCompletion::ForceTxRx(completion))?;
                PhyBluetoothTxGainInitStep::Gain(child)
            }
            (
                PhyBluetoothTxGainInitStep::Complete(_) | PhyBluetoothTxGainInitStep::Failed(_),
                _,
            ) => return Err(PhyBluetoothTxGainInitTransitionError::AlreadyComplete),
            _ => return Err(PhyBluetoothTxGainInitTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothExternalBindingError {
    UnsupportedAction,
    IncompleteTransaction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyBluetoothI2cAction {
    StartCommand,
    AwaitCompletionEdge,
    Complete,
}

/// Non-cloneable owner of one PAC-owned Bluetooth TX-power I²C transaction.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyBluetoothI2cBinding {
    transaction: oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlTransaction,
}

impl PhyBluetoothI2cBinding {
    pub const fn new(
        operation: oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlOperation,
    ) -> Self {
        Self {
            transaction: oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlTransaction::new(
                operation,
            ),
        }
    }

    pub const fn action(&self) -> PhyBluetoothI2cAction {
        match self.transaction.action() {
            oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlAction::StartCommand => {
                PhyBluetoothI2cAction::StartCommand
            }
            oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlAction::AwaitCompletionEdge => {
                PhyBluetoothI2cAction::AwaitCompletionEdge
            }
            oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlAction::Complete(_) => {
                PhyBluetoothI2cAction::Complete
            }
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        platform: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        oer_esp32s31_hal::phy::i2c::start_bluetooth_tx_power_control(
            &mut self.transaction,
            platform,
        )
        .map_err(|error| match error {
            oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlError::BusyAtStart => {
                crate::calibration::cold::PhyColdI2cError::BusyAtStart
            }
            _ => crate::calibration::cold::PhyColdI2cError::WrongEdge,
        })
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        platform: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        oer_esp32s31_hal::phy::i2c::observe_bluetooth_tx_power_control(
            &mut self.transaction,
            platform,
        )
        .map(|observation| match observation {
            oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlObservation::StillPending => {
                crate::calibration::cold::PhyColdI2cObservation::StillPending
            }
            oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlObservation::EdgeConsumed => {
                crate::calibration::cold::PhyColdI2cObservation::EdgeConsumed
            }
        })
        .map_err(|_| crate::calibration::cold::PhyColdI2cError::WrongEdge)
    }

    pub fn into_completion(
        self,
    ) -> Result<PhyBluetoothTxPowerCompletion, PhyBluetoothExternalBindingError> {
        let oer_esp32s31_hal::phy::i2c::BluetoothTxPowerControlAction::Complete(completion) =
            self.transaction.action()
        else {
            return Err(PhyBluetoothExternalBindingError::IncompleteTransaction);
        };
        Ok(PhyBluetoothTxPowerCompletion::I2cControl(completion))
    }
}

/// Bounded PBus force-test used by the Bluetooth power parent.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyBluetoothPbusBinding {
    inner: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyBluetoothPbusBinding {
    pub const fn new(transaction: crate::analog::pbus::PhyPbusForceTest) -> Self {
        Self {
            inner: crate::analog::pbus::PhyPbusHardwareBinding::new(transaction),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<(), crate::analog::pbus::PhyPbusHardwareBindingError> {
        self.inner.start_target(registers)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<
        crate::analog::pbus::PhyPbusHardwareObservation,
        crate::analog::pbus::PhyPbusHardwareBindingError,
    > {
        self.inner.observe_target_edge(registers)
    }

    pub fn into_completion(
        self,
    ) -> Result<PhyBluetoothTxPowerCompletion, PhyBluetoothExternalBindingError> {
        self.inner
            .into_transaction()
            .map(PhyBluetoothTxPowerCompletion::PbusCompleted)
            .map_err(|_| PhyBluetoothExternalBindingError::IncompleteTransaction)
    }

    pub fn into_timeout_completion(self) -> PhyBluetoothTxPowerCompletion {
        PhyBluetoothTxPowerCompletion::PbusTimedOut(self.inner.into_timeout_transaction())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyBluetoothPbusReadBinding {
    selector: u8,
    path: u8,
}

impl PhyBluetoothPbusReadBinding {
    pub const fn new(selector: u8, path: u8) -> Self {
        Self { selector, path }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyBluetoothTxPowerCompletion {
        let value = oer_esp32s31_hal::phy::pbus::read_result(registers, self.selector, self.path);
        debug_assert!(
            value.is_some(),
            "Bluetooth parent emitted an unknown PBus selector"
        );
        PhyBluetoothTxPowerCompletion::PbusRead {
            selector: self.selector,
            path: self.path,
            value: value.unwrap_or(0),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxPowerExternalBinding {
    I2c(PhyBluetoothI2cBinding),
    Prepare(crate::tx::calibration::PhyTxCalibrationEnvironmentExternalBinding),
    Cleanup(crate::tx::calibration::PhyTxCalibrationEnvironmentExternalBinding),
    Pbus(PhyBluetoothPbusBinding),
    ReadPbus(PhyBluetoothPbusReadBinding),
    Calibration(crate::tx::power::PhyTxPowerExternalBinding),
}

impl PhyBluetoothTxPowerExternalBinding {
    pub fn lower(
        action: PhyBluetoothTxPowerAction,
    ) -> Result<Self, PhyBluetoothExternalBindingError> {
        match action {
            PhyBluetoothTxPowerAction::I2cControl(operation) => {
                Ok(Self::I2c(PhyBluetoothI2cBinding::new(operation)))
            }
            PhyBluetoothTxPowerAction::Prepare(action) => {
                crate::tx::calibration::PhyTxCalibrationEnvironmentExternalBinding::lower(action)
                    .map(Self::Prepare)
                    .map_err(|_| PhyBluetoothExternalBindingError::UnsupportedAction)
            }
            PhyBluetoothTxPowerAction::Cleanup(action) => {
                crate::tx::calibration::PhyTxCalibrationEnvironmentExternalBinding::lower(action)
                    .map(Self::Cleanup)
                    .map_err(|_| PhyBluetoothExternalBindingError::UnsupportedAction)
            }
            PhyBluetoothTxPowerAction::ForcePbus(transaction) => {
                Ok(Self::Pbus(PhyBluetoothPbusBinding::new(transaction)))
            }
            PhyBluetoothTxPowerAction::ReadPbus { selector, path } => Ok(Self::ReadPbus(
                PhyBluetoothPbusReadBinding::new(selector, path),
            )),
            PhyBluetoothTxPowerAction::Calibration(action) => {
                crate::tx::power::PhyTxPowerExternalBinding::lower(action)
                    .map(Self::Calibration)
                    .map_err(|_| PhyBluetoothExternalBindingError::UnsupportedAction)
            }
            PhyBluetoothTxPowerAction::Complete(_) | PhyBluetoothTxPowerAction::Failed(_) => {
                Err(PhyBluetoothExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyBluetoothTxGainPublicationBinding {
    publication: PhyBluetoothTxGainPublication,
}

impl PhyBluetoothTxGainPublicationBinding {
    pub const fn new(publication: PhyBluetoothTxGainPublication) -> Self {
        Self { publication }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyBluetoothTxGainInitCompletion {
        self.publication.execute_target(registers);
        PhyBluetoothTxGainInitCompletion::Published(self.publication)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyBluetoothTxGainInitExternalBinding {
    Rfpll(crate::analog::rfpll::RfpllFrequencyExternalBinding),
    TxCap(crate::tx::power::PhyTxPowerExternalBinding),
    TxDc(crate::tx::dc_offset::PhyTxDcExternalBinding),
    TxPower(PhyBluetoothTxPowerExternalBinding),
    TxDcPwdet(crate::tx::dc_power_detector::PhyTxDcPwdetExternalBinding),
    ForceTxRx(crate::analog::pbus::PhyForceTxRxExternalBinding),
    Publish(PhyBluetoothTxGainPublicationBinding),
}

impl PhyBluetoothTxGainInitExternalBinding {
    pub fn lower(
        action: PhyBluetoothTxGainInitAction,
    ) -> Result<Self, PhyBluetoothExternalBindingError> {
        match action {
            PhyBluetoothTxGainInitAction::Rfpll(action) => {
                crate::analog::rfpll::RfpllFrequencyExternalBinding::lower(action)
                    .map(Self::Rfpll)
                    .map_err(|_| PhyBluetoothExternalBindingError::UnsupportedAction)
            }
            PhyBluetoothTxGainInitAction::TxCap(action) => {
                crate::tx::power::PhyTxPowerExternalBinding::lower(action)
                    .map(Self::TxCap)
                    .map_err(|_| PhyBluetoothExternalBindingError::UnsupportedAction)
            }
            PhyBluetoothTxGainInitAction::TxDc(action) => {
                crate::tx::dc_offset::PhyTxDcExternalBinding::lower(action)
                    .map(Self::TxDc)
                    .map_err(|_| PhyBluetoothExternalBindingError::UnsupportedAction)
            }
            PhyBluetoothTxGainInitAction::TxPower(action) => {
                PhyBluetoothTxPowerExternalBinding::lower(action).map(Self::TxPower)
            }
            PhyBluetoothTxGainInitAction::TxDcPwdet(action) => {
                crate::tx::dc_power_detector::PhyTxDcPwdetExternalBinding::lower(action)
                    .map(Self::TxDcPwdet)
                    .map_err(|_| PhyBluetoothExternalBindingError::UnsupportedAction)
            }
            PhyBluetoothTxGainInitAction::ForceTxRx(action) => {
                crate::analog::pbus::PhyForceTxRxExternalBinding::lower(action)
                    .map(Self::ForceTxRx)
                    .map_err(|_| PhyBluetoothExternalBindingError::UnsupportedAction)
            }
            PhyBluetoothTxGainInitAction::Publish(publication) => Ok(Self::Publish(
                PhyBluetoothTxGainPublicationBinding::new(publication),
            )),
        }
    }
}

#[cfg(test)]
mod tests;
