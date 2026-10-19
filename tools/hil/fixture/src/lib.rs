//! Versioned contract between the unprivileged runner and the finite Linux
//! fixture helpers (`tools/hil/fixture`).
//!
//! `open-radio-bluetooth` and `open-radio-probe` run through the fixed
//! launchers of `tools/lab/fixture-install`. The runner parses
//! their requests and reports with the same types.

#![forbid(unsafe_code)]

pub mod bluetooth;
#[cfg(target_os = "linux")]
pub mod probe;
