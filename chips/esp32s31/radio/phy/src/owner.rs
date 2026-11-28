//! The PHY's one owner: its state's place, the handle of the filled place
//! and the proofs of RF.
//!
//! Its owner makes the place of the PHY's state ([`PhyPlace`]) from the one
//! right the radio's split hands out ([`PhyStateRight`]) and memory of its
//! own, and fills it at once: [`PhyPlace::fill`] gives the handle
//! ([`PhyHandle`]) and the proof that RF is cold ([`RfCold`]). The handle
//! stays with the owner for good; what an operation takes is a proof. A
//! registration is lent the handle and takes `RfCold`, and gives
//! [`RfOpen`]; the close takes `RfOpen` and gives [`RfClosed`]; the wake
//! takes `RfClosed` and gives `RfOpen`; the channel and a tracking pass take
//! `RfOpen` and give it back. No proof is `Copy` nor `Clone`, and only the
//! operations' ends and the fill make them.
//!
//! Every right, place, handle and proof carries the PHY's brand `P`, so a
//! proof of one PHY cannot serve another's handle.
//!
//! ```compile_fail,E0277
//! fn requires_clone<T: Clone>() {}
//! requires_clone::<oer_chips_esp32s31_radio_phy::owner::RfOpen<oer_chips_esp32s31_hal::radio::domain::ChipPhy>>();
//! ```
//!
//! ```compile_fail,E0277
//! fn requires_clone<T: Clone>() {}
//! requires_clone::<oer_chips_esp32s31_radio_phy::owner::RfClosed<oer_chips_esp32s31_hal::radio::domain::ChipPhy>>();
//! ```
//!
//! ```compile_fail,E0451
//! use oer_chips_esp32s31_hal::radio::domain::ChipPhy;
//! let _forged = oer_chips_esp32s31_radio_phy::owner::RfOpen::<ChipPhy> { brand: core::marker::PhantomData };
//! ```
//!
//! One right makes one place: a second place needs a second right, and the
//! right moves into the first.
//!
//! ```compile_fail,E0382
//! use core::mem::MaybeUninit;
//! use oer_chips_esp32s31_hal::radio::domain::{ChipPhy, PhyStateRight};
//! use oer_chips_esp32s31_radio_phy::owner::PhyPlace;
//! fn two(right: PhyStateRight<ChipPhy>) {
//!     let (mut first, mut second) = (MaybeUninit::uninit(), MaybeUninit::uninit());
//!     let _first = PhyPlace::new(right, &mut first);
//!     let _second = PhyPlace::new(right, &mut second);
//! }
//! ```
//!
//! The handle's state is read only with a proof of a registration: a
//! reference to `RfOpen` or `RfClosed`. A filled place not yet registered
//! has a state nothing reads.
//!
//! ```compile_fail,E0277
//! use oer_chips_esp32s31_hal::radio::domain::ChipPhy;
//! use oer_chips_esp32s31_radio_phy::owner::{PhyHandle, RfCold};
//! fn read(handle: &PhyHandle<'_, ChipPhy>, cold: &RfCold<ChipPhy>) {
//!     let _state = handle.state(cold);
//! }
//! ```
//!
//! A filled place becomes empty again in two ways: before any register was
//! written, with `RfCold` ([`PhyHandle::empty`]), and after a failed
//! operation, with the one right it leaves ([`Vacate`],
//! [`PhyHandle::vacate`]). A failed operation returns its leaf cause and has
//! consumed its proof; the owner can vacate the place, and nothing more.
//! The PHY's clock modules stay held as the failure left them.
//!
//! ```compile_fail,E0451
//! use oer_chips_esp32s31_hal::radio::domain::ChipPhy;
//! let _forged = oer_chips_esp32s31_radio_phy::owner::Vacate::<ChipPhy> { brand: core::marker::PhantomData };
//! ```
//!
//! A proof let go leaves the handle and nothing to call; the handle let go
//! loses the place: Rust has no type that cannot be dropped, and
//! `#[must_use]` only warns.

use core::{marker::PhantomData, mem::MaybeUninit};

use oer_portable_memory::Occupied;
use oer_portable_time::{Duration, Instant};

pub use oer_chips_esp32s31_hal::radio::domain::{ChipPhy, PhyStateRight};

use crate::{PhyConfig, PhyState};

/// The PHY's one place: the right the radio's split handed out and the
/// memory it names. Only an empty place exists as a value; a filled one is
/// inside its [`PhyHandle`].
#[must_use = "the place is the owner's to fill, or its right for the reunion"]
pub struct PhyPlace<'s, P> {
    memory: &'s mut MaybeUninit<PhyState>,
    right: PhyStateRight<P>,
}

impl<'s, P> PhyPlace<'s, P> {
    /// The place, from the split's one right and the owner's memory.
    pub const fn new(right: PhyStateRight<P>, memory: &'s mut MaybeUninit<PhyState>) -> Self {
        Self { memory, right }
    }

    /// The right back, for the radio's reunion. The memory is the owner's
    /// again.
    pub fn into_right(self) -> PhyStateRight<P> {
        let Self { memory: _, right } = self;
        right
    }

    /// Fill the place with a state nothing has registered: the handle and
    /// the proof that RF is cold. This writes no register and cannot refuse.
    pub fn fill(self) -> (PhyHandle<'s, P>, RfCold<P>) {
        let Self { memory, right } = self;
        (
            PhyHandle {
                state: Occupied::fill(memory, PhyState::new(PhyConfig::esp32s31_default())),
                right,
            },
            RfCold { brand: PhantomData },
        )
    }
}

/// The handle of the PHY's filled place, with the right to it: made once by
/// the fill and held by the owner; an operation is lent it.
///
/// ```compile_fail,E0277
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<oer_chips_esp32s31_radio_phy::owner::PhyHandle<'static, oer_chips_esp32s31_hal::radio::domain::ChipPhy>>();
/// ```
#[must_use = "the place stays filled until the handle empties or vacates it"]
pub struct PhyHandle<'s, P> {
    state: Occupied<'s, PhyState>,
    right: PhyStateRight<P>,
}

impl<'s, P> PhyHandle<'s, P> {
    /// Empty the place, nothing having been written since the fill: the
    /// inverse of [`PhyPlace::fill`].
    pub fn empty(self, cold: RfCold<P>) -> PhyPlace<'s, P> {
        let RfCold { brand: _ } = cold;
        self.into_place()
    }

    /// Empty the place, with the right a failed operation left: the place
    /// is the owner's for the next fill.
    pub fn vacate(self, failed: Vacate<P>) -> PhyPlace<'s, P> {
        let Vacate { brand: _ } = failed;
        self.into_place()
    }

    fn into_place(self) -> PhyPlace<'s, P> {
        let Self { state, right } = self;
        let (_state, memory) = state.vacate();
        PhyPlace { memory, right }
    }

    /// The registered state, read with a proof of the registration.
    pub fn state(&self, _registered: &impl Registration<P>) -> &PhyState {
        &self.state
    }

    /// Capture the registered state's current calibration as a cache for a
    /// later cold registration: the final-state handoff. RF may be open or
    /// closed; the state stays registered in both.
    pub fn calibration_cache(
        &self,
        registered: &impl Registration<P>,
        identity: crate::PhyCalibrationIdentity,
    ) -> crate::PhyCalibrationCache {
        self.state(registered).calibration_cache(identity)
    }

    /// Select the calibration tracking's thermal threshold for the next
    /// passes of this registration.
    pub fn set_calibration_threshold(
        &mut self,
        _registered: &impl Registration<P>,
        threshold: crate::CalibrationThreshold,
    ) {
        self.state.set_calibration_threshold(threshold);
    }

    /// The state, for an operation of this crate lent the handle with its
    /// proof.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn state_mut(&mut self) -> &mut PhyState {
        &mut self.state
    }
}

mod sealed {
    pub trait Registration<P> {}
}

/// A proof of a registration: `RfOpen` or `RfClosed`, never `RfCold`.
pub trait Registration<P>: sealed::Registration<P> {}
impl<P> sealed::Registration<P> for RfOpen<P> {}
impl<P> Registration<P> for RfOpen<P> {}
impl<P> sealed::Registration<P> for RfClosed<P> {}
impl<P> Registration<P> for RfClosed<P> {}

/// Proof that RF is cold: the place is filled and no register is written.
/// Made by the fill and given back by a registration refused before its
/// acceptance point.
#[derive(Debug)]
#[must_use = "only a registration or the place's emptying takes this proof"]
pub struct RfCold<P> {
    brand: PhantomData<P>,
}

/// The one right a failed operation leaves its owner: to empty the place.
#[derive(Debug)]
#[must_use = "the place stays filled until this right empties it"]
pub struct Vacate<P> {
    brand: PhantomData<P>,
}

/// Proof that RF is open: made by a registration's and a wake's end.
#[derive(Debug)]
#[must_use = "RF stays open until a close takes this proof"]
pub struct RfOpen<P> {
    brand: PhantomData<P>,
}

/// Proof that RF is closed while the registration stays: made by a close's
/// end.
#[derive(Debug)]
#[must_use = "RF stays closed until a wake takes this proof"]
pub struct RfClosed<P> {
    brand: PhantomData<P>,
}

impl<P> RfCold<P> {
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn registered(self) -> RfOpen<P> {
        let Self { brand } = self;
        RfOpen { brand }
    }

    /// A failed registration consumed this proof: the right to empty the
    /// place is what is left.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn fail(self) -> Vacate<P> {
        let Self { brand } = self;
        Vacate { brand }
    }
}

impl<P> RfOpen<P> {
    /// A failed operation consumed this proof: the right to empty the place
    /// is what is left.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn fail(self) -> Vacate<P> {
        let Self { brand } = self;
        Vacate { brand }
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn close(self) -> RfClosed<P> {
        let Self { brand } = self;
        RfClosed { brand }
    }
}

impl<P> RfClosed<P> {
    /// A failed wake consumed this proof: the right to empty the place is
    /// what is left.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn fail(self) -> Vacate<P> {
        let Self { brand } = self;
        Vacate { brand }
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn open(self) -> RfOpen<P> {
        let Self { brand } = self;
        RfOpen { brand }
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
