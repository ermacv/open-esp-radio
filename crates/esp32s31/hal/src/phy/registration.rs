//! PHY registration epochs of the neutral radio root.
//!
//! A registration result held apart from its hardware (for example the PHY
//! client set of a registered owner) is valid only while its epoch is the
//! current one of the route that lent the PHY. The counter lives in the HAL
//! radio root and travels with each route, so epochs never repeat across
//! route changes within one boot.

/// Identity of one PHY registration on the unique radio-PHY partition.
///
/// A registration issues a new epoch before it touches hardware. The epoch
/// stays current until another registration begins or the route returns the
/// PHY to the neutral radio root.
///
/// The value is 32 bits wide so that the registered PHY owners carrying it
/// keep their size; those owners live inside reviewed async stack frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRegistrationEpoch(core::num::NonZeroU32);

/// Registration bookkeeping carried by the root and by the active route.
///
/// One word holds the last issued epoch in its low 31 bits and whether that
/// epoch is current in its top bit.
#[derive(Debug)]
pub(crate) struct PhyRegistration(u32);

impl PhyRegistration {
    const CURRENT: u32 = 1 << 31;
    const COUNTER: u32 = Self::CURRENT - 1;

    /// No epoch has been issued in this boot.
    pub(crate) const fn new() -> Self {
        Self(0)
    }

    const fn issued(&self) -> u32 {
        self.0 & Self::COUNTER
    }

    /// Begin a registration and retire every earlier epoch.
    pub(crate) fn begin(&mut self) -> PhyRegistrationEpoch {
        // Each registration runs a full calibration graph, so 2^31 of them
        // cannot occur in one boot. Wrapping past zero keeps this total.
        let next = self.issued().wrapping_add(1) & Self::COUNTER;
        let issued = core::num::NonZeroU32::new(next).unwrap_or(core::num::NonZeroU32::MIN);
        self.0 = issued.get() | Self::CURRENT;
        PhyRegistrationEpoch(issued)
    }

    /// The registration that currently describes the PHY, if any.
    pub(crate) const fn current(&self) -> Option<PhyRegistrationEpoch> {
        if self.0 & Self::CURRENT == 0 {
            return None;
        }
        match core::num::NonZeroU32::new(self.issued()) {
            Some(issued) => Some(PhyRegistrationEpoch(issued)),
            None => None,
        }
    }

    /// Retire the current registration while keeping the issued counter.
    pub(crate) const fn retired(self) -> Self {
        Self(self.issued())
    }
}
