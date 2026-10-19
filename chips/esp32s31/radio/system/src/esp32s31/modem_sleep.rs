//! Wi-Fi modem sleep on the radio domain, outside the R0 client contract.
//!
//! The vendor modem sleep puts Wi-Fi's RF to sleep (`wifi_rf_phy_disable`)
//! and wakes it (`wifi_rf_phy_enable`) while the MAC stays up. It joins the
//! client contract with doze; until then its domain half stays here,
//! compiled. The client lends the proof that its clocks are on.

use oer_esp32s31_hal::{domain::RadioDomain, ieee80211::client::WifiClocksOn};
use oer_esp32s31_phy::{
    concurrent::{ConcurrentAcquire, ConcurrentPhy, ConcurrentPhyError, phy_ready},
    wifi_client::{WifiPhyMembership, WifiPhySuspended, resume_wifi, set_wifi_rx, suspend_wifi},
};

/// Put Wi-Fi's RF to sleep: clear the Wi-Fi receive path, then leave the PHY
/// client set, keeping the registration and calibration. Returns whether
/// Wi-Fi was the last client; the actor then closes RF.
///
/// # Errors
///
/// The domain rejected the release; the receive path is restored and the
/// membership returned.
#[expect(
    dead_code,
    reason = "modem sleep joins the client contract with doze (R3)"
)]
pub(crate) fn suspend_wifi_rf(
    radio: &mut RadioDomain<ConcurrentPhy>,
    clocks: &WifiClocksOn,
    membership: WifiPhyMembership,
) -> Result<(WifiPhySuspended, bool), (WifiPhyMembership, ConcurrentPhyError)> {
    set_wifi_rx(radio, &membership, false);
    match suspend_wifi(radio, clocks, membership) {
        Ok(suspended) => Ok(suspended),
        Err(failure) => {
            let error = failure.error();
            let membership = failure.into_membership();
            set_wifi_rx(radio, &membership, true);
            Err((membership, error))
        }
    }
}

/// Wake Wi-Fi's RF: re-enter the PHY client set, then enable the Wi-Fi
/// receive path. The actor must have woken closed RF first. A returned
/// [`ConcurrentAcquire::TrackingDue`] means the domain must run its tracking
/// before the MAC uses RF.
///
/// # Errors
///
/// The domain rejected the client; nothing changed and the suspended token
/// is returned.
#[expect(
    dead_code,
    reason = "modem sleep joins the client contract with doze (R3)"
)]
pub(crate) fn resume_wifi_rf(
    radio: &mut RadioDomain<ConcurrentPhy>,
    clocks: &WifiClocksOn,
    suspended: WifiPhySuspended,
    clock: &impl oer_time::Clock,
) -> Result<(WifiPhyMembership, ConcurrentAcquire), (WifiPhySuspended, ConcurrentPhyError)> {
    let ready = match phy_ready(radio) {
        Ok(ready) => ready,
        Err(error) => return Err((suspended, error)),
    };
    match resume_wifi(ready, clocks, suspended, clock) {
        Ok(joined) => {
            set_wifi_rx(joined.radio, &joined.membership, true);
            Ok((joined.membership, joined.acquired))
        }
        Err(refused) => {
            let error = refused.error.error();
            Err((refused.error.into_suspended(), error))
        }
    }
}
