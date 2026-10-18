#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! The S31 backend's ownership and execution form, before hardware migration.
//!
//! [`scope`] owns the narrow pinned-registration boundary between a caller
//! task and a separate driver task, independently of priority. Accepted context belongs to the
//! caller; only independently retained execution state reaches the driver.

pub mod scope;
