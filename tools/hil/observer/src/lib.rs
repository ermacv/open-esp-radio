//! The HIL observer's producer: building the runner ([`compile`]),
//! resolving its graph ([`resolve`]) and keeping its private copy
//! ([`prepare`]). The identity they produce is data of the run bundle
//! format (`oer_hil_runs::format::observer`), which evaluators read.
#![forbid(unsafe_code)]

pub mod compile;
pub mod prepare;
pub mod resolve;
