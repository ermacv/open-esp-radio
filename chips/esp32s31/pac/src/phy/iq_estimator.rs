//! Ownership-bound DC/IQ estimator operations.

#![forbid(unsafe_code)]

use crate::RadioPhyRegisters;
use crate::generated::{IqEstimatorControlWord, IqEstimatorEnableState};

const fn iq_estimator_enable_state(enabled: bool) -> IqEstimatorEnableState {
    if enabled {
        IqEstimatorEnableState::Enabled
    } else {
        IqEstimatorEnableState::Disabled
    }
}

impl RadioPhyRegisters {
    /// Publish the complete rev0 ROM estimator configuration prefix.
    ///
    /// Complete `phy_iq_est_enable` performs three separate fresh-read RMW
    /// operations in this order. Bit 15 of `control` is discarded by the ROM
    /// shift/mask sequence and therefore by the reviewed generated transform.
    pub fn configure_iq_estimator(&mut self, control: u16) {
        let registers = &mut self.peripherals.phy_iq_estimator_oracle;
        registers.configure_iq_estimator_config_mode();
        registers.configure_iq_estimator_mode();
        registers.configure_iq_estimator_control_window(
            IqEstimatorControlWord::new(u32::from(control))
                .expect("u16 always fits the reviewed estimator-control domain"),
        );
    }

    /// Set or clear the first estimator-enable phase through one fresh RMW.
    pub fn set_iq_estimator_start_enabled(&mut self, enabled: bool) {
        self.peripherals
            .phy_iq_estimator_oracle
            .set_iq_estimator_start_state(iq_estimator_enable_state(enabled));
    }

    /// Set or clear the second estimator-enable phase through one fresh RMW.
    pub fn set_iq_estimator_measurement_enabled(&mut self, enabled: bool) {
        self.peripherals
            .phy_iq_estimator_oracle
            .set_iq_estimator_measurement_state(iq_estimator_enable_state(enabled));
    }

    /// Sample readiness, and sample activity only while readiness is clear.
    ///
    /// Complete ROM `phy_iq_est_enable` returns immediately from a ready
    /// observation. Its activity read belongs only to the not-ready branch.
    pub fn sample_iq_estimator_readiness(&mut self) -> (bool, bool) {
        let registers = &mut self.peripherals.phy_iq_estimator_oracle;
        let ready = registers.read_estimator_ready_status().ready();
        let activity = !ready
            && registers
                .read_estimator_activity_status()
                .bits_20_21_opaque()
                .get()
                != 0;
        (ready, activity)
    }

    /// Read I, Q and power accumulators in complete `phy_dc_iq_est` order.
    pub fn read_iq_estimator_dc_accumulators(&mut self) -> [i32; 3] {
        let registers = &mut self.peripherals.phy_iq_estimator_oracle;
        let i = registers.read_dc_i_accumulator().value().cast_signed();
        let q = registers.read_dc_q_accumulator().value().cast_signed();
        let power = registers.read_power_accumulator().value().cast_signed();
        [i, q, power]
    }

    /// Read the signed total-power accumulator exactly once.
    pub fn read_iq_estimator_total_power(&mut self) -> i32 {
        self.peripherals
            .phy_iq_estimator_oracle
            .read_power_accumulator()
            .value()
            .cast_signed()
    }

    /// Read signal words in complete `phy_rxiq_get_mis` physical order.
    ///
    /// The returned semantic order is sum-I, difference-I, difference-Q,
    /// sum-Q. The hardware reads remain sum-I, sum-Q, difference-Q,
    /// difference-I as proved by the complete rev0 ROM body.
    pub fn read_iq_estimator_rxiq_mismatch(&mut self) -> [i32; 4] {
        let registers = &mut self.peripherals.phy_iq_estimator_oracle;
        let sum_i = registers.read_signal_power_sum_i().value().cast_signed();
        let sum_q = registers.read_signal_power_sum_q().value().cast_signed();
        let difference_q = registers
            .read_signal_power_difference_q()
            .value()
            .cast_signed();
        let difference_i = registers
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
    pub fn read_iq_estimator_signal_power(&mut self) -> [i32; 4] {
        let registers = &mut self.peripherals.phy_iq_estimator_oracle;
        let sum_i = registers.read_signal_power_sum_i().value().cast_signed();
        let sum_q = registers.read_signal_power_sum_q().value().cast_signed();
        let difference_i = registers
            .read_signal_power_difference_i()
            .value()
            .cast_signed();
        let difference_q = registers
            .read_signal_power_difference_q()
            .value()
            .cast_signed();
        [sum_i, difference_i, difference_q, sum_q]
    }

    /// Sample the shared estimator-activity field once.
    ///
    /// The bounded 100-sample policy remains in the upper PHY transition;
    /// physical register identity and field extraction remain in the PAC.
    pub fn iq_estimator_active(&mut self) -> bool {
        self.peripherals
            .phy_iq_estimator_oracle
            .read_estimator_activity_status()
            .bits_20_21_opaque()
            .get()
            != 0
    }
}
