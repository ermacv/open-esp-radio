//! Cross-layer ownership boundary for the PHY low-rate path.

use oer_esp32s31_hal::ieee80211::mac::{WifiMacColdHal, WifiMacHal};

/// Narrow PHY capability needed by the MAC cold-start policy.
///
/// The MAC chooses whether low-rate operation is wanted, while the generated
/// PAC owns the PHY register identities and the ordered hardware edges.
pub trait MacLowRateHardware {
    fn disable_phy_low_rate(&mut self);
}

impl MacLowRateHardware for WifiMacColdHal<'_> {
    fn disable_phy_low_rate(&mut self) {
        WifiMacColdHal::disable_phy_low_rate(self);
    }
}

/// Runtime authority for the complete reviewed PHY low-rate gate.
///
/// This deliberately says nothing about LR PLCP encoding, rate selection or
/// interoperability. It only owns the exact three register edges and the ROM
/// status readback.
pub trait MacRuntimeLowRateHardware {
    fn phy_low_rate_enabled(&self) -> bool;

    fn configure_phy_low_rate(&mut self, enabled: bool);
}

impl MacRuntimeLowRateHardware for WifiMacHal<'_> {
    fn phy_low_rate_enabled(&self) -> bool {
        WifiMacHal::phy_low_rate_enabled(self)
    }

    fn configure_phy_low_rate(&mut self, enabled: bool) {
        WifiMacHal::configure_phy_low_rate(self, enabled);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacLowRateState {
    Disabled,
    Enabled,
}

impl MacLowRateState {
    const fn from_enabled(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }

    const fn enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacLowRateTransition {
    Activate,
    Restore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacLowRateTransitionError {
    pub transition: MacLowRateTransition,
    pub expected: MacLowRateState,
    pub observed: MacLowRateState,
}

/// Result of exercising the reviewed runtime low-rate gate without retaining
/// it beyond the current synchronous hardware transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacLowRateGateProbe {
    /// This [`crate::tx::TxHardware`] implementation does not own the PHY
    /// low-rate registers. No register was touched.
    OwnerUnavailable,
    /// The gate transaction was applied and the complete matching vendor leaf
    /// restored the ROM status observation to its entry value.
    Restored { previous: MacLowRateState },
}

/// Exclusive, scoped low-rate activation around one runtime MAC authority.
///
/// The session remembers the entry state instead of assuming cold-start
/// `Disabled`. [`Self::restore`] must succeed before the hardware authority is
/// returned, so stop/error paths cannot accidentally strand the shared PHY in
/// LR mode.
#[must_use = "the low-rate session must be restored to return its hardware owner"]
pub struct MacLowRateSession<'hardware, H: MacRuntimeLowRateHardware> {
    hardware: Option<&'hardware mut H>,
    previous: MacLowRateState,
}

impl<'hardware, H: MacRuntimeLowRateHardware> MacLowRateSession<'hardware, H> {
    pub fn activate(hardware: &'hardware mut H) -> Result<Self, MacLowRateTransitionError> {
        let previous = MacLowRateState::from_enabled(hardware.phy_low_rate_enabled());
        let mut session = Self {
            hardware: Some(hardware),
            previous,
        };
        session.hardware_mut().configure_phy_low_rate(true);
        let observed = MacLowRateState::from_enabled(session.hardware_mut().phy_low_rate_enabled());
        if observed != MacLowRateState::Enabled {
            let error = MacLowRateTransitionError {
                transition: MacLowRateTransition::Activate,
                expected: MacLowRateState::Enabled,
                observed,
            };
            // Dropping the still-armed session restores and verifies the
            // exact entry state before the caller can recover its owner.
            drop(session);
            return Err(error);
        }
        Ok(session)
    }

    pub const fn previous_state(&self) -> MacLowRateState {
        self.previous
    }

    /// Borrow the same exclusive MAC authority for the bounded operation that
    /// requires the low-rate gate. The session remains the restore owner.
    pub fn hardware_mut(&mut self) -> &mut H {
        self.hardware
            .as_deref_mut()
            .expect("an armed low-rate session retains its hardware owner")
    }

    /// Restore the exact entry state and return the runtime hardware owner.
    /// A failed readback returns the complete session so the caller can retry
    /// restoration or escalate to a radio reset without losing authority.
    #[allow(clippy::result_large_err)]
    pub fn restore(mut self) -> Result<&'hardware mut H, (MacLowRateTransitionError, Self)> {
        if let Err(error) = self.restore_entry_state() {
            return Err((error, self));
        }
        Ok(self
            .hardware
            .take()
            .expect("a restored low-rate session returns its hardware owner"))
    }

    fn restore_entry_state(&mut self) -> Result<(), MacLowRateTransitionError> {
        let previous = self.previous;
        let hardware = self
            .hardware
            .as_deref_mut()
            .expect("an armed low-rate session retains its hardware owner");
        hardware.configure_phy_low_rate(previous.enabled());
        let observed = MacLowRateState::from_enabled(hardware.phy_low_rate_enabled());
        if observed != previous {
            Err(MacLowRateTransitionError {
                transition: MacLowRateTransition::Restore,
                expected: previous,
                observed,
            })
        } else {
            Ok(())
        }
    }
}

impl<H: MacRuntimeLowRateHardware> Drop for MacLowRateSession<'_, H> {
    fn drop(&mut self) {
        if self.hardware.is_none() {
            return;
        }
        if self.restore_entry_state().is_err() && self.restore_entry_state().is_err() {
            panic!("PHY low-rate rollback failed twice; cannot safely release the TX owner")
        }
    }
}

/// Exercise and restore the complete reviewed low-rate gate transaction.
///
/// A first restore readback mismatch is followed by the same exact restore
/// transaction once more. If both explicit attempts fail, the armed drop
/// guard performs at most two additional restore attempts. A persistent
/// mismatch reaches a terminal panic after exactly four bounded restore
/// writes rather than returning with unknown shared PHY state. The normal
/// unsupported-LR frontier therefore returns only after the matching vendor
/// restore leaf and its status readback have completed.
pub fn probe_phy_low_rate_gate<H: MacRuntimeLowRateHardware>(
    hardware: &mut H,
) -> Result<MacLowRateGateProbe, MacLowRateTransitionError> {
    let session = MacLowRateSession::activate(hardware)?;
    let previous = session.previous_state();
    match session.restore() {
        Ok(_) => Ok(MacLowRateGateProbe::Restored { previous }),
        Err((error, retry)) => match retry.restore() {
            Ok(_) => Err(error),
            Err((_retry_error, restore_obligation)) => {
                // Drop is the terminal verified rollback invariant. It makes
                // at most two additional attempts and otherwise panics; this
                // branch does not promise affine owner recovery from panic.
                drop(restore_obligation);
                Err(error)
            }
        },
    }
}

#[cfg(test)]
mod tests;
