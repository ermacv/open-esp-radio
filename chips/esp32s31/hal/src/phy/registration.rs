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

impl PhyRegistrationEpoch {
    /// Opaque epoch observation for retained fault diagnostics.
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// The one owner's right over one PHY registration: [`begin`] gives it, and
/// it is neither `Clone` nor `Copy`, so only the owner that began the
/// registration (the registered PHY domain) holds it. A BTBB gain is
/// published only with it ([`crate::domain::BtbbPublisher::publish`]).
///
/// [`begin`]: crate::owner::PhyInitializationAccess::begin_registration
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRegistrationGrant {
    epoch: PhyRegistrationEpoch,
}

impl PhyRegistrationGrant {
    pub(crate) const fn new(epoch: PhyRegistrationEpoch) -> Self {
        Self { epoch }
    }

    /// The registration this grant is the right over.
    pub const fn epoch(&self) -> PhyRegistrationEpoch {
        self.epoch
    }
}

/// Registration bookkeeping carried by the root and by the active route.
///
/// One word holds the last issued epoch in its low 31 bits and whether that
/// epoch is current in its top bit.
#[derive(Debug)]
pub(crate) struct PhyRegistration(oer_espressif_radio::epoch::Registration);
impl PhyRegistration {
    pub(crate) const fn new() -> Self {
        Self(oer_espressif_radio::epoch::Registration::new())
    }
    pub(crate) fn begin(&mut self) -> PhyRegistrationEpoch {
        PhyRegistrationEpoch(self.0.begin())
    }
    pub(crate) const fn current(&self) -> Option<PhyRegistrationEpoch> {
        match self.0.current() {
            Some(epoch) => Some(PhyRegistrationEpoch(epoch)),
            None => None,
        }
    }
    pub(crate) const fn retired(self) -> Self {
        Self(self.0.retired())
    }
}
