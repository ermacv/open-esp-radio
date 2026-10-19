//! A lower-MAC backend's host model on the phase shape.
//!
//! The `test-support` feature adds `model`, an in-memory lower-MAC backend
//! on the phase shape (`oer_ieee80211_lower_mac::phase`), with its driver,
//! for host tests of the code that drives a port.

pub mod model;

pub mod conformance;
