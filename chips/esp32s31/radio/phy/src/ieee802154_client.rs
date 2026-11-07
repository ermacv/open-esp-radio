//! IEEE 802.15.4 as a client of the shared PHY domain.
//!
//! The HAL chain holds the IEEE 802.15.4 partition; the registered PHY domain
//! lives in the radio domain as [`ConcurrentPhy`]. Between the module clocks and
//! the MAC reset, ESP-IDF's `esp_ieee802154_enable` runs `esp_phy_enable` and
//! `esp_btbb_enable`, and `ieee802154_mac_init` then overrides the shared
//! transmit-on delay. [`join_ieee802154`] performs those steps on the
//! [`Ieee802154Clocked`] owner and issues the affine
//! [`Ieee802154PhyMembership`]; [`leave_ieee802154`] consumes it again.
//!
//! The vendor puts the radio to sleep without closing RF: on the ESP32-S31
//! `ieee802154_rf_disable` closes RF only together with the REGDMA PHY
//! retention, which the pinned libraries do not provide, so a sleeping
//! client stays a PHY client.
//!
//! The IEEE 802.15.4 driver keeps that membership beside the operational MAC
//! owners, so an interrupt-driven MAC epoch cannot exist apart from a PHY
//! client and a BTBB reference.
//!
//! The BTBB gain parameter is projected here from the settled registration
//! that describes the radio and published to the domain right before the
//! acquisition, never supplied by a caller.

use oer_chips_esp32s31_hal::power::PlatformClockBackend;

use core::fmt;
use oer_chips_esp32s31_hal::{
    radio::domain::{BtbbError, RadioClient, RadioDomain},
    radio::ieee802154::Ieee802154Clocked,
};

use crate::concurrent::{
    BtbbClientAcquireError, ConcurrentPhy, ConcurrentPhyError, JoinRefused, Joined, PhyClientReady,
    acquire_btbb_client, release_client,
};

/// IEEE 802.15.4 holds a client bit in the shared PHY domain and a reference
/// on the shared BTBB baseband, with its transmit-on delay applied.
///
/// ```compile_fail
/// use oer_chips_esp32s31_radio_phy::ieee802154_client::Ieee802154PhyMembership;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<Ieee802154PhyMembership>();
/// ```
#[must_use = "the IEEE 802.15.4 PHY membership must be left through leave_ieee802154"]
pub struct Ieee802154PhyMembership {
    key: crate::state::client::PhyClientKey,
}

impl fmt::Debug for Ieee802154PhyMembership {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Ieee802154PhyMembership")
    }
}

/// Why IEEE 802.15.4 could not join or leave the shared PHY and BTBB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154PhyClientError {
    /// The PHY domain rejected the operation.
    Phy(ConcurrentPhyError),
    /// The BTBB reference is not in the state the operation requires.
    Btbb(BtbbError),
}

/// Enter IEEE 802.15.4 into the shared PHY domain and the shared BTBB
/// baseband.
///
/// This is `esp_phy_enable(PHY_MODEM_IEEE802154)` followed by
/// `esp_btbb_enable`; the pinned `ieee802154_txon_delay_set` writes only the
/// MAC's own delays, so the shared transmit-on delay keeps the value of the
/// BTBB initialization. The
/// returned [`ConcurrentAcquire::TrackingDue`](crate::concurrent::ConcurrentAcquire::TrackingDue) means the domain must run its
/// tracking before the MAC may use RF.
///
/// # Errors
///
/// Every check runs before the client set or BTBB changes: IEEE 802.15.4 already
/// holds BTBB, or the client set rejects the client. The radio comes back
/// either way.
pub fn join_ieee802154<'l, B: PlatformClockBackend>(
    ready: PhyClientReady<'l, B, Ieee802154PhyMembership>,
    clocked: &Ieee802154Clocked,
    clock: &impl oer_portable_time::Clock,
) -> Result<Joined<'l, B, Ieee802154PhyMembership>, JoinRefused<'l, B, Ieee802154PhyClientError>> {
    let key = ready.key();
    let radio = ready.into_radio();
    let acquired =
        match acquire_btbb_client(radio, RadioClient::Ieee802154, clock, |radio, gain| {
            clocked.acquire_btbb(radio, gain)
        }) {
            Ok(acquired) => acquired,
            Err(error) => {
                let error = match error {
                    BtbbClientAcquireError::Phy(error) => Ieee802154PhyClientError::Phy(error),
                    BtbbClientAcquireError::Btbb(error) => Ieee802154PhyClientError::Btbb(error),
                };
                return Err(JoinRefused { error, radio });
            }
        };
    Ok(Joined {
        membership: Ieee802154PhyMembership { key },
        acquired,
        radio,
    })
}

/// Failed leave retaining the membership.
#[must_use = "a failed leave still holds the IEEE 802.15.4 PHY membership"]
pub struct Ieee802154PhyLeaveFailure {
    membership: Ieee802154PhyMembership,
    error: Ieee802154PhyClientError,
}

impl Ieee802154PhyLeaveFailure {
    pub const fn error(&self) -> Ieee802154PhyClientError {
        self.error
    }

    /// Recover the membership for a retry.
    pub fn into_membership(self) -> Ieee802154PhyMembership {
        self.membership
    }
}

impl fmt::Debug for Ieee802154PhyLeaveFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154PhyLeaveFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Leave the shared PHY domain and drop the BTBB reference. Returns whether
/// IEEE 802.15.4 was the last PHY client.
///
/// This is `esp_btbb_disable` and `esp_phy_disable(PHY_MODEM_IEEE802154)`;
/// neither performs register access here.
///
/// # Errors
///
/// The domain rejects the release (tracking pending, poisoned) or the BTBB
/// reference is missing; nothing changed and the membership is returned.
pub fn leave_ieee802154<B: PlatformClockBackend>(
    radio: &mut RadioDomain<B, ConcurrentPhy>,
    clocked: &Ieee802154Clocked,
    membership: Ieee802154PhyMembership,
) -> Result<bool, Ieee802154PhyLeaveFailure> {
    if !radio.holds_btbb(RadioClient::Ieee802154) {
        return Err(Ieee802154PhyLeaveFailure {
            membership,
            error: Ieee802154PhyClientError::Btbb(BtbbError::NotAcquired),
        });
    }
    let key = membership.key;
    let last = match release_client(radio, RadioClient::Ieee802154, key) {
        Ok(last) => last,
        Err(error) => {
            return Err(Ieee802154PhyLeaveFailure {
                membership,
                error: Ieee802154PhyClientError::Phy(error),
            });
        }
    };
    let Ieee802154PhyMembership { key: _ } = membership;
    // The reference was checked above, so the release cannot be rejected.
    let _ = clocked.release_btbb(radio);
    Ok(last)
}

#[cfg(test)]
impl Ieee802154PhyMembership {
    fn for_test() -> Self {
        Self {
            key: crate::state::client::model_permission_key_for_test(),
        }
    }
}

#[cfg(test)]
mod tests;
