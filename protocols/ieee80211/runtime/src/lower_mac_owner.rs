//! A lower-MAC backend's host model on the phase shape.
//!
//! The `test-support` feature adds `model`, an in-memory lower-MAC backend
//! on the phase shape (`oer_ieee80211_port::phase`), with its driver,
//! for host tests of the code that drives a port. Its tests implement the
//! shared harnesses in `oer_ieee80211_port::conformance` with virtual time.

pub mod model;
