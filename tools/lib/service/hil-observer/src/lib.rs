//! The HIL observer's producer: building the runner ([`compile`]),
//! resolving its graph ([`resolve`]) and preparing its receipt
//! ([`prepare`]). The identity they produce is data of the run bundle
//! format (`oer_hil_run_bundle_format::observer`), which evaluators read.
#![forbid(unsafe_code)]

pub mod compile;
pub mod prepare;
pub mod resolve;
