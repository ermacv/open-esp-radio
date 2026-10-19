//! Versioned contract between the unprivileged runner and the finite Linux
//! fixture helpers (`tools/hil-fixture`).
//!
//! `open-radio-bluetooth` and `open-radio-probe` run through the fixed
//! launchers of `tools/fixture-install`. The runner parses
//! their requests and reports with the same types.

#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod bluetooth;
#[cfg(target_os = "linux")]
pub mod linux_socket;
pub mod probe;
