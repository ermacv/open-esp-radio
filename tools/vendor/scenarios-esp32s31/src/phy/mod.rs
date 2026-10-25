//! ESP32-S31 PHY scenarios over the engine's PHY session
//! (`oer_vendor_scenario::phy`) and [`crate::layout::PHY_LAYOUT`].

pub mod calibration_leaves;
pub use oer_esp32s31_knowledge::phy_relation::committed;
pub mod gain;
pub mod gain_state;
pub mod i2c;
pub use oer_vendor_scenario::phy::*;
