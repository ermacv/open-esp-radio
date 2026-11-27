//! The PHY's one owner: its state's place, the registered handle and the
//! proofs of RF open and closed.
//!
//! Its owner makes the place of the PHY's state ([`PhyPlace`]) from the one
//! right the radio's split hands out ([`PhyStateRight`]) and memory of its
//! own. A registration fills it and returns [`RegisteredPhy`], the handle of
//! the filled place, with [`RfOpen`]. The close takes `RfOpen` and gives
//! [`RfClosed`]; the wake takes `RfClosed` and gives `RfOpen`. The channel
//! and a tracking pass take `RfOpen` and give it back. Neither proof is
//! `Copy` nor `Clone`, and only the operations' ends make them.
//!
//! ```compile_fail
//! fn requires_clone<T: Clone>() {}
//! requires_clone::<oer_chips_esp32s31_radio_phy::owner::RfOpen>();
//! ```
//!
//! ```compile_fail
//! fn requires_clone<T: Clone>() {}
//! requires_clone::<oer_chips_esp32s31_radio_phy::owner::RfClosed>();
//! ```
//!
//! ```compile_fail
//! let _forged = oer_chips_esp32s31_radio_phy::owner::RfOpen { sealed: () };
//! ```
//!
//! One right makes one place: a second place needs a second right, and the
//! right moves into the first.
//!
//! ```compile_fail,E0382
//! use core::mem::MaybeUninit;
//! use oer_chips_esp32s31_radio_phy::owner::PhyPlace;
//! fn two(right: oer_chips_esp32s31_hal::radio::domain::PhyStateRight) {
//!     let (mut first, mut second) = (MaybeUninit::uninit(), MaybeUninit::uninit());
//!     let _first = PhyPlace::new(right, &mut first);
//!     let _second = PhyPlace::new(right, &mut second);
//! }
//! ```
//!
//! A registered place becomes empty only through a failed operation: its
//! value carries the one right [`RegisteredPhy::vacate`] takes ([`Vacate`]).
//! A failed operation returns its leaf cause and has consumed its proof; the
//! owner can vacate the place, and nothing more. The PHY's clock modules
//! stay held as the failure left them, so a registration into the emptied
//! place ends in `RegistrationCause::Clock`: this is the poison of the
//! shared-domain owner it replaces, carried. A registered place has no way
//! back but a failure.
//!
//! ```compile_fail,E0061
//! fn empty(phy: oer_chips_esp32s31_radio_phy::owner::RegisteredPhy<'_>) {
//!     let _place = phy.vacate();
//! }
//! ```
//!
//! ```compile_fail,E0451
//! let _forged = oer_chips_esp32s31_radio_phy::owner::Vacate { sealed: () };
//! ```
//!
//! ```compile_fail
//! fn requires_clone<T: Clone>() {}
//! requires_clone::<oer_chips_esp32s31_radio_phy::owner::Vacate>();
//! ```
//!
//! A handle, a proof or a [`Vacate`] let go leaves the owner nothing to call
//! until the next split: Rust has no type that cannot be dropped, and
//! `#[must_use]` only warns.

use core::mem::MaybeUninit;

use oer_portable_memory::Occupied;
use oer_portable_time::{Duration, Instant};

pub use oer_chips_esp32s31_hal::radio::domain::PhyStateRight;

use crate::PhyState;

/// The PHY's one place: the right the radio's split handed out and the
/// memory it names. Only an empty place exists as a value; a filled one is
/// inside its [`RegisteredPhy`].
#[must_use = "the place is the owner's for a registration, or its right for the reunion"]
pub struct PhyPlace<'s> {
    memory: &'s mut MaybeUninit<PhyState>,
    right: PhyStateRight,
}

impl<'s> PhyPlace<'s> {
    /// The place, from the split's one right and the owner's memory.
    pub const fn new(right: PhyStateRight, memory: &'s mut MaybeUninit<PhyState>) -> Self {
        Self { memory, right }
    }

    /// The right back, for the radio's reunion. The memory is the owner's
    /// again.
    pub fn into_right(self) -> PhyStateRight {
        let Self { memory: _, right } = self;
        right
    }

    /// Fill the place with `state`.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn fill(self, state: PhyState) -> Filled<'s> {
        let Self { memory, right } = self;
        Filled {
            state: Occupied::fill(memory, state),
            right,
        }
    }
}

/// A place being registered: filled, and not yet the owner's handle.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct Filled<'s> {
    pub(crate) state: Occupied<'s, PhyState>,
    right: PhyStateRight,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'s> Filled<'s> {
    /// A registration that did not complete empties the place again.
    pub(crate) fn empty(self) -> PhyPlace<'s> {
        let Self { state, right } = self;
        let (_state, memory) = state.vacate();
        PhyPlace { memory, right }
    }
}

/// A registered PHY: the handle of the place a completed registration
/// filled, with the right to it.
///
/// ```compile_fail
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<oer_chips_esp32s31_radio_phy::owner::RegisteredPhy<'static>>();
/// ```
pub struct RegisteredPhy<'s> {
    state: Occupied<'s, PhyState>,
    right: PhyStateRight,
}

impl<'s> RegisteredPhy<'s> {
    /// The handle of a place a target registration completed.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn registered(
        filled: Filled<'s>,
        witness: crate::target_port::TargetRegistrationWitness,
    ) -> Self {
        witness.spend();
        let Filled { state, right } = filled;
        Self { state, right }
    }

    /// The registered state.
    pub fn state(&self) -> &PhyState {
        &self.state
    }

    /// Capture the registered state's current calibration as a cache for a
    /// later cold registration: the final-state handoff. RF may be open or
    /// closed; the state stays registered in both.
    pub fn calibration_cache(
        &self,
        identity: crate::PhyCalibrationIdentity,
    ) -> crate::PhyCalibrationCache {
        self.state.calibration_cache(identity)
    }

    /// The registered state, for an operation of this crate.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn state_mut(&mut self) -> &mut PhyState {
        &mut self.state
    }

    /// Empty the place, with the right a failed operation left: the place
    /// is the owner's for the next registration.
    pub fn vacate(self, failed: Vacate) -> PhyPlace<'s> {
        let Vacate { sealed: () } = failed;
        let Self { state, right } = self;
        let (_state, memory) = state.vacate();
        PhyPlace { memory, right }
    }
}

/// The one right a failed operation leaves its owner: to empty the place.
/// Only a failure that consumed its proof of RF makes it.
#[derive(Debug)]
#[must_use = "the place stays registered until this right empties it"]
pub struct Vacate {
    sealed: (),
}

impl Vacate {
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn failed() -> Self {
        Self { sealed: () }
    }
}

/// Proof that RF is open: made by a registration's and a wake's end.
#[derive(Debug)]
#[must_use = "RF stays open until a close takes this proof"]
pub struct RfOpen {
    sealed: (),
}

/// Proof that RF is closed while the registration stays: made by a close's
/// end.
#[derive(Debug)]
#[must_use = "RF stays closed until a wake takes this proof"]
pub struct RfClosed {
    sealed: (),
}

impl RfOpen {
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn opened() -> Self {
        Self { sealed: () }
    }

    /// A failed operation consumed this proof: the right to empty the place
    /// is what is left.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn fail(self) -> Vacate {
        let Self { sealed: () } = self;
        Vacate::failed()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn close(self) -> RfClosed {
        let Self { sealed } = self;
        RfClosed { sealed }
    }
}

impl RfClosed {
    /// A failed wake consumed this proof: the right to empty the place is
    /// what is left.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn fail(self) -> Vacate {
        let Self { sealed: () } = self;
        Vacate::failed()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn open(self) -> RfOpen {
        let Self { sealed } = self;
        RfOpen { sealed }
    }
}

/// The period of Wi-Fi's tracking.
///
/// SOURCE: ESP-IDF `components/esp_phy/src/phy_common.c` (the pinned
/// tree), `PHY_TRACK_PLL_PERIOD_IN_US`: 1 000 000 µs; the timer callback
/// and every enable ask whether more than the period has passed since the
/// last tracking.
pub const TRACKING_PERIOD: Duration = Duration::from_micros(1_000_000);

/// What the owner learns of tracking from the state and the time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tracking {
    /// A tracking pass is due now.
    Due,
    /// No pass is due before this instant.
    NotBefore(Instant),
}

/// Whether a tracking pass is due at `now`: at once after a registration,
/// whose state was never tracked, and otherwise once more than
/// [`TRACKING_PERIOD`] has passed since the last pass. The owner asks after
/// every registration and every wake, as the vendor asks at every enable.
pub const fn tracking_due(state: &PhyState, now: Instant) -> Tracking {
    let Some(last) = state.tracked_at() else {
        return Tracking::Due;
    };
    match last.checked_add(TRACKING_PERIOD) {
        Some(at) if now.as_micros() <= at.as_micros() => Tracking::NotBefore(at),
        Some(_) | None => Tracking::Due,
    }
}

#[cfg(test)]
mod tests;
