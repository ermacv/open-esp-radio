//! The radio probe image's verifier session.
//!
//! The probe image reaches the hardware through the production crates'
//! public API alone: its session entry takes the reset root once, splits it
//! and brings Wi-Fi up, and every later entry of the run borrows the
//! session's owners. Every case of a suite over the session runs the entry
//! first, as a setup of which nothing is compared.

/// The probe image's session entry.
pub const ENTRY: &str = "open_probe_session_init";
