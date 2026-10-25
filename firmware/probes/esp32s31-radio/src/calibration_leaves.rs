//! Scalar ABI setup for shipping calibration leaves.

oer_probe_macros::probe! {
    /// Profiles select TX-gain restore (enabled domain), forced gain and
    /// post-init AGC on the session's shared PHY. The wrapper only lowers
    /// scalar arguments; production owns every computation and MMIO access.
    /// Unknown profiles, and a session that did not start, return 0x10003
    /// without touching hardware.
    pub fn open_phy_calibration_leaf(profile: u32, a: u32, b: u32, c: u32) -> u32 {
        crate::session::with(|session| {
            let mut phy = session.radio.phy_hal();
            match profile {
                0 => oer_esp32s31_hal::phy::baseband::restore_tx_gain_compensation(&mut phy),
                1 => oer_esp32s31_hal::phy::baseband::configure_forced_digital_gain(
                    &mut phy,
                    a & 1 != 0,
                    b as i8,
                    c as i8,
                ),
                3 => oer_esp32s31_hal::phy::agc::update_post_initialization(&mut phy),
                _ => return 0x10003,
            }
            0
        })
        .unwrap_or(0x10003)
    }
}
