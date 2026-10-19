//! Reviewed observation decisions of the ESP32-C5 scenarios.
use oer_vendor_scenario_engine::observation::Decision;

/// Reviewed unobserved lines.
pub const DECISIONS: &[Decision] = &[Decision {
    reason: "the analog I2C owner's record that the host map was rewritten for the current \
        command: private owner state no register effect carries, which only decides whether a \
        busy retry rewrites the map again; that retry is compared by the writeReg leaf on a \
        busy port",
    places: &[("pac/src/phy/i2c.rs", "self.host_map_pending = true;")],
}];
