//! Wi-Fi as a client of the shared PHY domain.
//!
//! The HAL chain holds the Wi-Fi partition; the registered PHY domain lives
//! under the arbiter as [`ConcurrentPhy`]. After its module clocks, ESP-IDF's
//! Wi-Fi start runs `esp_phy_enable(PHY_MODEM_WIFI)`. [`join_wifi`] records
//! that on the [`WifiClocked`] owner and issues the affine
//! [`WifiPhyMembership`]; [`leave_wifi`] consumes it again. While it holds
//! the membership, Wi-Fi switches its baseband receive path with
//! [`set_wifi_rx`], the vendor `phy_wifi_enable_set`. Modem sleep trades the
//! membership for a [`WifiPhySuspended`] token ([`suspend_wifi`], the vendor
//! `wifi_rf_phy_disable`) and back ([`resume_wifi`], `wifi_rf_phy_enable`).
//! Wi-Fi uses no BTBB baseband.

use core::fmt;

use oer_esp32s31_hal::{
    ieee80211::client::{WifiClocked, WifiClocksOn},
    shared_radio::SharedRadioLease,
};

use crate::{
    concurrent::{
        ConcurrentPhy, ConcurrentPhyError, JoinRefused, Joined, PhyReady, acquire_client,
        release_client,
    },
    state::client::RadioClient,
};

/// Wi-Fi holds a client bit in the shared PHY domain.
///
/// ```compile_fail
/// use oer_esp32s31_phy::wifi_client::WifiPhyMembership;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<WifiPhyMembership>();
/// ```
#[must_use = "the Wi-Fi PHY membership must be left through leave_wifi"]
pub struct WifiPhyMembership {
    _private: (),
}

impl fmt::Debug for WifiPhyMembership {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WifiPhyMembership")
    }
}

/// Enter Wi-Fi into the shared PHY domain: `esp_phy_enable(PHY_MODEM_WIFI)`.
///
/// `ready` proves the domain prepared and `clocked` the Wi-Fi module
/// clocks. The returned
/// [`ConcurrentAcquire::TrackingDue`](crate::concurrent::ConcurrentAcquire::TrackingDue) means the domain must run its tracking
/// before the MAC may use RF.
///
/// # Errors
///
/// The client set rejects the client; nothing changed.
pub fn join_wifi<'l, 'r>(
    ready: PhyReady<'l, 'r>,
    clocked: &WifiClocked,
    clock: &impl oer_time::Clock,
) -> Result<Joined<'l, 'r, WifiPhyMembership>, JoinRefused<'l, 'r, ConcurrentPhyError>> {
    let _ = clocked;
    let lease = ready.into_lease();
    match acquire_client(lease, RadioClient::Wifi, clock) {
        Ok(acquired) => Ok(Joined {
            membership: WifiPhyMembership { _private: () },
            acquired,
            lease,
        }),
        Err(error) => Err(JoinRefused { error, lease }),
    }
}

/// Enable (`true`) or disable (`false`) the Wi-Fi baseband receive path of
/// the shared PHY: the vendor `phy_wifi_enable_set`, which ESP-IDF's
/// `enable_phy_with_wifi_rx` runs after the PHY is enabled and which the
/// Wi-Fi client clears before it leaves.
#[cfg(target_arch = "riscv32")]
pub fn set_wifi_rx(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    membership: &WifiPhyMembership,
    enabled: bool,
) {
    let _ = membership;
    oer_esp32s31_hal::phy::frequency::set_wifi_enabled(&mut lease.phy_hal(), enabled);
}

/// Failed leave retaining the membership.
#[must_use = "a failed leave still holds the Wi-Fi PHY membership"]
pub struct WifiPhyLeaveFailure {
    membership: WifiPhyMembership,
    error: ConcurrentPhyError,
}

impl WifiPhyLeaveFailure {
    pub const fn error(&self) -> ConcurrentPhyError {
        self.error
    }

    /// Recover the membership for a retry.
    pub fn into_membership(self) -> WifiPhyMembership {
        self.membership
    }
}

impl fmt::Debug for WifiPhyLeaveFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WifiPhyLeaveFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Leave the shared PHY domain: `esp_phy_disable(PHY_MODEM_WIFI)`. Returns
/// whether Wi-Fi was the last PHY client; the caller then closes RF through
/// the shared radio's owner. No register access happens here.
///
/// # Errors
///
/// The domain rejects the release (tracking pending, poisoned); nothing
/// changed and the membership is returned.
pub fn leave_wifi(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocked: &WifiClocked,
    membership: WifiPhyMembership,
) -> Result<bool, WifiPhyLeaveFailure> {
    let _ = clocked;
    match release_client(lease, RadioClient::Wifi) {
        Ok(last) => {
            let WifiPhyMembership { _private: () } = membership;
            Ok(last)
        }
        Err(error) => Err(WifiPhyLeaveFailure { membership, error }),
    }
}

/// Wi-Fi left the PHY client set for modem sleep and may re-enter it.
///
/// ```compile_fail
/// use oer_esp32s31_phy::wifi_client::WifiPhySuspended;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<WifiPhySuspended>();
/// ```
#[must_use = "the suspended Wi-Fi client must resume or leave"]
pub struct WifiPhySuspended {
    _private: (),
}

impl fmt::Debug for WifiPhySuspended {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WifiPhySuspended")
    }
}

/// Failed resume retaining the suspended client.
#[must_use = "a failed resume still holds the suspended Wi-Fi client"]
pub struct WifiPhySuspendedFailure {
    suspended: WifiPhySuspended,
    error: ConcurrentPhyError,
}

impl WifiPhySuspendedFailure {
    pub const fn error(&self) -> ConcurrentPhyError {
        self.error
    }

    /// Recover the suspended client for a retry.
    pub fn into_suspended(self) -> WifiPhySuspended {
        self.suspended
    }
}

impl fmt::Debug for WifiPhySuspendedFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WifiPhySuspendedFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Put Wi-Fi's RF to sleep: the vendor modem sleep's `wifi_rf_phy_disable`,
/// which is `esp_phy_disable(PHY_MODEM_WIFI)`. The registration and its
/// calibration stay. Returns whether Wi-Fi was the last PHY client; the
/// caller then closes RF through the shared radio's owner. No register
/// access happens here.
///
/// # Errors
///
/// As [`leave_wifi`]; the membership is returned.
///
/// A running role proves its clocks with the [`WifiClocksOn`] its clocked
/// client handed out beside the runtime epoch.
pub fn suspend_wifi(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocks: &WifiClocksOn,
    membership: WifiPhyMembership,
) -> Result<(WifiPhySuspended, bool), WifiPhyLeaveFailure> {
    let _ = clocks;
    match release_client(lease, RadioClient::Wifi) {
        Ok(last) => {
            let WifiPhyMembership { _private: () } = membership;
            Ok((WifiPhySuspended { _private: () }, last))
        }
        Err(error) => Err(WifiPhyLeaveFailure { membership, error }),
    }
}

/// Wake Wi-Fi's RF: the vendor modem wake's `wifi_rf_phy_enable`, which is
/// `esp_phy_enable(PHY_MODEM_WIFI)`. The shared radio's owner must have woken
/// closed RF first, which `ready` proves. The returned
/// [`ConcurrentAcquire::TrackingDue`](crate::concurrent::ConcurrentAcquire::TrackingDue)
/// means the domain must run its tracking before the MAC uses RF.
///
/// # Errors
///
/// As [`join_wifi`]; the suspended client is returned.
pub fn resume_wifi<'l, 'r>(
    ready: PhyReady<'l, 'r>,
    clocks: &WifiClocksOn,
    suspended: WifiPhySuspended,
    clock: &impl oer_time::Clock,
) -> Result<Joined<'l, 'r, WifiPhyMembership>, JoinRefused<'l, 'r, WifiPhySuspendedFailure>> {
    let _ = clocks;
    let lease = ready.into_lease();
    match acquire_client(lease, RadioClient::Wifi, clock) {
        Ok(acquired) => {
            let WifiPhySuspended { _private: () } = suspended;
            Ok(Joined {
                membership: WifiPhyMembership { _private: () },
                acquired,
                lease,
            })
        }
        Err(error) => Err(JoinRefused {
            error: WifiPhySuspendedFailure { suspended, error },
            lease,
        }),
    }
}

/// Stop Wi-Fi while its RF sleeps: the client already left the PHY domain,
/// so only the token ends. No register access happens here.
pub fn leave_suspended_wifi(suspended: WifiPhySuspended) {
    let WifiPhySuspended { _private: () } = suspended;
}

#[cfg(test)]
mod tests;
