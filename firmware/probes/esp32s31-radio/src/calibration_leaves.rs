//! Capability and scalar ABI setup for shipping calibration leaves.

oer_probe_macros::probe! {
    /// Profiles select TX-gain restore (enabled domain), forced gain, temperature
    /// conversion and post-init AGC. The wrapper only acquires the validation owner
    /// and lowers scalar arguments; production owns every computation and MMIO access.
    /// Unknown profiles return 0x10003 without touching hardware.
    pub fn open_phy_calibration_leaf(profile: u32, a: u32, b: u32, c: u32) -> u32 {
        if profile == 2 {
            return super::open_phy_calibration_trace_temperature_to_power(a, b, c) as u32;
        }
        if profile > 3 {
            return 0x10003;
        }
        super::with_phy(|registers| {
            match profile {
                0 => super::open_phy_calibration_trace_restore_tx_gain_compensation(a, registers),
                1 => super::open_phy_calibration_trace_force_digital_gain(a, b, c, registers),
                3 => super::open_phy_calibration_trace_post_init_agc(registers),
                _ => unreachable!(),
            }
            0
        })
    }
}
