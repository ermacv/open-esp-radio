//! Cold-path PHY low-rate gate.

use crate::ieee80211::mac::WifiMacColdHal;

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
