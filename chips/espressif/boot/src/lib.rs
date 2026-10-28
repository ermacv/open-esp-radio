#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
//! Espressif bootstrap sequences over a chip's typed register traits.
//!
//! Before an executor exists there is nothing to yield to: hardware
//! bring-up polls. Every poll here is a named boot wait ([`wait::BootWait`])
//! with its own bound, and all of them run under one boot deadline
//! ([`wait::BootDeadline`]) whose budget lies below the RTC watchdog the
//! bootloader left armed ([`wait::BootBudget`]). A wait that passes its bound
//! or the deadline is a typed [`BootFailure`]; no sequence falls back to a
//! value it did not read.
//!
//! The chip PAC implements the register traits of each module with its typed
//! field accessors, one register operation per call; this crate holds the
//! order of the operations and contains no address. Every trait method takes
//! an [`Access`] only this crate makes, so the traits are reached only
//! through the sequences:
//!
//! ```compile_fail,E0624
//! # use oer_espressif_boot::{Access, cache::FetchSuspendRegisters};
//! fn shut<R: FetchSuspendRegisters>(registers: &mut R) {
//!     registers.shut_fetch(Access::new(), true); // only this crate calls the traits
//! }
//! ```
//!
//! - [`clock`]: the CPU clock profile, its PLL calibrated only when the CPU
//!   does not already run from it, every divider applied by one update;
//! - [`cache`]: code published to instruction fetch (data written back, the
//!   instruction caches invalidated) and instruction fetch suspended around a
//!   flash access;
//! - [`mmu`]: an MMU entry read with its valid flag and target checked, and a
//!   range mapped into free entries;
//! - [`flash`]: the flash read through the SPI1 user command port and the
//!   flash clock's timing sweep;
//! - [`psram`]: an octal PSRAM's cold power, clock and controller, its mode
//!   registers and its density, a density the boot does not know refused;
//! - [`watchdog`]: the boot's watchdogs, the RTC watchdog kept armed as the
//!   boot's backstop until the boot hands off.
//!
//! A command the hardware clears when it completes (a clock update, a
//! cache sync, an SPI user command) starts only while none runs: its start
//! returns [`Busy`] otherwise, writing nothing, and a sequence retries it
//! under the command's boot wait.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod cache;
pub mod clock;
pub mod flash;
pub mod mmu;
pub mod psram;
pub mod wait;
pub mod watchdog;

#[cfg(test)]
mod tests;

pub use cache::CacheFailure;
pub use clock::ClockFailure;
pub use flash::FlashFailure;
pub use mmu::MmuFailure;
pub use psram::PsramFailure;
pub use wait::WaitFailure;

/// Proof that a register call comes from this crate's sequences.
///
/// Every register trait method takes one by value, and only this crate
/// makes one, so a chip PAC implements the traits but no other code calls
/// them.
pub struct Access<'a>(core::marker::PhantomData<&'a mut ()>);

impl Access<'_> {
    const fn new() -> Self {
        Self(core::marker::PhantomData)
    }
}

/// A command register reads a running command: the start wrote nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Busy;

/// Why the boot stopped: the bootstrap reports it and halts; nothing falls
/// back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootFailure {
    /// A named boot wait was refused or passed its bound or the boot deadline.
    Wait(WaitFailure),
    /// The CPU clock did not reach its profile.
    Clock(ClockFailure),
    /// A cache operation's range was refused.
    Cache(CacheFailure),
    /// An MMU entry or range was refused.
    Mmu(MmuFailure),
    /// A flash read or the flash timing was refused.
    Flash(FlashFailure),
    /// The PSRAM did not come up.
    Psram(PsramFailure),
}

impl BootFailure {
    /// The `reason=` word of the bootstrap's failure line.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Wait(failure) => failure.reason(),
            Self::Clock(failure) => failure.reason(),
            Self::Cache(failure) => failure.reason(),
            Self::Mmu(failure) => failure.reason(),
            Self::Flash(failure) => failure.reason(),
            Self::Psram(failure) => failure.reason(),
        }
    }

    /// The named boot wait that failed, if the failure is a wait's.
    pub const fn wait(self) -> Option<&'static str> {
        match self {
            Self::Wait(failure) => Some(failure.wait()),
            _ => None,
        }
    }
}

impl From<WaitFailure> for BootFailure {
    fn from(failure: WaitFailure) -> Self {
        Self::Wait(failure)
    }
}

impl From<ClockFailure> for BootFailure {
    fn from(failure: ClockFailure) -> Self {
        Self::Clock(failure)
    }
}

impl From<CacheFailure> for BootFailure {
    fn from(failure: CacheFailure) -> Self {
        Self::Cache(failure)
    }
}

impl From<MmuFailure> for BootFailure {
    fn from(failure: MmuFailure) -> Self {
        Self::Mmu(failure)
    }
}

impl From<FlashFailure> for BootFailure {
    fn from(failure: FlashFailure) -> Self {
        Self::Flash(failure)
    }
}

impl From<PsramFailure> for BootFailure {
    fn from(failure: PsramFailure) -> Self {
        Self::Psram(failure)
    }
}
