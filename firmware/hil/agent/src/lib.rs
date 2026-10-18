//! Chip-independent HIL target logic.
//!
//! The HIL target runtimes compose these modules with their executor and
//! console; host tests exercise the same compiled code.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod base;
pub mod console;
pub mod postmortem;

#[doc(hidden)]
pub use oer_tool_data_hil_protocol as __protocol;
