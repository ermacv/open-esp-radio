//! The low-power clock as a type.

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicBool, Ordering},
};

/// A source of the low-power clock.
pub trait LpSource: sealed::Sealed {}

mod sealed {
    pub trait Sealed {}
}

/// The internal RC slow clock: ESP32-S31-Function-CoreBoard-1 has no 32 kHz
/// crystal.
#[derive(Debug)]
pub enum RcSlow {}

/// The external 32 kHz crystal. No board of this repository fits one, so
/// nothing claims it; operations that need it are bounded on it.
#[derive(Debug)]
pub enum Xtal32k {}

impl sealed::Sealed for RcSlow {}
impl LpSource for RcSlow {}
impl sealed::Sealed for Xtal32k {}
impl LpSource for Xtal32k {}

/// The board's low-power clock source `S`, claimed once per image and held
/// by the actor: the Wi-Fi low-power clock selection follows `S` by type.
#[must_use = "the LP clock capability is claimed once per image"]
pub struct LpClock<S: LpSource> {
    source: PhantomData<fn() -> S>,
}

impl<S: LpSource> core::fmt::Debug for LpClock<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("LpClock")
    }
}

static RC_SLOW_CLAIMED: AtomicBool = AtomicBool::new(false);

impl LpClock<RcSlow> {
    /// The RC slow clock capability; `None` once it was claimed.
    pub fn claim() -> Option<Self> {
        (!RC_SLOW_CLAIMED.swap(true, Ordering::AcqRel)).then_some(Self {
            source: PhantomData,
        })
    }
}

impl<S: LpSource> LpClock<S> {
    /// A fresh capability for one host test, outside the image's one claim.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_host_test() -> Self {
        Self {
            source: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rc_slow_clock_is_claimed_once() {
        let first = LpClock::<RcSlow>::claim();
        assert!(first.is_some());
        assert!(LpClock::<RcSlow>::claim().is_none());
    }
}
