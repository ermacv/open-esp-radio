//! Owned ESP32-S31 DC/IQ estimator register leaves.
//!
//! The methods in this module perform only finite PAC-described MMIO. Timer
//! boundaries, readiness repetition, estimator lifetime and calibration
//! arithmetic remain explicit in the PHY state machines.

#[cfg(target_arch = "riscv32")]
use crate::{owner::SharedPhyAccess, phy_blocks_mut};

pub mod ops;

/// Estimator readiness and the separately sampled opaque indication pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadinessSnapshot {
    pub ready: bool,
    /// Nonzero FE +0x8d0[21:20], sampled only while READY is clear.
    /// RX saturation is inferred; this is not an estimator-busy predicate.
    pub activity: bool,
}

/// Three signed accumulators consumed by `phy_dc_iq_est`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DcIqAccumulatorSnapshot {
    pub i: i32,
    pub q: i32,
    pub power: i32,
}

/// Four signed signal-power words consumed by IQ mismatch calculations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalPowerSnapshot {
    pub sum_i: i32,
    pub difference_i: i32,
    pub difference_q: i32,
    pub sum_q: i32,
}

/// Apply the finite register prefix of complete rev0 ROM
/// `phy_iq_est_enable`.
///
/// The body at `0x2f82_89d4`, size `0xb4`, first replaces
/// `ESTIMATOR_CONFIG.BITS_26_27_OPAQUE` with one, then replaces
/// `ESTIMATOR_CONTROL.BITS_19_20_OPAQUE` with two, and finally publishes the
/// caller's low fifteen control bits. The diagnostic halfword write, delays
/// and readiness loop are deliberately owned by the PHY transition.
#[cfg(target_arch = "riscv32")]
pub fn configure(registers: &mut impl SharedPhyAccess, control: u16) {
    ops::configure(
        &mut phy_blocks_mut(registers).phy_iq_estimator_oracle,
        control,
    );
}

/// Set or clear the first estimator enable phase.
///
/// Complete rev0 ROM `phy_iq_est_enable` sets bit zero before its first
/// one-microsecond delay. Complete `phy_iq_est_disable` at `0x2f82_8a88`,
/// size `0x2c`, clears the same bit after its final delay.
#[cfg(target_arch = "riscv32")]
pub fn set_start_enabled(registers: &mut impl SharedPhyAccess, enabled: bool) {
    ops::set_start_enabled(
        &mut phy_blocks_mut(registers).phy_iq_estimator_oracle,
        enabled,
    );
}

/// Set or clear the second estimator enable phase.
///
/// Complete rev0 ROM `phy_iq_est_enable` sets bit one after the first
/// one-microsecond delay. Complete `phy_iq_est_disable` clears it before its
/// one-microsecond delay.
#[cfg(target_arch = "riscv32")]
pub fn set_measurement_enabled(registers: &mut impl SharedPhyAccess, enabled: bool) {
    ops::set_measurement_enabled(
        &mut phy_blocks_mut(registers).phy_iq_estimator_oracle,
        enabled,
    );
}

/// Sample READY once and the opaque indication pair only while READY is clear.
///
/// Complete rev0 ROM `phy_iq_est_enable` tests the PAC ready field and, while
/// it is clear, tests the opaque pair for diagnostic counting. Rust retains
/// repeat policy outside this finite observation leaf.
#[cfg(target_arch = "riscv32")]
pub fn sample_readiness(registers: &mut impl SharedPhyAccess) -> ReadinessSnapshot {
    let (ready, activity) =
        ops::sample_readiness(&phy_blocks_mut(registers).phy_iq_estimator_oracle);
    ReadinessSnapshot { ready, activity }
}

/// Read the three signed accumulators in complete `phy_dc_iq_est` order.
///
/// The complete rev0 ROM body at `0x2f82_8ab4`, size `0x84`, reads
/// `DC_I_ACCUMULATOR`, `DC_Q_ACCUMULATOR`, then `POWER_ACCUMULATOR`.
#[cfg(target_arch = "riscv32")]
pub fn read_dc_iq_accumulators(registers: &mut impl SharedPhyAccess) -> DcIqAccumulatorSnapshot {
    let [i, q, power] =
        ops::read_dc_accumulators(&phy_blocks_mut(registers).phy_iq_estimator_oracle);
    DcIqAccumulatorSnapshot { i, q, power }
}

/// Read the signed total-power accumulator exactly once.
///
/// Complete rev0 ROM `phy_set_rx_gain_cal_iq` at `0x2f82_964c`, size
/// `0x20c`, and complete `phy_rxiq_get_mis` both consume this identity.
#[cfg(target_arch = "riscv32")]
pub fn read_total_power(registers: &mut impl SharedPhyAccess) -> i32 {
    ops::read_total_power(&phy_blocks_mut(registers).phy_iq_estimator_oracle)
}

/// Read four signed words in complete `phy_rxiq_get_mis` order.
///
/// The complete rev0 ROM body at `0x2f82_8b84`, size `0x13e`, reads the
/// physical addresses in order `0x0454`, `0x0460`, `0x045c`, `0x0458`.
#[cfg(target_arch = "riscv32")]
pub fn read_rxiq_mismatch(registers: &mut impl SharedPhyAccess) -> SignalPowerSnapshot {
    let [sum_i, difference_i, difference_q, sum_q] =
        ops::read_rxiq_mismatch(&phy_blocks_mut(registers).phy_iq_estimator_oracle);
    SignalPowerSnapshot {
        sum_i,
        difference_i,
        difference_q,
        sum_q,
    }
}

/// Read four signed words in complete `phy_get_rx_sig_pwr` order.
///
/// The complete rev0 ROM body at `0x2f82_9ea2`, size `0x76`, reads the
/// physical addresses in order `0x0454`, `0x0460`, `0x0458`, `0x045c`.
#[cfg(target_arch = "riscv32")]
pub fn read_signal_power(registers: &mut impl SharedPhyAccess) -> SignalPowerSnapshot {
    let [sum_i, difference_i, difference_q, sum_q] =
        ops::read_signal_power(&phy_blocks_mut(registers).phy_iq_estimator_oracle);
    SignalPowerSnapshot {
        sum_i,
        difference_i,
        difference_q,
        sum_q,
    }
}

/// Sample the shared activity/saturation field exactly once.
///
/// Complete pinned `libphy.a[phy_rx_cal.o]::phy_check_rx_sat`, size `0x76`,
/// samples the PAC activity field exactly 100 times. The bounded repeat count
/// remains in the caller-driven PHY transition.
#[cfg(target_arch = "riscv32")]
pub fn sample_activity(registers: &mut impl SharedPhyAccess) -> bool {
    ops::active(&phy_blocks_mut(registers).phy_iq_estimator_oracle)
}
