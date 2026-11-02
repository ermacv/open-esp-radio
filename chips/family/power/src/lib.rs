#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
//! Espressif PMU light sleep over a chip's typed register traits.
//!
//! The chips with a PMU (the ESP32-C5, C6, P4 and S31) put the HP system to
//! sleep the same way: the PMU holds the state it applies in sleep (domain
//! power, clock gates, PLL and oscillator power, regulators), the software
//! selects the wake and reject sources and requests sleep, and the PMU
//! stalls the CPU until a wake source fires or a reject source refuses the
//! request. The LP main timer, counting the slow clock, wakes it at a
//! target.
//!
//! The chip HAL implements [`sleep::Registers`] with its typed field
//! accessors, one register operation per call; this crate holds the order of
//! the operations and contains no address. Every trait method takes an
//! [`Access`] only this crate makes, so the trait is reached only through
//! the sequence:
//!
//! ```compile_fail,E0624
//! # use oer_chips_family_power::{Access, sleep::Registers};
//! fn request<R: Registers>(registers: &mut R) {
//!     registers.request_sleep(Access::new()); // only this crate calls the trait
//! }
//! ```
#![no_std]
#![forbid(unsafe_code)]

pub mod sleep;

/// The proof that a register operation runs inside this crate's sequence.
///
/// Every register trait method takes one by value, and only this crate
/// makes one, so a chip implements the traits but no other code calls
/// them.
pub struct Access<'a>(core::marker::PhantomData<&'a mut ()>);

impl Access<'_> {
    const fn new() -> Self {
        Self(core::marker::PhantomData)
    }
}
