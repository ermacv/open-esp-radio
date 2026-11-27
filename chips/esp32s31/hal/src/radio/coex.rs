//! Coexistence as the radio domain holds it.
//!
//! The priority of a coexistence event is the family's vendor table
//! ([`CoexPtiTable::VENDOR`]), which no code changes; every protocol
//! programs its own MAC PTI fields from it. A PTI value does not report RF
//! admission or grant revocation: the hardware arbitrates by priority and
//! gives the CPU no grant acknowledgement.
//!
//! Timer 5 carries the PHY grant-protect request ([`PhyGrantProtect`]); no
//! other coexistence timer is programmed. Each client's coexistence state is
//! one affine value ([`participation`]).

use crate::radio::modem::{
    CoexGrantTimerRegisters,
    values::{CoexTimerClientValue, CoexTimerPtiValue},
};

/// The event priority table every Espressif chip
/// shares; the domain keeps one table as shared state.
pub use oer_chips_family_radio_coex::{COEX_EVENT_COUNT, CoexEventId, CoexPti, CoexPtiTable};

pub(crate) mod participation;
pub mod timer_bank;

/// Why the PHY grant-protect request cannot change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyGrantProtectError {
    /// The request is already programmed; the vendor event has no nesting.
    AlreadyProtected,
    /// No request is programmed.
    NotProtected,
    /// The client or the priority is outside its register domain.
    Domain,
}

/// The vendor PHY grant-protect event.
pub(crate) const PHY_GRANT_PROTECT_EVENT: CoexEventId = match CoexEventId::new(48) {
    Some(event) => event,
    None => unreachable!(),
};

/// The PHY grant-protect request of one radio owner.
///
/// This is the pinned archive's `phy_acquire_grant_protect` /
/// `phy_release_grant_protect` pair: `coex_core_request(2, 48, 0, 0)` and
/// `coex_core_release(2, 48)` on timer 5. The owner keeps one flag, so the
/// request is serialized as the vendor event has no nesting. It is a priority
/// request to the hardware arbiter, never a grant acknowledgement: it does not
/// prove that any other radio stopped.
pub struct PhyGrantProtect<'owner> {
    timers: CoexGrantTimerRegisters<'owner>,
    pti: CoexPti,
    protected: &'owner mut bool,
}

impl<'owner> PhyGrantProtect<'owner> {
    pub(crate) fn new(
        timers: CoexGrantTimerRegisters<'owner>,
        pti: CoexPti,
        protected: &'owner mut bool,
    ) -> Self {
        Self {
            timers,
            pti,
            protected,
        }
    }

    /// Program the request: timer 5 receives the request-kind client field,
    /// the event-48 priority and zero primary and secondary targets, then is
    /// enabled.
    ///
    /// Zero timing arguments convert to zero tick images for every clock
    /// selection, so no clock sample is taken.
    ///
    /// # Errors
    ///
    /// The request is already programmed; nothing is written.
    pub fn acquire(&mut self) -> Result<(), PhyGrantProtectError> {
        if *self.protected {
            return Err(PhyGrantProtectError::AlreadyProtected);
        }
        let client = CoexTimerClientValue::from(
            oer_chips_esp32s31_pac::coex::configuration::Parameter1::PhyGrantProtect,
        );
        let Some(pti) = CoexTimerPtiValue::new(u32::from(self.pti.value())) else {
            return Err(PhyGrantProtectError::Domain);
        };
        self.timers.configure(client, pti);
        self.timers.set_primary_target(0);
        self.timers.set_secondary_target(0);
        self.timers.enable();
        *self.protected = true;
        Ok(())
    }

    /// Withdraw the request by disabling timer 5.
    ///
    /// # Errors
    ///
    /// No request is programmed; nothing is written.
    pub fn release(&mut self) -> Result<(), PhyGrantProtectError> {
        if !*self.protected {
            return Err(PhyGrantProtectError::NotProtected);
        }
        self.timers.disable();
        *self.protected = false;
        Ok(())
    }
}
