//! ESP32-S31 PHY scenarios over the engine's PHY session
//! (`oer_vendor_scenario_engine::phy`) and [`crate::layout::PHY_LAYOUT`].

pub mod calibration_leaves;
pub mod calibration_prefix;
pub mod channel;
pub use oer_esp32s31_phy_relation::committed;
pub mod gain;
pub mod gain_state;
pub mod i2c;
pub mod i2c_transport;
pub mod rfpll;
pub mod rx_gain;
pub mod tracking;
pub mod tracking_graph;
pub mod tx_dc;
pub use oer_vendor_scenario_engine::phy::*;
