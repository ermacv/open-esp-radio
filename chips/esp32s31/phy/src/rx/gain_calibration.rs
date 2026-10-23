//! Rust-owned ESP32-S31 RX gain DC-calibration primitives.
//!
//! The one-step control flow is recovered from archive
//! `phy_pbus_rx_dco_cal_1step_new`; its measurement children are rev0 ROM
//! `phy_rxdc_est_min` and `phy_dc_iq_est`.
//! Pure search arithmetic and result vocabulary. The retained RX program
//! owns hardware command ordering, settles, readiness and coefficient handoff.

mod quality;
pub use quality::PhyRxGainDcQuality;

use crate::{
    analog::{pbus::PhyPbusForceTest, rfpll::RfpllFrequencyFailure},
    rx::dc_offset::PhyRxDcMinimumFailure,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationStage {
    Radio,
    Baseband,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationRequest {
    /// Selects the Wi-Fi (`false`) or shared-radio (`true`) calibration bank.
    pub shared_radio: bool,
    pub stage: PhyRxDcCalibrationStage,
    pub control: u16,
    pub initial: [u16; 2],
    pub gain_index: u8,
    pub rx_saturation_detected: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxDcCalibrationOutcome {
    pub request: PhyRxDcCalibrationRequest,
    pub configuration: [u16; 2],
    pub iterations: u8,
    pub converged: bool,
    pub readiness_activity_edges: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxDcCalibrationFailure {
    PbusForceTimedOut(PhyPbusForceTest),
    Minimum(PhyRxDcMinimumFailure),
}

const fn saturate_9bit(value: i32) -> u16 {
    if value < 0 {
        0
    } else if value > 0x1ff {
        0x1ff
    } else {
        value as u16
    }
}

const fn saturate_signed_5(value: i32) -> i32 {
    if value < -5 {
        -5
    } else if value > 5 {
        5
    } else {
        value
    }
}

/// Exact non-I/O correction arithmetic from `phy_pbus_rx_dco_cal_1step_new`:
/// an observation below the threshold contributes no correction.
pub const fn rx_dc_calibration_correction(delta: i32, threshold: i32, shift: u8) -> i32 {
    if delta.wrapping_abs() >= threshold {
        delta >> shift
    } else {
        0
    }
}

/// ROM `phy_pbus_rx_dco_cal_1step_new` keeps one output slot for the
/// baseband low search and one for the radio or baseband high search. Both
/// start cleared and keep the last search's estimate across iterations,
/// because `phy_rxdc_est_min` writes a slot only for an admitted estimate.
/// The ROM never initializes the radio slot, so it reads stack contents
/// before the first admitted radio estimate; Rust starts it cleared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PhyRxDcCalibrationPolicyState {
    pub(super) current: [u16; 2],
    pub(super) iteration: u8,
    pub(super) low: crate::calibration::estimator::PhyDcIqEstimate,
    pub(super) measurement: crate::calibration::estimator::PhyDcIqEstimate,
    pub(super) readiness_activity_edges: u16,
}

impl PhyRxDcCalibrationPolicyState {
    pub(super) const fn new(initial: [u16; 2]) -> Self {
        Self {
            current: initial,
            iteration: 0,
            low: crate::calibration::estimator::PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0,
            },
            measurement: crate::calibration::estimator::PhyDcIqEstimate {
                i: 0,
                q: 0,
                power: 0,
            },
            readiness_activity_edges: 0,
        }
    }

    pub(super) fn accept(
        &mut self,
        request: PhyRxDcCalibrationRequest,
        population: u8,
        threshold: i32,
        high: bool,
        estimate: crate::calibration::estimator::PhyDcIqEstimate,
        readiness_activity_edges: u16,
    ) -> Option<PhyRxDcCalibrationOutcome> {
        self.readiness_activity_edges = self
            .readiness_activity_edges
            .wrapping_add(readiness_activity_edges);
        if request.stage == PhyRxDcCalibrationStage::Baseband && !high {
            self.low = estimate;
            return None;
        }
        self.measurement = estimate;

        let (delta_i, delta_q, power, shift) = match request.stage {
            PhyRxDcCalibrationStage::Radio => (
                estimate.i,
                estimate.q,
                estimate.power,
                population.max(2) - 2,
            ),
            PhyRxDcCalibrationStage::Baseband => (
                estimate.i.wrapping_sub(self.low.i),
                estimate.q.wrapping_sub(self.low.q),
                estimate.power.max(self.low.power),
                if request.shared_radio {
                    SHARED_BASEBAND_SHIFT
                } else {
                    WIFI_BASEBAND_SHIFT
                },
            ),
        };

        let mut correction_i = rx_dc_calibration_correction(delta_i, threshold, shift);
        let mut correction_q = rx_dc_calibration_correction(delta_q, threshold, shift);
        if request.stage == PhyRxDcCalibrationStage::Baseband {
            if power >= 45 {
                correction_i = 0;
                correction_q = 0;
            }
            if !request.shared_radio && request.gain_index > 1 {
                correction_i = saturate_signed_5(correction_i);
                correction_q = saturate_signed_5(correction_q);
            }
        }

        let converged = delta_i.wrapping_abs() <= threshold
            && delta_q.wrapping_abs() <= threshold
            && power < 46;
        // ROM `phy_pbus_rx_dco_cal_1step_new` stores the codes it forces at
        // the start of each iteration into its caller's output and returns
        // that output, so the correction of the final unconverged iteration
        // is computed but never published.
        let last = self.iteration + 1 >= maximum_iterations(request.stage);
        if !converged && !last {
            if delta_i.wrapping_abs() > threshold {
                self.current[0] =
                    saturate_9bit(i32::from(self.current[0]).wrapping_sub(correction_i));
            }
            if delta_q.wrapping_abs() > threshold {
                self.current[1] =
                    saturate_9bit(i32::from(self.current[1]).wrapping_sub(correction_q));
            }
        }

        self.iteration += 1;
        if !converged && self.iteration < maximum_iterations(request.stage) {
            return None;
        }
        let configuration = if !converged && request.stage == PhyRxDcCalibrationStage::Baseband {
            request.initial
        } else {
            self.current
        };
        Some(PhyRxDcCalibrationOutcome {
            request,
            configuration,
            iterations: self.iteration,
            converged,
            readiness_activity_edges: self.readiness_activity_edges,
        })
    }
}

/// Right shifts applied to baseband observations of each bank.
const SHARED_BASEBAND_SHIFT: u8 = 5;
const WIFI_BASEBAND_SHIFT: u8 = 2;
/// Baseband observation thresholds of each bank.
const SHARED_BASEBAND_THRESHOLD: i32 = 25;
const WIFI_BASEBAND_THRESHOLD: i32 = 3;

/// A radio-stage threshold grows with the population of the six low control
/// bits read before the search: `2 * population + 1`.
pub(super) const fn calibration_threshold(
    request: PhyRxDcCalibrationRequest,
    population: u8,
) -> i32 {
    match request.stage {
        PhyRxDcCalibrationStage::Radio => 2 * population as i32 + 1,
        PhyRxDcCalibrationStage::Baseband if request.shared_radio => SHARED_BASEBAND_THRESHOLD,
        PhyRxDcCalibrationStage::Baseband => WIFI_BASEBAND_THRESHOLD,
    }
}

pub(super) const fn calibration_path(stage: PhyRxDcCalibrationStage) -> u8 {
    match stage {
        PhyRxDcCalibrationStage::Radio => 2,
        PhyRxDcCalibrationStage::Baseband => 1,
    }
}

const fn maximum_iterations(stage: PhyRxDcCalibrationStage) -> u8 {
    match stage {
        PhyRxDcCalibrationStage::Radio => 8,
        PhyRxDcCalibrationStage::Baseband => 12,
    }
}

// Exact halfword tables at `.LANCHOR0 + 0x08` and `.LANCHOR0 + 0x18`
// in vendor `phy_rx_cal.o`.  The final shared entries select the extended
// 0x027f and 0x017f gain encodings; truncating them to 0x007f skips four
// calibration points later consumed by the RX gain-memory setup.
pub(crate) const WIFI_CALIBRATION_GAIN: [u16; 8] = [0x40, 0x41, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x7f];
pub(crate) const SHARED_CALIBRATION_GAIN: [u16; 11] = [
    0x40, 0x41, 0x42, 0x43, 0x6e, 0x78, 0x79, 0x7b, 0x027f, 0x017f, 0x007f,
];
const RX_ON_COUNT: u8 = 7;
const RX_OFF_COUNT: u8 = 3;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcBank {
    Wifi,
    Shared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainDcParameters {
    pub crystal_selector: u8,
    pub pbus_rx_path_value: u8,
    pub rx_saturation_detected: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainDcOutcome {
    /// Per-search convergence, distinct from successful transaction completion.
    pub quality: PhyRxGainDcQuality,
    pub wifi_index_dc: [[u16; 2]; 8],
    /// Five `phy_rxdc_fine_cal` radio DC pairs beginning at
    /// `phy_param[0x16e]`, one per [`fine_code`]. The first is the radio DC
    /// base of every later Wi-Fi gain; RX gain memory selects one per entry.
    pub wifi_fine_dc: [[u16; 2]; FINE_CODES],
    /// Eleven calibrated entries beginning at `phy_param[0x1b4]`.
    pub shared_index_dc: [[u16; 2]; 11],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDcFailure {
    Rfpll(RfpllFrequencyFailure),
    Pbus {
        bank: PhyRxGainDcBank,
        transaction: PhyPbusForceTest,
    },
    Calibration(PhyRxDcCalibrationFailure),
    Minimum(PhyRxDcMinimumFailure),
}

pub(crate) const fn rx_on(index: u8, pbus_rx_path_value: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 1),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(0, 1, 0x40),
        4 => PhyPbusForceTest::new(0, 2, pbus_rx_path_value as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x189),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

pub(crate) const fn rx_off(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0),
        1 => PhyPbusForceTest::new(1, 1, 0),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

/// Radio control codes `phy_rxdc_fine_cal` forces before each of its searches.
const FINE_CODE_TABLE: [u16; FINE_CODES] = [0x00, 0x20, 0x30, 0x38, 0x3c];
/// Searches of `phy_rxdc_fine_cal`.
pub const FINE_CODES: usize = 5;

pub(crate) const fn fine_code(index: u8) -> u16 {
    FINE_CODE_TABLE[index as usize]
}

pub(super) const fn bank_count(bank: PhyRxGainDcBank) -> u8 {
    match bank {
        PhyRxGainDcBank::Wifi => WIFI_CALIBRATION_GAIN.len() as u8,
        PhyRxGainDcBank::Shared => SHARED_CALIBRATION_GAIN.len() as u8,
    }
}

pub(super) const fn gain(bank: PhyRxGainDcBank, index: u8) -> u16 {
    match bank {
        PhyRxGainDcBank::Wifi => WIFI_CALIBRATION_GAIN[index as usize],
        PhyRxGainDcBank::Shared => SHARED_CALIBRATION_GAIN[index as usize],
    }
}

/// Expand complete ROM `phy_pbus_set_rxgain`, size 92, into its three
/// independently completed PBus commands. The final command's vendor
/// `phy_param[0x002]` dependency is an explicit Rust-owned input.
pub(crate) const fn set_rx_gain_transaction(
    encoded_gain: u32,
    pbus_rx_path_value: u8,
    transaction: u8,
) -> PhyPbusForceTest {
    match transaction {
        0 => PhyPbusForceTest::new(
            1,
            2,
            (((encoded_gain << 6) & 0x1c0) | ((encoded_gain >> 4) & 0x3f)) as u16,
        ),
        1 => PhyPbusForceTest::new(
            0,
            1,
            (((encoded_gain >> 12) & 0x38) | ((encoded_gain >> 12) & 7) | 0x40) as u16,
        ),
        _ => PhyPbusForceTest::new(0, 2, pbus_rx_path_value as u16),
    }
}

/// Exact stack-local lookup constructed by vendor `phy_bt_rx_mx_dgain`.
///
/// Indices 0..=8 select zero, index 9 selects four, and index 10 (plus the
/// out-of-range fallback) selects seven.
const fn shared_mixer_dgain(index: u8) -> u8 {
    if index >= 10 {
        7
    } else if index == 9 {
        4
    } else {
        0
    }
}

pub(crate) const fn shared_mixer_dgain_transaction(
    index: u8,
    pbus_rx_path_value: u8,
) -> PhyPbusForceTest {
    PhyPbusForceTest::new(
        0,
        2,
        ((pbus_rx_path_value & 0xf8) | shared_mixer_dgain(index)) as u16,
    )
}
