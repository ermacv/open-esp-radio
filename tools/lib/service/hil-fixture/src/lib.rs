//! Versioned contract between the unprivileged runner and the finite Linux
//! fixture helpers, which are this package's binaries.
//!
//! `open-radio-probe` runs through the fixed launcher of
//! `oer-stand-fixture-install`. The runner parses its requests and reports
//! with the same types.

#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

#[cfg(target_os = "linux")]
pub mod linux_socket;
pub mod probe;
