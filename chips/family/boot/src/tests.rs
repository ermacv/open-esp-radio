//! The sequences over recording fakes of the register traits, timed on
//! virtual time: each test checks which operations run in which order and
//! how a wait ends, never a word's address.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;

mod cache;
mod clock;
mod flash;
mod mmu;
mod psram;
mod soc_drivers;
mod wait;
mod watchdog;

/// Virtual time that moves `step` at every read, as a busy-polling boot
/// sees the hardware's clock move.
pub(crate) struct Ticking<'a> {
    pub(crate) clock: &'a VirtualClock,
    pub(crate) step: Duration,
}

impl<'a> Ticking<'a> {
    pub(crate) fn new(clock: &'a VirtualClock) -> Self {
        Self {
            clock,
            step: Duration::from_micros(1),
        }
    }
}

impl Clock for Ticking<'_> {
    fn now(&self) -> Instant {
        self.clock
            .advance(self.step)
            .expect("the test's virtual time does not overflow")
    }
}
