//! The chip-neutral scenario engine (`oer-vendor-scenario`) under
//! its crate paths, and the ESP32-S31 layout, PHY contracts and analog
//! transport edges every scenario shares.
pub mod contracts;
pub mod harness_edges;
pub mod layout;

pub use oer_vendor_scenario::{
    artifacts, coverage, evidence, harness, observation, session, state,
};
