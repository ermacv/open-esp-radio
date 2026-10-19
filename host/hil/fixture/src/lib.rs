//! Versioned contract between the unprivileged runner and the finite Linux
//! fixture helpers, which are this package's binaries.
//!
//! `open-radio-bluetooth` and `open-radio-probe` run through the fixed
//! launchers of `oer-lab-fixtures`. The runner parses
//! their requests and reports with the same types.

#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod bluetooth;
#[cfg(target_os = "linux")]
pub mod linux_socket;
pub mod probe;
