//! DC/IQ estimator operations over the generated PHY IQ-estimator block.

#![forbid(unsafe_code)]

use oer_esp32s31_pac::phy_iq_estimator_oracle::PhyIqEstimatorOracle;
use oer_mmio::{Bits, Placement};

/// Publish the complete rev0 ROM estimator configuration prefix.
///
/// Complete `phy_iq_est_enable` performs three separate fresh-read RMW
/// operations in this order. Bit 15 of `control` is discarded by the ROM
/// shift and mask, as the fifteen-bit control window keeps it.
pub fn configure<P: Placement>(estimator: &mut PhyIqEstimatorOracle<P>, control: u16) {
    // SOURCE: ROM_REV0_PHY_IQ_ESTIMATOR, S31_ESP_PACS_SVD.
    estimator.modify_estimator_config(|w| {
        w.set_bits_26_27_opaque(Bits::constant::<0x1>());
    });
    // SOURCE: ROM_REV0_PHY_IQ_ESTIMATOR, S31_ESP_PACS_SVD.
    estimator.modify_estimator_control(|w| {
        w.set_bits_19_20_opaque(Bits::constant::<0x2>());
    });
    let window = Bits::<15>::truncate(u32::from(control));
    estimator.modify_estimator_control(|w| {
        w.set_bits_2_16_opaque(window);
    });
}

/// Set or clear the first estimator-enable phase through one fresh RMW.
pub fn set_start_enabled<P: Placement>(estimator: &mut PhyIqEstimatorOracle<P>, enabled: bool) {
    estimator.modify_estimator_control(|w| {
        w.set_start_enable(enabled);
    });
}

/// Set or clear the second estimator-enable phase through one fresh RMW.
pub fn set_measurement_enabled<P: Placement>(
    estimator: &mut PhyIqEstimatorOracle<P>,
    enabled: bool,
) {
    estimator.modify_estimator_control(|w| {
        w.set_measurement_enable(enabled);
    });
}

/// Sample readiness, and sample activity only while readiness is clear.
///
/// Complete ROM `phy_iq_est_enable` returns immediately from a ready
/// observation. Its activity read belongs only to the not-ready branch.
pub fn sample_readiness<P: Placement>(estimator: &PhyIqEstimatorOracle<P>) -> (bool, bool) {
    let ready = estimator.read_estimator_ready_status().ready();
    let activity = !ready && active(estimator);
    (ready, activity)
}

/// Read I, Q and power accumulators in complete `phy_dc_iq_est` order.
pub fn read_dc_accumulators<P: Placement>(estimator: &PhyIqEstimatorOracle<P>) -> [i32; 3] {
    let i = estimator.read_dc_i_accumulator().value().cast_signed();
    let q = estimator.read_dc_q_accumulator().value().cast_signed();
    let power = estimator.read_power_accumulator().value().cast_signed();
    [i, q, power]
}

/// Read the signed total-power accumulator exactly once.
pub fn read_total_power<P: Placement>(estimator: &PhyIqEstimatorOracle<P>) -> i32 {
    estimator.read_power_accumulator().value().cast_signed()
}

/// Read signal words in complete `phy_rxiq_get_mis` physical order.
///
/// The returned semantic order is sum-I, difference-I, difference-Q,
/// sum-Q. The hardware reads remain sum-I, sum-Q, difference-Q,
/// difference-I as proved by the complete rev0 ROM body.
pub fn read_rxiq_mismatch<P: Placement>(estimator: &PhyIqEstimatorOracle<P>) -> [i32; 4] {
    let sum_i = estimator.read_signal_power_sum_i().value().cast_signed();
    let sum_q = estimator.read_signal_power_sum_q().value().cast_signed();
    let difference_q = estimator
        .read_signal_power_difference_q()
        .value()
        .cast_signed();
    let difference_i = estimator
        .read_signal_power_difference_i()
        .value()
        .cast_signed();
    [sum_i, difference_i, difference_q, sum_q]
}

/// Read signal words in complete `phy_get_rx_sig_pwr` physical order.
///
/// Hardware reads remain sum-I, sum-Q, difference-I, difference-Q. The
/// returned common semantic order is sum-I, difference-I, difference-Q,
/// sum-Q.
pub fn read_signal_power<P: Placement>(estimator: &PhyIqEstimatorOracle<P>) -> [i32; 4] {
    let sum_i = estimator.read_signal_power_sum_i().value().cast_signed();
    let sum_q = estimator.read_signal_power_sum_q().value().cast_signed();
    let difference_i = estimator
        .read_signal_power_difference_i()
        .value()
        .cast_signed();
    let difference_q = estimator
        .read_signal_power_difference_q()
        .value()
        .cast_signed();
    [sum_i, difference_i, difference_q, sum_q]
}

/// Sample the shared estimator-activity field once.
///
/// The bounded 100-sample policy remains in the upper PHY transition.
pub fn active<P: Placement>(estimator: &PhyIqEstimatorOracle<P>) -> bool {
    estimator
        .read_estimator_activity_status()
        .bits_20_21_opaque()
        .get()
        != 0
}
